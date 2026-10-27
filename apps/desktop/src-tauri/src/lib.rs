mod tray;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Mutex, RwLock};

use ampp_core::install::{self, HOMEBREW};
use ampp_core::state::{PhpSettings, Ports, Project};
use ampp_core::{
    AppPaths, Extra, Manifest, Platform, ServiceStatus, State, Supervisor, Tool, config, installer, projects, services,
    tls, tools,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};
use ampp_core::tr;
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_opener::OpenerExt;

/// Trailing log bytes shown in the log viewer.
const LOG_VIEW_BYTES: u64 = 64 * 1024;

struct AppState {
    paths: AppPaths,
    platform: Option<Platform>,
    /// Bundled catalog at first, replaced by the published one once it downloads.
    manifest: RwLock<Manifest>,
    state: Mutex<State>,
    supervisor: Supervisor,
    /// Installs in progress with their latest progress. It is the source of truth for the UI, which
    /// may be showing another page (or be reloaded) while a download runs.
    installing: Mutex<HashMap<(Tool, String), installer::Progress>>,
}

impl AppState {
    fn manifest(&self) -> Manifest {
        self.manifest.read().expect("manifest lock").clone()
    }

    /// Current state. Re-read from disk because the `ampp` CLI changes it too.
    fn snapshot(&self) -> State {
        let mut state = self.state.lock().expect("state lock");
        if let Ok(fresh) = State::load(&self.paths.state_file()) {
            *state = fresh;
        }
        state.clone()
    }

    /// Applies `f` to the state, persists it and refreshes the terminal shims.
    fn update<T>(&self, f: impl FnOnce(&mut State) -> ampp_core::Result<T>) -> CommandResult<T> {
        let mut state = self.state.lock().expect("state lock");
        if let Ok(fresh) = State::load(&self.paths.state_file()) {
            *state = fresh;
        }
        let out = f(&mut state)?;
        state.save(&self.paths.state_file())?;
        let _ = tools::write_shims(&self.paths, &state, cli_path().as_deref());
        Ok(out)
    }

    /// Restarts a service only if the app is running it, to apply configuration changes.
    async fn restart_if_running(&self, tool: Tool) -> CommandResult<bool> {
        let running = self.supervisor.status().await.iter().any(|s| s.tool == tool && s.running && !s.external);
        if running {
            self.supervisor.stop(tool).await?;
            self.supervisor.start(tool, &self.snapshot()).await?;
        }
        Ok(running)
    }

    /// Applies project changes: starts the PHP versions they need and regenerates Apache's virtual
    /// hosts. Returns `true` if Apache was restarted.
    async fn apply_projects(&self) -> CommandResult<bool> {
        let php_running =
            self.supervisor.status().await.iter().any(|s| s.tool == Tool::Php && s.running && !s.external);
        if php_running {
            self.supervisor.start(Tool::Php, &self.snapshot()).await?;
        }
        self.restart_if_running(Tool::Apache).await
    }
}

/// The `ampp` CLI sits next to the app executable (a sidecar in installers, `target/` in development).
fn cli_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let cli = exe.parent()?.join(install::exe("ampp"));
    cli.exists().then_some(cli)
}

/// Error serializable for the UI.
#[derive(Debug, Serialize)]
struct CommandError(String);

impl From<ampp_core::Error> for CommandError {
    fn from(e: ampp_core::Error) -> Self {
        Self(e.to_string())
    }
}

type CommandResult<T> = Result<T, CommandError>;

#[derive(Serialize)]
struct CatalogEntry {
    tool: Tool,
    /// Id (`8.3.12` or `homebrew`).
    version: String,
    /// Actual version when it differs from the id (Homebrew).
    detail: Option<String>,
    size: Option<u64>,
    installed: bool,
    active: bool,
    system: bool,
    /// Newer patch release of an installed version (e.g. a security fix).
    update: Option<String>,
}

#[tauri::command]
fn get_paths(app: tauri::State<AppState>) -> AppPaths {
    app.paths.clone()
}

#[tauri::command]
fn get_platform(app: tauri::State<AppState>) -> Option<Platform> {
    app.platform
}

#[tauri::command]
fn get_state(app: tauri::State<AppState>) -> State {
    app.snapshot()
}

