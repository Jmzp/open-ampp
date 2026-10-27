//! Supervisor end to end, without network or real servers: this test binary doubles as the
//! fake Apache, PHP and MariaDB. It is copied into fake installations as `httpd`, `php-cgi`
//! / `php-fpm` and `mariadbd`; when started by the supervisor (marked by an environment variable
//! the children inherit) it listens on the port found in its arguments or config file.
//!
//! Runs without the libtest harness (`harness = false`) because the supervisor passes service
//! arguments (`-b`, `-f`, `--defaults-file=`…) that libtest would reject.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ampp_core::state::Project;
use ampp_core::{AppPaths, State, Supervisor, Tool, ports, projects};

/// Set in this process before starting services, so the copies know to act as fakes.
const FAKE: &str = "OPEN_AMPP_FAKE_SERVICE";
/// When set, the fake writes to stderr and exits with this code instead of listening.
const FAIL: &str = "OPEN_AMPP_FAKE_FAIL";

const PHP: &str = "8.4.1";
const APACHE: &str = "2.4.69";
const MARIADB: &str = "11.4.5";
/// Second PHP series, for a project.
const PHP_OLD: &str = "8.2.30";

fn main() {
    if std::env::var_os(FAKE).is_some() {
        fake_service();
    }
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let tests: &[(&str, fn(Env) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>)] = &[
        ("starts_reports_and_stops_all_services", |e| Box::pin(starts_reports_and_stops_all_services(e))),
        ("other_process_sees_services_as_external_and_can_stop_them", |e| {
            Box::pin(other_process_sees_services_as_external_and_can_stop_them(e))
        }),
        ("shutdown_only_stops_own_services", |e| Box::pin(shutdown_only_stops_own_services(e))),
        ("detects_a_service_that_dies", |e| Box::pin(detects_a_service_that_dies(e))),
        ("refuses_a_busy_port", |e| Box::pin(refuses_a_busy_port(e))),
        ("reports_the_log_when_a_service_exits_at_start", |e| {
            Box::pin(reports_the_log_when_a_service_exits_at_start(e))
        }),
        ("errors_without_active_or_installed_version", |e| Box::pin(errors_without_active_or_installed_version(e))),
        ("cleanup_removes_stale_pid_files", |e| Box::pin(cleanup_removes_stale_pid_files(e))),
        ("projects_run_their_own_php_version", |e| Box::pin(projects_run_their_own_php_version(e))),
        ("mailpit_runs_with_the_services_when_installed", |e| Box::pin(mailpit_runs_with_the_services_when_installed(e))),
    ];
    let filter = std::env::args().skip(1).find(|a| !a.starts_with('-'));
    let mut failed = 0;
    for (name, test) in tests {
        if filter.as_deref().is_some_and(|f| !name.contains(f)) {
            continue;
        }
        let env = Env::new();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rt.block_on(test(env))));
        println!("test {name} ... {}", if result.is_ok() { "ok" } else { "FAILED" });
        failed += result.is_err() as i32;
    }
    // Return normally on success: `process::exit` would skip writing coverage data.
    if failed > 0 {
        std::process::exit(1);
    }
}

/// Fake service: listens on its port until it is killed.
fn fake_service() -> ! {
    if let Some(code) = std::env::var_os(FAIL) {
        eprintln!("fake service: failing on purpose");
        std::process::exit(code.to_string_lossy().parse().unwrap());
    }
    let port = port_from_args().expect("a port in the arguments or config file");
    let listener = TcpListener::bind(("127.0.0.1", port)).expect("bind the fake service port");
    for conn in listener.incoming() {
        drop(conn);
    }
    unreachable!()
}

/// `-b 127.0.0.1:N` (php-cgi), or the port in the file passed with `-f` (httpd),
/// `-y` (php-fpm) or `--defaults-file=` (mariadbd).
fn port_from_args() -> Option<u16> {
    let args: Vec<String> = std::env::args().collect();
    for (i, arg) in args.iter().enumerate() {
        let next = args.get(i + 1);
        let file = match arg.as_str() {
            "-b" | "--listen" => return next?.rsplit(':').next()?.parse().ok(),
            "-f" | "-y" => next.cloned(),
            a => a.strip_prefix("--defaults-file=").map(String::from),
        };
        if let Some(port) = file.and_then(|f| std::fs::read_to_string(f).ok()).and_then(|t| port_in(&t)) {
            return Some(port);
        }
    }
    None
}

