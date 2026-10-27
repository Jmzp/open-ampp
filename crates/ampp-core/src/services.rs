//! Starting, stopping and supervising Apache, PHP (FastCGI), MariaDB and, when installed, Mailpit.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use crate::config;
use crate::error::{Error, IoContext, Result};
use crate::install::{self, Installation};
use crate::paths::{AppPaths, Tool};
use crate::ports;
use crate::projects;
use crate::state::State;
use crate::tools::{self, Extra};

const START_TIMEOUT: Duration = Duration::from_secs(20);
const STOP_TIMEOUT: Duration = Duration::from_secs(10);
/// Trailing log bytes included in the message when a service fails.
const LOG_TAIL_ON_ERROR: u64 = 1500;
/// PID files of the extra PHP versions: `run/php@8.2.30.pid`.
const EXTRA_PHP_PREFIX: &str = "php@";

#[derive(Debug, Clone, Serialize)]
pub struct ServiceStatus {
    pub tool: Tool,
    pub running: bool,
    pub pid: Option<u32>,
    /// Actual running version.
    pub version: Option<String>,
    pub port: Option<u16>,
    /// Last error (failed start or unexpected exit).
    pub error: Option<String>,
    /// Launched by another Open AMPP process (e.g. `ampp up` in a terminal).
    pub external: bool,
    /// PHP only: other versions running for projects, newest first.
    pub extra_php: Vec<ExtraPhp>,
}

/// Mailpit, the local inbox: its web UI and the SMTP server PHP sends to.
#[derive(Debug, Clone, Serialize)]
pub struct MailStatus {
    pub installed: bool,
    pub running: bool,
    pub pid: Option<u32>,
    pub version: Option<String>,
    /// Web inbox port (the one checked to know it is up).
    pub web_port: Option<u16>,
    pub error: Option<String>,
    pub external: bool,
}

/// A PHP version running for projects besides the active one.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExtraPhp {
    pub version: String,
    pub port: u16,
    pub pid: Option<u32>,
}

/// Contents of `run/<tool>.pid`: the service, who launched it, and with which version and port.
#[derive(Debug, Clone, PartialEq)]
struct PidRecord {
    pid: u32,
    port: u16,
    id: String,
    /// Open AMPP process (app or CLI) that launched it.
    owner: u32,
}

impl PidRecord {
    fn parse(text: &str) -> Option<Self> {
        let mut lines = text.lines().map(str::trim);
        Some(Self {
            pid: lines.next()?.parse().ok()?,
            port: lines.next()?.parse().ok()?,
            id: lines.next()?.to_string(),
            owner: lines.next()?.parse().ok()?,
        })
    }

    fn render(&self) -> String {
        format!("{}\n{}\n{}\n{}\n", self.pid, self.port, self.id, self.owner)
    }
}

/// A supervised process: each service, one PHP per extra version that projects need, and Mailpit.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Unit {
    Service(Tool),
    /// PHP version (id) used by a project besides the active one.
    Php(String),
    Mail,
}

impl Unit {
    /// Order in which units stop: Apache first, MariaDB last.
    fn stop_rank(&self) -> u8 {
        match self {
            Unit::Service(Tool::Apache) => 0,
            Unit::Mail => 1,
            Unit::Php(_) => 2,
            Unit::Service(Tool::Php) => 3,
            Unit::Service(Tool::Mariadb) => 4,
        }
    }

    /// Where its errors are reported: the extra PHP versions under PHP.
    fn error_key(&self) -> Unit {
        match self {
            Unit::Php(_) => Unit::Service(Tool::Php),
            other => other.clone(),
        }
    }

    fn process_names(&self) -> &'static [&'static str] {
        match self {
            Unit::Service(tool) => expected_process_names(*tool),
            Unit::Php(_) => expected_process_names(Tool::Php),
            Unit::Mail => &["mailpit"],
        }
    }
}