/// Packages available for this platform plus installed versions missing from the catalog
/// (Homebrew ones included).
#[tauri::command]
fn get_catalog(app: tauri::State<AppState>) -> Vec<CatalogEntry> {
    let state = app.snapshot();
    let is_installed = |tool, v: &str| state.installed.get(&tool).is_some_and(|vs| vs.iter().any(|i| i == v));
    let is_active = |tool, v: &str| state.active.get(&tool).is_some_and(|a| a == v);

    let manifest = app.manifest();
    let update = |tool, v: &str| {
        app.platform.and_then(|p| manifest.patch_update(tool, v, p)).map(|p| p.version.clone())
    };
    let mut entries: Vec<CatalogEntry> = app
        .platform
        .map(|p| manifest.for_platform(p))
        .unwrap_or_default()
        .into_iter()
        .map(|p| CatalogEntry {
            tool: p.tool,
            version: p.version.clone(),
            detail: None,
            size: p.size,
            installed: is_installed(p.tool, &p.version),
            active: is_active(p.tool, &p.version),
            system: false,
            update: None,
        })
        .collect();

    for (tool, versions) in &state.installed {
        for v in versions {
            if entries.iter().any(|e| e.tool == *tool && e.version == *v) {
                continue;
            }
            let system = v == HOMEBREW;
            entries.push(CatalogEntry {
                tool: *tool,
                version: v.clone(),
                detail: system.then(|| install::homebrew(*tool).map(|i| i.version)).flatten(),
                size: None,
                installed: true,
                active: is_active(*tool, v),
                system,
                update: update(*tool, v),
            });
        }
    }
    entries
}

#[tauri::command]
async fn install_package(handle: AppHandle, tool: Tool, version: String) -> CommandResult<()> {
    let app = handle.state::<AppState>();
    let platform = app.platform.ok_or_else(|| CommandError(tr!("plataforma no soportada", "unsupported platform")))?;
    let pkg = app.manifest().find(tool, &version, platform)?.clone();

    let key = (tool, version.clone());
    {
        let mut installing = app.installing.lock().expect("installing lock");
        if installing.contains_key(&key) {
            return Err(CommandError(tr!(
                "{tool} {version} ya se está instalando",
                "{tool} {version} is already being installed"
            )));
        }
        let start = installer::Progress {
            tool,
            version: version.clone(),
            phase: installer::Phase::Downloading,
            downloaded: 0,
            total: pkg.size,
        };
        installing.insert(key.clone(), start);
    }
    let result = installer::install(&app.paths, &pkg, |p| {
        app.installing.lock().expect("installing lock").insert(key.clone(), p.clone());
        let _ = handle.emit("install-progress", p);
    })
    .await
    .map_err(CommandError::from)
    .and_then(|_| {
        app.update(|s| {
            s.mark_installed(tool, &version);
            Ok(())
        })
    });
    app.installing.lock().expect("installing lock").remove(&key);

    let error = result.as_ref().err().map(|e| e.0.clone());
    let _ = handle.emit("install-finished", InstallFinished { tool, version, error });
    result
}

/// Sent when an install ends, so the UI can catch up even if it was not watching.
#[derive(Clone, Serialize)]
struct InstallFinished {
    tool: Tool,
    version: String,
    error: Option<String>,
}

/// Installs running right now, with their latest progress.
#[tauri::command]
fn active_installs(app: tauri::State<AppState>) -> Vec<installer::Progress> {
    app.installing.lock().expect("installing lock").values().cloned().collect()
}

#[tauri::command]
async fn uninstall_package(app: tauri::State<'_, AppState>, tool: Tool, version: String) -> CommandResult<()> {
    if version == HOMEBREW {
        return Err(ampp_core::Error::SystemInstall.into());
    }
    let active = app.snapshot().active.get(&tool) == Some(&version);
    let in_use = app.supervisor.status().await.iter().any(|s| {
        s.tool == tool && s.running && (active || s.extra_php.iter().any(|e| e.version == version))
    });
    if in_use {
        return Err(CommandError(tr!(
            "detén el servicio antes de desinstalar la versión en uso",
            "stop the service before uninstalling the version in use"
        )));
    }
    installer::uninstall(&app.paths, tool, &version)?;
    app.update(|s| {
        s.mark_uninstalled(tool, &version);
        Ok(())
    })
}

