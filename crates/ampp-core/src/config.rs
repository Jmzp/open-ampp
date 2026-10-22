//! Generates each service's configuration files from `templates/`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{IoContext, Result};
use crate::install::Installation;
use crate::paths::AppPaths;
use crate::ports;
use crate::state::{PhpSettings, State};

const HTTPD_CONF: &str = include_str!("../../../templates/httpd.conf");
const PHP_INI: &str = include_str!("../../../templates/php.ini");
const PHP_FPM_CONF: &str = include_str!("../../../templates/php-fpm.conf");
const MY_CNF: &str = include_str!("../../../templates/my.cnf");
/// Mozilla root certificates as published by curl (curl.se/ca); XAMPP ships its own bundle too.
const CA_BUNDLE: &str = include_str!("../../../templates/cacert.pem");

/// Apache modules loaded when present (built-in ones such as mpm_winnt have no `.so`).
const APACHE_MODULES: &[&str] = &[
    "mpm_event",
    "unixd",
    "authz_core",
    "authz_host",
    "alias",
    "autoindex",
    "dir",
    "env",
    "headers",
    "log_config",
    "mime",
    "proxy",
    "proxy_fcgi",
    "rewrite",
    "setenvif",
];

/// PHP extensions enabled by default on Windows: the same as XAMPP, in its order
/// (`exif` after `mbstring`).
pub const PHP_WINDOWS_EXTENSIONS: &[&str] = &[
    "bz2", "curl", "fileinfo", "gettext", "mbstring", "exif", "mysqli", "pdo_mysql", "pdo_sqlite", "openssl", "ftp",
];

const WELCOME_PHP: &str = r#"<!doctype html>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Open AMPP</title>
<style>
  body { font-family: system-ui, sans-serif; max-width: 640px; margin: 64px auto; padding: 0 16px; line-height: 1.55; color: #1b1f24; }
  h1 { background: linear-gradient(135deg, #4f46e5, #7c3aed, #db2777); -webkit-background-clip: text; color: transparent; }
  code { background: #f1f2f6; padding: 2px 6px; border-radius: 6px; }
  a { color: #4f46e5; }
</style>
<h1>{{heading}}</h1>
<p>{{serving}}</p>
<ul>
  <li><a href="info.php">phpinfo()</a></li>
  <li><a href="/phpmyadmin/">phpMyAdmin</a></li>
</ul>
<p>{{hint}}</p>
"#;

/// Replaces `{{key}}` with its value.
pub fn render(template: &str, vars: &[(&str, &str)]) -> String {
    vars.iter().fold(template.to_string(), |acc, (k, v)| acc.replace(&format!("{{{{{k}}}}}"), v))
}

/// Path with `/` separators (Apache and PHP accept it on Windows and it avoids escaping).
pub fn slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn write(path: &Path, contents: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).at(dir)?;
    }
    fs::write(path, contents).at(path)
}

/// Creates the default docroot (`www/`) with a welcome page if it does not exist.
pub fn ensure_docroot(paths: &AppPaths) -> Result<PathBuf> {
    let www = paths.www();
    if !www.exists() {
        let serving = tr!(
            "PHP <?= PHP_VERSION ?> atendiendo desde <code><?= htmlspecialchars(__DIR__) ?></code>.",
            "PHP <?= PHP_VERSION ?> serving from <code><?= htmlspecialchars(__DIR__) ?></code>."
        );
        let hint = tr!(
            "Reemplaza este archivo con tu proyecto. phpMyAdmin se instala desde Herramientas.",
            "Replace this file with your project. phpMyAdmin can be installed from Tools."
        );
        let page = render(
            WELCOME_PHP,
            &[("heading", &tr!("Open AMPP funciona", "Open AMPP is running")), ("serving", &serving), ("hint", &hint)],
        );
        write(&www.join("index.php"), &page)?;
        write(&www.join("info.php"), "<?php phpinfo();\n")?;
    }
    Ok(www)
}

pub struct PhpFiles {
    pub ini: PathBuf,
    /// Unix only (php-fpm).
    pub fpm_conf: Option<PathBuf>,
}

/// Extensions that can be toggled: the DLLs in `ext/` (Windows only).
/// On macOS and Linux the extensions are compiled into the binary.
pub fn php_optional_extensions(inst: &Installation) -> Vec<String> {
    if !cfg!(windows) {
        return Vec::new();
    }
    let Some(ext) = inst.php_extension_dir() else {
        return Vec::new();
    };
    let mut names: Vec<String> = fs::read_dir(ext)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            Some(name.strip_prefix("php_")?.strip_suffix(".dll")?.to_string())
        })
        .collect();
    names.sort();
    names
}

/// Enabled extensions: the user's choice or, if there is none, the defaults.
pub fn php_enabled_extensions(inst: &Installation, settings: &PhpSettings) -> Vec<String> {
    let available = php_optional_extensions(inst);
    let mut enabled: Vec<String> = match &settings.extensions {
        Some(chosen) => chosen.iter().filter(|e| available.contains(e)).cloned().collect(),
        None => PHP_WINDOWS_EXTENSIONS.iter().map(|e| e.to_string()).filter(|e| available.contains(e)).collect(),
    };
    // XAMPP's first, in its order, then the rest alphabetically.
    let rank = |e: &String| PHP_WINDOWS_EXTENSIONS.iter().position(|d| d == e).unwrap_or(usize::MAX);
    enabled.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.cmp(b)));
    enabled
}

