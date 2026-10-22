//! Localiza los ejecutables de una versión instalada, ya sea gestionada por
//! OPEN_AMPP (`bin/<tool>/<version>`) o del sistema (Homebrew en macOS).

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{Error, Result};
use crate::paths::{AppPaths, Tool};

/// Identificador de versión para las instalaciones de Homebrew.
pub const HOMEBREW: &str = "homebrew";

#[derive(Debug, Clone, Serialize)]
pub struct Installation {
    pub tool: Tool,
    /// Identificador guardado en el estado (`8.3.12` o `homebrew`).
    pub id: String,
    /// Versión real del software.
    pub version: String,
    /// Raíz de la instalación (prefix).
    pub root: PathBuf,
    pub system: bool,
}

impl Installation {
    /// `major.minor`, p. ej. `11.4`.
    pub fn minor_version(&self) -> String {
        self.version.split('.').take(2).collect::<Vec<_>>().join(".")
    }

    pub fn httpd(&self) -> Option<PathBuf> {
        first_existing(&[self.root.join("bin").join(exe("httpd"))])
    }

    pub fn apache_modules(&self) -> Option<PathBuf> {
        first_existing(&[self.root.join("modules"), self.root.join("lib/httpd/modules")])
    }

    /// `php-fpm` en Unix, `php-cgi.exe` en Windows.
    pub fn php_fastcgi(&self) -> Option<PathBuf> {
        if cfg!(windows) {
            first_existing(&[self.root.join("php-cgi.exe")])
        } else {
            first_existing(&[self.root.join("php-fpm"), self.root.join("sbin/php-fpm")])
        }
    }

    pub fn php_extension_dir(&self) -> Option<PathBuf> {
        first_existing(&[self.root.join("ext")])
    }

    pub fn mariadbd(&self) -> Option<PathBuf> {
        first_existing(&[self.root.join("bin").join(exe("mariadbd")), self.root.join("sbin").join(exe("mariadbd"))])
    }

    pub fn mariadb_install_db(&self) -> Option<PathBuf> {
        first_existing(&[
            self.root.join("scripts/mariadb-install-db"),
            self.root.join("bin").join(exe("mariadb-install-db")),
        ])
    }

    pub fn mariadb_admin(&self) -> Option<PathBuf> {
        first_existing(&[self.root.join("bin").join(exe("mariadb-admin"))])
    }
}

/// Resuelve la instalación de `tool` con identificador `id`.
pub fn locate(paths: &AppPaths, tool: Tool, id: &str) -> Result<Installation> {
    let not_installed = || Error::NotInstalled { tool, version: id.to_string() };
    if id == HOMEBREW {
        return homebrew(tool).ok_or_else(not_installed);
    }
    let mut root = paths.bin(tool, id);
    if !root.is_dir() {
        return Err(not_installed());
    }
    // El zip de Apache Lounge trae `Apache24/` junto a archivos sueltos (ReadMe, etc.).
    if tool == Tool::Apache && root.join("Apache24").is_dir() {
        root = root.join("Apache24");
    }
    Ok(Installation { tool, id: id.to_string(), version: id.to_string(), root, system: false })
}

/// Detecta la fórmula de Homebrew correspondiente (solo macOS).
pub fn homebrew(tool: Tool) -> Option<Installation> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let formula = match tool {
        Tool::Apache => "httpd",
        Tool::Php => "php",
        Tool::Mariadb => "mariadb",
    };
    ["/opt/homebrew/opt", "/usr/local/opt"].iter().find_map(|base| {
        let link = Path::new(base).join(formula);
        // `opt/<formula>` es un symlink a `Cellar/<formula>/<versión>`.
        let version = std::fs::read_link(&link).ok()?.file_name()?.to_string_lossy().into_owned();
        let inst = Installation { tool, id: HOMEBREW.into(), version, root: link, system: true };
        let usable = match tool {
            Tool::Apache => inst.httpd().is_some(),
            Tool::Php => inst.php_fastcgi().is_some(),
            Tool::Mariadb => inst.mariadbd().is_some(),
        };
        usable.then_some(inst)
    })
}

/// Añade la extensión `.exe` en Windows.
pub fn exe(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

fn first_existing(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find(|p| p.exists()).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locates_managed_install_and_unwraps_apache24() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path());
        let apache = paths.bin(Tool::Apache, "2.4.69");
        std::fs::create_dir_all(apache.join("Apache24/bin")).unwrap();
        std::fs::write(apache.join("Apache24/bin").join(exe("httpd")), "").unwrap();

        let inst = locate(&paths, Tool::Apache, "2.4.69").unwrap();
        assert_eq!(inst.root, apache.join("Apache24"));
        assert!(inst.httpd().is_some());
        assert!(locate(&paths, Tool::Php, "8.3.0").is_err());
    }

    #[test]
    fn minor_version() {
        let inst = Installation {
            tool: Tool::Mariadb,
            id: "11.4.13".into(),
            version: "11.4.13".into(),
            root: PathBuf::new(),
            system: false,
        };
        assert_eq!(inst.minor_version(), "11.4");
    }
}