struct Running {
    child: Child,
    /// `None` for Mailpit, which is a tool rather than an installed version.
    inst: Option<Installation>,
    version: String,
    port: u16,
}

/// Everything needed to launch an already configured service.
struct Launch {
    program: PathBuf,
    args: Vec<String>,
    env: Vec<(String, String)>,
    port: u16,
}

pub struct Supervisor {
    paths: AppPaths,
    procs: Mutex<HashMap<Unit, Running>>,
    errors: Mutex<HashMap<Unit, String>>,
}

impl Supervisor {
    pub fn new(paths: AppPaths) -> Self {
        Self { paths, procs: Mutex::new(HashMap::new()), errors: Mutex::new(HashMap::new()) }
    }

    pub fn log_file(&self, tool: Tool) -> PathBuf {
        self.paths.logs().join(format!("{}.log", tool.as_str()))
    }

    pub fn mail_log_file(&self) -> PathBuf {
        self.paths.logs().join("mailpit.log")
    }

    /// Forgets processes that exited on their own and records why.
    fn reap(&self, procs: &mut HashMap<Unit, Running>, errors: &mut HashMap<Unit, String>) {
        let exited: Vec<Unit> = procs
            .iter_mut()
            .filter_map(|(unit, r)| r.child.try_wait().ok().flatten().map(|code| (unit.clone(), code)))
            .map(|(unit, code)| {
                errors.insert(unit.error_key(), tr!("el proceso terminó inesperadamente ({code})", "the process exited unexpectedly ({code})"));
                unit
            })
            .collect();
        for unit in exited {
            procs.remove(&unit);
            let _ = fs::remove_file(self.pid_file(&unit));
        }
    }

    /// Status of the three services. Detects processes that exited on their own.
    pub async fn status(&self) -> Vec<ServiceStatus> {
        let mut procs = self.procs.lock().await;
        let mut errors = self.errors.lock().await;
        self.reap(&mut procs, &mut errors);

        // Extra PHP versions for projects, launched by us or by another process.
        let mut extra_php: Vec<ExtraPhp> = procs
            .iter()
            .filter(|(unit, _)| matches!(unit, Unit::Php(_)))
            .map(|(_, r)| ExtraPhp { version: r.version.clone(), port: r.port, pid: r.child.id() })
            .chain(
                self.extra_php_units()
                    .into_iter()
                    .filter(|unit| !procs.contains_key(unit))
                    .filter_map(|unit| self.external(&unit))
                    .map(|rec| ExtraPhp { version: rec.id, port: rec.port, pid: Some(rec.pid) }),
            )
            .collect();
        extra_php.sort_by(|a, b| crate::catalog::compare_versions(&b.version, &a.version));

        Tool::ALL
            .iter()
            .map(|&tool| {
                let extra_php = if tool == Tool::Php { extra_php.clone() } else { Vec::new() };
                match procs.get(&Unit::Service(tool)) {
                    Some(r) => ServiceStatus {
                        tool,
                        running: true,
                        pid: r.child.id(),
                        version: Some(r.version.clone()),
                        port: Some(r.port),
                        error: None,
                        external: false,
                        extra_php,
                    },
                    None => match self.external(&Unit::Service(tool)) {
                        Some(rec) => ServiceStatus {
                            tool,
                            running: true,
                            pid: Some(rec.pid),
                            version: Some(rec.id),
                            port: Some(rec.port),
                            error: None,
                            external: true,
                            extra_php,
                        },
                        None => ServiceStatus {
                            tool,
                            running: false,
                            pid: None,
                            version: None,
                            port: None,
                            error: errors.get(&Unit::Service(tool)).cloned(),
                            external: false,
                            extra_php,
                        },
                    },
                }
            })
            .collect()
    }