/// Moves an installed version to its newest patch release: installs it, makes it active if the
/// old one was, restarts the service if the app runs it, and removes the old version.
/// Returns the new version.
#[tauri::command]
async fn upgrade_package(handle: AppHandle, tool: Tool, version: String) -> CommandResult<String> {
    let app = handle.state::<AppState>();
    let platform = app.platform.ok_or_else(|| CommandError(tr!("plataforma no soportada", "unsupported platform")))?;
    let new = app
        .manifest()
        .patch_update(tool, &version, platform)
        .map(|p| p.version.clone())
        .ok_or_else(|| ampp_core::Error::NotInCatalog { tool, version: version.clone() })?;

    install_package(handle.clone(), tool, new.clone()).await?;
    let serves_project =
        app.supervisor.status().await.iter().any(|s| s.tool == tool && s.extra_php.iter().any(|e| e.version == version));
    if app.snapshot().active.get(&tool) == Some(&version) {
        app.update(|s| s.set_active(tool, &new))?;
        app.restart_if_running(tool).await?;
    } else if serves_project {
        app.restart_if_running(tool).await?;
    }
    // Keep the old version if something launched from a terminal still runs it.
    let in_use = app.supervisor.status().await.iter().any(|s| {
        s.tool == tool && s.running && (s.version.as_deref() == Some(&version) || s.extra_php.iter().any(|e| e.version == version))
    });
    if !in_use {
        installer::uninstall(&app.paths, tool, &version)?;
        app.update(|s| {
            s.mark_uninstalled(tool, &version);
            Ok(())
        })?;
    }
    Ok(new)
}

#[tauri::command]
fn set_active(app: tauri::State<AppState>, tool: Tool, version: String) -> CommandResult<()> {
    app.update(|s| s.set_active(tool, &version))
}

#[tauri::command]
async fn service_status(app: tauri::State<'_, AppState>) -> CommandResult<Vec<ServiceStatus>> {
    Ok(app.supervisor.status().await)
}

#[tauri::command]
async fn start_service(app: tauri::State<'_, AppState>, tool: Tool) -> CommandResult<()> {
    let state = app.snapshot();
    Ok(app.supervisor.start(tool, &state).await?)
}

#[tauri::command]
async fn stop_service(app: tauri::State<'_, AppState>, tool: Tool) -> CommandResult<()> {
    Ok(app.supervisor.stop(tool).await?)
}

#[tauri::command]
async fn restart_service(app: tauri::State<'_, AppState>, tool: Tool) -> CommandResult<()> {
    app.supervisor.stop(tool).await?;
    let state = app.snapshot();
    Ok(app.supervisor.start(tool, &state).await?)
}

/// Returns the errors of the services that could not start.
#[tauri::command]
async fn start_all(app: tauri::State<'_, AppState>) -> CommandResult<Vec<String>> {
    let state = app.snapshot();
    // Without any version there is nothing to start: say why instead of silently doing nothing.
    if state.active.is_empty() {
        return Err(CommandError(tr!(
            "No hay nada que iniciar: aún no instalaste ninguna versión. Instala Apache, PHP y MariaDB en Versiones.",
            "There is nothing to start: no version is installed yet. Install Apache, PHP and MariaDB in Versions."
        )));
    }
    let failures = app.supervisor.start_all(&state).await;
    Ok(failures.into_iter().map(|e| e.to_string()).collect())
}

#[tauri::command]
async fn stop_all(app: tauri::State<'_, AppState>) -> CommandResult<()> {
    app.supervisor.stop_all().await;
    Ok(())
}


/// Logs the viewer can show, by name. Only these files can be read from the UI.
const LOG_FILES: &[&str] = &["apache", "apache-access", "php", "mariadb", "mailpit"];

#[tauri::command]
fn read_log(app: tauri::State<AppState>, name: String) -> CommandResult<String> {
    if !LOG_FILES.contains(&name.as_str()) {
        return Err(CommandError(tr!("log desconocido: {name}", "unknown log: {name}")));
    }
    Ok(services::tail(&app.paths.logs().join(format!("{name}.log")), LOG_VIEW_BYTES))
}

