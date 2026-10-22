//! Real download from the bundled catalog. Needs network:
//! `cargo test -p ampp-core --test real_install -- --ignored`

use ampp_core::catalog::compare_versions;
use ampp_core::{AppPaths, Manifest, Platform, Tool, installer};

#[tokio::test]
#[ignore = "needs network"]
async fn installs_latest_php_for_this_platform() {
    let platform = Platform::current().expect("plataforma soportada");
    let manifest = Manifest::bundled();
    let pkg = manifest
        .for_platform(platform)
        .into_iter()
        .filter(|p| p.tool == Tool::Php)
        .max_by(|a, b| compare_versions(&a.version, &b.version))
        .expect("the catalog has PHP for this platform");

    let tmp = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(tmp.path());
    let dest = installer::install(&paths, pkg, |p| eprintln!("{:?} {}/{:?}", p.phase, p.downloaded, p.total))
        .await
        .unwrap();

    let entries: Vec<_> = std::fs::read_dir(&dest).unwrap().map(|e| e.unwrap().file_name()).collect();
    eprintln!("installed at {} → {entries:?}", dest.display());
    assert!(!entries.is_empty());
}
