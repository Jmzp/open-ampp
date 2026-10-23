use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::catalog::compare_versions;
use crate::error::{Error, IoContext, Result};
use crate::paths::Tool;
use crate::tools::Extra;

/// Persistent app state (`state.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// Installed versions per tool.
    pub installed: BTreeMap<Tool, Vec<String>>,
    /// Default version per tool.
    pub active: BTreeMap<Tool, String>,
    pub ports: Ports,
    pub projects: Vec<Project>,
    /// User PHP settings, shared by every version.
    pub php: PhpSettings,
    /// Installed helper tools (phpMyAdmin, Composer) and their version.
    #[serde(deserialize_with = "known_extras")]
    pub extras: BTreeMap<Extra, String>,
}

/// Skips tools this build does not know (e.g. ones removed in an update) instead of failing to load.
fn known_extras<'de, D>(d: D) -> std::result::Result<BTreeMap<Extra, String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = BTreeMap::<String, String>::deserialize(d)?;
    Ok(raw
        .into_iter()
        .filter_map(|(k, v)| serde_json::from_value(serde_json::Value::String(k)).ok().map(|e| (e, v)))
        .collect())
}

/// Applied on top of the base `php.ini`: in a `php.ini` the last definition wins.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PhpSettings {
    /// Directives edited from the form (`memory_limit` → `512M`).
    pub directives: BTreeMap<String, String>,
    /// Enabled extensions (Windows only, where they are DLLs). `None` means the defaults.
    pub extensions: Option<Vec<String>>,
    /// Free text appended to the end of `php.ini`.
    pub custom: String,
}

impl PhpSettings {
    /// Rejects directives that would break `php.ini` (line breaks, odd keys).
    pub fn validate(&self) -> Result<()> {
        for (key, value) in &self.directives {
            let key_ok = !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || "._".contains(c));
            if !key_ok || value.contains(['\n', '\r']) {
                return Err(Error::InvalidSetting(format!("{key} = {value}")));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ports {
    pub http: u16,
    pub https: u16,
    pub mariadb: u16,
    /// Base for PHP FastCGI: each version uses `php_fcgi_base + major*10 + minor`.
    pub php_fcgi_base: u16,
}

impl Default for Ports {
    fn default() -> Self {
        Self { http: 8080, https: 8443, mariadb: 3306, php_fcgi_base: 9000 }
    }
}

/// A site served by Apache on its own domain, from any folder (see `projects`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    /// Domain, e.g. `blog.localhost`.
    pub domain: String,
    pub docroot: PathBuf,
    /// PHP series (`8.2`), so patch upgrades keep working; `None` uses the active version.
    pub php: Option<String>,
    #[serde(default)]
    pub https: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            installed: BTreeMap::new(),
            active: BTreeMap::new(),
            ports: Ports::default(),
            projects: Vec::new(),
            php: PhpSettings::default(),
            extras: BTreeMap::new(),
        }
    }
}

impl State {
    /// Loads the state; returns the default state if the file does not exist.
    pub fn load(path: &Path) -> Result<Self> {
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|source| Error::Json { path: path.to_path_buf(), source }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).at(path),
        }
    }

    /// Records an installed version; it becomes active if there is no active one.
    pub fn mark_installed(&mut self, tool: Tool, version: &str) {
        let versions = self.installed.entry(tool).or_default();
        if !versions.iter().any(|v| v == version) {
            versions.push(version.to_string());
            versions.sort_by(|a, b| compare_versions(b, a));
        }
        self.active.entry(tool).or_insert_with(|| version.to_string());
    }

    /// Removes a version; if it was active, the newest remaining one becomes active.
    pub fn mark_uninstalled(&mut self, tool: Tool, version: &str) {
        let versions = self.installed.entry(tool).or_default();
        versions.retain(|v| v != version);
        let next = versions.first().cloned();
        if versions.is_empty() {
            self.installed.remove(&tool);
        }
        if self.active.get(&tool).is_some_and(|v| v == version) {
            match next {
                Some(v) => self.active.insert(tool, v),
                None => self.active.remove(&tool),
            };
        }
    }

    pub fn set_active(&mut self, tool: Tool, version: &str) -> Result<()> {
        let installed = self.installed.get(&tool).is_some_and(|vs| vs.iter().any(|v| v == version));
        if !installed {
            return Err(Error::NotInstalled { tool, version: version.to_string() });
        }
        self.active.insert(tool, version.to_string());
        Ok(())
    }

    /// Saves atomically (writes a temp file and renames it).
    pub fn save(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_vec_pretty(self)
            .map_err(|source| Error::Json { path: path.to_path_buf(), source })?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json).at(&tmp)?;
        fs::rename(&tmp, path).at(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_yields_default() {
        let tmp = tempfile::tempdir().unwrap();
        let state = State::load(&tmp.path().join("state.json")).unwrap();
        assert_eq!(state, State::default());
        assert_eq!(state.ports.http, 8080);
    }

    #[test]
    fn roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state.json");
        let mut state = State::default();
        state.installed.insert(Tool::Php, vec!["8.1.30".into(), "8.3.12".into()]);
        state.active.insert(Tool::Php, "8.3.12".into());
        state.projects.push(Project {
            name: "blog".into(),
            domain: "blog.localhost".into(),
            docroot: "/srv/blog/public".into(),
            php: Some("8.1".into()),
            https: false,
        });
        state.save(&path).unwrap();
        assert_eq!(State::load(&path).unwrap(), state);
    }

    #[test]
    fn install_and_uninstall_keep_active_consistent() {
        let mut state = State::default();
        state.mark_installed(Tool::Php, "8.1.30");
        state.mark_installed(Tool::Php, "8.3.12");
        assert_eq!(state.installed[&Tool::Php], ["8.3.12", "8.1.30"]);
        assert_eq!(state.active[&Tool::Php], "8.1.30", "the first installed version becomes active");

        state.set_active(Tool::Php, "8.3.12").unwrap();
        assert!(state.set_active(Tool::Php, "7.4.0").is_err());

        state.mark_uninstalled(Tool::Php, "8.3.12");
        assert_eq!(state.active[&Tool::Php], "8.1.30");
        state.mark_uninstalled(Tool::Php, "8.1.30");
        assert!(!state.active.contains_key(&Tool::Php));
        assert!(!state.installed.contains_key(&Tool::Php));
    }

    #[test]
    fn partial_file_fills_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state.json");
        fs::write(&path, r#"{"ports":{"http":80}}"#).unwrap();
        let state = State::load(&path).unwrap();
        assert_eq!(state.ports.http, 80);
        assert_eq!(state.ports.mariadb, 3306);
    }

    #[test]
    fn unknown_extras_are_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state.json");
        fs::write(&path, r#"{"extras":{"adminer":"5.0","composer":"2.8.0"}}"#).unwrap();
        let state = State::load(&path).unwrap();
        assert_eq!(state.extras.len(), 1);
        assert_eq!(state.extras[&Extra::Composer], "2.8.0");
    }

    #[test]
    fn php_settings_reject_injected_lines() {
        let mut php = PhpSettings::default();
        php.directives.insert("memory_limit".into(), "512M".into());
        assert!(php.validate().is_ok());
        php.directives.insert("memory_limit".into(), "1\nextension=evil".into());
        assert!(php.validate().is_err());
        php.directives.clear();
        php.directives.insert("bad key".into(), "1".into());
        assert!(php.validate().is_err());
    }
}
