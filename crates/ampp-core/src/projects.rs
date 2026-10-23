//! Projects: sites served on their own domain (`blog.localhost`) from any folder, each one with
//! its own PHP version. Browsers resolve `*.localhost` to 127.0.0.1, so no hosts file is needed.

use crate::config::Vhost;
use crate::error::{Error, Result};
use crate::install::{self, Installation};
use crate::paths::{AppPaths, Tool};
use crate::ports;
use crate::state::{Project, State};

/// `major.minor` of a version (`8.2.30` → `8.2`).
pub fn series(version: &str) -> String {
    version.split('.').take(2).collect::<Vec<_>>().join(".")
}

/// PHP that serves `series`: the active version when it matches (or when there is no series),
/// otherwise the newest installed version of that series.
pub fn php_for(paths: &AppPaths, state: &State, series: Option<&str>) -> Result<Installation> {
    let active_id = state.active.get(&Tool::Php).ok_or(Error::NoActiveVersion(Tool::Php))?;
    let active = install::locate(paths, Tool::Php, active_id)?;
    let Some(wanted) = series else {
        return Ok(active);
    };
    if active.minor_version() == wanted {
        return Ok(active);
    }
    // `installed` is sorted newest first.
    state
        .installed
        .get(&Tool::Php)
        .into_iter()
        .flatten()
        .filter_map(|id| install::locate(paths, Tool::Php, id).ok())
        .find(|inst| inst.minor_version() == wanted)
        .ok_or_else(|| Error::NotInstalled { tool: Tool::Php, version: wanted.to_string() })
}

/// PHP versions that projects need besides the active one, one per series.
pub fn extra_php(paths: &AppPaths, state: &State) -> Vec<Installation> {
    let active = state.active.get(&Tool::Php);
    let mut extra: Vec<Installation> = Vec::new();
    for project in &state.projects {
        let Ok(inst) = php_for(paths, state, project.php.as_deref()) else { continue };
        if Some(&inst.id) != active && !extra.iter().any(|e| e.id == inst.id) {
            extra.push(inst);
        }
    }
    extra
}

/// Apache virtual hosts for the projects. A project whose PHP series is no longer installed falls
/// back to the active version instead of failing.
pub fn vhosts(paths: &AppPaths, state: &State, active_php_port: u16) -> Vec<Vhost> {
    state
        .projects
        .iter()
        .map(|p| Vhost {
            domain: p.domain.clone(),
            docroot: p.docroot.clone(),
            php_port: php_for(paths, state, p.php.as_deref())
                .map(|inst| ports::php_fcgi_port(state.ports.php_fcgi_base, &inst.version))
                .unwrap_or(active_php_port),
        })
        .collect()
}

/// Address of a project in the browser.
pub fn url(state: &State, project: &Project) -> String {
    match state.ports.http {
        80 => format!("http://{}/", project.domain),
        port => format!("http://{}:{port}/", project.domain),
    }
}

/// A hostname with at least two labels (`blog.localhost`), lowercase, without the bare `localhost`.
pub fn valid_domain(domain: &str) -> bool {
    let labels: Vec<&str> = domain.split('.').collect();
    domain.len() <= 253
        && labels.len() >= 2
        && labels.iter().all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
}

/// Adds a project, or replaces the one called `previous`, after normalizing and validating it.
pub fn save(paths: &AppPaths, state: &mut State, previous: Option<&str>, mut project: Project) -> Result<()> {
    project.name = project.name.trim().to_string();
    project.domain = project.domain.trim().trim_end_matches('.').to_lowercase();
    project.php = project.php.map(|v| series(v.trim())).filter(|v| !v.is_empty());

    let invalid = |why: String| Err(Error::InvalidProject(why));
    let others = || state.projects.iter().filter(|p| Some(p.name.as_str()) != previous);
    let (name, domain) = (&project.name, &project.domain);
    if name.is_empty() {
        return invalid(tr!("ponle un nombre", "give it a name"));
    }
    if others().any(|p| p.name == *name) {
        return invalid(tr!("ya existe un proyecto llamado {name}", "there is already a project called {name}"));
    }
    if !valid_domain(domain) {
        return invalid(tr!(
            "«{domain}» no es un dominio válido (p. ej. blog.localhost)",
            "“{domain}” is not a valid domain (e.g. blog.localhost)"
        ));
    }
    if others().any(|p| p.domain == *domain) {
        return invalid(tr!("{domain} ya lo usa otro proyecto", "{domain} is already used by another project"));
    }
    if !project.docroot.is_absolute() || !project.docroot.is_dir() {
        let dir = project.docroot.display();
        return invalid(tr!("la carpeta {dir} no existe", "the folder {dir} does not exist"));
    }
    if let Some(php) = &project.php {
        php_for(paths, state, Some(php))?;
    }

    match previous.and_then(|old| state.projects.iter_mut().find(|p| p.name == old)) {
        Some(slot) => *slot = project,
        None => state.projects.push(project),
    }
    state.projects.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(())
}

/// Removes a project; its folder is left untouched.
pub fn remove(state: &mut State, name: &str) -> Result<Project> {
    let i = state.projects.iter().position(|p| p.name == name).ok_or_else(|| {
        Error::InvalidProject(tr!("no existe el proyecto {name}", "there is no project called {name}"))
    })?;
    Ok(state.projects.remove(i))
}

