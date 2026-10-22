use std::path::PathBuf;

use crate::paths::Tool;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no se pudo determinar el directorio de datos del usuario")]
    NoDataDir,
    #[error("error de E/S en {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("JSON inválido en {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("catálogo inválido: {0}")]
    Catalog(String),
    #[error("error de red descargando {url}: {source}")]
    Http {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("checksum inválido para {file}: se esperaba {expected}, se obtuvo {actual}")]
    Checksum { file: String, expected: String, actual: String },
    #[error("archivo comprimido inválido {path}: {reason}")]
    Archive { path: PathBuf, reason: String },
    #[error("{tool:?} {version} no está disponible para esta plataforma")]
    NotInCatalog { tool: Tool, version: String },
    #[error("{tool:?} {version} no está instalado")]
    NotInstalled { tool: Tool, version: String },
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