    /// Starts a service. For PHP it also starts the other versions that projects need (and stops
    /// the ones no longer needed), even when the active one is already running.
    pub async fn start(&self, tool: Tool, state: &State) -> Result<()> {
        let unit = Unit::Service(tool);
        let running = self.procs.lock().await.contains_key(&unit) || self.external(&unit).is_some();
        if !running {
            self.errors.lock().await.remove(&unit);
            let result = self.start_inner(tool, state).await;
            if let Err(e) = &result {
                self.errors.lock().await.insert(unit, e.to_string());
            }
            result?;
        }
        if tool == Tool::Php { self.sync_php_extras(state).await } else { Ok(()) }
    }

    async fn start_inner(&self, tool: Tool, state: &State) -> Result<()> {
        let id = state.active.get(&tool).ok_or(Error::NoActiveVersion(tool))?;
        let inst = install::locate(&self.paths, tool, id)?;
        let unit = Unit::Service(tool);
        let running = self.launch(&unit, tool, inst, state).await?;
        self.procs.lock().await.insert(unit, running);
        Ok(())
    }

    /// Starts the extra PHP versions that projects need and stops the ones no longer needed.
    async fn sync_php_extras(&self, state: &State) -> Result<()> {
        let wanted = projects::extra_php(&self.paths, state);
        let stale: Vec<Unit> = self
            .procs
            .lock()
            .await
            .keys()
            .filter(|unit| matches!(unit, Unit::Php(id) if !wanted.iter().any(|w| &w.id == id)))
            .cloned()
            .collect();
        for unit in stale {
            self.stop_unit(&unit).await;
        }
        for inst in wanted {
            let unit = Unit::Php(inst.id.clone());
            if self.procs.lock().await.contains_key(&unit) || self.external(&unit).is_some() {
                continue;
            }
            let running = self.launch(&unit, Tool::Php, inst, state).await?;
            self.procs.lock().await.insert(unit, running);
        }
        Ok(())
    }

    /// Configures and spawns a service unit, then waits until its port accepts connections.
    async fn launch(&self, unit: &Unit, tool: Tool, inst: Installation, state: &State) -> Result<Running> {
        check_runtime(tool)?;
        let launch = self.prepare(&inst, state).await?;

        if !ports::is_free(launch.port) {
            return Err(Error::PortInUse { tool, port: launch.port });
        }
        let fail = |reason| Error::ServiceFailed { tool, reason };
        let child = self.spawn_and_wait(unit, &launch, &inst.id, &self.log_file(tool), fail).await?;
        Ok(Running { child, version: inst.version.clone(), inst: Some(inst), port: launch.port })
    }