/// Suggested domain for a project name (`My Blog` → `my-blog.localhost`).
pub fn suggested_domain(name: &str) -> String {
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug.split('-').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("-");
    format!("{}.localhost", if slug.is_empty() { "site" } else { &slug })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Data folder with fake PHP installs (only their folders) for `versions`; the first is active.
    fn env(versions: &[&str]) -> (tempfile::TempDir, AppPaths, State) {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().join("data"));
        let mut state = State::default();
        for v in versions {
            std::fs::create_dir_all(paths.bin(Tool::Php, v)).unwrap();
            state.mark_installed(Tool::Php, v);
        }
        (tmp, paths, state)
    }

    fn project(name: &str, docroot: &std::path::Path, php: Option<&str>) -> Project {
        Project {
            name: name.into(),
            domain: suggested_domain(name),
            docroot: docroot.to_path_buf(),
            php: php.map(String::from),
            https: false,
        }
    }

    #[test]
    fn domains() {
        for ok in ["blog.localhost", "a-b.test", "x1.y2.localhost"] {
            assert!(valid_domain(ok), "{ok}");
        }
        for bad in ["localhost", "blog", "Blog.localhost", "-a.localhost", "a..localhost", "a b.localhost", "a_b.localhost"] {
            assert!(!valid_domain(bad), "{bad}");
        }
        assert_eq!(suggested_domain("My Blog!"), "my-blog.localhost");
        assert_eq!(suggested_domain("¿?"), "site.localhost");
    }

    #[test]
    fn php_resolves_by_series() {
        let (_tmp, paths, mut state) = env(&["8.4.27", "8.2.35", "8.2.30"]);
        assert_eq!(php_for(&paths, &state, None).unwrap().id, "8.4.27", "the first installed is active");
        assert_eq!(php_for(&paths, &state, Some("8.2")).unwrap().id, "8.2.35", "newest of the series");
        assert!(matches!(php_for(&paths, &state, Some("8.3")), Err(Error::NotInstalled { .. })));

        state.set_active(Tool::Php, "8.2.30").unwrap();
        assert_eq!(php_for(&paths, &state, Some("8.2")).unwrap().id, "8.2.30", "the active one wins in its series");
    }

    #[test]
    fn extra_php_lists_each_other_series_once() {
        let (tmp, paths, mut state) = env(&["8.4.27", "8.2.35", "8.3.20"]);
        for (name, php) in [("a", Some("8.2")), ("b", Some("8.2.1")), ("c", None), ("d", Some("8.4")), ("e", Some("8.3"))] {
            save(&paths, &mut state, None, project(name, tmp.path(), php)).unwrap();
        }
        let ids: Vec<String> = extra_php(&paths, &state).into_iter().map(|i| i.id).collect();
        assert_eq!(ids, ["8.2.35", "8.3.20"]);

        state.ports.php_fcgi_base = 9000;
        let ports: Vec<u16> = vhosts(&paths, &state, 9084).into_iter().map(|v| v.php_port).collect();
        assert_eq!(ports, [9082, 9082, 9084, 9084, 9083]);
    }

    #[test]
    fn save_normalizes_validates_and_replaces() {
        let (tmp, paths, mut state) = env(&["8.4.27"]);
        let mut p = project("  Blog ", tmp.path(), Some("8.4.27"));
        p.domain = " Blog.Localhost. ".into();
        save(&paths, &mut state, None, p).unwrap();
        assert_eq!(state.projects[0].name, "Blog");
        assert_eq!(state.projects[0].domain, "blog.localhost");
        assert_eq!(state.projects[0].php.as_deref(), Some("8.4"));

        let dup = |name: &str, domain: &str| Project { domain: domain.into(), ..project(name, tmp.path(), None) };
        let err = |r: Result<()>| assert!(matches!(r, Err(Error::InvalidProject(_))), "{r:?}");
        err(save(&paths, &mut state, None, dup("Blog", "other.localhost")));
        err(save(&paths, &mut state, None, dup("Other", "blog.localhost")));
        err(save(&paths, &mut state, None, dup("", "x.localhost")));
        err(save(&paths, &mut state, None, dup("Other", "nodot")));
        err(save(&paths, &mut state, None, project("Missing", &tmp.path().join("nope"), None)));
        err(save(&paths, &mut state, None, project("Relative", std::path::Path::new("www"), None)));
        assert!(matches!(
            save(&paths, &mut state, None, project("Old", tmp.path(), Some("7.4"))),
            Err(Error::NotInstalled { .. })
        ));

        // Editing keeps its own name and domain without clashing with itself.
        save(&paths, &mut state, Some("Blog"), dup("Blog", "blog.localhost")).unwrap();
        save(&paths, &mut state, Some("Blog"), dup("Journal", "journal.localhost")).unwrap();
        assert_eq!(state.projects.len(), 1);
        assert_eq!(state.projects[0].name, "Journal");
        assert_eq!(url(&state, &state.projects[0]), "http://journal.localhost:8080/");

        assert_eq!(remove(&mut state, "Journal").unwrap().domain, "journal.localhost");
        assert!(remove(&mut state, "Journal").is_err());
    }
}