/// Directives the PHP tab lets users edit through a form.
const PHP_FORM_KEYS: &[&str] = &[
    "memory_limit",
    "upload_max_filesize",
    "post_max_size",
    "max_execution_time",
    "max_input_time",
    "max_input_vars",
    "display_errors",
    "error_reporting",
    "date.timezone",
];

#[derive(Serialize)]
struct ExtensionToggle {
    name: String,
    enabled: bool,
}

#[derive(Serialize)]
struct PhpConfig {
    settings: PhpSettings,
    /// Base php.ini (XAMPP) values for the form directives.
    defaults: BTreeMap<String, String>,
    /// Active PHP version.
    version: Option<String>,
    /// Every installed PHP version (the settings apply to all of them), newest first.
    installed: Vec<String>,
    /// Loaded modules (`php -m`).
    loaded: Vec<String>,
    /// Extensions that can be toggled (Windows DLLs).
    optional: Vec<ExtensionToggle>,
    ini_path: Option<String>,
}

#[tauri::command]
async fn get_php_config(app: tauri::State<'_, AppState>) -> CommandResult<PhpConfig> {
    let state = app.snapshot();
    let defaults = PHP_FORM_KEYS
        .iter()
        .filter_map(|k| config::php_default(k).map(|v| (k.to_string(), v)))
        .collect();
    let php = state.active.get(&Tool::Php).and_then(|id| install::locate(&app.paths, Tool::Php, id).ok());
    let (version, loaded, optional, ini_path) = match &php {
        Some(inst) => {
            let enabled = config::php_enabled_extensions(inst, &state.php);
            let optional = config::php_optional_extensions(inst)
                .into_iter()
                .map(|name| ExtensionToggle { enabled: enabled.contains(&name), name })
                .collect();
            let loaded = config::php_loaded_modules(&app.paths, inst, &state).await?;
            let ini = app.paths.config(Tool::Php, &inst.id).join("php.ini");
            (Some(inst.version.clone()), loaded, optional, Some(ini.display().to_string()))
        }
        None => (None, Vec::new(), Vec::new(), None),
    };
    let installed = state
        .installed
        .get(&Tool::Php)
        .into_iter()
        .flatten()
        .filter_map(|id| install::locate(&app.paths, Tool::Php, id).ok().map(|i| i.version))
        .collect();
    Ok(PhpConfig { settings: state.php, defaults, version, installed, loaded, optional, ini_path })
}

/// Saves the PHP settings. Returns `true` if PHP was restarted to apply them.
#[tauri::command]
async fn save_php_settings(app: tauri::State<'_, AppState>, settings: PhpSettings) -> CommandResult<bool> {
    settings.validate()?;
    app.update(|s| {
        s.php = settings;
        Ok(())
    })?;
    app.restart_if_running(Tool::Php).await
}

#[derive(Serialize)]
struct ExtraInfo {
    name: Extra,
    installed: Option<String>,
    available: Option<String>,
}

#[derive(Serialize)]
struct ToolsInfo {
    extras: Vec<ExtraInfo>,
    shims_dir: String,
    on_path: bool,
    /// Commands available in the shims folder.
    commands: Vec<String>,
}

#[tauri::command]
fn get_tools(app: tauri::State<AppState>) -> CommandResult<ToolsInfo> {
    let state = app.snapshot();
    let extras = Extra::ALL
        .iter()
        .map(|&name| ExtraInfo {
            name,
            installed: state.extras.get(&name).cloned(),
            available: app.manifest().extra(name).map(|e| e.version.clone()),
        })
        .collect();
    let commands = tools::write_shims(&app.paths, &state, cli_path().as_deref())?;
    Ok(ToolsInfo {
        extras,
        shims_dir: tools::shims_dir(&app.paths).display().to_string(),
        on_path: tools::shims_on_path(&app.paths),
        commands,
    })
}

#[tauri::command]
async fn install_extra(app: tauri::State<'_, AppState>, name: Extra) -> CommandResult<()> {
    let file = app.manifest().extra(name).cloned();
    let file = file.ok_or_else(|| ampp_core::Error::ExtraNotInCatalog(name.as_str().into()))?;
    tools::install(&app.paths, &file).await?;
    app.update(|s| {
        s.extras.insert(name, file.version.clone());
        Ok(())
    })?;
    match name {
        // Apache serves phpMyAdmin at /phpmyadmin, so its config must be regenerated.
        Extra::Phpmyadmin => {
            app.restart_if_running(Tool::Apache).await?;
        }
        // PHP's php.ini now sends mail to it; start it if the stack is up.
        Extra::Mailpit => {
            if app.restart_if_running(Tool::Php).await? {
                app.supervisor.start_mail(&app.snapshot()).await?;
            }
        }
        Extra::Composer => {}
    }
    Ok(())
}

