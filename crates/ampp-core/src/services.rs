//! Starting, stopping and supervising Apache, PHP (FastCGI) and MariaDB.

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
use crate::state::State;
use crate::tools;

const START_TIMEOUT: Duration = Duration::from_secs(20);
const STOP_TIMEOUT: Duration = Duration::from_secs(10);
/// Trailing log bytes included in the message when a service fails.
const LOG_TAIL_ON_ERROR: u64 = 1500;

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

struct Running {
    child: Child,
    inst: Installation,
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
    procs: Mutex<HashMap<Tool, Running>>,
    errors: Mutex<HashMap<Tool, String>>,
}

impl Supervisor {
    pub fn new(paths: AppPaths) -> Self {
        Self { paths, procs: Mutex::new(HashMap::new()), errors: Mutex::new(HashMap::new()) }
    }

    pub fn log_file(&self, tool: Tool) -> PathBuf {
        self.paths.logs().join(format!("{}.log", tool.as_str()))
    }

    /// Status of the three services. Detects processes that exited on their own.
    pub async fn status(&self) -> Vec<ServiceStatus> {
        let mut procs = self.procs.lock().await;
        let mut errors = self.errors.lock().await;

        let exited: Vec<Tool> = procs
            .iter_mut()
            .filter_map(|(tool, r)| r.child.try_wait().ok().flatten().map(|code| (*tool, code)))
            .map(|(tool, code)| {
                errors.insert(tool, tr!("el proceso terminó inesperadamente ({code})", "the process exited unexpectedly ({code})"));
                tool
            })
            .collect();
        for tool in exited {
            procs.remove(&tool);
            let _ = fs::remove_file(self.pid_file(tool));
        }

        Tool::ALL
            .iter()
            .map(|&tool| match procs.get(&tool) {
                Some(r) => ServiceStatus {
                    tool,
                    running: true,
                    pid: r.child.id(),
                    version: Some(r.inst.version.clone()),
                    port: Some(r.port),
                    error: None,
                    external: false,
                },
                None => match self.external(tool) {
                    Some(rec) => ServiceStatus {
                        tool,
                        running: true,
                        pid: Some(rec.pid),
                        version: Some(rec.id),
                        port: Some(rec.port),
                        error: None,
                        external: true,
                    },
                    None => ServiceStatus {
                        tool,
                        running: false,
                        pid: None,
                        version: None,
                        port: None,
                        error: errors.get(&tool).cloned(),
                        external: false,
                    },
                },
            })
            .collect()
    }

    pub async fn start(&self, tool: Tool, state: &State) -> Result<()> {
        if self.procs.lock().await.contains_key(&tool) || self.external(tool).is_some() {
            return Ok(());
        }
        self.errors.lock().await.remove(&tool);
        let result = self.start_inner(tool, state).await;
        if let Err(e) = &result {
            self.errors.lock().await.insert(tool, e.to_string());
        }
        result
    }

