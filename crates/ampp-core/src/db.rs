//! MariaDB connection details and database management through the bundled `mariadb` client.

use std::fs;
use std::path::{Path, PathBuf};
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
    /// Empty unless one was set for the active series (MariaDB only listens on 127.0.0.1).
    pub password: String,
    /// Unix socket that `localhost` resolves to in PHP (macOS and Linux).
    pub socket: Option<PathBuf>,
}

pub fn connection(paths: &AppPaths, state: &State) -> Connection {
    Connection {
        host: "127.0.0.1".into(),
        port: state.ports.mariadb,
        user: "root".into(),
        password: root_password(paths, state),
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

/// Root password of the active MariaDB's data folder; empty by default.
pub fn root_password(paths: &AppPaths, state: &State) -> String {
    active(paths, state)
        .ok()
        .and_then(|inst| state.mariadb_root_passwords.get(&inst.minor_version()).cloned())
        .unwrap_or_default()
}

/// Option file with the root password for the MariaDB clients, so it never shows up in the
/// process list. Only the user can read it.
pub fn client_options_path(paths: &AppPaths) -> PathBuf {
    paths.root().join("config").join("mariadb").join("client.cnf")
}

/// Writes (or removes, without a password) the client option file and returns the arguments that
/// load it. They must come first on the command line.
pub fn client_options(paths: &AppPaths, state: &State) -> Result<Vec<String>> {
    let path = client_options_path(paths);
    let password = root_password(paths, state);
    if password.is_empty() {
        let _ = fs::remove_file(&path);
        return Ok(Vec::new());
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).at(dir)?;
    }
    let escaped = password.replace('\\', "\\\\").replace('"', "\\\"");
    fs::write(&path, format!("[client]\npassword=\"{escaped}\"\n")).at(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).at(&path)?;
    }
    Ok(defaults_arg(&path))
}

/// `--defaults-extra-file=…` for an existing option file (e.g. to shut the server down).
pub fn defaults_arg(path: &Path) -> Vec<String> {
    if path.exists() { vec![format!("--defaults-extra-file={}", path.display())] } else { Vec::new() }
}

/// A password MariaDB and the option file can hold: no control characters, at most 80 characters.
pub fn valid_password(password: &str) -> bool {
    password.chars().count() <= 80 && !password.chars().any(char::is_control)
}

/// Changes the password of every `root` account on the running server (empty removes it).
/// The caller then stores it in the state for the active series.
pub async fn set_root_password(paths: &AppPaths, state: &State, password: &str) -> Result<()> {
    if !valid_password(password) {
        return Err(Error::InvalidSetting(tr!(
            "la contraseña no puede tener saltos de línea ni pasar de 80 caracteres",
            "the password cannot contain line breaks or exceed 80 characters"
        )));
    }
    let accounts = query(paths, state, "SELECT CONCAT(QUOTE(User), '@', QUOTE(Host)) FROM mysql.user WHERE User = 'root'").await?;
    let literal = format!("'{}'", password.replace('\\', "\\\\").replace('\'', "\\'"));
    let sql: String = accounts
        .lines()
        .filter(|a| !a.trim().is_empty())
        .map(|account| format!("ALTER USER {account} IDENTIFIED BY {literal}; "))
        .collect();
    if sql.is_empty() {
        return Err(Error::InvalidSetting(tr!("no se encontró el usuario root", "the root user was not found")));
    }
    query(paths, state, &format!("{sql}FLUSH PRIVILEGES")).await.map(|_| ())
}

/// Runs a statement with the active MariaDB client as root and returns its tab-separated output.
async fn query(paths: &AppPaths, state: &State, sql: &str) -> Result<String> {
    let inst = active(paths, state)?;
    let client = inst.mariadb_client().ok_or_else(|| Error::NotInstalled { tool: Tool::Mariadb, version: inst.id.clone() })?;
    let port = state.ports.mariadb.to_string();
    let mut cmd = tokio::process::Command::new(&client);
    cmd.args(client_options(paths, state)?)
        .args(["--protocol=tcp", "-h", "127.0.0.1", "-P", &port, "-u", "root", "--skip-ssl-verify-server-cert"])
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
    fn the_password_belongs_to_the_series_and_lives_in_a_private_option_file() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path());
        let mut state = State::default();
        for v in ["11.4.5", "11.8.2"] {
            std::fs::create_dir_all(paths.bin(Tool::Mariadb, v)).unwrap();
            state.mark_installed(Tool::Mariadb, v);
        }
        state.set_active(Tool::Mariadb, "11.4.5").unwrap();
        assert!(client_options(&paths, &state).unwrap().is_empty(), "no password, no option file");
        assert!(!client_options_path(&paths).exists());

        state.mariadb_root_passwords.insert("11.4".into(), r#"se"cr\et"#.into());
        assert_eq!(connection(&paths, &state).password, r#"se"cr\et"#);
        let args = client_options(&paths, &state).unwrap();
        assert_eq!(args, [format!("--defaults-extra-file={}", client_options_path(&paths).display())]);
        let text = std::fs::read_to_string(client_options_path(&paths)).unwrap();
        assert_eq!(text, "[client]\npassword=\"se\\\"cr\\\\et\"\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(client_options_path(&paths)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        // Another series has its own data folder, still without a password.
        state.set_active(Tool::Mariadb, "11.8.2").unwrap();
        assert_eq!(connection(&paths, &state).password, "");
        assert!(client_options(&paths, &state).unwrap().is_empty());
        assert!(!client_options_path(&paths).exists(), "a stale password file is removed");
    }

    #[test]
    fn passwords_without_control_characters() {
        assert!(valid_password("") && valid_password("p@ss w'rd\"%"));
        assert!(!valid_password("a\nb") && !valid_password(&"x".repeat(81)));
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
