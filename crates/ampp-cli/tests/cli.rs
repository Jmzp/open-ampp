//! The `ampp` binary against a throwaway data folder (`OPEN_AMPP_HOME`); nothing here needs the network.

use std::path::Path;
use std::process::{Command, Output};

use ampp_core::{AppPaths, State, Tool};

fn ampp(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ampp"))
        .args(args)
        .env("OPEN_AMPP_HOME", home)
        .env("OPEN_AMPP_LANG", "en")
        .output()
        .unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Data folder with PHP 8.4.1 and 8.3.1 "installed" (empty folders) and 8.4.1 active.
fn home_with_two_phps() -> (tempfile::TempDir, AppPaths) {
    let tmp = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(tmp.path());
    paths.ensure().unwrap();
    let mut state = State::default();
    for v in ["8.4.1", "8.3.1"] {
        std::fs::create_dir_all(paths.bin(Tool::Php, v)).unwrap();
        state.mark_installed(Tool::Php, v);
    }
    state.save(&paths.state_file()).unwrap();
    (tmp, paths)
}

#[test]
fn paths_prints_the_data_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let out = ampp(tmp.path(), &["paths"]);
    assert!(out.status.success());
    assert_eq!(stdout(&out).trim(), tmp.path().display().to_string());
}

#[test]
fn status_and_stop_on_a_fresh_install() {
    let tmp = tempfile::tempdir().unwrap();
    for cmd in [&["status"][..], &["stop"], &["stop", "php"]] {
        let out = ampp(tmp.path(), cmd);
        assert!(out.status.success(), "{cmd:?}: {}", stderr(&out));
        let text = stdout(&out);
        for name in ["Apache", "PHP", "MariaDB"] {
            assert!(text.contains(&format!("○ {name}")), "{cmd:?}:\n{text}");
        }
        assert_eq!(text.matches("stopped (not installed)").count(), 3, "{text}");
    }
}

#[test]
fn use_switches_the_active_version() {
    let (tmp, paths) = home_with_two_phps();
    let out = ampp(tmp.path(), &["use", "php", "8.3.1"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("PHP 8.3.1 is now active"));
    let state = State::load(&paths.state_file()).unwrap();
    assert_eq!(state.active.get(&Tool::Php).map(String::as_str), Some("8.3.1"));
    assert!(stdout(&ampp(tmp.path(), &["status"])).contains("stopped (8.3.1)"));

    let out = ampp(tmp.path(), &["use", "php", "7.4.0"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("PHP 7.4.0 is not installed"), "{}", stderr(&out));
}

#[test]
fn uninstall_removes_the_version_and_falls_back() {
    let (tmp, paths) = home_with_two_phps();
    let out = ampp(tmp.path(), &["uninstall", "php"]);
    assert!(!out.status.success(), "a version is required");
    assert!(stderr(&out).contains("say which version"));

    let out = ampp(tmp.path(), &["uninstall", "php", "8.4.1"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!paths.bin(Tool::Php, "8.4.1").exists());
    let state = State::load(&paths.state_file()).unwrap();
    assert_eq!(state.installed.get(&Tool::Php).unwrap(), &["8.3.1"]);
    assert_eq!(state.active.get(&Tool::Php).map(String::as_str), Some("8.3.1"), "the remaining version becomes active");

    let out = ampp(tmp.path(), &["uninstall", "composer"]);
    assert!(out.status.success());
    assert!(stdout(&out).contains("Composer uninstalled"));
}

#[test]
fn passthrough_explains_missing_tools() {
    let tmp = tempfile::tempdir().unwrap();
    let out = ampp(tmp.path(), &["php", "--version"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("ampp install php"), "{}", stderr(&out));
}

#[test]
fn shims_are_written_to_the_data_folder() {
    let (tmp, paths) = home_with_two_phps();
    let out = ampp(tmp.path(), &["shims"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("Shims in"));
    assert!(ampp_core::tools::shims_dir(&paths).is_dir());
}

#[test]
fn rejects_unknown_commands_and_prints_its_version() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(!ampp(tmp.path(), &["frobnicate"]).status.success());
    assert!(!ampp(tmp.path(), &["use", "nginx", "1.0"]).status.success());
    let out = ampp(tmp.path(), &["--version"]);
    assert!(out.status.success());
    assert!(stdout(&out).contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn project_add_list_and_remove() {
    let (tmp, paths) = home_with_two_phps();
    let site = tmp.path().join("sites/blog");
    std::fs::create_dir_all(&site).unwrap();

    let out = ampp(tmp.path(), &["project", "add", "blog", site.to_str().unwrap(), "--php", "8.3"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("blog → http://blog.localhost:8080/"), "{}", stdout(&out));
    let state = State::load(&paths.state_file()).unwrap();
    assert_eq!(state.projects[0].php.as_deref(), Some("8.3"));
    assert_eq!(state.projects[0].docroot, std::fs::canonicalize(&site).unwrap());

    let out = ampp(tmp.path(), &["project", "ls"]);
    assert!(stdout(&out).contains("PHP 8.3.1"), "{}", stdout(&out));

    // Invalid ones are rejected without touching the state.
    for args in [
        &["project", "add", "blog", site.to_str().unwrap()][..],
        &["project", "add", "old", site.to_str().unwrap(), "--php", "7.4"],
        &["project", "add", "gone", "/definitely/not/here"],
    ] {
        let out = ampp(tmp.path(), args);
        assert!(!out.status.success(), "{args:?} must fail");
    }
    assert_eq!(State::load(&paths.state_file()).unwrap().projects.len(), 1);

    assert!(ampp(tmp.path(), &["project", "rm", "blog"]).status.success());
    assert!(State::load(&paths.state_file()).unwrap().projects.is_empty());
    assert!(!ampp(tmp.path(), &["project", "rm", "blog"]).status.success());
    assert!(site.is_dir(), "the folder is never deleted");
}

#[test]
fn https_projects_must_use_localhost() {
    let (tmp, paths) = home_with_two_phps();
    let site = tmp.path().join("site");
    std::fs::create_dir_all(&site).unwrap();
    let dir = site.to_str().unwrap();

    let out = ampp(tmp.path(), &["project", "add", "secure", dir, "--https"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("secure → https://secure.localhost:8443/"), "{}", stdout(&out));
    assert!(State::load(&paths.state_file()).unwrap().projects[0].https);

    let out = ampp(tmp.path(), &["project", "add", "other", dir, "--domain", "other.test", "--https"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains(".localhost"), "{}", stderr(&out));
}