    async fn start_inner(&self, tool: Tool, state: &State) -> Result<()> {
        let id = state.active.get(&tool).ok_or(Error::NoActiveVersion(tool))?;
        let inst = install::locate(&self.paths, tool, id)?;
        check_runtime(tool)?;
        let launch = self.prepare(&inst, state).await?;

        if !ports::is_free(launch.port) {
            return Err(Error::PortInUse { tool, port: launch.port });
        }

        let log_path = self.log_file(tool);
        let mut child = spawn(&launch, &log_path)?;
        if let Some(pid) = child.id() {
            let record = PidRecord { pid, port: launch.port, id: inst.id.clone(), owner: std::process::id() };
            let pid_file = self.pid_file(tool);
            fs::write(&pid_file, record.render()).at(&pid_file)?;
        }

        // Wait until the port accepts connections or the process dies.
        let deadline = tokio::time::Instant::now() + START_TIMEOUT;
        loop {
            if let Some(code) = child.try_wait().at(&launch.program)? {
                let _ = fs::remove_file(self.pid_file(tool));
                return Err(Error::ServiceFailed {
                    tool,
                    reason: {
                        let log = tail(&log_path, LOG_TAIL_ON_ERROR);
                        tr!("terminó con {code}.\n{log}", "exited with {code}.\n{log}")
                    },
                });
            }
            if ports::is_open(launch.port).await {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                terminate(&mut child, None).await;
                let _ = fs::remove_file(self.pid_file(tool));
                return Err(Error::ServiceFailed {
                    tool,
                    reason: {
                        let (port, secs) = (launch.port, START_TIMEOUT.as_secs());
                        let log = tail(&log_path, LOG_TAIL_ON_ERROR);
                        tr!(
                            "no respondió en el puerto {port} tras {secs}s.\n{log}",
                            "did not answer on port {port} after {secs}s.\n{log}"
                        )
                    },
                });
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }

        self.procs.lock().await.insert(tool, Running { child, inst, port: launch.port });
        Ok(())
    }

    pub async fn stop(&self, tool: Tool) -> Result<()> {
        let Some(mut running) = self.procs.lock().await.remove(&tool) else {
            if let Some(rec) = self.external(tool) {
                self.stop_external(tool, &rec).await;
            }
            return Ok(());
        };
        let graceful =
            if tool == Tool::Mariadb { mariadb_shutdown_command(&running.inst, running.port) } else { None };
        terminate(&mut running.child, graceful).await;
        let _ = fs::remove_file(self.pid_file(tool));
        self.errors.lock().await.remove(&tool);
        Ok(())
    }

    /// Starts in dependency order: PHP and MariaDB before Apache.
    pub async fn start_all(&self, state: &State) -> Vec<(Tool, Error)> {
        let mut failures = Vec::new();
        for tool in [Tool::Php, Tool::Mariadb, Tool::Apache] {
            if state.active.contains_key(&tool) {
                if let Err(e) = self.start(tool, state).await {
                    failures.push((tool, e));
                }
            }
        }
        failures
    }

    pub async fn stop_all(&self) {
        for tool in [Tool::Apache, Tool::Php, Tool::Mariadb] {
            let _ = self.stop(tool).await;
        }
    }

    /// Stops only the services launched by this process (when the app or the CLI exits).
    pub async fn shutdown(&self) {
        for tool in [Tool::Apache, Tool::Php, Tool::Mariadb] {
            let owned = self.procs.lock().await.contains_key(&tool);
            if owned {
                let _ = self.stop(tool).await;
            }
        }
    }

    /// Stops services left running by a previous session whose launcher is gone (e.g. the app
    /// crashed). Services owned by another live process, such as `ampp up`, are left alone.
    pub async fn cleanup_orphans(&self) {
        for tool in Tool::ALL {
            let Some(rec) = self.read_pid(tool) else {
                let _ = fs::remove_file(self.pid_file(tool));
                continue;
            };
            match process_alive(rec.pid, expected_process_names(tool)) {
                false => {
                    let _ = fs::remove_file(self.pid_file(tool));
                }
                true if !owner_alive(rec.owner) => self.stop_external(tool, &rec).await,
                true => {}
            }
        }
    }

    /// Running service launched by another process, according to its PID file.
    fn external(&self, tool: Tool) -> Option<PidRecord> {
        let rec = self.read_pid(tool)?;
        (rec.owner != std::process::id() && process_alive(rec.pid, expected_process_names(tool))).then_some(rec)
    }

    /// Graceful stop of a process that is not our child.
    async fn stop_external(&self, tool: Tool, rec: &PidRecord) {
        #[cfg(unix)]
        let _ = nix::sys::signal::killpg(nix::unistd::Pid::from_raw(rec.pid as i32), nix::sys::signal::Signal::SIGTERM);
        #[cfg(windows)]
        {
            let graceful = (tool == Tool::Mariadb)
                .then(|| install::locate(&self.paths, tool, &rec.id).ok())
                .flatten()
                .and_then(|inst| mariadb_shutdown_command(&inst, rec.port));
            match graceful {
                Some(mut cmd) => {
                    let _ = cmd.status().await;
                }
                None => kill_tree(rec.pid),
            }
        }
        let names = expected_process_names(tool);
        let deadline = tokio::time::Instant::now() + STOP_TIMEOUT;
        while process_alive(rec.pid, names) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        if process_alive(rec.pid, names) {
            kill_tree(rec.pid);
        }
        let _ = fs::remove_file(self.pid_file(tool));
    }

    fn read_pid(&self, tool: Tool) -> Option<PidRecord> {
        PidRecord::parse(&fs::read_to_string(self.pid_file(tool)).ok()?)
    }

    fn pid_file(&self, tool: Tool) -> PathBuf {
        self.paths.run().join(format!("{}.pid", tool.as_str()))
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
                let extra = tools::phpmyadmin_apache_conf(&self.paths, state.ports.mariadb)?;
                let conf =
                    config::write_apache(&self.paths, inst, state.ports.http, php_port(state)?, &docroot, &extra)?;
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
            cmd.args([
                "--no-defaults".to_string(),
                format!("--basedir={}", path_arg(&inst.root)),
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

fn mariadb_shutdown_command(inst: &Installation, port: u16) -> Option<Command> {
    let admin = inst.mariadb_admin()?;
    let mut cmd = Command::new(admin);
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
