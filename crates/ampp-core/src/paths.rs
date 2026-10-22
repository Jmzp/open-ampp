use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, IoContext, Result};

/// Tools managed by Open AMPP.
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

    /// Display name (`MariaDB`).
    pub fn label(self) -> &'static str {
        match self {
            Tool::Apache => "Apache",
            Tool::Php => "PHP",
            Tool::Mariadb => "MariaDB",
        }
    }
}

impl std::fmt::Display for Tool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// On-disk directory layout:
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
    /// The OS standard data directory (e.g. `~/Library/Application Support/dev.openampp.open-ampp`),
    /// or `OPEN_AMPP_HOME` when set (portable installs, tests).
    pub fn from_system() -> Result<Self> {
        if let Some(home) = std::env::var_os("OPEN_AMPP_HOME").filter(|h| !h.is_empty()) {
            return Ok(Self::new(home));
        }
        let dirs = directories::ProjectDirs::from("dev", "openampp", "open-ampp").ok_or(Error::NoDataDir)?;
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

    /// Default DocumentRoot (XAMPP's `htdocs`).
    pub fn www(&self) -> PathBuf {
        self.root.join("www")
    }

    /// Downloads in progress.
    pub fn downloads(&self) -> PathBuf {
        self.root.join("downloads")
    }

    pub fn certs(&self) -> PathBuf {
        self.root.join("certs")
    }

    pub fn state_file(&self) -> PathBuf {
        self.root.join("state.json")
    }

    /// Creates the base directories if missing.
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
    fn tools_have_names_and_labels() {
        assert_eq!(Tool::ALL.map(|t| t.to_string()), ["Apache", "PHP", "MariaDB"]);
        let paths = AppPaths::new("/srv/ampp");
        assert_eq!(paths.state_file(), Path::new("/srv/ampp/state.json"));
        assert_eq!(paths.mariadb_data("11.4"), Path::new("/srv/ampp/data/mariadb/11.4"));
    }

    #[test]
    fn open_ampp_home_overrides_the_data_dir() {
        // Only read here, so setting it does not race with other tests that use explicit paths.
        let tmp = tempfile::tempdir().unwrap();
        // SAFETY: no other test reads or writes OPEN_AMPP_HOME.
        unsafe { std::env::set_var("OPEN_AMPP_HOME", tmp.path()) };
        let paths = AppPaths::from_system().unwrap();
        unsafe { std::env::remove_var("OPEN_AMPP_HOME") };
        assert_eq!(paths.root(), tmp.path());
    }

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
