//! `ampp doctor`: checks that explain why something does not start or does not work.

use std::fs;

use serde::Serialize;

use crate::install;
use crate::paths::{AppPaths, Tool};
use crate::ports;
use crate::projects;
use crate::services::{self, Supervisor};
use crate::state::State;
use crate::tls;
use crate::tools::{self, Extra};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Ok,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Check {
    pub level: Level,
    pub title: String,
    /// What to do about it.
    pub hint: Option<String>,
}

impl Check {
    fn ok(title: String) -> Self {
        Self { level: Level::Ok, title, hint: None }
    }

    fn warn(title: String, hint: String) -> Self {
        Self { level: Level::Warn, title, hint: Some(hint) }
    }

    fn error(title: String, hint: String) -> Self {
        Self { level: Level::Error, title, hint: Some(hint) }
    }
}

/// Runs every local check (the catalog, which needs the network, is up to the caller).
pub async fn run(paths: &AppPaths, state: &State, sup: &Supervisor) -> Vec<Check> {
    let mut checks = vec![data_folder(paths)];
    checks.extend(versions(paths, state));
    if services::vc_runtime_missing() {
        let url = services::VC_REDIST_URL;
        checks.push(Check::error(
            tr!("Falta el Microsoft Visual C++ Redistributable", "The Microsoft Visual C++ Redistributable is missing"),
            tr!("Apache y PHP lo necesitan: {url}", "Apache and PHP need it: {url}"),
        ));
    }
    checks.extend(port_checks(paths, state, sup).await);
    if !tools::shims_on_path(paths) {
        checks.push(Check::warn(
            tr!("Los comandos php, composer y mariadb no están en tu PATH", "The php, composer and mariadb commands are not on your PATH"),
            tr!("ampp shims --add-to-path, o Herramientas → Terminal", "ampp shims --add-to-path, or Tools → Terminal"),
        ));
    }
    checks.extend(project_checks(paths, state));
    checks
}

fn data_folder(paths: &AppPaths) -> Check {
    let root = paths.root().display().to_string();
    let probe = paths.root().join(".doctor-write-test");
    let writable = fs::write(&probe, b"ok").is_ok();
    let _ = fs::remove_file(&probe);
    if writable {
        Check::ok(tr!("Carpeta de datos {root}", "Data folder {root}"))
    } else {
        Check::error(
            tr!("No se puede escribir en la carpeta de datos {root}", "The data folder {root} is not writable"),
            tr!("revisa sus permisos, o usa otra con OPEN_AMPP_HOME", "check its permissions, or use another one with OPEN_AMPP_HOME"),
        )
    }
}

fn versions(paths: &AppPaths, state: &State) -> Vec<Check> {
    Tool::ALL
        .into_iter()
        .map(|tool| {
            let cmd = tool.as_str();
            match state.active.get(&tool) {
                None => Check::warn(
                    tr!("{tool}: no hay ninguna versión instalada", "{tool}: no version is installed"),
                    tr!("ampp install {cmd}, o Versiones en la app", "ampp install {cmd}, or Versions in the app"),
                ),
                Some(id) => match install::locate(paths, tool, id) {
                    Ok(inst) => Check::ok(format!("{tool} {}", inst.version)),
                    Err(_) => Check::error(
                        tr!("{tool} {id} está activa pero no instalada", "{tool} {id} is active but not installed"),
                        tr!("ampp install {cmd} {id}, o elige otra con ampp use", "ampp install {cmd} {id}, or pick another one with ampp use"),
                    ),
                },
            }
        })
        .collect()
}