fn port_in(config: &str) -> Option<u16> {
    config.lines().map(str::trim).find_map(|l| {
        let value = l.strip_prefix("Listen ").or_else(|| l.strip_prefix("listen = ")).or_else(|| l.strip_prefix("port="))?;
        value.rsplit(':').next()?.parse().ok()
    })
}

/// Temporary data folder with fake installations of the three services and a state using free ports.
struct Env {
    _tmp: tempfile::TempDir,
    paths: AppPaths,
    state: State,
}

impl Env {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path());
        paths.ensure().unwrap();
        let exe = |name: &str| if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };

        let php = if cfg!(windows) { exe("php-cgi") } else { "php-fpm".into() };
        fake_binary(&paths.bin(Tool::Php, PHP).join(php));
        fake_binary(&paths.bin(Tool::Apache, APACHE).join("bin").join(exe("httpd")));
        fake_binary(&paths.bin(Tool::Mariadb, MARIADB).join("bin").join(exe("mariadbd")));
        // An existing system schema means the datadir is initialized: no mariadb-install-db.
        std::fs::create_dir_all(paths.mariadb_data("11.4").join("mysql")).unwrap();

        let mut state = State::default();
        state.ports.http = free_port();
        state.ports.mariadb = free_port();
        // php_fcgi_port(base, "8.4.x") = base + 84.
        state.ports.php_fcgi_base = free_port() - 84;
        for (tool, version) in [(Tool::Php, PHP), (Tool::Apache, APACHE), (Tool::Mariadb, MARIADB)] {
            state.mark_installed(tool, version);
        }
        // SAFETY: the tests run one after another on this thread before any service starts.
        unsafe {
            std::env::set_var(FAKE, "1");
            std::env::remove_var(FAIL);
        }
        Self { _tmp: tmp, paths, state }
    }

    fn pid_file(&self, tool: Tool) -> PathBuf {
        self.paths.run().join(format!("{}.pid", tool.as_str()))
    }

    fn php_port(&self) -> u16 {
        ports::php_fcgi_port(self.state.ports.php_fcgi_base, PHP)
    }
}

fn fake_binary(dest: &Path) {
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    std::fs::copy(std::env::current_exe().unwrap(), dest).unwrap();
}

fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port()
}

