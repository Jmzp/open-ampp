//! Catálogo de versiones descargables (`manifest.json`).

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::paths::Tool;

/// Versión del esquema del manifiesto que entiende esta build.
pub const SCHEMA_VERSION: u32 = 1;

/// Catálogo incluido en la app; se usa si no hay red o como punto de partida.
pub const BUNDLED_MANIFEST: &str = include_str!("../../../catalog/manifest.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    Windows,
    Macos,
    Linux,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Arch {
    X64,
    Arm64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Platform {
    pub os: Os,
    pub arch: Arch,
}

impl Platform {
    /// Plataforma en la que corre el proceso, si está soportada.
    pub fn current() -> Option<Self> {
        let os = match std::env::consts::OS {
            "windows" => Os::Windows,
            "macos" => Os::Macos,
            "linux" => Os::Linux,
            _ => return None,
        };
        let arch = match std::env::consts::ARCH {
            "x86_64" => Arch::X64,
            "aarch64" => Arch::Arm64,
            _ => return None,
        };
        Some(Self { os, arch })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArchiveKind {
    Zip,
    TarGz,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Package {
    pub tool: Tool,
    pub version: String,
    pub os: Os,
    pub arch: Arch,
    /// URL `https://` o `file://` (útil para catálogos locales y tests).
    pub url: String,
    /// SHA-256 en hexadecimal.
    pub sha256: String,
    pub archive: ArchiveKind,
    /// Tamaño en bytes, informativo para la UI.
    #[serde(default)]
    pub size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    pub packages: Vec<Package>,
}

impl Manifest {
    pub fn parse(json: &str) -> Result<Self> {
        let manifest: Self = serde_json::from_str(json).map_err(|e| Error::Catalog(e.to_string()))?;
        if manifest.schema != SCHEMA_VERSION {
            return Err(Error::Catalog(format!(
                "esquema {} no soportado (se esperaba {SCHEMA_VERSION})",
                manifest.schema
            )));
        }
        Ok(manifest)
    }

    pub fn bundled() -> Self {
        Self::parse(BUNDLED_MANIFEST).expect("el catálogo incluido debe ser válido")
    }

    /// Descarga el catálogo remoto.
    pub async fn fetch(url: &str) -> Result<Self> {
        let http = |source| Error::Http { url: url.to_string(), source };
        let body = reqwest::get(url)
            .await
            .and_then(|r| r.error_for_status())
            .map_err(http)?
            .text()
            .await
            .map_err(http)?;
        Self::parse(&body)
    }

    /// Paquetes de la plataforma, ordenados por herramienta y versión descendente.
    pub fn for_platform(&self, platform: Platform) -> Vec<&Package> {
        let mut pkgs: Vec<_> = self
            .packages
            .iter()
            .filter(|p| p.os == platform.os && p.arch == platform.arch)
            .collect();
        pkgs.sort_by(|a, b| a.tool.cmp(&b.tool).then_with(|| compare_versions(&b.version, &a.version)));
        pkgs
    }

    pub fn find(&self, tool: Tool, version: &str, platform: Platform) -> Result<&Package> {
        self.packages
            .iter()
            .find(|p| p.tool == tool && p.version == version && p.os == platform.os && p.arch == platform.arch)
            .ok_or_else(|| Error::NotInCatalog { tool, version: version.to_string() })
    }
}

/// Compara versiones tipo `8.3.12` numéricamente por componentes.
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let parts = |v: &str| v.split('.').map(|p| p.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    parts(a).cmp(&parts(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINUX_X64: Platform = Platform { os: Os::Linux, arch: Arch::X64 };

    fn pkg(tool: Tool, version: &str, os: Os) -> Package {
        Package {
            tool,
            version: version.into(),
            os,
            arch: Arch::X64,
            url: "https://example.com/x.tar.gz".into(),
            sha256: "00".into(),
            archive: ArchiveKind::TarGz,
            size: None,
        }
    }

    #[test]
    fn bundled_manifest_is_valid() {
        let m = Manifest::bundled();
        assert!(m.packages.iter().all(|p| p.sha256.len() == 64));
    }

    #[test]
    fn rejects_unknown_schema() {
        assert!(Manifest::parse(r#"{"schema":99,"packages":[]}"#).is_err());
    }

    #[test]
    fn filters_and_sorts_by_platform() {
        let m = Manifest {
            schema: 1,
            packages: vec![
                pkg(Tool::Php, "8.1.30", Os::Linux),
                pkg(Tool::Php, "8.3.12", Os::Linux),
                pkg(Tool::Php, "8.10.0", Os::Linux),
                pkg(Tool::Php, "8.4.0", Os::Windows),
                pkg(Tool::Apache, "2.4.62", Os::Linux),
            ],
        };
        let versions: Vec<_> = m.for_platform(LINUX_X64).iter().map(|p| p.version.as_str()).collect();
        assert_eq!(versions, ["2.4.62", "8.10.0", "8.3.12", "8.1.30"]);
        assert!(m.find(Tool::Php, "8.4.0", LINUX_X64).is_err());
    }
}