/// Each port Open AMPP needs: fine if free or held by its own service, an error if another
/// program holds it.
async fn port_checks(paths: &AppPaths, state: &State, sup: &Supervisor) -> Vec<Check> {
    if let Err(e) = state.ports.validate() {
        return vec![Check::error(e.to_string(), tr!("corrígelo en Herramientas → Puertos", "fix it in Tools → Ports"))];
    }
    let status = sup.status().await;
    let mail = sup.mail_status().await;
    let ours = |port: u16| {
        status.iter().any(|s| s.running && (s.port == Some(port) || s.extra_php.iter().any(|e| e.port == port)))
            || (mail.running && (mail.web_port == Some(port) || port == state.ports.mail_smtp))
    };

    let mut wanted: Vec<(String, u16)> = Vec::new();
    if state.active.contains_key(&Tool::Apache) {
        wanted.push(("Apache".into(), state.ports.http));
        if state.projects.iter().any(|p| p.https) {
            wanted.push(("Apache HTTPS".into(), state.ports.https));
        }
    }
    if state.active.contains_key(&Tool::Mariadb) {
        wanted.push(("MariaDB".into(), state.ports.mariadb));
    }
    let active_php = projects::php_for(paths, state, None).ok();
    for inst in active_php.into_iter().chain(projects::extra_php(paths, state)) {
        wanted.push((format!("PHP {}", inst.version), ports::php_fcgi_port(state.ports.php_fcgi_base, &inst.version)));
    }
    if tools::file(paths, Extra::Mailpit).exists() {
        wanted.push(("Mailpit SMTP".into(), state.ports.mail_smtp));
        wanted.push(("Mailpit".into(), state.ports.mail_web));
    }

    wanted
        .into_iter()
        .map(|(name, port)| {
            if ours(port) {
                Check::ok(tr!("Puerto {port} ({name}): en uso por Open AMPP", "Port {port} ({name}): in use by Open AMPP"))
            } else if ports::is_free(port) {
                Check::ok(tr!("Puerto {port} ({name}): libre", "Port {port} ({name}): free"))
            } else {
                Check::error(
                    tr!("Puerto {port} ({name}): lo usa otro programa", "Port {port} ({name}): another program is using it"),
                    tr!("ciérralo, o cambia el puerto en Herramientas → Puertos", "stop it, or change the port in Tools → Ports"),
                )
            }
        })
        .collect()
}

fn project_checks(paths: &AppPaths, state: &State) -> Vec<Check> {
    let mut checks = Vec::new();
    for p in &state.projects {
        let (name, dir) = (&p.name, p.docroot.display());
        if !p.docroot.is_dir() {
            checks.push(Check::error(
                tr!("Proyecto {name}: la carpeta {dir} no existe", "Project {name}: the folder {dir} does not exist"),
                tr!("edítalo en Proyectos o quítalo con ampp project rm", "edit it in Projects or remove it with ampp project rm"),
            ));
        }
        if let Some(series) = &p.php {
            if projects::php_for(paths, state, Some(series)).is_err() {
                checks.push(Check::warn(
                    tr!("Proyecto {name}: PHP {series} no está instalado", "Project {name}: PHP {series} is not installed"),
                    tr!("lo atiende la versión activa; instálalo con ampp install php", "the active version serves it; install it with ampp install php"),
                ));
            }
        }
    }
    if state.projects.iter().any(|p| p.https) && tls::is_trusted(paths) == Some(false) {
        checks.push(Check::warn(
            tr!("La CA local de HTTPS no es de confianza", "The local HTTPS CA is not trusted"),
            tr!("ampp trust, o Proyectos → HTTPS local", "ampp trust, or Projects → Local HTTPS"),
        ));
    }
    checks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Project;

    #[tokio::test]
    async fn reports_missing_versions_busy_ports_and_broken_projects() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path());
        paths.ensure().unwrap();
        let mut state = State::default();
        std::fs::create_dir_all(paths.bin(Tool::Apache, "2.4.69")).unwrap();
        state.mark_installed(Tool::Apache, "2.4.69");
        state.active.insert(Tool::Php, "8.4.1".into()); // active but its folder is gone
        let busy = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        state.ports.http = busy.local_addr().unwrap().port();
        state.projects.push(Project {
            name: "gone".into(),
            domain: "gone.localhost".into(),
            docroot: tmp.path().join("missing"),
            php: Some("7.4".into()),
            https: false,
        });

        let checks = run(&paths, &state, &Supervisor::new(paths.clone())).await;
        let find = |part: &str| checks.iter().find(|c| c.title.contains(part)).unwrap_or_else(|| panic!("{part}: {checks:#?}"));
        // Matched on language-neutral parts: messages follow the system language.
        assert_eq!(find(&paths.root().display().to_string()).level, Level::Ok);
        assert_eq!(find("Apache 2.4.69").level, Level::Ok);
        assert_eq!(find("8.4.1").level, Level::Error, "active but not installed");
        assert_eq!(find("MariaDB:").level, Level::Warn, "no version");
        let port = state.ports.http;
        assert_eq!(find(&format!("{port} (Apache)")).level, Level::Error, "another program holds it");
        assert_eq!(find("missing").level, Level::Error, "the project folder is gone");
        assert_eq!(find("7.4").level, Level::Warn, "its PHP series is not installed");
        assert!(checks.iter().all(|c| c.level == Level::Ok || c.hint.is_some()), "every problem says what to do");
        drop(busy);
    }

    #[tokio::test]
    async fn invalid_ports_are_one_clear_error() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path());
        paths.ensure().unwrap();
        let mut state = State::default();
        state.ports.https = state.ports.http;
        let checks = port_checks(&paths, &state, &Supervisor::new(paths.clone())).await;
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].level, Level::Error);
    }
}
