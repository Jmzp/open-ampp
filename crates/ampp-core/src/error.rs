use std::path::PathBuf;

use crate::paths::Tool;

#[derive(Debug)]
pub enum Error {
    NoDataDir,
    Io { path: PathBuf, source: std::io::Error },
    Json { path: PathBuf, source: serde_json::Error },
    Catalog(String),
    Http { url: String, source: reqwest::Error },
    Checksum { file: String, expected: String, actual: String },
    Archive { path: PathBuf, reason: String },
    NotInCatalog { tool: Tool, version: String },
    NotInstalled { tool: Tool, version: String },
    NoActiveVersion(Tool),
    PortInUse { tool: Tool, port: u16 },
    ServiceFailed { tool: Tool, reason: String },
    InvalidSetting(String),
    InvalidProject(String),
    Tls(String),
    ExtraNotInCatalog(String),
    SystemInstall,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            Error::NoDataDir => tr!(
                "no se pudo determinar el directorio de datos del usuario",
                "could not determine the user's data directory"
            ),
            Error::Io { path, source } => {
                let path = path.display();
                tr!("error de E/S en {path}: {source}", "I/O error at {path}: {source}")
            }
            Error::Json { path, source } => {
                let path = path.display();
                tr!("JSON inválido en {path}: {source}", "invalid JSON in {path}: {source}")
            }
            Error::Catalog(reason) => tr!("catálogo inválido: {reason}", "invalid catalog: {reason}"),
            Error::Http { url, source } => {
                tr!("error de red descargando {url}: {source}", "network error downloading {url}: {source}")
            }
            Error::Checksum { file, expected, actual } => tr!(
                "checksum inválido para {file}: se esperaba {expected}, se obtuvo {actual}",
                "checksum mismatch for {file}: expected {expected}, got {actual}"
            ),
            Error::Archive { path, reason } => {
                let path = path.display();
                tr!("archivo comprimido inválido {path}: {reason}", "invalid archive {path}: {reason}")
            }
            Error::NotInCatalog { tool, version } => tr!(
                "{tool} {version} no está disponible para esta plataforma",
                "{tool} {version} is not available for this platform"
            ),
            Error::NotInstalled { tool, version } => {
                tr!("{tool} {version} no está instalado", "{tool} {version} is not installed")
            }
            Error::NoActiveVersion(tool) => tr!(
                "no hay una versión activa de {tool}; instala una en Versiones",
                "there is no active {tool} version; install one in Versions"
            ),
            Error::PortInUse { tool, port } => tr!(
                "el puerto {port} de {tool} ya está en uso; cierra lo que lo ocupa o cámbialo en Herramientas → Puertos",
                "port {port} for {tool} is already in use; stop whatever is using it or change it in Tools → Ports"
            ),
            Error::ServiceFailed { tool, reason } => {
                tr!("{tool} no pudo iniciarse: {reason}", "{tool} could not start: {reason}")
            }
            Error::InvalidSetting(what) => tr!("ajuste inválido: {what}", "invalid setting: {what}"),
            Error::InvalidProject(why) => tr!("proyecto inválido: {why}", "invalid project: {why}"),
            Error::Tls(why) => tr!("HTTPS: {why}", "HTTPS: {why}"),
            Error::ExtraNotInCatalog(name) => {
                tr!("{name} no está disponible en el catálogo", "{name} is not available in the catalog")
            }
            Error::SystemInstall => tr!(
                "las instalaciones del sistema (Homebrew) se gestionan fuera de Open AMPP",
                "system installs (Homebrew) are managed outside Open AMPP"
            ),
        };
        f.write_str(&msg)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            Error::Json { source, .. } => Some(source),
            Error::Http { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) trait IoContext<T> {
    fn at(self, path: impl Into<PathBuf>) -> Result<T>;
}

impl<T> IoContext<T> for std::io::Result<T> {
    fn at(self, path: impl Into<PathBuf>) -> Result<T> {
        self.map_err(|source| Error::Io { path: path.into(), source })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_carry_the_details() {
        let io = || std::io::Error::new(std::io::ErrorKind::NotFound, "gone");
        let cases = [
            (Error::Io { path: "/data/state.json".into(), source: io() }, vec!["/data/state.json", "gone"]),
            (
                Error::Json { path: "/data/state.json".into(), source: serde_json::from_str::<u8>("x").unwrap_err() },
                vec!["/data/state.json"],
            ),
            (Error::Catalog("no packages".into()), vec!["no packages"]),
            (
                Error::Checksum { file: "php.zip".into(), expected: "aa".into(), actual: "bb".into() },
                vec!["php.zip", "aa", "bb"],
            ),
            (Error::Archive { path: "/tmp/x.zip".into(), reason: "truncated".into() }, vec!["/tmp/x.zip", "truncated"]),
            (Error::NotInCatalog { tool: Tool::Php, version: "7.4".into() }, vec!["PHP", "7.4"]),
            (Error::NotInstalled { tool: Tool::Mariadb, version: "11.4.5".into() }, vec!["MariaDB", "11.4.5"]),
            (Error::NoActiveVersion(Tool::Apache), vec!["Apache"]),
            (Error::PortInUse { tool: Tool::Apache, port: 8080 }, vec!["Apache", "8080"]),
            (Error::ServiceFailed { tool: Tool::Php, reason: "boom".into() }, vec!["PHP", "boom"]),
            (Error::InvalidSetting("memory_limit".into()), vec!["memory_limit"]),
            (Error::InvalidProject("blog".into()), vec!["blog"]),
            (Error::Tls("bad key".into()), vec!["HTTPS", "bad key"]),
            (Error::ExtraNotInCatalog("composer".into()), vec!["composer"]),
            (Error::NoDataDir, vec![]),
            (Error::SystemInstall, vec!["Homebrew"]),
        ];
        for (err, parts) in cases {
            let msg = err.to_string();
            assert!(!msg.is_empty());
            for part in parts {
                assert!(msg.contains(part), "{msg:?} should mention {part:?}");
            }
        }
    }

    #[test]
    fn io_errors_keep_their_path_and_source() {
        let err = std::fs::read("/definitely/not/here").at("/definitely/not/here").unwrap_err();
        assert!(matches!(&err, Error::Io { path, .. } if path == std::path::Path::new("/definitely/not/here")));
        assert!(std::error::Error::source(&err).is_some());
        assert!(std::error::Error::source(&Error::NoDataDir).is_none());
    }
}
