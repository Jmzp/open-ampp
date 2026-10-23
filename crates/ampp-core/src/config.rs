//! Generates each service's configuration files from `templates/`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, IoContext, Result};
use crate::install::Installation;
use crate::paths::AppPaths;
use crate::ports;
use crate::state::{PhpSettings, State};
use crate::tls::ServerCert;

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

/// Same palette as the desktop app (`apps/desktop/src/App.css`), with the logo as the only gradient.
const WELCOME_PHP: &str = r##"<?php $projects = glob(__DIR__ . '/*', GLOB_ONLYDIR) ?: []; ?>
<!doctype html>
<html lang="{{lang}}">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Open AMPP</title>
<style>
  :root {
    --bg: #f6f7f9; --surface: #fff; --text: #1b1f24; --muted: #6b7280; --border: #e3e6ea;
    --accent: #6366f1; --ok: #16a34a;
    color-scheme: light dark;
  }
  @media (prefers-color-scheme: dark) {
    :root { --bg: #111418; --surface: #1a1e24; --text: #e6e8eb; --muted: #9aa3ad; --border: #2a2f37; }
  }
  * { box-sizing: border-box; }
  body {
    margin: 0; min-height: 100vh; padding: 64px 16px;
    font-family: Inter, system-ui, -apple-system, "Segoe UI", sans-serif; line-height: 1.55;
    color: var(--text);
    background: radial-gradient(900px 360px at 50% -120px, color-mix(in srgb, var(--accent) 12%, transparent), transparent), var(--bg);
  }
  main { max-width: 720px; margin: 0 auto; }
  header { display: flex; align-items: center; gap: 16px; }
  .logo { width: 56px; height: 56px; flex: none; }
  h1 { margin: 0; font-size: 1.6rem; letter-spacing: -0.01em; }
  .status { margin: 2px 0 0; color: var(--muted); display: flex; align-items: center; gap: 8px; }
  .status::before { content: ""; width: 8px; height: 8px; border-radius: 50%; background: var(--ok); box-shadow: 0 0 0 3px color-mix(in srgb, var(--ok) 20%, transparent); }
  .serving { margin: 28px 0 0; color: var(--muted); }
  code { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 0.88em; background: color-mix(in srgb, var(--accent) 9%, transparent); padding: 2px 7px; border-radius: 6px; color: var(--text); word-break: break-all; }
  h2 { margin: 36px 0 12px; font-size: 0.78rem; font-weight: 600; text-transform: uppercase; letter-spacing: 0.06em; color: var(--muted); }
  .grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap: 12px; }
  .card { display: block; background: var(--surface); border: 1px solid var(--border); border-radius: 12px; padding: 16px 18px; color: inherit; text-decoration: none; transition: border-color .15s, transform .15s; }
  .card:hover { border-color: var(--accent); transform: translateY(-1px); }
  .card strong { display: block; color: var(--accent); }
  .card span { font-size: 0.9rem; color: var(--muted); }
  .projects { list-style: none; margin: 0; padding: 0; background: var(--surface); border: 1px solid var(--border); border-radius: 12px; overflow: hidden; }
  .projects li + li { border-top: 1px solid var(--border); }
  .projects a { display: block; padding: 10px 18px; color: var(--text); text-decoration: none; font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 0.92rem; }
  .projects a:hover { background: color-mix(in srgb, var(--accent) 7%, transparent); color: var(--accent); }
  footer { margin-top: 36px; padding-top: 16px; border-top: 1px solid var(--border); font-size: 0.85rem; color: var(--muted); }
