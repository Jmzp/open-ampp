//! Arranque, parada y supervisión de Apache, PHP (FastCGI) y MariaDB.

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

const START_TIMEOUT: Duration = Duration::from_secs(20);
const STOP_TIMEOUT: Duration = Duration::from_secs(10);
/// Bytes finales del log que se incluyen en el mensaje cuando un servicio falla.
const LOG_TAIL_ON_ERROR: u64 = 1500;

#[derive(Debug, Clone, Serialize)]
pub struct ServiceStatus {
    pub tool: Tool,
    pub running: bool,
    pub pid: Option<u32>,
    /// Versión real en ejecución.
    pub version: Option<String>,
    pub port: Option<u16>,
    /// Último error (arranque fallido o caída inesperada).
    pub error: Option<String>,
}

struct Running {
    child: Child,
    inst: Installation,
    port: u16,
}

/// Lo necesario para lanzar un servicio ya configurado.
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

    /// Estado de los tres servicios. Detecta procesos que terminaron por su cuenta.
    pub async fn status(&self) -> Vec<ServiceStatus> {
        let mut procs = self.procs.lock().await;
        let mut errors = self.errors.lock().await;

        let exited: Vec<Tool> = procs
            .iter_mut()
            .filter_map(|(tool, r)| r.child.try_wait().ok().flatten().map(|code| (*tool, code)))
            .map(|(tool, code)| {
                errors.insert(tool, format!("el proceso terminó inesperadamente ({code})"));
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
                },
                None => ServiceStatus {
                    tool,
                    running: false,
                    pid: None,
                    version: None,
                    port: None,
                    error: errors.get(&tool).cloned(),
                },
            })
            .collect()
    }

    pub async fn start(&self, tool: Tool, state: &State) -> Result<()> {
        if self.procs.lock().await.contains_key(&tool) {
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
        let launch = self.prepare(&inst, state).await?;

        if !ports::is_free(launch.port) {
            return Err(Error::PortInUse { tool, port: launch.port });
        }

        let log_path = self.log_file(tool);
        let mut child = spawn(&launch, &log_path)?;
        if let Some(pid) = child.id() {
            let pid_file = self.pid_file(tool);
            fs::write(&pid_file, pid.to_string()).at(&pid_file)?;
        }

        // Espera a que el puerto acepte conexiones o a que el proceso muera.
        let deadline = tokio::time::Instant::now() + START_TIMEOUT;
        loop {
            if let Some(code) = child.try_wait().at(&launch.program)? {
                let _ = fs::remove_file(self.pid_file(tool));
                return Err(Error::ServiceFailed {
                    tool,
                    reason: format!("terminó con {code}.\n{}", tail(&log_path, LOG_TAIL_ON_ERROR)),
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
                    reason: format!(
                        "no respondió en el puerto {} tras {}s.\n{}",
                        launch.port,
                        START_TIMEOUT.as_secs(),
                        tail(&log_path, LOG_TAIL_ON_ERROR)
                    ),
                });
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }

        self.procs.lock().await.insert(tool, Running { child, inst, port: launch.port });
        Ok(())
    }

    pub async fn stop(&self, tool: Tool) -> Result<()> {
        let Some(mut running) = self.procs.lock().await.remove(&tool) else {
            return Ok(());
        };
        let graceful =
            if tool == Tool::Mariadb { mariadb_shutdown_command(&running.inst, running.port) } else { None };
        terminate(&mut running.child, graceful).await;
        let _ = fs::remove_file(self.pid_file(tool));
        self.errors.lock().await.remove(&tool);
        Ok(())
    }

    /// Arranca en orden de dependencias: PHP y MariaDB antes que Apache.
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

    /// Mata procesos que quedaron vivos de una sesión anterior (p. ej. si la app se cerró de golpe).
    /// Solo actúa si el PID sigue apuntando a un ejecutable del servicio esperado.
    pub fn cleanup_orphans(&self) {
        use sysinfo::{Pid, ProcessesToUpdate, System};

        let mut sys = System::new();
        sys.refresh_processes(ProcessesToUpdate::All, true);
        for tool in Tool::ALL {
            let pid_file = self.pid_file(tool);
            let Some(pid) = fs::read_to_string(&pid_file).ok().and_then(|s| s.trim().parse::<usize>().ok()) else {
                continue;
            };
            if let Some(proc) = sys.process(Pid::from(pid)) {
                let name = proc.name().to_string_lossy().to_lowercase();
                if expected_process_names(tool).iter().any(|n| name.starts_with(n)) {
                    kill_tree(pid as u32);
                }
            }
            let _ = fs::remove_file(&pid_file);
        }
    }

    fn pid_file(&self, tool: Tool) -> PathBuf {
        self.paths.run().join(format!("{}.pid", tool.as_str()))
    }

    /// Escribe la configuración y devuelve cómo lanzar el servicio.
    async fn prepare(&self, inst: &Installation, state: &State) -> Result<Launch> {
        let missing = |what: &str| Error::ServiceFailed {
            tool: inst.tool,
            reason: format!("no se encontró {what} en {}", inst.root.display()),
        };
        let php_port = |s: &State| -> Result<u16> {
            let id = s.active.get(&Tool::Php).ok_or(Error::NoActiveVersion(Tool::Php))?;
            let php = install::locate(&self.paths, Tool::Php, id)?;
            Ok(ports::php_fcgi_port(s.ports.php_fcgi_base, &php.version))
        };

        match inst.tool {
            Tool::Php => {
                let port = ports::php_fcgi_port(state.ports.php_fcgi_base, &inst.version);
                let files = config::write_php(&self.paths, inst, port)?;
                if cfg!(windows) {
                    let program = inst.php_fastcgi().ok_or_else(|| missing("php-cgi.exe"))?;
                    Ok(Launch {
                        program,
                        args: vec!["-b".into(), format!("127.0.0.1:{port}"), "-c".into(), path_arg(&files.ini)],
                        // Por defecto php-cgi se cierra tras 500 peticiones.
                        env: vec![("PHP_FCGI_MAX_REQUESTS".into(), "0".into())],
                        port,
                    })
                } else {
                    let program = inst.php_fastcgi().ok_or_else(|| missing("php-fpm"))?;
                    let fpm = files.fpm_conf.expect("php-fpm.conf en Unix");
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
                let conf = config::write_apache(&self.paths, inst, state.ports.http, php_port(state)?, &docroot)?;
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
                // `--defaults-file` debe ser el primer argumento.
                Ok(Launch { program, args: vec![format!("--defaults-file={}", path_arg(&conf))], env: Vec::new(), port })
            }
        }
    }

    /// Inicializa el datadir la primera vez. Root queda sin contraseña (solo escucha en 127.0.0.1).
    async fn init_mariadb(&self, inst: &Installation, datadir: &Path, port: u16) -> Result<()> {
        if datadir.join("mysql").is_dir() {
            return Ok(());
        }
        let tool = Tool::Mariadb;
        let script = inst.mariadb_install_db().ok_or_else(|| Error::ServiceFailed {
            tool,
            reason: format!("no se encontró mariadb-install-db en {}", inst.root.display()),
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
            // No dejar un datadir a medias: el siguiente intento debe empezar de cero.
            let _ = fs::remove_dir_all(datadir);
            return Err(Error::ServiceFailed {
                tool,
                reason: format!("mariadb-install-db falló ({status}).\n{}", tail(&log_path, LOG_TAIL_ON_ERROR)),
            });
        }
        Ok(())
    }
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
        // En Windows las DLL se buscan junto al ejecutable y en el directorio actual.
        cmd.current_dir(dir);
    }
    // Grupo de procesos propio para poder señalizar también a los hijos (workers).
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

/// Parada ordenada; si no termina a tiempo, se fuerza.
/// En Unix se ignora `graceful`: SIGTERM ya es un apagado limpio para los tres servicios.
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
        // Windows no tiene SIGTERM: MariaDB se apaga con mariadb-admin; el resto no guarda estado.
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

/// Mata el proceso y sus descendientes sin esperar.
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

fn expected_process_names(tool: Tool) -> &'static [&'static str] {
    match tool {
        Tool::Apache => &["httpd"],
        Tool::Php => &["php-fpm", "php-cgi"],
        Tool::Mariadb => &["mariadbd", "mysqld"],
    }
}

/// Últimos `max` bytes de un archivo de log (vacío si no existe).
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
