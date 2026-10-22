//! End-to-end test with local packages (e.g. freshly built with `binaries/`):
//!
//! ```sh
//! OPEN_AMPP_APACHE_PKG=/path/apache-2.4.69-macos-arm64.tar.gz \
//! OPEN_AMPP_PHP_PKG=/path/php-8.4.23-macos-arm64.tar.gz \
//! OPEN_AMPP_MARIADB_PKG=/path/mariadb-11.4.13-macos-arm64.tar.gz \
//!   cargo test -p ampp-core --test stack -- --ignored --nocapture
//! ```
//!
//! Every variable is optional; whatever is given gets tested.

use std::path::Path;

use ampp_core::catalog::{ArchiveKind, Package};
use ampp_core::{AppPaths, Extra, Manifest, Platform, State, Supervisor, Tool, installer, ports, tools};

/// Local package; the version comes from the file name (`<tool>-<version>-<os>-<arch>.tar.gz`).
fn local_package(tool: Tool, path: &Path) -> Package {
    use sha2::{Digest, Sha256};
    let name = path.file_name().unwrap().to_string_lossy();
    let version = name.split('-').nth(1).expect("name <tool>-<version>-…").to_string();
    let platform = Platform::current().unwrap();
    Package {
        tool,
        version,
        os: platform.os,
        arch: platform.arch,
        url: format!("file://{}", path.display()),
        sha256: hex::encode(Sha256::digest(std::fs::read(path).unwrap())),
        archive: ArchiveKind::TarGz,
        size: None,
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs local packages"]
async fn full_stack_serves_php_and_mariadb() {
    let env = |k: &str| std::env::var(k).ok().map(std::path::PathBuf::from);
    let (apache, php, mariadb) =
        (env("OPEN_AMPP_APACHE_PKG"), env("OPEN_AMPP_PHP_PKG"), env("OPEN_AMPP_MARIADB_PKG"));

    let tmp = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(tmp.path());
    paths.ensure().unwrap();
    let mut state = State::default();
    // High ports so they do not clash with the user's services.
    state.ports.http = 38080;
    state.ports.mariadb = 33306;
    state.ports.php_fcgi_base = 39000;

    for (tool, pkg) in [(Tool::Php, &php), (Tool::Apache, &apache), (Tool::Mariadb, &mariadb)] {
        if let Some(path) = pkg {
            let pkg = local_package(tool, path);
            let dest = installer::install(&paths, &pkg, |_| {}).await.unwrap();
            state.mark_installed(tool, &pkg.version);
            let entries: Vec<_> = std::fs::read_dir(&dest).unwrap().map(|e| e.unwrap().file_name()).collect();
            eprintln!("installed {tool:?} {} → {entries:?}", pkg.version);
        }
    }

    let full = apache.is_some() && php.is_some() && mariadb.is_some();
    if full {
        // Real phpMyAdmin from the bundled catalog.
        let pma = Manifest::bundled().extra(Extra::Phpmyadmin).cloned().expect("phpMyAdmin in the catalog");
        tools::install(&paths, &pma).await.unwrap();
        state.extras.insert(Extra::Phpmyadmin, pma.version);
    }

    let supervisor = Supervisor::new(paths.clone());
    let failures = supervisor.start_all(&state).await;
    for (tool, e) in &failures {
        eprintln!("--- {tool:?} failed:\n{e}");
    }
    assert!(failures.is_empty());

    // The checks run separately so the services are stopped even if one fails.
    let checks = std::panic::AssertUnwindSafe(async {
        if apache.is_some() && php.is_some() {
            let body = reqwest::get("http://127.0.0.1:38080/").await.unwrap().text().await.unwrap();
            eprintln!("GET / →\n{}", body.lines().take(6).collect::<Vec<_>>().join("\n"));
            assert!(body.contains("<h1>Open AMPP"), "{body}");
            assert!(body.contains("PHP 8."), "PHP must run the script: {body}");
        }

        if let Some(pkg) = &mariadb {
            // The package's own client: `SELECT VERSION()` over TCP as passwordless root.
            let version = local_package(Tool::Mariadb, pkg).version;
            let client = paths.bin(Tool::Mariadb, &version).join("bin/mariadb");
            let out = std::process::Command::new(client)
                .args(["--protocol=tcp", "-h", "127.0.0.1", "-P", "33306", "-u", "root", "-N", "-e", "SELECT VERSION()"])
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout);
            eprintln!("SELECT VERSION() → {stdout}{}", String::from_utf8_lossy(&out.stderr));
            assert!(stdout.starts_with(&version), "{stdout}");
        }

        if full {
            // PHP → MariaDB through PDO, like a real app: over TCP and, on Unix, with `localhost`
            // (the php.ini default socket) and no port.
            let host = if cfg!(windows) { "127.0.0.1" } else { "localhost" };
            std::fs::write(
                paths.www().join("db.php"),
                format!(
                    "<?php $db = new PDO('mysql:host={host}', 'root', '');\n\
                     echo 'DB OK ', $db->query('SELECT VERSION()')->fetchColumn(), ' TZ ', date_default_timezone_get();"
                ),
            )
            .unwrap();
            let body = reqwest::get("http://127.0.0.1:38080/db.php").await.unwrap().text().await.unwrap();
            eprintln!("GET /db.php → {body}");
            assert!(body.starts_with("DB OK 11.") || body.starts_with("DB OK 10."), "{body}");

            // phpMyAdmin logs in by itself (passwordless root) and lists the databases without PHP errors.
            let client = reqwest::Client::builder().cookie_store(true).build().unwrap();
            let url = "http://127.0.0.1:38080/phpmyadmin/index.php?route=/server/databases";
            let resp = client.get(url).send().await.unwrap();
            let status = resp.status();
            let body = resp.text().await.unwrap();
            eprintln!("GET /phpmyadmin/ → {status}, {} bytes", body.len());
            let excerpt = &body[..body.len().min(3000)];
            assert!(body.contains("information_schema"), "phpMyAdmin should list the databases:\n{excerpt}");
            for noise in ["Deprecated:", "Warning:", "Fatal error"] {
                assert!(!body.contains(noise), "phpMyAdmin printed `{noise}`:\n{excerpt}");
            }
        }

        if let Some(php) = &php {
            // The XAMPP-based php.ini must not produce startup warnings.
            let version = local_package(Tool::Php, php).version;
            let inst = ampp_core::install::locate(&paths, Tool::Php, &version).unwrap();
            let ini = ampp_core::config::write_php(&paths, &inst, &state).unwrap().ini;
            let out = std::process::Command::new(inst.php_cli().unwrap())
                .args(["-c", &ini.to_string_lossy(), "-r", "echo ini_get('memory_limit'), ' ', ini_get('upload_max_filesize');"])
                .output()
                .unwrap();
            let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            eprintln!("php -r → {stdout} {stderr}");
            assert!(stdout.starts_with("512M 40M"), "XAMPP values: {stdout}");
            assert!(!stdout.contains("Warning") && stderr.is_empty(), "no warnings: {stdout}{stderr}");
        }
    });
    let outcome = futures_util::FutureExt::catch_unwind(checks).await;
    supervisor.stop_all().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
    for port in [38080, 33306] {
        assert!(ports::is_free(port), "port {port} must be released");
    }
}
