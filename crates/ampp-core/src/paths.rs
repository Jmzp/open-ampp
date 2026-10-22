use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, IoContext, Result};

/// Herramientas gestionadas por OPEN_AMPP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tool {
    Apache,
    Php,
    Mariadb,
}

impl Tool {
    pub const ALL: [Tool; 3] = [Tool::Apache, Tool::Php, Tool::Mariadb];

    pub fn as_str(self) -> &'static str {
        match self {
            Tool::Apache => "apache",
            Tool::Php => "php",
            Tool::Mariadb => "mariadb",
        }
    }
}

/// Layout de directorios en disco:
///
/// ```text
/// <root>/
/// ├── bin/<tool>/<version>/
/// ├── config/<tool>/<version>/
/// ├── data/mariadb/<major>/
/// ├── logs/  run/  certs/
/// └── state.json
/// ```
#[derive(Debug, Clone, Serialize)]
pub struct AppPaths {
    root: PathBuf,
}

impl AppPaths {
    /// Directorio de datos estándar del SO (p. ej. `~/Library/Application Support/dev.openampp.app`).
    pub fn from_system() -> Result<Self> {
        let dirs = directories::ProjectDirs::from("dev", "openampp", "app").ok_or(Error::NoDataDir)?;
        Ok(Self::new(dirs.data_dir()))
    }

    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn bin(&self, tool: Tool, version: &str) -> PathBuf {
        self.root.join("bin").join(tool.as_str()).join(version)
    }

    pub fn config(&self, tool: Tool, version: &str) -> PathBuf {
        self.root.join("config").join(tool.as_str()).join(version)
    }

    pub fn mariadb_data(&self, major: &str) -> PathBuf {
        self.root.join("data").join("mariadb").join(major)
    }

    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }

    pub fn run(&self) -> PathBuf {
        self.root.join("run")
    }

    pub fn certs(&self) -> PathBuf {
        self.root.join("certs")
    }

    pub fn state_file(&self) -> PathBuf {
        self.root.join("state.json")
    }

    /// Crea los directorios base si no existen.
    pub fn ensure(&self) -> Result<()> {
        let mut dirs = vec![self.logs(), self.run(), self.certs()];
        for tool in Tool::ALL {
            dirs.push(self.root.join("bin").join(tool.as_str()));
            dirs.push(self.root.join("config").join(tool.as_str()));
        }
        dirs.push(self.root.join("data").join("mariadb"));
        for dir in dirs {
            fs::create_dir_all(&dir).at(&dir)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_is_created_under_root() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path());
        paths.ensure().unwrap();

        assert!(paths.logs().is_dir());
        assert!(tmp.path().join("bin/php").is_dir());
        assert!(tmp.path().join("data/mariadb").is_dir());
        assert_eq!(paths.bin(Tool::Php, "8.3.12"), tmp.path().join("bin/php/8.3.12"));
    }
}