    /// Spawns a unit with its PID file, then waits until its port accepts connections or it dies.
    async fn spawn_and_wait(
        &self,
        unit: &Unit,
        launch: &Launch,
        id: &str,
        log_path: &Path,
        fail: impl Fn(String) -> Error,
    ) -> Result<Child> {
        let pid_file = self.pid_file(unit);
        let mut child = spawn(launch, log_path)?;
        if let Some(pid) = child.id() {
            let record = PidRecord { pid, port: launch.port, id: id.to_string(), owner: std::process::id() };
            fs::write(&pid_file, record.render()).at(&pid_file)?;
        }

        // Wait until the port accepts connections or the process dies.
        let deadline = tokio::time::Instant::now() + START_TIMEOUT;
        loop {
            if let Some(code) = child.try_wait().at(&launch.program)? {
                let _ = fs::remove_file(&pid_file);
                let log = tail(log_path, LOG_TAIL_ON_ERROR);
                return Err(fail(tr!("terminó con {code}.\n{log}", "exited with {code}.\n{log}")));
            }
            if ports::is_open(launch.port).await {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                terminate(&mut child, None).await;
                let _ = fs::remove_file(&pid_file);
                let (port, secs) = (launch.port, START_TIMEOUT.as_secs());
                let log = tail(log_path, LOG_TAIL_ON_ERROR);
                return Err(fail(tr!(
                    "no respondió en el puerto {port} tras {secs}s.\n{log}",
                    "did not answer on port {port} after {secs}s.\n{log}"
                )));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        Ok(child)
    }

    /// Starts Mailpit if it is installed and not running yet. Its messages are kept in
    /// `data/mailpit/` so they survive restarts.
    pub async fn start_mail(&self, state: &State) -> Result<()> {
        let unit = Unit::Mail;
        if self.procs.lock().await.contains_key(&unit) || self.external(&unit).is_some() {
            return Ok(());
        }
        self.errors.lock().await.remove(&unit);
        let result = self.start_mail_inner(state).await;
        if let Err(e) = &result {
            self.errors.lock().await.insert(unit, e.to_string());
        }
        result
    }

    async fn start_mail_inner(&self, state: &State) -> Result<()> {
        let name = Extra::Mailpit.label();
        let fail = |reason: String| Error::ToolFailed { name: name.into(), reason };
        let program = tools::file(&self.paths, Extra::Mailpit);
        if !program.exists() {
            return Err(fail(tr!("no está instalado", "it is not installed")));
        }
        let (smtp, web) = (state.ports.mail_smtp, state.ports.mail_web);
        for port in [smtp, web] {
            if !ports::is_free(port) {
                return Err(fail(tr!("el puerto {port} ya está en uso", "port {port} is already in use")));
            }
        }
        let data = self.paths.root().join("data").join("mailpit");
        fs::create_dir_all(&data).at(&data)?;
        let launch = Launch {
            program,
            args: vec![
                "--listen".into(),
                format!("127.0.0.1:{web}"),
                "--smtp".into(),
                format!("127.0.0.1:{smtp}"),
                "--database".into(),
                path_arg(&data.join("mailpit.db")),
            ],
            env: Vec::new(),
            port: web,
        };
        let version = state.extras.get(&Extra::Mailpit).cloned().unwrap_or_default();
        let child = self.spawn_and_wait(&Unit::Mail, &launch, &version, &self.mail_log_file(), fail).await?;
        self.procs.lock().await.insert(Unit::Mail, Running { child, inst: None, version, port: web });
        Ok(())
    }

    pub async fn stop_mail(&self) {
        self.stop_unit(&Unit::Mail).await;
        self.errors.lock().await.remove(&Unit::Mail);
    }

    pub async fn mail_status(&self) -> MailStatus {
        let mut procs = self.procs.lock().await;
        let mut errors = self.errors.lock().await;
        self.reap(&mut procs, &mut errors);
        let installed = tools::file(&self.paths, Extra::Mailpit).exists();
        match (procs.get(&Unit::Mail), self.external(&Unit::Mail)) {
            (Some(r), _) => MailStatus {
                installed,
                running: true,
                pid: r.child.id(),
                version: Some(r.version.clone()),
                web_port: Some(r.port),
                error: None,
                external: false,
            },
            (None, Some(rec)) => MailStatus {
                installed,
                running: true,
                pid: Some(rec.pid),
                version: Some(rec.id),
                web_port: Some(rec.port),
                error: None,
                external: true,
            },
            (None, None) => MailStatus {
                installed,
                running: false,
                pid: None,
                version: None,
                web_port: None,
                error: errors.get(&Unit::Mail).cloned(),
                external: false,
            },
        }
    }

    /// Stops a service, even if another process launched it. Stopping PHP stops every version.
    pub async fn stop(&self, tool: Tool) -> Result<()> {
        if tool == Tool::Php {
            let mut extras = self.extra_php_units();
            extras.extend(self.procs.lock().await.keys().filter(|u| matches!(u, Unit::Php(_))).cloned());
            extras.sort();
            extras.dedup();
            for unit in extras {
                self.stop_unit(&unit).await;
            }
        }
        self.stop_unit(&Unit::Service(tool)).await;
        self.errors.lock().await.remove(&Unit::Service(tool));
        Ok(())
    }

    async fn stop_unit(&self, unit: &Unit) {
        let Some(mut running) = self.procs.lock().await.remove(unit) else {
            if let Some(rec) = self.external(unit) {
                self.stop_external(unit, &rec).await;
            }
            return;
        };
        let graceful = match (unit, &running.inst) {
            (Unit::Service(Tool::Mariadb), Some(inst)) => mariadb_shutdown_command(&self.paths, inst, running.port),
            _ => None,
        };
        terminate(&mut running.child, graceful).await;
        let _ = fs::remove_file(self.pid_file(unit));
    }

    /// Starts in dependency order (PHP and MariaDB before Apache), plus Mailpit if installed.
    /// Each error names the service it comes from.
    pub async fn start_all(&self, state: &State) -> Vec<Error> {
        let mut failures = Vec::new();
        for tool in [Tool::Php, Tool::Mariadb, Tool::Apache] {
            if state.active.contains_key(&tool) {
                if let Err(e) = self.start(tool, state).await {
                    failures.push(e);
                }
            }
        }
        if tools::file(&self.paths, Extra::Mailpit).exists() {
            if let Err(e) = self.start_mail(state).await {
                failures.push(e);
            }
        }
        failures
    }

    pub async fn stop_all(&self) {
        self.stop_mail().await;
        for tool in [Tool::Apache, Tool::Php, Tool::Mariadb] {
            let _ = self.stop(tool).await;
        }
    }

    /// Stops only the services launched by this process (when the app or the CLI exits).
    pub async fn shutdown(&self) {
        let mut owned: Vec<Unit> = self.procs.lock().await.keys().cloned().collect();
        owned.sort_by_key(Unit::stop_rank);
        for unit in owned {
            self.stop_unit(&unit).await;
        }
    }

    /// Stops services left running by a previous session whose launcher is gone (e.g. the app
    /// crashed). Services owned by another live process, such as `ampp up`, are left alone.
    pub async fn cleanup_orphans(&self) {
        let units = Tool::ALL.into_iter().map(Unit::Service).chain(self.extra_php_units()).chain([Unit::Mail]);
        for unit in units {
            let Some(rec) = self.read_pid(&unit) else {
                let _ = fs::remove_file(self.pid_file(&unit));
                continue;
            };
            match process_alive(rec.pid, unit.process_names()) {
                false => {
                    let _ = fs::remove_file(self.pid_file(&unit));
                }
                true if !owner_alive(rec.owner) => self.stop_external(&unit, &rec).await,
                true => {}
            }
        }
    }

    /// Running unit launched by another process, according to its PID file.
    fn external(&self, unit: &Unit) -> Option<PidRecord> {
        let rec = self.read_pid(unit)?;
        (rec.owner != std::process::id() && process_alive(rec.pid, unit.process_names())).then_some(rec)
    }

    /// Graceful stop of a process that is not our child.
    async fn stop_external(&self, unit: &Unit, rec: &PidRecord) {
        #[cfg(unix)]
        let _ = nix::sys::signal::killpg(nix::unistd::Pid::from_raw(rec.pid as i32), nix::sys::signal::Signal::SIGTERM);
        #[cfg(windows)]
        {
            let graceful = matches!(unit, Unit::Service(Tool::Mariadb))
                .then(|| install::locate(&self.paths, Tool::Mariadb, &rec.id).ok())
                .flatten()
                .and_then(|inst| mariadb_shutdown_command(&self.paths, &inst, rec.port));
            match graceful {
                Some(mut cmd) => {
                    let _ = cmd.status().await;
                }
                None => kill_tree(rec.pid),
            }
        }
        let names = unit.process_names();
        let deadline = tokio::time::Instant::now() + STOP_TIMEOUT;
        while process_alive(rec.pid, names) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        if process_alive(rec.pid, names) {
            kill_tree(rec.pid);
        }
        let _ = fs::remove_file(self.pid_file(unit));
    }

    fn read_pid(&self, unit: &Unit) -> Option<PidRecord> {
        PidRecord::parse(&fs::read_to_string(self.pid_file(unit)).ok()?)
    }

    /// `run/<tool>.pid` for the services, `run/php@<version>.pid` for the extra PHP versions,
    /// `run/mailpit.pid` for Mailpit.
    fn pid_file(&self, unit: &Unit) -> PathBuf {
        let name = match unit {
            Unit::Service(tool) => tool.as_str().to_string(),
            Unit::Php(id) => format!("{EXTRA_PHP_PREFIX}{id}"),
            Unit::Mail => "mailpit".to_string(),
        };
        self.paths.run().join(format!("{name}.pid"))
    }

    /// Extra PHP versions with a PID file, whoever launched them.
    fn extra_php_units(&self) -> Vec<Unit> {
        let Ok(entries) = fs::read_dir(self.paths.run()) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let id = name.strip_prefix(EXTRA_PHP_PREFIX)?.strip_suffix(".pid")?;
                Some(Unit::Php(id.to_string()))
            })
            .collect()
    }