#[tauri::command]
async fn uninstall_extra(app: tauri::State<'_, AppState>, name: Extra) -> CommandResult<()> {
    // A running executable cannot be deleted on Windows.
    if name == Extra::Mailpit {
        app.supervisor.stop_mail().await;
    }
    tools::uninstall(&app.paths, name)?;
    app.update(|s| {
        s.extras.remove(&name);
        Ok(())
    })?;
    match name {
        Extra::Phpmyadmin => {
            app.restart_if_running(Tool::Apache).await?;
        }
        Extra::Mailpit => {
            app.restart_if_running(Tool::Php).await?;
        }
        Extra::Composer => {}
    }
    Ok(())
}

#[tauri::command]
async fn mail_status(app: tauri::State<'_, AppState>) -> CommandResult<ampp_core::MailStatus> {
    Ok(app.supervisor.mail_status().await)
}

#[tauri::command]
async fn start_mail(app: tauri::State<'_, AppState>) -> CommandResult<()> {
    Ok(app.supervisor.start_mail(&app.snapshot()).await?)
}

#[tauri::command]
async fn stop_mail(app: tauri::State<'_, AppState>) -> CommandResult<()> {
    app.supervisor.stop_mail().await;
    Ok(())
}

/// Changes the ports. Running services keep the old ones until restarted.
#[tauri::command]
fn set_ports(app: tauri::State<AppState>, ports: Ports) -> CommandResult<()> {
    ports.validate()?;
    app.update(|s| {
        s.ports = ports;
        Ok(())
    })
}

#[tauri::command]
fn add_shims_to_path(app: tauri::State<AppState>) -> CommandResult<Vec<String>> {
    Ok(tools::add_shims_to_path(&app.paths)?)
}

/// Opens an Open AMPP folder or file with the system handler.
#[tauri::command]
fn open_location(handle: AppHandle, what: String) -> CommandResult<()> {
    let app = handle.state::<AppState>();
    let path = match what.as_str() {
        "www" => config::ensure_docroot(&app.paths)?,
        "data" => app.paths.root().to_path_buf(),
        "logs" => app.paths.logs(),
        "certs" => {
            tls::ensure_ca(&app.paths)?;
            tls::ca_cert_path(&app.paths).parent().expect("certs folder").to_path_buf()
        }
        "php_ini" => {
            let state = app.snapshot();
            let id = state.active.get(&Tool::Php).ok_or(ampp_core::Error::NoActiveVersion(Tool::Php))?;
            let inst = install::locate(&app.paths, Tool::Php, id)?;
            config::write_php(&app.paths, &inst, &state)?.ini
        }
        _ => return Err(CommandError(tr!("ubicación desconocida: {what}", "unknown location: {what}"))),
    };
    handle.opener().open_path(path.display().to_string(), None::<&str>).map_err(|e| CommandError(e.to_string()))
}

#[derive(Serialize)]
struct MariadbInfo {
    connection: ampp_core::db::Connection,
    /// `None` while MariaDB is not running.
    databases: Option<Vec<ampp_core::db::Database>>,
}

/// Connection details and, if the server is up, its databases.
#[tauri::command]
async fn mariadb_info(app: tauri::State<'_, AppState>) -> CommandResult<MariadbInfo> {
    let state = app.snapshot();
    let connection = ampp_core::db::connection(&app.paths, &state);
    let running = app.supervisor.status().await.iter().any(|s| s.tool == Tool::Mariadb && s.running);
    let databases = if running { Some(ampp_core::db::list(&app.paths, &state).await?) } else { None };
    Ok(MariadbInfo { connection, databases })
}

#[tauri::command]
async fn create_database(app: tauri::State<'_, AppState>, name: String) -> CommandResult<()> {
    Ok(ampp_core::db::create(&app.paths, &app.snapshot(), &name).await?)
}

