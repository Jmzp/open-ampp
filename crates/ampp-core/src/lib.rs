//! Motor de OPEN_AMPP. No depende de Tauri: lo usan la app de escritorio y el CLI.

pub mod catalog;
pub mod error;
pub mod installer;
pub mod paths;
pub mod state;

pub use catalog::{Manifest, Package, Platform};
pub use error::{Error, Result};
pub use paths::{AppPaths, Tool};
pub use state::State;