    /// Writes the configuration and returns how to launch the service.
    async fn prepare(&self, inst: &Installation, state: &State) -> Result<Launch> {
        let missing = |what: &str| Error::ServiceFailed {
            tool: inst.tool,
            reason: {
                let root = inst.root.display();
                tr!("no se encontró {what} en {root}", "{what} was not found in {root}")
            },
        };
        let php_port = |s: &State| -> Result<u16> {
            let id = s.active.get(&Tool::Php).ok_or(Error::NoActiveVersion(Tool::Php))?;
            let php = install::locate(&self.paths, Tool::Php, id)?;
            Ok(ports::php_fcgi_port(s.ports.php_fcgi_base, &php.version))
        };

        match inst.tool {
            Tool::Php => {
                let port = ports::php_fcgi_port(state.ports.php_fcgi_base, &inst.version);
                let files = config::write_php(&self.paths, inst, state)?;
                if cfg!(windows) {
                    let program = inst.php_fastcgi().ok_or_else(|| missing("php-cgi.exe"))?;
                    Ok(Launch {
                        program,
                        args: vec!["-b".into(), format!("127.0.0.1:{port}"), "-c".into(), path_arg(&files.ini)],
                        // By default php-cgi exits after 500 requests.
                        env: vec![("PHP_FCGI_MAX_REQUESTS".into(), "0".into())],
                        port,
                    })
                } else {
                    let program = inst.php_fastcgi().ok_or_else(|| missing("php-fpm"))?;
                    let fpm = files.fpm_conf.expect("php-fpm.conf on Unix");
                    Ok(Launch {
                        program,
                        args: vec!["-F".into(), "-y".into(), path_arg(&fpm), "-c".into(), path_arg(&files.ini)],
                        env: Vec::new(),
                        port,
                    })
                }
            }
            Tool::Apache => {
                let program = inst.httpd().ok_or_else(|| missing("httpd"))?;
                let docroot = config::ensure_docroot(&self.paths)?;
                let extra = tools::phpmyadmin_apache_conf(&self.paths, state)?;
                let php_port = php_port(state)?;
                let sites = projects::sites(&self.paths, state, php_port)?;
                if let Some(tls) = &sites.tls {
                    if !ports::is_free(tls.port) {
                        return Err(Error::PortInUse { tool: Tool::Apache, port: tls.port });
                    }
                }
                let conf =
                    config::write_apache(&self.paths, inst, state.ports.http, php_port, &docroot, &sites, &extra)?;
                let mut args = vec!["-f".into(), path_arg(&conf)];
                if !cfg!(windows) {
                    args.push("-DFOREGROUND".into());
                }
                Ok(Launch { program, args, env: Vec::new(), port: state.ports.http })
            }
            Tool::Mariadb => {
                let program = inst.mariadbd().ok_or_else(|| missing("mariadbd"))?;
                let port = state.ports.mariadb;
                let datadir = self.paths.mariadb_data(&inst.minor_version());
                let conf = config::write_mariadb(&self.paths, inst, port, &datadir)?;
                self.init_mariadb(inst, &datadir, port).await?;
                // `--defaults-file` must be the first argument.
                Ok(Launch { program, args: vec![format!("--defaults-file={}", path_arg(&conf))], env: Vec::new(), port })
            }
        }
    }