#[derive(Serialize)]
struct ProjectInfo {
    #[serde(flatten)]
    project: Project,
    url: String,
    /// PHP version that serves it; `None` if its series is no longer installed (the active one
    /// serves it meanwhile).
    php_version: Option<String>,
    folder_exists: bool,
}

#[derive(Serialize)]
struct ProjectsInfo {
    projects: Vec<ProjectInfo>,
    /// Installed PHP series (`8.4`), newest first.
    php_series: Vec<String>,
    active_series: Option<String>,
}

#[tauri::command]
fn get_projects(app: tauri::State<AppState>) -> ProjectsInfo {
    let state = app.snapshot();
    let mut php_series: Vec<String> = Vec::new();
    for id in state.installed.get(&Tool::Php).into_iter().flatten() {
        if let Ok(inst) = install::locate(&app.paths, Tool::Php, id) {
            let series = inst.minor_version();
            if !php_series.contains(&series) {
                php_series.push(series);
            }
        }
    }
    let active_series = projects::php_for(&app.paths, &state, None).ok().map(|i| i.minor_version());
    let projects = state
        .projects
        .iter()
        .map(|p| ProjectInfo {
            url: projects::url(&state, p),
            php_version: projects::php_for(&app.paths, &state, p.php.as_deref()).ok().map(|i| i.version),
            folder_exists: p.docroot.is_dir(),
            project: p.clone(),
        })
        .collect();
    ProjectsInfo { projects, php_series, active_series }
}

/// Adds a project, or replaces the one called `previous`. Returns `true` if Apache was restarted.
#[tauri::command]
async fn save_project(app: tauri::State<'_, AppState>, previous: Option<String>, project: Project) -> CommandResult<bool> {
    let paths = app.paths.clone();
    app.update(|s| projects::save(&paths, s, previous.as_deref(), project))?;
    app.apply_projects().await
}

/// Removes a project (its folder stays). Returns `true` if Apache was restarted.
#[tauri::command]
async fn remove_project(app: tauri::State<'_, AppState>, name: String) -> CommandResult<bool> {
    app.update(|s| projects::remove(s, &name).map(drop))?;
    app.apply_projects().await
}

#[derive(Serialize)]
struct TlsInfo {
    /// Some project uses HTTPS.
    used: bool,
    https_port: u16,
    /// The local CA exists (it is created with the first HTTPS project).
    ca_exists: bool,
    ca_path: String,
    /// `None` when it cannot be told (Linux).
    trusted: Option<bool>,
    /// Linux only: commands to trust the CA, since that needs `sudo`.
    linux_commands: Option<String>,
}

#[tauri::command]
async fn get_tls(app: tauri::State<'_, AppState>) -> CommandResult<TlsInfo> {
    let state = app.snapshot();
    let paths = app.paths.clone();
    let ca = tls::ca_cert_path(&paths);
    let trusted = tauri::async_runtime::spawn_blocking(move || tls::is_trusted(&paths)).await.unwrap_or(None);
    Ok(TlsInfo {
        used: state.projects.iter().any(|p| p.https),
        https_port: state.ports.https,
        ca_exists: ca.exists(),
        ca_path: ca.display().to_string(),
        trusted,
        linux_commands: (!cfg!(windows) && !cfg!(target_os = "macos")).then(|| tls::linux_trust_commands(&ca)),
    })
}

/// Adds the local CA to the user's trust store; the system asks for confirmation.
#[tauri::command]
async fn trust_ca(app: tauri::State<'_, AppState>) -> CommandResult<()> {
    let paths = app.paths.clone();
    tauri::async_runtime::spawn_blocking(move || tls::trust(&paths))
        .await
        .map_err(|e| CommandError(e.to_string()))??;
    Ok(())
}

#[tauri::command]
fn open_project_folder(handle: AppHandle, name: String) -> CommandResult<()> {
    let state = handle.state::<AppState>().snapshot();
    let project = state.projects.iter().find(|p| p.name == name).ok_or_else(|| {
        CommandError(tr!("no existe el proyecto {name}", "there is no project called {name}"))
    })?;
    handle.opener().open_path(project.docroot.display().to_string(), None::<&str>).map_err(|e| CommandError(e.to_string()))
}

