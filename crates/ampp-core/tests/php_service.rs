//! Arranca PHP real con el supervisor y le hace una petición FastCGI. Requiere red:
//! `cargo test -p ampp-core --test php_service -- --ignored`

use std::io::{Read, Write};
use std::net::TcpStream;

use ampp_core::catalog::compare_versions;
use ampp_core::{AppPaths, Manifest, Platform, State, Supervisor, Tool, installer, ports};

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requiere red"]
async fn php_fastcgi_serves_a_script() {
    let platform = Platform::current().expect("plataforma soportada");
    let manifest = Manifest::bundled();
    let pkg = manifest
        .for_platform(platform)
        .into_iter()
        .filter(|p| p.tool == Tool::Php)
        .max_by(|a, b| compare_versions(&a.version, &b.version))
        .expect("hay PHP en el catálogo");

    let tmp = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(tmp.path());
    paths.ensure().unwrap();
    installer::install(&paths, pkg, |_| {}).await.unwrap();

    let mut state = State::default();
    // Puerto base alto para no chocar con un PHP que el usuario tenga corriendo.
    state.ports.php_fcgi_base = 39000;
    state.mark_installed(Tool::Php, &pkg.version);

    let supervisor = Supervisor::new(paths.clone());
    supervisor.start(Tool::Php, &state).await.unwrap();
    let status = supervisor.status().await.into_iter().find(|s| s.tool == Tool::Php).unwrap();
    assert!(status.running, "{status:?}");
    let port = status.port.unwrap();

    let script = tmp.path().join("hello.php");
    std::fs::write(&script, "<?php echo 'hola desde PHP ', PHP_VERSION;").unwrap();
    let body = fastcgi_get(port, &script.to_string_lossy());
    assert!(body.contains(&format!("hola desde PHP {}", pkg.version)), "respuesta: {body}");

    supervisor.stop(Tool::Php).await.unwrap();
    assert!(ports::is_free(port), "el puerto debe liberarse al detener");
}

/// Cliente FastCGI mínimo: una petición GET y devuelve el STDOUT.
fn fastcgi_get(port: u16, script: &str) -> String {
    fn record(kind: u8, content: &[u8]) -> Vec<u8> {
        let len = content.len() as u16;
        let mut r = vec![1, kind, 0, 1, (len >> 8) as u8, len as u8, 0, 0];
        r.extend_from_slice(content);
        r
    }
    fn param(out: &mut Vec<u8>, k: &str, v: &str) {
        out.push(k.len() as u8);
        out.push(v.len() as u8);
        out.extend_from_slice(k.as_bytes());
        out.extend_from_slice(v.as_bytes());
    }

    let mut params = Vec::new();
    param(&mut params, "SCRIPT_FILENAME", script);
    param(&mut params, "REQUEST_METHOD", "GET");
    param(&mut params, "SERVER_PROTOCOL", "HTTP/1.1");

    let mut req = record(1, &[0, 1, 0, 0, 0, 0, 0, 0]); // BEGIN_REQUEST, rol RESPONDER
    req.extend(record(4, &params));
    req.extend(record(4, &[]));
    req.extend(record(5, &[]));

    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.write_all(&req).unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();

    let mut out = Vec::new();
    let mut i = 0;
    while i + 8 <= raw.len() {
        let kind = raw[i + 1];
        let len = u16::from_be_bytes([raw[i + 4], raw[i + 5]]) as usize;
        let pad = raw[i + 6] as usize;
        if kind == 6 {
            out.extend_from_slice(&raw[i + 8..i + 8 + len]);
        }
        i += 8 + len + pad;
    }
    String::from_utf8_lossy(&out).into_owned()
}