    /// Initializes the datadir the first time. Root has no password (it only listens on 127.0.0.1).
    async fn init_mariadb(&self, inst: &Installation, datadir: &Path, port: u16) -> Result<()> {
        if datadir.join("mysql").is_dir() {
            return Ok(());
        }
        let tool = Tool::Mariadb;
        let script = inst.mariadb_install_db().ok_or_else(|| Error::ServiceFailed {
            tool,
            reason: {
                let root = inst.root.display();
                tr!("no se encontró mariadb-install-db en {root}", "mariadb-install-db was not found in {root}")
            },
        })?;
        fs::create_dir_all(datadir).at(datadir)?;

        let mut cmd = Command::new(&script);
        cmd.arg(format!("--datadir={}", path_arg(datadir)));
        if cfg!(windows) {
            cmd.arg(format!("--port={port}"));
        } else {
            // The script runs from the install root. A relative basedir dodges its unquoted
            // `$basedir/bin`, which breaks on paths with spaces (macOS's "Application Support").
            cmd.args([
                "--no-defaults".to_string(),
                "--basedir=.".into(),
                "--auth-root-authentication-method=normal".into(),
                "--skip-test-db".into(),
            ]);
        }
        let log_path = self.log_file(tool);
        let log = open_log(&log_path)?;
        cmd.current_dir(&inst.root)
            .stdin(Stdio::null())
            .stdout(log.try_clone().at(&log_path)?)
            .stderr(log);
        let status = cmd.status().await.at(&script)?;
        if !status.success() {
            // Never leave a half-initialized datadir: the next attempt must start from scratch.
            let _ = fs::remove_dir_all(datadir);
            return Err(Error::ServiceFailed {
                tool,
                reason: {
                    let log = tail(&log_path, LOG_TAIL_ON_ERROR);
                    tr!("mariadb-install-db falló ({status}).\n{log}", "mariadb-install-db failed ({status}).\n{log}")
                },
            });
        }
        Ok(())
    }
}

