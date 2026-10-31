//! Catalog of downloadable versions (`manifest.json`).

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::paths::Tool;
use crate::tools::Extra;

/// Manifest schema version this build understands.
pub const SCHEMA_VERSION: u32 = 1;

/// Published catalog: updated whenever new builds or security releases land, without waiting
/// for a new app release.
pub const CATALOG_URL: &str = "https://raw.githubusercontent.com/Jmzp/open-ampp/main/catalog/manifest.json";

/// Catalog bundled with the app, used offline.
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
    /// Platform the process runs on, if supported.
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
    /// `https://` or `file://` URL (handy for local catalogs and tests).
    pub url: String,
    /// SHA-256 in hex.
    pub sha256: String,
    pub archive: ArchiveKind,
    /// Size in bytes, informational for the UI.
    #[serde(default)]
    pub size: Option<u64>,
}

/// Helper tool (phpMyAdmin, Composer, Mailpit): a single file or an archive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtraFile {
    pub name: Extra,
    pub version: String,
    pub url: String,
    pub sha256: String,
    /// Set when the download is an archive to extract (phpMyAdmin); `None` for a single file.
    #[serde(default)]
    pub archive: Option<ArchiveKind>,
    #[serde(default)]
    pub size: Option<u64>,
    /// Native builds only (Mailpit): the platform the file is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<Os>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch: Option<Arch>,
}

