use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::catalog::compare_versions;
use crate::error::{Error, IoContext, Result};
use crate::paths::Tool;

/// Estado persistente de la app (`state.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// Versiones instaladas por herramienta.
    pub installed: BTreeMap<Tool, Vec<String>>,
    /// Versión por defecto por herramienta.
    pub active: BTreeMap<Tool, String>,
    pub ports: Ports,
    pub projects: Vec<Project>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ports {
    pub http: u16,
    pub https: u16,
    pub mariadb: u16,
    /// Base para PHP FastCGI: cada versión usa `php_fcgi_base + minor_index`.
    pub php_fcgi_base: u16,
}

impl Default for Ports {
    fn default() -> Self {
        Self { http: 8080, https: 8443, mariadb: 3306, php_fcgi_base: 9000 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    /// Dominio, p. ej. `blog.localhost`.
    pub domain: String,
    pub docroot: PathBuf,
    /// Versión de PHP; `None` usa la activa.
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
        }
    }
}

impl State {
    /// Carga el estado; si el archivo no existe devuelve el estado por defecto.
    pub fn load(path: &Path) -> Result<Self> {
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|source| Error::Json { path: path.to_path_buf(), source }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).at(path),
        }
    }

    /// Registra una versión instalada; si no hay versión activa, pasa a serlo.
    pub fn mark_installed(&mut self, tool: Tool, version: &str) {
        let versions = self.installed.entry(tool).or_default();
        if !versions.iter().any(|v| v == version) {
            versions.push(version.to_string());
            versions.sort_by(|a, b| compare_versions(b, a));
        }
        self.active.entry(tool).or_insert_with(|| version.to_string());
    }

    /// Quita una versión; si era la activa, activa la más reciente que quede.
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

    /// Guarda de forma atómica (escribe a un temporal y renombra).
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
            php: Some("8.1.30".into()),
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
        assert_eq!(state.active[&Tool::Php], "8.1.30", "la primera instalada queda activa");

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
}
