//! Genera los archivos de configuración de cada servicio desde `templates/`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{IoContext, Result};
use crate::install::Installation;
use crate::paths::AppPaths;

const HTTPD_CONF: &str = include_str!("../../../templates/httpd.conf");
const PHP_INI: &str = include_str!("../../../templates/php.ini");
const PHP_FPM_CONF: &str = include_str!("../../../templates/php-fpm.conf");
const MY_CNF: &str = include_str!("../../../templates/my.cnf");

/// Módulos de Apache que se cargan si existen (los integrados, como mpm_winnt, no tienen `.so`).
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

/// Extensiones de PHP para Windows que se habilitan si su DLL existe.
const PHP_WINDOWS_EXTENSIONS: &[&str] = &[
    "curl", "fileinfo", "gd", "intl", "mbstring", "mysqli", "openssl", "pdo_mysql", "pdo_sqlite", "sqlite3", "zip",
];

const WELCOME_PHP: &str = r#"<!doctype html>
<meta charset="utf-8">
<title>OPEN_AMPP</title>
<style>body{font-family:system-ui,sans-serif;max-width:640px;margin:64px auto;padding:0 16px;line-height:1.5}</style>
<h1>OPEN_AMPP funciona</h1>
<p>PHP <?= PHP_VERSION ?> atendiendo desde <code><?= htmlspecialchars(__DIR__) ?></code>.</p>
<p><a href="info.php">Ver phpinfo()</a></p>
"#;

/// Sustituye `{{clave}}` por su valor.
pub fn render(template: &str, vars: &[(&str, &str)]) -> String {
    vars.iter().fold(template.to_string(), |acc, (k, v)| acc.replace(&format!("{{{{{k}}}}}"), v))
}

/// Ruta con `/` como separador (Apache y PHP en Windows la aceptan y evita escapes).
pub fn slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn write(path: &Path, contents: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).at(dir)?;
    }
    fs::write(path, contents).at(path)
}

/// Crea el docroot por defecto (`www/`) con una página de bienvenida si no existe.
pub fn ensure_docroot(paths: &AppPaths) -> Result<PathBuf> {
    let www = paths.www();
    if !www.exists() {
        write(&www.join("index.php"), WELCOME_PHP)?;
        write(&www.join("info.php"), "<?php phpinfo();\n")?;
    }
    Ok(www)
}

pub struct PhpFiles {
    pub ini: PathBuf,
    /// Solo en Unix (php-fpm).
    pub fpm_conf: Option<PathBuf>,
}

pub fn write_php(paths: &AppPaths, inst: &Installation, port: u16) -> Result<PhpFiles> {
    let dir = paths.config(inst.tool, &inst.id);
    let log = slash(&paths.logs().join("php.log"));

    let (ext_dir, extensions) = match inst.php_extension_dir() {
        Some(ext) if cfg!(windows) => {
            let enabled: Vec<String> = PHP_WINDOWS_EXTENSIONS
                .iter()
                .filter(|name| ext.join(format!("php_{name}.dll")).exists())
                .map(|name| format!("extension={name}"))
                .collect();
            (format!("extension_dir = \"{}\"", slash(&ext)), enabled.join("\n"))
        }
        _ => (String::new(), String::new()),
    };
    let ini = dir.join("php.ini");
    write(&ini, &render(PHP_INI, &[("error_log", &log), ("extension_dir", &ext_dir), ("extensions", &extensions)]))?;

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

pub fn write_apache(
    paths: &AppPaths,
    inst: &Installation,
    http_port: u16,
    php_port: u16,
    docroot: &Path,
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
            ],
        ),
    )?;
    Ok(conf)
}

/// Ruta del socket Unix de MariaDB. macOS limita las rutas de socket a 104 bytes,
/// así que si la ruta del usuario es demasiado larga se usa el directorio temporal.
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

        let conf = write_apache(&paths, &inst, 8080, 9083, Path::new("/srv/www")).unwrap();
        let text = fs::read_to_string(conf).unwrap();
        assert!(text.contains("LoadModule proxy_fcgi_module"));
        assert!(!text.contains("LoadModule rewrite_module"));
        assert!(text.contains("Listen 127.0.0.1:8080"));
        assert!(text.contains("proxy:fcgi://127.0.0.1:9083"));
        assert!(!text.contains("{{"), "no deben quedar variables sin reemplazar");
    }

    #[test]
    fn docroot_is_created_once() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path());
        let www = ensure_docroot(&paths).unwrap();
        fs::write(www.join("index.php"), "mío").unwrap();
        ensure_docroot(&paths).unwrap();
        assert_eq!(fs::read_to_string(www.join("index.php")).unwrap(), "mío");
    }
}
