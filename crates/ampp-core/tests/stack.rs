//! Prueba de punta a punta con paquetes locales (p. ej. recién compilados en `binaries/`):
//!
//! ```sh
//! OPEN_AMPP_APACHE_PKG=/ruta/apache-2.4.69-macos-arm64.tar.gz \
//! OPEN_AMPP_PHP_PKG=/ruta/php-8.4.23-macos-arm64.tar.gz \
//! OPEN_AMPP_MARIADB_PKG=/ruta/mariadb-11.4.13-macos-arm64.tar.gz \
//!   cargo test -p ampp-core --test stack -- --ignored --nocapture
//! ```
//!
//! Cada variable es opcional; se prueba lo que se haya indicado.

use std::path::Path;

use ampp_core::catalog::{ArchiveKind, Package};
use ampp_core::{AppPaths, Platform, State, Supervisor, Tool, installer, ports};

/// Paquete local; la versión se toma del nombre (`<tool>-<versión>-<os>-<arch>.tar.gz`).
fn local_package(tool: Tool, path: &Path) -> Package {
    use sha2::{Digest, Sha256};
    let name = path.file_name().unwrap().to_string_lossy();
    let version = name.split('-').nth(1).expect("nombre <tool>-<versión>-…").to_string();
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
#[ignore = "requiere paquetes locales"]
async fn full_stack_serves_php_and_mariadb() {
    let env = |k: &str| std::env::var(k).ok().map(std::path::PathBuf::from);
    let (apache, php, mariadb) =
        (env("OPEN_AMPP_APACHE_PKG"), env("OPEN_AMPP_PHP_PKG"), env("OPEN_AMPP_MARIADB_PKG"));

    let tmp = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(tmp.path());
    paths.ensure().unwrap();
    let mut state = State::default();
    // Puertos altos para no chocar con servicios del usuario.
    state.ports.http = 38080;
    state.ports.mariadb = 33306;
    state.ports.php_fcgi_base = 39000;

    for (tool, pkg) in [(Tool::Php, &php), (Tool::Apache, &apache), (Tool::Mariadb, &mariadb)] {
        if let Some(path) = pkg {
            let pkg = local_package(tool, path);
            let dest = installer::install(&paths, &pkg, |_| {}).await.unwrap();
            state.mark_installed(tool, &pkg.version);
            let entries: Vec<_> = std::fs::read_dir(&dest).unwrap().map(|e| e.unwrap().file_name()).collect();
            eprintln!("instalado {tool:?} {} → {entries:?}", pkg.version);
        }
    }

    let supervisor = Supervisor::new(paths.clone());
    let failures = supervisor.start_all(&state).await;
    for (tool, e) in &failures {
        eprintln!("--- {tool:?} falló:\n{e}");
    }
    assert!(failures.is_empty());

    if apache.is_some() && php.is_some() {
        let body = reqwest::get("http://127.0.0.1:38080/").await.unwrap().text().await.unwrap();
        eprintln!("GET / →\n{}", body.lines().take(6).collect::<Vec<_>>().join("\n"));
        assert!(body.contains("Open Ampp funciona"), "{body}");
        assert!(body.contains("PHP 8."), "PHP debe ejecutar el script: {body}");
    }

    if let Some(pkg) = &mariadb {
        // Cliente del propio paquete: `SELECT VERSION()` por TCP como root sin contraseña.
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

    if apache.is_some() && php.is_some() && mariadb.is_some() {
        // PHP → MariaDB por PDO, como lo haría una app real.
        std::fs::write(
            paths.www().join("db.php"),
            "<?php $db = new PDO('mysql:host=127.0.0.1;port=33306', 'root', '');\n\
             echo 'DB OK ', $db->query('SELECT VERSION()')->fetchColumn();",
        )
        .unwrap();
        let body = reqwest::get("http://127.0.0.1:38080/db.php").await.unwrap().text().await.unwrap();
        eprintln!("GET /db.php → {body}");
        assert!(body.starts_with("DB OK 11.") || body.starts_with("DB OK 10."), "{body}");
    }

    supervisor.stop_all().await;
    for port in [38080, 33306] {
        assert!(ports::is_free(port), "el puerto {port} debe quedar libre");
    }
}