/// Argument the login item passes, so the app opens straight into the tray.
const HIDDEN_ARG: &str = "--hidden";

#[derive(Serialize)]
struct Startup {
    /// Open AMPP opens (in the tray) when the user signs in.
    open_at_login: bool,
    start_services: bool,
}

#[tauri::command]
fn get_startup(handle: AppHandle) -> Startup {
    Startup {
        open_at_login: handle.autolaunch().is_enabled().unwrap_or(false),
        start_services: handle.state::<AppState>().snapshot().start_services_on_open,
    }
}

#[tauri::command]
fn set_startup(handle: AppHandle, open_at_login: bool, start_services: bool) -> CommandResult<()> {
    let autolaunch = handle.autolaunch();
    let result = if open_at_login { autolaunch.enable() } else { autolaunch.disable() };
    result.map_err(|e| CommandError(e.to_string()))?;
    handle.state::<AppState>().update(|s| {
        s.start_services_on_open = start_services;
        Ok(())
    })
}

/// Language for Rust-side messages, so the UI can match it.
#[tauri::command]
fn get_lang() -> ampp_core::i18n::Lang {
    ampp_core::i18n::lang()
}

/// Adds or removes Homebrew installs to match what is on the system.
fn sync_homebrew(state: &mut State) {
    for tool in Tool::ALL {
        if install::homebrew(tool).is_some() {
            state.mark_installed(tool, HOMEBREW);
        } else if state.installed.get(&tool).is_some_and(|vs| vs.iter().any(|v| v == HOMEBREW)) {
            state.mark_uninstalled(tool, HOMEBREW);
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::Builder::new().args([HIDDEN_ARG]).build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let paths = AppPaths::from_system()?;
            paths.ensure()?;
            let mut state = State::load(&paths.state_file())?;
            let start_services = state.start_services_on_open;
            sync_homebrew(&mut state);
            state.save(&paths.state_file())?;
            let _ = tools::write_shims(&paths, &state, cli_path().as_deref());

            let supervisor = Supervisor::new(paths.clone());
            tauri::async_runtime::block_on(supervisor.cleanup_orphans());

            app.manage(AppState {
                paths,
                platform: Platform::current(),
                manifest: RwLock::new(Manifest::bundled()),
                state: Mutex::new(state),
                supervisor,
                installing: Mutex::new(HashMap::new()),
            });
            tray::create(app.handle())?;

            // The window starts hidden (tauri.conf.json): the login item keeps it in the tray.
            if !std::env::args().any(|a| a == HIDDEN_ARG) {
                tray::show_main_window(app.handle());
            }
            if start_services {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let app = handle.state::<AppState>();
                    app.supervisor.start_all(&app.snapshot()).await;
                });
            }

            // Swap in the published catalog (security releases, new versions) once it arrives.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let latest = Manifest::latest().await;
                *handle.state::<AppState>().manifest.write().expect("manifest lock") = latest;
                let _ = handle.emit("catalog-updated", ());
            });
            Ok(())
        })
        // Closing the window hides it: the services keep running and are controlled from the tray.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_paths,
            get_platform,
            get_state,
            get_catalog,
            install_package,
            active_installs,
            uninstall_package,
            upgrade_package,
            set_active,
            service_status,
            start_service,
            stop_service,
            restart_service,
            start_all,
            stop_all,
            read_log,
            get_php_config,
            save_php_settings,
            get_tools,
            install_extra,
            uninstall_extra,
            add_shims_to_path,
            open_location,
            set_ports,
            get_lang,
            mariadb_info,
            create_database,
            get_projects,
            save_project,
            remove_project,
            open_project_folder,
            get_tls,
            trust_ca,
            mail_status,
            start_mail,
            stop_mail,
            get_startup,
            set_startup,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|handle, event| match event {
            // On exit, gracefully stop what the app launched (MariaDB must never be killed).
            // Whatever `ampp up` started from a terminal keeps running.
            RunEvent::Exit => {
                if let Some(app) = handle.try_state::<AppState>() {
                    tauri::async_runtime::block_on(app.supervisor.shutdown());
                }
            }
            // macOS: Dock click while the window is hidden.
            #[cfg(target_os = "macos")]
            RunEvent::Reopen { .. } => tray::show_main_window(handle),
            _ => {}
        });
}