/// The official Windows builds of Apache and PHP need the Visual C++ runtime; without it they exit
/// with a cryptic "DLL not found" error, so check for it up front.
fn check_runtime(tool: Tool) -> Result<()> {
    if !cfg!(windows) || tool == Tool::Mariadb {
        return Ok(());
    }
    let system = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    if system.join("System32").join("vcruntime140.dll").exists() {
        return Ok(());
    }
    let url = "https://aka.ms/vs/17/release/vc_redist.x64.exe";
    Err(Error::ServiceFailed {
        tool,
        reason: tr!(
            "falta el Microsoft Visual C++ Redistributable. Instálalo desde {url} y vuelve a intentarlo.",
            "the Microsoft Visual C++ Redistributable is missing. Install it from {url} and try again."
        ),
    })
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn open_log(path: &Path) -> Result<fs::File> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).at(dir)?;
    }
    fs::OpenOptions::new().create(true).append(true).open(path).at(path)
}

fn spawn(launch: &Launch, log_path: &Path) -> Result<Child> {
    let log = open_log(log_path)?;
    let mut cmd = Command::new(&launch.program);
    cmd.args(&launch.args)
        .envs(launch.env.iter().cloned())
        .stdin(Stdio::null())
        .stdout(log.try_clone().at(log_path)?)
        .stderr(log);
    if let Some(dir) = launch.program.parent() {
        // On Windows, DLLs are looked up next to the executable and in the current directory.
        cmd.current_dir(dir);
    }
    // Own process group so signals also reach the children (workers).
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    cmd.spawn().at(&launch.program)
}