async fn wait_until(mut cond: impl FnMut() -> bool) -> bool {
    for _ in 0..50 {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

fn kill(pid: u32) {
    #[cfg(windows)]
    let _ = std::process::Command::new("taskkill").args(["/PID", &pid.to_string(), "/F"]).output();
    #[cfg(unix)]
    let _ = std::process::Command::new("kill").args(["-9", &pid.to_string()]).output();
}

async fn starts_reports_and_stops_all_services(env: Env) {
    let sup = Supervisor::new(env.paths.clone());
    let failures = sup.start_all(&env.state).await;
    assert!(failures.is_empty(), "{:?}", failures.iter().map(|e| e.to_string()).collect::<Vec<_>>());

    let status = sup.status().await;
    for (tool, version, port) in [
        (Tool::Apache, APACHE, env.state.ports.http),
        (Tool::Php, PHP, env.php_port()),
        (Tool::Mariadb, MARIADB, env.state.ports.mariadb),
    ] {
        let s = status.iter().find(|s| s.tool == tool).unwrap();
        assert!(s.running && !s.external && s.error.is_none(), "{s:?}");
        assert_eq!((s.version.as_deref(), s.port), (Some(version), Some(port)));
        assert!(s.pid.is_some());
        assert!(ports::is_open(port).await, "{tool:?} must accept connections");
        // PID file: pid, port, version and the launching process.
        let record = std::fs::read_to_string(env.pid_file(tool)).unwrap();
        let lines: Vec<&str> = record.lines().collect();
        assert_eq!(lines, [s.pid.unwrap().to_string(), port.to_string(), version.to_string(), std::process::id().to_string()]);
    }
    // Apache was configured for the fake PHP and the docroot got its welcome page.
    let conf = std::fs::read_to_string(env.paths.config(Tool::Apache, APACHE).join("httpd.conf")).unwrap();
    assert!(conf.contains(&format!("proxy:fcgi://127.0.0.1:{}", env.php_port())));
    assert!(env.paths.www().join("index.php").exists());

    // Starting again is a no-op.
    sup.start(Tool::Apache, &env.state).await.unwrap();

    sup.stop_all().await;
    for s in sup.status().await {
        assert!(!s.running && s.error.is_none(), "{s:?}");
        assert!(!env.pid_file(s.tool).exists());
    }
    assert!(wait_until(|| ports::is_free(env.state.ports.http)).await, "the port must be released");
}

async fn other_process_sees_services_as_external_and_can_stop_them(env: Env) {
    let app = Supervisor::new(env.paths.clone());
    app.start(Tool::Php, &env.state).await.unwrap();
    let pid = app.status().await.into_iter().find(|s| s.tool == Tool::Php).unwrap().pid.unwrap();

    // The CLI is another process: simulate it by rewriting the owner in the PID file.
    let pid_file = env.pid_file(Tool::Php);
    let record = std::fs::read_to_string(&pid_file).unwrap().replace(&format!("\n{}\n", std::process::id()), "\n1\n");
    std::fs::write(&pid_file, record).unwrap();

    let cli = Supervisor::new(env.paths.clone());
    let php = cli.status().await.into_iter().find(|s| s.tool == Tool::Php).unwrap();
    assert!(php.running && php.external, "{php:?}");
    assert_eq!((php.pid, php.port, php.version.as_deref()), (Some(pid), Some(env.php_port()), Some(PHP)));
    cli.start(Tool::Php, &env.state).await.unwrap();

    cli.stop(Tool::Php).await.unwrap();
    assert!(!pid_file.exists());
    assert!(wait_until(|| ports::is_free(env.php_port())).await, "the external service must be stopped");
    // Reap our own child so its handle does not linger.
    app.stop(Tool::Php).await.unwrap();
}

async fn shutdown_only_stops_own_services(env: Env) {
    let sup = Supervisor::new(env.paths.clone());
    sup.start(Tool::Mariadb, &env.state).await.unwrap();
    // Another supervisor in the same process (the app next to a CLI) has no children of its own.
    let other = Supervisor::new(env.paths.clone());
    other.shutdown().await;
    assert!(ports::is_open(env.state.ports.mariadb).await, "shutdown must not stop other launchers' services");

    sup.shutdown().await;
    assert!(wait_until(|| ports::is_free(env.state.ports.mariadb)).await);
    assert!(!env.pid_file(Tool::Mariadb).exists());
}

async fn detects_a_service_that_dies(env: Env) {
    let sup = Supervisor::new(env.paths.clone());
    sup.start(Tool::Php, &env.state).await.unwrap();
    let pid = sup.status().await.into_iter().find(|s| s.tool == Tool::Php).unwrap().pid.unwrap();
    kill(pid);

    let mut php = None;
    for _ in 0..50 {
        let s = sup.status().await.into_iter().find(|s| s.tool == Tool::Php).unwrap();
        if !s.running {
            php = Some(s);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let php = php.expect("the supervisor must notice the exit");
    assert!(php.error.is_some_and(|e| !e.is_empty()));
    assert!(!env.pid_file(Tool::Php).exists());

    // A new start clears the error.
    sup.start(Tool::Php, &env.state).await.unwrap();
    let php = sup.status().await.into_iter().find(|s| s.tool == Tool::Php).unwrap();
    assert!(php.running && php.error.is_none());
    sup.stop_all().await;
}

async fn refuses_a_busy_port(env: Env) {
    let _busy = TcpListener::bind(("127.0.0.1", env.state.ports.http)).unwrap();
    let sup = Supervisor::new(env.paths.clone());
    let err = sup.start(Tool::Apache, &env.state).await.unwrap_err();
    assert!(matches!(err, ampp_core::Error::PortInUse { tool: Tool::Apache, port } if port == env.state.ports.http));
    let apache = sup.status().await.into_iter().find(|s| s.tool == Tool::Apache).unwrap();
    assert!(!apache.running && apache.error.is_some(), "{apache:?}");
    assert!(!env.pid_file(Tool::Apache).exists());
}

async fn reports_the_log_when_a_service_exits_at_start(env: Env) {
    // SAFETY: single-threaded test sequencing; the variable is cleared by the next Env::new.
    unsafe { std::env::set_var(FAIL, "3") };
    let sup = Supervisor::new(env.paths.clone());
    let err = sup.start(Tool::Php, &env.state).await.unwrap_err();
    let msg = err.to_string();
    assert!(matches!(err, ampp_core::Error::ServiceFailed { tool: Tool::Php, .. }), "{msg}");
    assert!(msg.contains("failing on purpose"), "the log tail must be in the error: {msg}");
    assert!(!env.pid_file(Tool::Php).exists());
    assert!(sup.log_file(Tool::Php).exists());
}

async fn errors_without_active_or_installed_version(mut env: Env) {
    let sup = Supervisor::new(env.paths.clone());

    std::fs::remove_dir_all(env.paths.bin(Tool::Mariadb, MARIADB)).unwrap();
    let err = sup.start(Tool::Mariadb, &env.state).await.unwrap_err();
    assert!(matches!(err, ampp_core::Error::NotInstalled { tool: Tool::Mariadb, .. }), "{err}");

    env.state.active.remove(&Tool::Php);
    let err = sup.start(Tool::Php, &env.state).await.unwrap_err();
    assert!(matches!(err, ampp_core::Error::NoActiveVersion(Tool::Php)), "{err}");
    // Apache needs the active PHP to know where to proxy.
    let err = sup.start(Tool::Apache, &env.state).await.unwrap_err();
    assert!(matches!(err, ampp_core::Error::NoActiveVersion(Tool::Php)), "{err}");

    // start_all skips tools without an active version and reports the rest.
    let failures = sup.start_all(&env.state).await;
    assert_eq!(failures.len(), 2, "{failures:?}");
    assert!(matches!(failures[0], ampp_core::Error::NotInstalled { tool: Tool::Mariadb, .. }));
    assert!(matches!(failures[1], ampp_core::Error::NoActiveVersion(Tool::Php)), "Apache needs the active PHP");
}

async fn cleanup_removes_stale_pid_files(env: Env) {
    // Garbage, and a record whose process does not exist.
    std::fs::write(env.pid_file(Tool::Apache), "not a pid file").unwrap();
    std::fs::write(env.pid_file(Tool::Php), format!("{}\n{}\n{PHP}\n1\n", u32::MAX - 1, env.php_port())).unwrap();

    // A live MariaDB whose launcher is gone: it is an orphan and must be stopped.
    let sup = Supervisor::new(env.paths.clone());
    sup.start(Tool::Mariadb, &env.state).await.unwrap();
    let pid_file = env.pid_file(Tool::Mariadb);
    let record = std::fs::read_to_string(&pid_file).unwrap().replace(&format!("\n{}\n", std::process::id()), &format!("\n{}\n", u32::MAX - 2));
    std::fs::write(&pid_file, record).unwrap();

    Supervisor::new(env.paths.clone()).cleanup_orphans().await;
    for tool in Tool::ALL {
        assert!(!env.pid_file(tool).exists(), "{tool:?} PID file must be gone");
    }
    assert!(wait_until(|| ports::is_free(env.state.ports.mariadb)).await, "the orphan must be stopped");
    sup.stop_all().await;
}

async fn projects_run_their_own_php_version(mut env: Env) {
    let php = if cfg!(windows) { "php-cgi.exe" } else { "php-fpm" };
    fake_binary(&env.paths.bin(Tool::Php, PHP_OLD).join(php));
    env.state.mark_installed(Tool::Php, PHP_OLD);
    let site = env.paths.root().join("sites/legacy");
    std::fs::create_dir_all(&site).unwrap();
    let legacy = Project {
        name: "legacy".into(),
        domain: "legacy.localhost".into(),
        docroot: site.clone(),
        php: Some("8.2".into()),
        https: false,
    };
    projects::save(&env.paths, &mut env.state, None, legacy).unwrap();
    // Both FastCGI ports (base + 84 and base + 82) must be free and apart from the others.
    let taken = [env.state.ports.http, env.state.ports.mariadb];
    env.state.ports.php_fcgi_base = loop {
        let base = free_port() - 84;
        if [base + 82, base + 84].iter().all(|p| ports::is_free(*p) && !taken.contains(p)) {
            break base;
        }
    };
    let old_port = ports::php_fcgi_port(env.state.ports.php_fcgi_base, PHP_OLD);
    let extra_pid = env.paths.run().join(format!("php@{PHP_OLD}.pid"));

    let sup = Supervisor::new(env.paths.clone());
    let failures = sup.start_all(&env.state).await;
    assert!(failures.is_empty(), "{:?}", failures.iter().map(|e| e.to_string()).collect::<Vec<_>>());
    assert!(ports::is_open(env.php_port()).await && ports::is_open(old_port).await, "both PHP versions must run");
    let status = sup.status().await;
    let php_status = status.iter().find(|s| s.tool == Tool::Php).unwrap();
    assert_eq!(php_status.version.as_deref(), Some(PHP));
    let extra: Vec<(&str, u16)> = php_status.extra_php.iter().map(|e| (e.version.as_str(), e.port)).collect();
    assert_eq!(extra, [(PHP_OLD, old_port)]);
    assert!(php_status.extra_php[0].pid.is_some());
    assert!(extra_pid.exists());
    let conf = std::fs::read_to_string(env.paths.config(Tool::Apache, APACHE).join("httpd.conf")).unwrap();
    assert!(conf.contains("ServerName legacy.localhost"));
    assert!(conf.contains(&format!("proxy:fcgi://127.0.0.1:{old_port}")));

    // Another process sees the extra version too.
    let record = std::fs::read_to_string(&extra_pid).unwrap().replace(&format!("\n{}\n", std::process::id()), "\n1\n");
    std::fs::write(&extra_pid, record).unwrap();
    let cli = Supervisor::new(env.paths.clone());
    let php_status = cli.status().await.into_iter().find(|s| s.tool == Tool::Php).unwrap();
    let extra: Vec<(&str, u16)> = php_status.extra_php.iter().map(|e| (e.version.as_str(), e.port)).collect();
    assert_eq!(extra, [(PHP_OLD, old_port)]);
    std::fs::write(&extra_pid, std::fs::read_to_string(&extra_pid).unwrap().replace("\n1\n", &format!("\n{}\n", std::process::id()))).unwrap();

    // Without the project, starting PHP again stops the version it no longer needs.
    projects::remove(&mut env.state, "legacy").unwrap();
    sup.start(Tool::Php, &env.state).await.unwrap();
    assert!(wait_until(|| ports::is_free(old_port)).await, "the unneeded version must stop");
    assert!(!extra_pid.exists());
    assert!(sup.status().await.into_iter().find(|s| s.tool == Tool::Php).unwrap().extra_php.is_empty());

    // Stopping PHP stops every version.
    projects::save(&env.paths, &mut env.state, None, Project {
        name: "legacy".into(),
        domain: "legacy.localhost".into(),
        docroot: site,
        php: Some("8.2".into()),
        https: false,
    })
    .unwrap();
    sup.start(Tool::Php, &env.state).await.unwrap();
    assert!(ports::is_open(old_port).await);
    sup.stop(Tool::Php).await.unwrap();
    assert!(wait_until(|| ports::is_free(old_port) && ports::is_free(env.php_port())).await);
    assert!(!extra_pid.exists());

    // A stale extra PID file is cleaned up like the others.
    std::fs::write(&extra_pid, format!("{}\n{old_port}\n{PHP_OLD}\n1\n", u32::MAX - 1)).unwrap();
    Supervisor::new(env.paths.clone()).cleanup_orphans().await;
    assert!(!extra_pid.exists());
    sup.stop_all().await;
}

async fn mailpit_runs_with_the_services_when_installed(mut env: Env) {
    use ampp_core::tools::{self, Extra};
    env.state.ports.mail_smtp = free_port();
    env.state.ports.mail_web = free_port();
    let web = env.state.ports.mail_web;
    let sup = Supervisor::new(env.paths.clone());

    // Not installed: start_all leaves it out and its status says so.
    assert!(sup.start_all(&env.state).await.is_empty());
    let mail = sup.mail_status().await;
    assert!(!mail.installed && !mail.running && mail.error.is_none(), "{mail:?}");
    sup.stop_all().await;

    fake_binary(&tools::file(&env.paths, Extra::Mailpit));
    env.state.extras.insert(Extra::Mailpit, "1.32.0".into());
    assert!(sup.start_all(&env.state).await.is_empty());
    let mail = sup.mail_status().await;
    assert!(mail.installed && mail.running && !mail.external, "{mail:?}");
    assert_eq!((mail.version.as_deref(), mail.web_port), (Some("1.32.0"), Some(web)));
    assert!(ports::is_open(web).await);
    assert!(env.paths.run().join("mailpit.pid").exists());
    assert!(env.paths.root().join("data/mailpit").is_dir(), "messages are kept between restarts");

    // A busy SMTP port is reported instead of failing silently.
    sup.stop_mail().await;
    assert!(wait_until(|| ports::is_free(web)).await);
    let _busy = TcpListener::bind(("127.0.0.1", env.state.ports.mail_smtp)).unwrap();
    let err = sup.start_mail(&env.state).await.unwrap_err();
    assert!(matches!(err, ampp_core::Error::ToolFailed { .. }) && err.to_string().contains("Mailpit"), "{err}");
    assert!(sup.mail_status().await.error.is_some());
    drop(_busy);

    sup.start_mail(&env.state).await.unwrap();
    sup.stop_all().await;
    assert!(wait_until(|| ports::is_free(web)).await, "stop_all stops Mailpit too");
    assert!(!env.paths.run().join("mailpit.pid").exists());
}
