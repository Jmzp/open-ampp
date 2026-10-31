//! Open AMPP engine. It does not depend on Tauri: both the desktop app and the CLI use it.

#[macro_use]
pub mod i18n;

pub mod catalog;
pub mod config;
pub mod db;
pub mod doctor;
pub mod error;
pub mod install;
pub mod installer;
pub mod paths;
pub mod ports;
pub mod projects;
pub mod services;
pub mod state;
pub mod tls;
pub mod tools;

pub use catalog::{Manifest, Package, Platform};
pub use error::{Error, Result};
pub use paths::{AppPaths, Tool};
pub use services::{ExtraPhp, MailStatus, ServiceStatus, Supervisor};
pub use state::State;
pub use tools::Extra;