</style>
<main>
  <header>
    <svg class="logo" viewBox="100 100 824 824" aria-hidden="true">
      <defs>
        <linearGradient id="g" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stop-color="#4F46E5"/><stop offset="0.55" stop-color="#7C3AED"/><stop offset="1" stop-color="#DB2777"/>
        </linearGradient>
      </defs>
      <rect x="100" y="100" width="824" height="824" rx="190" fill="url(#g)"/>
      <g fill="#fff"><rect x="242" y="282" width="540" height="128" rx="40"/><rect x="242" y="448" width="540" height="128" rx="40" opacity=".94"/><rect x="242" y="614" width="540" height="128" rx="40" opacity=".88"/></g>
      <circle cx="318" cy="346" r="24" fill="#F97316"/><circle cx="318" cy="512" r="24" fill="#6366F1"/><circle cx="318" cy="678" r="24" fill="#14B8A6"/>
    </svg>
    <div>
      <h1>Open AMPP</h1>
      <p class="status">{{heading}}</p>
    </div>
  </header>

  <p class="serving">{{serving}}</p>

  <h2>{{tools}}</h2>
  <div class="grid">
    <a class="card" href="info.php"><strong>phpinfo()</strong><span>{{phpinfo}}</span></a>
    <a class="card" href="/phpmyadmin/"><strong>phpMyAdmin</strong><span>{{phpmyadmin}}</span></a>
  </div>

<?php if ($projects): ?>
  <h2>{{projects}}</h2>
  <ul class="projects">
<?php foreach ($projects as $dir): $name = basename($dir); ?>
    <li><a href="<?= rawurlencode($name) ?>/"><?= htmlspecialchars($name) ?>/</a></li>
<?php endforeach; ?>
  </ul>
<?php endif; ?>

  <footer>{{hint}}</footer>
</main>
"##;

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
            &[
                ("lang", &tr!("es", "en")),
                ("heading", &tr!("Todo funciona", "Everything is running")),
                ("serving", &serving),
                ("tools", &tr!("Herramientas", "Tools")),
                ("phpinfo", &tr!("Configuración y extensiones de PHP", "PHP configuration and extensions")),
                ("phpmyadmin", &tr!("Administra tus bases de datos MariaDB", "Manage your MariaDB databases")),
                ("projects", &tr!("Tus proyectos", "Your projects")),
                ("hint", &hint),
            ],
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

/// A project served on its own domain (see `projects`).
#[derive(Debug, Clone, PartialEq)]
pub struct Vhost {
    pub domain: String,
    pub docroot: PathBuf,
    /// FastCGI port of the PHP version that serves it.
    pub php_port: u16,
    /// Also served over HTTPS.
    pub https: bool,
}

/// HTTPS listener: its port and the certificate of the local CA (see `tls`).
#[derive(Debug, Clone, PartialEq)]
pub struct Tls {
    pub port: u16,
    pub cert: ServerCert,
}

/// What Apache serves besides `www/`: the projects and, if any of them wants it, HTTPS.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sites {
    pub vhosts: Vec<Vhost>,
    pub tls: Option<Tls>,
}

/// `extra_conf` is appended to `httpd.conf` (e.g. the phpMyAdmin alias).
pub fn write_apache(
    paths: &AppPaths,
    inst: &Installation,
    http_port: u16,
    php_port: u16,
    docroot: &Path,
    sites: &Sites,
    extra_conf: &str,
) -> Result<PathBuf> {
    let modules_dir = inst.apache_modules();
    let module = |m: &str| Some(modules_dir.as_ref()?.join(format!("mod_{m}.so"))).filter(|so| so.exists());
    // mod_ssl is only loaded when needed, so a problem with it can never take plain HTTP down.
    let tls_modules: &[&str] = if sites.tls.is_some() { &["ssl", "socache_shmcb"] } else { &[] };
    if sites.tls.is_some() && module("ssl").is_none() {
        return Err(Error::Tls(tr!(
            "esta versión de Apache no incluye mod_ssl",
            "this Apache build does not include mod_ssl"
        )));
    }
    let load_modules = APACHE_MODULES
        .iter()
        .chain(tls_modules)
        .filter_map(|m| module(m).map(|so| format!("LoadModule {m}_module \"{}\"", slash(&so))))
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
                ("php_handler", &php_handler(php_port)),
                ("run_dir", &slash(&paths.run())),
                ("log_dir", &slash(&paths.logs())),
                ("docroot", &slash(docroot)),
                ("load_modules", &load_modules),
                ("vhosts", &apache_vhosts(http_port, sites, &paths.run())),
                ("extra_conf", extra_conf),
            ],
        ),
    )?;
    Ok(conf)
}