/// Default value of a directive in the base `php.ini` (its last active definition).
pub fn php_default(key: &str) -> Option<String> {
    if key == "date.timezone" {
        return Some(system_timezone());
    }
    PHP_INI
        .lines()
        .filter_map(|l| l.split_once('='))
        .filter(|(k, _)| k.trim() == key)
        .map(|(_, v)| v.split(';').next().unwrap_or("").trim().trim_matches('"').to_string())
        .last()
}

/// System IANA time zone (`America/Bogota`); UTC if it cannot be determined.
fn system_timezone() -> String {
    iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".into())
}

/// User settings appended to the end of `php.ini` (the last definition wins).
fn php_user_section(settings: &PhpSettings) -> String {
    let mut out = String::new();
    if !settings.directives.is_empty() {
        out.push_str("\n; --- Open AMPP settings ---\n");
        for (key, value) in &settings.directives {
            out.push_str(&format!("{key} = {value}\n"));
        }
    }
    if !settings.custom.trim().is_empty() {
        out.push_str("\n; --- Custom php.ini ---\n");
        out.push_str(settings.custom.trim_end());
        out.push('\n');
    }
    out
}

pub fn write_php(paths: &AppPaths, inst: &Installation, state: &State) -> Result<PhpFiles> {
    let settings = &state.php;
    let port = ports::php_fcgi_port(state.ports.php_fcgi_base, &inst.version);
    let dir = paths.config(inst.tool, &inst.id);
    let log = slash(&paths.logs().join("php.log"));
    let tmp = paths.root().join("tmp");
    fs::create_dir_all(&tmp).at(&tmp)?;
    let ca_file = paths.certs().join("cacert.pem");
    if fs::read_to_string(&ca_file).ok().as_deref() != Some(CA_BUNDLE) {
        write(&ca_file, CA_BUNDLE)?;
    }
    // On Unix, `localhost` in pdo_mysql/mysqli goes through the Open AMPP MariaDB socket.
    let socket = if cfg!(windows) { String::new() } else { slash(&mariadb_socket(paths)) };

    let (ext_dir, extensions) = match inst.php_extension_dir() {
        Some(ext) if cfg!(windows) => {
            let enabled: Vec<String> = php_enabled_extensions(inst, settings)
                .into_iter()
                .map(|name| {
                    let directive = if name == "opcache" { "zend_extension" } else { "extension" };
                    format!("{directive}={name}")
                })
                .collect();
            (format!("extension_dir = \"{}\"", slash(&ext)), enabled.join("\n"))
        }
        _ => (String::new(), String::new()),
    };
    let ini = dir.join("php.ini");
    let base = render(
        PHP_INI,
        &[
            ("error_log", &log),
            ("extension_dir", &ext_dir),
            ("extensions", &extensions),
            ("tmp_dir", &slash(&tmp)),
            ("ca_file", &slash(&ca_file)),
            ("timezone", &system_timezone()),
            ("mysql_socket", &socket),
            ("mysql_port", &state.ports.mariadb.to_string()),
        ],
    );
    write(&ini, &(base + &php_user_section(settings)))?;

    let fpm_conf = if cfg!(windows) {
        None
    } else {
        let conf = dir.join("php-fpm.conf");
        let pid = slash(&paths.run().join(format!("php-fpm-{}.pid", inst.id)));
        write(&conf, &render(PHP_FPM_CONF, &[("pid_file", &pid), ("error_log", &log), ("port", &port.to_string())]))?;
        Some(conf)
    };
    Ok(PhpFiles { ini, fpm_conf })
}

/// Modules PHP loads with the Open AMPP configuration (`php -m`), lowercased.
pub async fn php_loaded_modules(paths: &AppPaths, inst: &Installation, state: &State) -> Result<Vec<String>> {
    let ini = write_php(paths, inst, state)?.ini;
    let Some(cli) = inst.php_cli() else {
        return Ok(Vec::new());
    };
    let mut cmd = tokio::process::Command::new(&cli);
    cmd.arg("-c").arg(&ini).arg("-m").stdin(std::process::Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let out = cmd.output().await.at(&cli)?;
    let mut modules: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().to_lowercase())
        .filter(|l| !l.is_empty() && !l.starts_with('['))
        .collect();
    modules.sort();
    modules.dedup();
    Ok(modules)
}