fn mariadb_shutdown_command(paths: &AppPaths, inst: &Installation, port: u16) -> Option<Command> {
    let admin = inst.mariadb_admin()?;
    let mut cmd = Command::new(admin);
    cmd.args(crate::db::defaults_arg(&crate::db::client_options_path(paths)));
    cmd.args(["--protocol=tcp", "-h", "127.0.0.1", "-P", &port.to_string(), "-u", "root", "shutdown"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    Some(cmd)
}

/// Graceful stop, forced if it does not finish in time.
/// On Unix `graceful` is ignored: SIGTERM already is a clean shutdown for all three services.
async fn terminate(child: &mut Child, graceful: Option<Command>) {
    let Some(pid) = child.id() else {
        return;
    };

    #[cfg(unix)]
    {
        drop(graceful);
        let _ = nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pid as i32), nix::sys::signal::Signal::SIGTERM);
    }
    #[cfg(windows)]
    {
        // Windows has no SIGTERM: MariaDB is shut down with mariadb-admin; the rest keep no state.
        match graceful {
            Some(mut cmd) => {
                let _ = cmd.status().await;
            }
            None => kill_tree(pid),
        }
    }

    if tokio::time::timeout(STOP_TIMEOUT, child.wait()).await.is_err() {
        kill_tree(pid);
        let _ = child.wait().await;
    }
}

/// Kills the process and its descendants without waiting.
fn kill_tree(pid: u32) {
    #[cfg(unix)]
    {
        let _ = nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pid as i32), nix::sys::signal::Signal::SIGKILL);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(0x0800_0000)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// `true` if `pid` exists and its executable is one of `names` (guards against reused PIDs).
fn process_alive(pid: u32, names: &[&str]) -> bool {
    use sysinfo::{Pid, ProcessesToUpdate, System};
    let pid = Pid::from_u32(pid);
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    sys.process(pid).is_some_and(|p| {
        let name = p.name().to_string_lossy().to_lowercase();
        names.iter().any(|n| name.starts_with(n))
    })
}

/// `true` if the Open AMPP process (app or CLI) that launched a service is still alive.
fn owner_alive(pid: u32) -> bool {
    process_alive(pid, &["open-ampp", "open ampp", "ampp"])
}

fn expected_process_names(tool: Tool) -> &'static [&'static str] {
    match tool {
        Tool::Apache => &["httpd"],
        Tool::Php => &["php-fpm", "php-cgi"],
        Tool::Mariadb => &["mariadbd", "mysqld"],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pid_record_roundtrip() {
        let rec = PidRecord { pid: 42, port: 3306, id: "11.8.9".into(), owner: 7 };
        assert_eq!(PidRecord::parse(&rec.render()), Some(rec));
        assert_eq!(PidRecord::parse("42"), None, "old format: ignored");
    }

    #[test]
    fn tail_returns_the_last_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("x.log");
        assert_eq!(tail(&log, 10), "", "a missing log is empty");
        fs::write(&log, "first line\nsecond line\n").unwrap();
        assert_eq!(tail(&log, 12), "second line\n");
        assert_eq!(tail(&log, 1000), "first line\nsecond line\n");
    }

    #[test]
    fn process_liveness_checks_the_name() {
        let me = std::process::id();
        let exe = std::env::current_exe().unwrap();
        let name = exe.file_stem().unwrap().to_string_lossy().to_lowercase();
        assert!(process_alive(me, &[&name[..4]]));
        assert!(!process_alive(me, &["httpd"]), "a reused PID with another name is not ours");
        assert!(!process_alive(u32::MAX - 1, &["httpd"]));
        assert!(!owner_alive(u32::MAX - 1));
        assert_eq!(expected_process_names(Tool::Php), ["php-fpm", "php-cgi"]);
    }
}

/// Last `max` bytes of a log file (empty if it does not exist).
pub fn tail(path: &Path, max: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = fs::File::open(path) else {
        return String::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = f.seek(SeekFrom::Start(len.saturating_sub(max)));
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}