/// Name-based virtual hosts. Once a port has one, Apache sends requests for unknown names to the
/// first, so `localhost` goes first and keeps the main configuration (`www/`, phpMyAdmin…).
fn apache_vhosts(http_port: u16, sites: &Sites, run_dir: &Path) -> String {
    let mut out = String::new();
    if !sites.vhosts.is_empty() {
        out.push_str(&vhost_block(http_port, "localhost", None, ""));
        for v in &sites.vhosts {
            out.push_str(&vhost_block(http_port, &v.domain, Some(v), ""));
        }
    }
    if let Some(tls) = &sites.tls {
        let (cert, key) = (slash(&tls.cert.cert), slash(&tls.cert.key));
        let ssl = format!(
            "    SSLEngine on\n    SSLCertificateFile \"{cert}\"\n    SSLCertificateKeyFile \"{key}\"\n"
        );
        out.push_str(&format!(
            "\nListen 127.0.0.1:{port}\n\
             <IfModule socache_shmcb_module>\n    SSLSessionCache \"shmcb:{run}/ssl_scache(512000)\"\n</IfModule>\n",
            port = tls.port,
            run = slash(run_dir)
        ));
        out.push_str(&vhost_block(tls.port, "localhost", None, &ssl));
        for v in sites.vhosts.iter().filter(|v| v.https) {
            out.push_str(&vhost_block(tls.port, &v.domain, Some(v), &ssl));
        }
    }
    out
}

/// One virtual host. Without a project it inherits the main configuration.
fn vhost_block(port: u16, name: &str, project: Option<&Vhost>, ssl: &str) -> String {
    let body = project
        .map(|v| {
            let (docroot, handler) = (slash(&v.docroot), php_handler(v.php_port));
            format!(
                "    DocumentRoot \"{docroot}\"\n    \
                 <Directory \"{docroot}\">\n        \
                 Options Indexes FollowSymLinks\n        \
                 AllowOverride All\n        \
                 Require all granted\n    \
                 </Directory>\n    \
                 <FilesMatch \"\\.php$\">\n    {handler}\n    </FilesMatch>\n"
            )
        })
        .unwrap_or_default();
    format!("\n<VirtualHost 127.0.0.1:{port}>\n    ServerName {name}\n{ssl}{body}</VirtualHost>\n")
}