/// `extra_conf` is appended to `httpd.conf` (e.g. the phpMyAdmin alias).
pub fn write_apache(
    paths: &AppPaths,
    inst: &Installation,
    http_port: u16,
    php_port: u16,
    docroot: &Path,
    extra_conf: &str,
) -> Result<PathBuf> {
    let modules_dir = inst.apache_modules();
    let load_modules = APACHE_MODULES
        .iter()
        .filter_map(|m| {
            let so = modules_dir.as_ref()?.join(format!("mod_{m}.so"));
            so.exists().then(|| format!("LoadModule {m}_module \"{}\"", slash(&so)))
        })
        .collect::<Vec<_>>()
        .join("\n");

    let conf = paths.config(inst.tool, &inst.id).join("httpd.conf");
    write(
        &conf,
        &render(
            HTTPD_CONF,
            &[
                ("server_root", &slash(&inst.root)),
                ("http_port", &http_port.to_string()),
                ("php_port", &php_port.to_string()),
                ("run_dir", &slash(&paths.run())),
                ("log_dir", &slash(&paths.logs())),
                ("docroot", &slash(docroot)),
                ("load_modules", &load_modules),
                ("extra_conf", extra_conf),
            ],
        ),
    )?;
    Ok(conf)
}

/// MariaDB Unix socket path. macOS limits socket paths to 104 bytes, so the temp directory
/// is used when the user's path is too long.
pub fn mariadb_socket(paths: &AppPaths) -> PathBuf {
    let sock = paths.run().join("mariadb.sock");
    if sock.as_os_str().len() < 100 { sock } else { std::env::temp_dir().join("open-ampp-mariadb.sock") }
}

pub fn write_mariadb(paths: &AppPaths, inst: &Installation, port: u16, datadir: &Path) -> Result<PathBuf> {
    let socket_line =
        if cfg!(windows) { String::new() } else { format!("socket=\"{}\"", slash(&mariadb_socket(paths))) };
    let conf = paths.config(inst.tool, &inst.id).join("my.cnf");
    write(
        &conf,
        &render(
            MY_CNF,
            &[
                ("basedir", &slash(&inst.root)),
                ("datadir", &slash(datadir)),
                ("port", &port.to_string()),
                ("socket_line", &socket_line),
                ("run_dir", &slash(&paths.run())),
                ("log_dir", &slash(&paths.logs())),
            ],
        ),
    )?;
    Ok(conf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Tool;

    #[test]
    fn render_replaces_all_occurrences() {
        assert_eq!(render("{{a}}-{{b}}-{{a}}", &[("a", "1"), ("b", "2")]), "1-2-1");
    }

    #[test]
    fn apache_conf_loads_only_existing_modules_and_proxies_php() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().join("data"));
        let root = tmp.path().join("httpd");
        fs::create_dir_all(root.join("modules")).unwrap();
        fs::write(root.join("modules/mod_proxy_fcgi.so"), "").unwrap();
        let inst = Installation { tool: Tool::Apache, id: "2.4.69".into(), version: "2.4.69".into(), root, system: false };

        let conf = write_apache(&paths, &inst, 8080, 9083, Path::new("/srv/www"), "Alias /x \"/y\"").unwrap();
        let text = fs::read_to_string(conf).unwrap();
        assert!(text.contains("LoadModule proxy_fcgi_module"));
        assert!(!text.contains("LoadModule rewrite_module"));
        assert!(text.contains("Listen 127.0.0.1:8080"));
        assert!(text.contains("proxy:fcgi://127.0.0.1:9083"));
        assert!(text.contains("Alias /x"));
        assert!(!text.contains("{{"), "no placeholders may remain");
    }

    #[test]
    fn php_ini_appends_user_settings_last() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().join("data"));
        let inst = Installation {
            tool: Tool::Php,
            id: "8.4.1".into(),
            version: "8.4.1".into(),
            root: tmp.path().join("php"),
            system: false,
        };
        let mut state = State::default();
        state.ports.mariadb = 3307;
        state.php.directives.insert("memory_limit".into(), "1G".into());
        state.php.custom = "xdebug.mode = debug\n".into();

        let files = write_php(&paths, &inst, &state).unwrap();
        let ini = fs::read_to_string(files.ini).unwrap();
        assert!(!ini.contains("{{"), "no placeholders may remain");
        let active = ini.lines().filter(|l| !l.trim_start().starts_with(';'));
        assert!(active.clone().all(|l| !l.contains("xampp")), "no active XAMPP paths may remain");
        assert!(ini.contains("mysqli.default_port = 3307"));
        let base = ini.find("memory_limit = 512M").unwrap();
        let user = ini.find("memory_limit = 1G").unwrap();
        assert!(user > base, "the user value must come after the base one");
        assert!(ini.trim_end().ends_with("xdebug.mode = debug"));
    }

    #[test]
    fn php_defaults_come_from_xampp_ini() {
        assert_eq!(php_default("memory_limit").as_deref(), Some("512M"));
        assert_eq!(php_default("upload_max_filesize").as_deref(), Some("40M"));
        assert_eq!(php_default("max_execution_time").as_deref(), Some("120"));
        assert_eq!(php_default("display_errors").as_deref(), Some("On"));
        assert_eq!(php_default("no_existe"), None);
    }

    #[test]
    fn docroot_is_created_once() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path());
        let www = ensure_docroot(&paths).unwrap();
        fs::write(www.join("index.php"), "mine").unwrap();
        ensure_docroot(&paths).unwrap();
        assert_eq!(fs::read_to_string(www.join("index.php")).unwrap(), "mine");
    }
}