impl ExtraFile {
    fn runs_on(&self, platform: Option<Platform>) -> bool {
        match (self.os, self.arch) {
            (None, None) => true,
            (os, arch) => platform.is_some_and(|p| os.is_none_or(|o| o == p.os) && arch.is_none_or(|a| a == p.arch)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    pub packages: Vec<Package>,
    /// Platform-independent tools.
    #[serde(default, deserialize_with = "known_extras")]
    pub extras: Vec<ExtraFile>,
    /// Tools with one build per platform. A key of its own because builds up to 0.2.0 fail to
    /// read the whole catalog on an unknown name in `extras`; this one they simply ignore.
    #[serde(default, deserialize_with = "known_extras")]
    pub native_extras: Vec<ExtraFile>,
}

/// Skips tools this build does not know, so adding one to the catalog never breaks older apps.
fn known_extras<'de, D>(d: D) -> std::result::Result<Vec<ExtraFile>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Vec::<serde_json::Value>::deserialize(d)?;
    Ok(raw.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect())
}

impl Manifest {
    pub fn parse(json: &str) -> Result<Self> {
        let manifest: Self = serde_json::from_str(json).map_err(|e| Error::Catalog(e.to_string()))?;
        if manifest.schema != SCHEMA_VERSION {
            let schema = manifest.schema;
            return Err(Error::Catalog(tr!(
                "esquema {schema} no soportado (se esperaba {SCHEMA_VERSION})",
                "unsupported schema {schema} (expected {SCHEMA_VERSION})"
            )));
        }
        Ok(manifest)
    }

    pub fn bundled() -> Self {
        Self::parse(BUNDLED_MANIFEST).expect("the bundled catalog must be valid")
    }

    /// The published catalog if it answers within a few seconds, otherwise the bundled one.
    pub async fn latest() -> Self {
        match tokio::time::timeout(std::time::Duration::from_secs(5), Self::fetch(CATALOG_URL)).await {
            Ok(Ok(manifest)) => manifest,
            _ => Self::bundled(),
        }
    }

    /// A newer patch release in the same minor series as `version` (8.4.23 → 8.4.27), if any.
    pub fn patch_update(&self, tool: Tool, version: &str, platform: Platform) -> Option<&Package> {
        let minor = |v: &str| v.split('.').take(2).collect::<Vec<_>>().join(".");
        if !version.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return None; // e.g. "homebrew"
        }
        self.for_platform(platform)
            .into_iter()
            .filter(|p| p.tool == tool && minor(&p.version) == minor(version))
            .filter(|p| compare_versions(&p.version, version).is_gt())
            .max_by(|a, b| compare_versions(&a.version, &b.version))
    }

    /// Downloads a remote catalog.
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

    /// Packages for the platform, sorted by tool and descending version.
    pub fn for_platform(&self, platform: Platform) -> Vec<&Package> {
        let mut pkgs: Vec<_> = self
            .packages
            .iter()
            .filter(|p| p.os == platform.os && p.arch == platform.arch)
            .collect();
        pkgs.sort_by(|a, b| a.tool.cmp(&b.tool).then_with(|| compare_versions(&b.version, &a.version)));
        pkgs
    }

    /// The tool's file for the platform this process runs on.
    pub fn extra(&self, name: Extra) -> Option<&ExtraFile> {
        self.extra_for(name, Platform::current())
    }

    pub fn extra_for(&self, name: Extra, platform: Option<Platform>) -> Option<&ExtraFile> {
        self.extras.iter().chain(&self.native_extras).find(|e| e.name == name && e.runs_on(platform))
    }

    pub fn find(&self, tool: Tool, version: &str, platform: Platform) -> Result<&Package> {
        self.packages
            .iter()
            .find(|p| p.tool == tool && p.version == version && p.os == platform.os && p.arch == platform.arch)
            .ok_or_else(|| Error::NotInCatalog { tool, version: version.to_string() })
    }
}

/// Compares `8.3.12`-style versions numerically, component by component.
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
    fn native_extras_match_the_platform_and_unknown_tools_are_skipped() {
        let json = r#"{"schema":1,"packages":[],
            "extras":[{"name":"composer","version":"2.8.0","url":"u","sha256":"00"},
                      {"name":"adminer","version":"5.0","url":"u","sha256":"00"}],
            "native_extras":[
                {"name":"mailpit","version":"1.32.0","url":"linux","sha256":"00","archive":"tar-gz","os":"linux","arch":"x64"},
                {"name":"mailpit","version":"1.32.0","url":"mac","sha256":"00","archive":"tar-gz","os":"macos","arch":"arm64"},
                {"name":"futuretool","version":"1","url":"u","sha256":"00","os":"linux","arch":"x64"}]}"#;
        let m = Manifest::parse(json).unwrap();
        assert_eq!(m.extras.len(), 1, "an unknown tool must not break the catalog");
        assert_eq!(m.native_extras.len(), 2);
        let mac = Platform { os: Os::Macos, arch: Arch::Arm64 };
        assert_eq!(m.extra_for(Extra::Mailpit, Some(LINUX_X64)).unwrap().url, "linux");
        assert_eq!(m.extra_for(Extra::Mailpit, Some(mac)).unwrap().url, "mac");
        assert!(m.extra_for(Extra::Mailpit, Some(Platform { os: Os::Windows, arch: Arch::X64 })).is_none());
        assert!(m.extra_for(Extra::Mailpit, None).is_none());
        assert!(m.extra_for(Extra::Composer, None).is_some(), "platform-independent tools run anywhere");
    }

    #[test]
    fn old_builds_still_read_a_catalog_with_native_extras() {
        // What 0.2.0 sees: it has no `native_extras` field, and serde ignores unknown keys.
        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct OldManifest {
            schema: u32,
            packages: Vec<Package>,
            #[serde(default)]
            extras: Vec<ExtraFile>,
        }
        assert!(serde_json::from_str::<OldManifest>(BUNDLED_MANIFEST).is_ok());
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
            extras: Vec::new(),
            native_extras: Vec::new(),
        };
        let versions: Vec<_> = m.for_platform(LINUX_X64).iter().map(|p| p.version.as_str()).collect();
        assert_eq!(versions, ["2.4.62", "8.10.0", "8.3.12", "8.1.30"]);
        assert!(m.find(Tool::Php, "8.4.0", LINUX_X64).is_err());
    }

    #[test]
    fn patch_updates_stay_in_the_same_minor() {
        let m = Manifest {
            schema: 1,
            packages: vec![
                pkg(Tool::Php, "8.3.10", Os::Linux),
                pkg(Tool::Php, "8.3.12", Os::Linux),
                pkg(Tool::Php, "8.4.1", Os::Linux),
            ],
            extras: Vec::new(),
            native_extras: Vec::new(),
        };
        let update = |v| m.patch_update(Tool::Php, v, LINUX_X64).map(|p| p.version.clone());
        assert_eq!(update("8.3.9").as_deref(), Some("8.3.12"));
        assert_eq!(update("8.3.12"), None);
        assert_eq!(update("8.4.1"), None);
        assert_eq!(update("homebrew"), None);
    }
}