/// Body of the `.php` handler block. On Windows Apache appends the script path to the handler URL,
/// so without a trailing slash the drive letter lands in the port (`127.0.0.1:9082C:`). With it,
/// SCRIPT_FILENAME arrives as `proxy:fcgi://host:port/C:/…`, which php-cgi (unlike php-fpm)
/// cannot open, so the prefix is stripped.
fn php_handler(php_port: u16) -> String {
    if cfg!(windows) {
        format!(
            "    SetHandler \"proxy:fcgi://127.0.0.1:{php_port}/\"\n    \
             ProxyFCGISetEnvIf \"reqenv('SCRIPT_FILENAME') =~ m#^proxy:fcgi://[^/]+/(.*)$#\" SCRIPT_FILENAME \"$1\""
        )
    } else {
        format!("    SetHandler \"proxy:fcgi://127.0.0.1:{php_port}\"")
    }
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

        let conf = write_apache(&paths, &inst, 8080, 9083, Path::new("/srv/www"), &Sites::default(), "Alias /x \"/y\"").unwrap();
        let text = fs::read_to_string(conf).unwrap();
        assert!(text.contains("LoadModule proxy_fcgi_module"));
        assert!(!text.contains("LoadModule rewrite_module"));
        assert!(text.contains("Listen 127.0.0.1:8080"));
        assert!(text.contains("proxy:fcgi://127.0.0.1:9083"));
        // Windows needs the trailing slash so the drive letter does not end up in the port.
        assert_eq!(text.contains("proxy:fcgi://127.0.0.1:9083/\""), cfg!(windows));
        assert!(text.contains("Alias /x"));
        assert!(!text.contains("<VirtualHost"), "no projects, no virtual hosts");
        assert!(!text.contains("{{"), "no placeholders may remain");
    }

    #[test]
    fn apache_conf_adds_a_virtual_host_per_project_after_localhost() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().join("data"));
        let inst = Installation {
            tool: Tool::Apache,
            id: "2.4.69".into(),
            version: "2.4.69".into(),
            root: tmp.path().join("httpd"),
            system: false,
        };
        let sites = Sites {
            vhosts: vec![
                Vhost { domain: "blog.localhost".into(), docroot: "/srv/blog/public".into(), php_port: 9082, https: false },
                Vhost { domain: "shop.localhost".into(), docroot: "/srv/shop".into(), php_port: 9084, https: false },
            ],
            tls: None,
        };
        let conf = write_apache(&paths, &inst, 8080, 9084, Path::new("/srv/www"), &sites, "").unwrap();
        let text = fs::read_to_string(conf).unwrap();
        assert_eq!(text.matches("<VirtualHost 127.0.0.1:8080>").count(), 3);
        let (default, blog, shop) = (
            text.find("ServerName localhost\n").unwrap(),
            text.find("ServerName blog.localhost").unwrap(),
            text.find("ServerName shop.localhost").unwrap(),
        );
        assert!(default < blog && blog < shop, "localhost must be the first (default) virtual host");
        assert!(text.contains("DocumentRoot \"/srv/blog/public\""));
        assert!(text.contains("<Directory \"/srv/blog/public\">"));
        let blog_block = &text[blog..shop];
        assert!(blog_block.contains("proxy:fcgi://127.0.0.1:9082"), "{blog_block}");
        assert!(text[shop..].contains("proxy:fcgi://127.0.0.1:9084"));
        assert!(!text.contains("SSLEngine") && !text.contains("ssl_module"), "no HTTPS unless a project asks for it");
        assert!(!text.contains("{{"));
    }

    #[test]
    fn apache_conf_serves_https_projects_with_the_local_certificate() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().join("data"));
        let root = tmp.path().join("httpd");
        fs::create_dir_all(root.join("modules")).unwrap();
        let inst = Installation { tool: Tool::Apache, id: "2.4.69".into(), version: "2.4.69".into(), root: root.clone(), system: false };
        let cert = ServerCert { cert: "/certs/server.pem".into(), key: "/certs/server.key".into() };
        let sites = Sites {
            vhosts: vec![
                Vhost { domain: "plain.localhost".into(), docroot: "/srv/plain".into(), php_port: 9084, https: false },
                Vhost { domain: "secure.localhost".into(), docroot: "/srv/secure".into(), php_port: 9082, https: true },
            ],
            tls: Some(Tls { port: 8443, cert }),
        };
        let err = write_apache(&paths, &inst, 8080, 9084, Path::new("/srv/www"), &sites, "").unwrap_err();
        assert!(matches!(err, Error::Tls(_)), "an Apache without mod_ssl cannot serve HTTPS: {err}");

        fs::write(root.join("modules/mod_ssl.so"), "").unwrap();
        let text = fs::read_to_string(write_apache(&paths, &inst, 8080, 9084, Path::new("/srv/www"), &sites, "").unwrap()).unwrap();
        assert!(text.contains("LoadModule ssl_module"));
        assert!(text.contains("Listen 127.0.0.1:8443"));
        assert_eq!(text.matches("<VirtualHost 127.0.0.1:8080>").count(), 3, "every project keeps plain HTTP");
        assert_eq!(text.matches("<VirtualHost 127.0.0.1:8443>").count(), 2, "localhost and the HTTPS project");
        let https = &text[text.find("<VirtualHost 127.0.0.1:8443>").unwrap()..];
        assert!(https.find("ServerName localhost").unwrap() < https.find("ServerName secure.localhost").unwrap());
        assert!(!https.contains("plain.localhost"));
        assert_eq!(https.matches("SSLEngine on").count(), 2);
        assert!(https.contains("SSLCertificateFile \"/certs/server.pem\""));
        assert!(https.contains("SSLCertificateKeyFile \"/certs/server.key\""));
        assert!(https.contains("proxy:fcgi://127.0.0.1:9082"));
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

    #[test]
    fn welcome_page_links_the_tools_and_lists_projects() {
        let tmp = tempfile::tempdir().unwrap();
        let www = ensure_docroot(&AppPaths::new(tmp.path())).unwrap();
        let page = fs::read_to_string(www.join("index.php")).unwrap();
        assert!(!page.contains("{{"), "no placeholders may remain");
        assert!(page.starts_with("<?php $projects = glob("), "the project list is computed before any output");
        for part in ["href=\"info.php\"", "href=\"/phpmyadmin/\"", "PHP_VERSION", "prefers-color-scheme: dark", "rawurlencode"] {
            assert!(page.contains(part), "missing {part}");
        }
        assert_eq!(fs::read_to_string(www.join("info.php")).unwrap(), "<?php phpinfo();\n");
    }

    #[test]
    fn mariadb_conf_points_at_the_datadir_and_port() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path());
        let inst =
            Installation { tool: Tool::Mariadb, id: "11.4.5".into(), version: "11.4.5".into(), root: "/opt/mdb".into(), system: false };
        let datadir = paths.mariadb_data("11.4");
        let text = fs::read_to_string(write_mariadb(&paths, &inst, 3307, &datadir).unwrap()).unwrap();
        assert!(text.contains("basedir=\"/opt/mdb\""));
        assert!(text.contains(&format!("datadir=\"{}\"", slash(&datadir))));
        assert_eq!(text.matches("port=3307").count(), 2, "server and client sections");
        assert!(text.contains("bind-address=127.0.0.1"));
        assert_eq!(text.contains("socket="), !cfg!(windows));
        assert!(!text.contains("{{"));
    }

    #[test]
    fn slash_uses_forward_slashes() {
        assert_eq!(slash(Path::new(r"C:\Users\me\www")), "C:/Users/me/www");
    }

    /// The toggleable extensions are the DLLs in `ext/`; enabled ones follow XAMPP's order.
    #[cfg(windows)]
    #[test]
    fn windows_extensions_come_from_ext_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("php");
        fs::create_dir_all(root.join("ext")).unwrap();
        for dll in ["php_curl.dll", "php_mbstring.dll", "php_bz2.dll", "php_intl.dll", "php_opcache.dll", "readme.txt"] {
            fs::write(root.join("ext").join(dll), "").unwrap();
        }
        let inst = Installation { tool: Tool::Php, id: "8.4.1".into(), version: "8.4.1".into(), root, system: false };
        assert_eq!(php_optional_extensions(&inst), ["bz2", "curl", "intl", "mbstring", "opcache"]);

        let mut settings = PhpSettings::default();
        assert_eq!(php_enabled_extensions(&inst, &settings), ["bz2", "curl", "mbstring"], "XAMPP's defaults that exist");
        settings.extensions = Some(vec!["opcache".into(), "intl".into(), "curl".into(), "missing".into()]);
        assert_eq!(php_enabled_extensions(&inst, &settings), ["curl", "intl", "opcache"]);

        let mut state = State::default();
        state.php = settings;
        let paths = AppPaths::new(tmp.path().join("data"));
        let ini = fs::read_to_string(write_php(&paths, &inst, &state).unwrap().ini).unwrap();
        assert!(ini.contains(&format!("extension_dir = \"{}\"", slash(&inst.root.join("ext")))));
        assert!(ini.contains("extension=curl\nextension=intl\nzend_extension=opcache"));
    }
}
