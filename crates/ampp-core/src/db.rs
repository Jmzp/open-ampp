//! MariaDB connection details and database management through the bundled `mariadb` client.

use std::path::PathBuf;
use std::process::Stdio;

use serde::Serialize;

use crate::config;
use crate::error::{Error, IoContext, Result};
use crate::install::{self, Installation};
use crate::paths::{AppPaths, Tool};
use crate::state::State;

/// Schemas MariaDB creates for itself.
const SYSTEM_SCHEMAS: &[&str] = &["information_schema", "mysql", "performance_schema", "sys"];

#[derive(Debug, Clone, Serialize)]
pub struct Connection {
    pub host: String,
    pub port: u16,
    pub user: String,
    /// Always empty: root has no password and MariaDB only listens on 127.0.0.1.
    pub password: String,
    /// Unix socket that `localhost` resolves to in PHP (macOS and Linux).
    pub socket: Option<PathBuf>,
}

pub fn connection(paths: &AppPaths, state: &State) -> Connection {
    Connection {
        host: "127.0.0.1".into(),
        port: state.ports.mariadb,
        user: "root".into(),
        password: String::new(),
        socket: (!cfg!(windows)).then(|| config::mariadb_socket(paths)),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Database {
    pub name: String,
    pub tables: u64,
    /// Data plus indexes, in bytes.
    pub size: u64,
    pub system: bool,
}

/// Databases on the running server, user ones first.
pub async fn list(paths: &AppPaths, state: &State) -> Result<Vec<Database>> {
    let out = query(
        paths,
        state,
        "SELECT s.schema_name, COUNT(t.table_name), COALESCE(SUM(t.data_length + t.index_length), 0) \
         FROM information_schema.schemata s \
         LEFT JOIN information_schema.tables t ON t.table_schema = s.schema_name \
         GROUP BY s.schema_name ORDER BY s.schema_name",
    )
    .await?;
    Ok(parse_databases(&out))
}

/// Parses `name<TAB>tables<TAB>bytes` rows, user databases first.
fn parse_databases(out: &str) -> Vec<Database> {
    let mut dbs: Vec<Database> = out
        .lines()
        .filter_map(|line| {
            let mut cols = line.split('\t');
            let name = cols.next()?.to_string();
            let tables = cols.next()?.parse().ok()?;
            let size = cols.next()?.parse().ok()?;
            let system = SYSTEM_SCHEMAS.contains(&name.as_str());
            Some(Database { name, tables, size, system })
        })
        .collect();
    dbs.sort_by(|a, b| a.system.cmp(&b.system).then_with(|| a.name.cmp(&b.name)));
    dbs
}

/// `true` for names that are safe to use unquoted-ish in SQL and as folder names.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Creates a database with utf8mb4, as XAMPP's phpMyAdmin suggests by default.
pub async fn create(paths: &AppPaths, state: &State, name: &str) -> Result<()> {
    if !valid_name(name) {
        return Err(Error::InvalidSetting(tr!(
            "nombre de base de datos inválido: usa letras, números, _ o -",
            "invalid database name: use letters, digits, _ or -"
        )));
    }
    let sql = format!("CREATE DATABASE `{name}` CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci");
    query(paths, state, &sql).await.map(|_| ())
}

/// Runs a statement with the active MariaDB client as root and returns its tab-separated output.
async fn query(paths: &AppPaths, state: &State, sql: &str) -> Result<String> {
    let inst = active(paths, state)?;
    let client = inst.mariadb_client().ok_or_else(|| Error::NotInstalled { tool: Tool::Mariadb, version: inst.id.clone() })?;
    let port = state.ports.mariadb.to_string();
    let mut cmd = tokio::process::Command::new(&client);
    cmd.args(["--protocol=tcp", "-h", "127.0.0.1", "-P", &port, "-u", "root", "--skip-ssl-verify-server-cert"])
        .args(["--batch", "--skip-column-names", "-e", sql])
        .stdin(Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let out = cmd.output().await.at(&client)?;
    if !out.status.success() {
        let reason = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(Error::ServiceFailed { tool: Tool::Mariadb, reason });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn active(paths: &AppPaths, state: &State) -> Result<Installation> {
    let id = state.active.get(&Tool::Mariadb).ok_or(Error::NoActiveVersion(Tool::Mariadb))?;
    install::locate(paths, Tool::Mariadb, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_names_are_restricted() {
        assert!(valid_name("blog_dev"));
        assert!(valid_name("shop-2"));
        assert!(!valid_name(""));
        assert!(!valid_name("x`; DROP DATABASE mysql; --"));
        assert!(!valid_name("con espacio"));
        assert!(valid_name(&"a".repeat(64)));
        assert!(!valid_name(&"a".repeat(65)), "MariaDB limits names to 64 characters");
    }

    #[test]
    fn parses_databases_user_ones_first() {
        let out = "information_schema\t79\t0\nmysql\t31\t2457600\nshop\t12\t98304\nblog\t0\t0\nbroken line\n";
        let dbs = parse_databases(out);
        let rows: Vec<(&str, u64, u64, bool)> = dbs.iter().map(|d| (d.name.as_str(), d.tables, d.size, d.system)).collect();
        assert_eq!(
            rows,
            [
                ("blog", 0, 0, false),
                ("shop", 12, 98304, false),
                ("information_schema", 79, 0, true),
                ("mysql", 31, 2457600, true),
            ]
        );
    }

    #[test]
    fn connection_is_local_root_without_password() {
        let paths = AppPaths::new("/srv/ampp");
        let mut state = State::default();
        state.ports.mariadb = 3307;
        let conn = connection(&paths, &state);
        assert_eq!((conn.host.as_str(), conn.port, conn.user.as_str(), conn.password.as_str()), ("127.0.0.1", 3307, "root", ""));
        assert_eq!(conn.socket.is_some(), !cfg!(windows), "PHP's `localhost` uses the socket on Unix only");
    }

    #[tokio::test]
    async fn create_rejects_bad_names_before_touching_mariadb() {
        let err = create(&AppPaths::new("/nonexistent"), &State::default(), "x`; DROP").await.unwrap_err();
        assert!(matches!(err, Error::InvalidSetting(_)));
    }

    #[tokio::test]
    async fn queries_need_an_active_installed_mariadb() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path());
        let mut state = State::default();
        assert!(matches!(list(&paths, &state).await, Err(Error::NoActiveVersion(Tool::Mariadb))));
        state.mark_installed(Tool::Mariadb, "11.4.5");
        assert!(matches!(list(&paths, &state).await, Err(Error::NotInstalled { tool: Tool::Mariadb, .. })));
        // Installed but without the client binary.
        std::fs::create_dir_all(paths.bin(Tool::Mariadb, "11.4.5")).unwrap();
        assert!(matches!(create(&paths, &state, "shop").await, Err(Error::NotInstalled { tool: Tool::Mariadb, .. })));
    }
}
