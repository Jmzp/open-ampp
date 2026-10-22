use std::collections::HashSet;
use std::sync::Mutex;

use ampp_core::install::{self, HOMEBREW};
use ampp_core::{AppPaths, Manifest, Platform, ServiceStatus, State, Supervisor, Tool, installer, services};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, RunEvent};

/// Bytes del final del log que se muestran en el visor.
const LOG_VIEW_BYTES: u64 = 64 * 1024;

struct AppState {
    paths: AppPaths,
    platform: Option<Platform>,
    manifest: Manifest,
    state: Mutex<State>,
    supervisor: Supervisor,
    /// Instalaciones en curso, para no lanzar dos veces la misma.
    installing: Mutex<HashSet<(Tool, String)>>,
}

impl AppState {
    fn snapshot(&self) -> State {
        self.state.lock().expect("state lock").clone()
    }

    /// Aplica `f` al estado y lo persiste.
    fn update<T>(&self, f: impl FnOnce(&mut State) -> ampp_core::Result<T>) -> CommandResult<T> {
        let mut state = self.state.lock().expect("state lock");
        let out = f(&mut state)?;
        state.save(&self.paths.state_file())?;
        Ok(out)
    }
}

/// Error serializable para la UI.
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
    /// Identificador (`8.3.12` o `homebrew`).
    version: String,
    /// Versión real cuando difiere del identificador (Homebrew).
    detail: Option<String>,
    size: Option<u64>,
    installed: bool,
    active: bool,
    system: bool,
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

/// Paquetes disponibles para esta plataforma más las versiones instaladas que no estén en el catálogo
/// (incluidas las de Homebrew).
#[tauri::command]
fn get_catalog(app: tauri::State<AppState>) -> Vec<CatalogEntry> {
    let state = app.snapshot();
    let is_installed = |tool, v: &str| state.installed.get(&tool).is_some_and(|vs| vs.iter().any(|i| i == v));
    let is_active = |tool, v: &str| state.active.get(&tool).is_some_and(|a| a == v);

    let mut entries: Vec<CatalogEntry> = app
        .platform
        .map(|p| app.manifest.for_platform(p))
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
            });
        }
    }
    entries
}

#[tauri::command]
async fn install_package(handle: AppHandle, tool: Tool, version: String) -> CommandResult<()> {
    let app = handle.state::<AppState>();
    let platform = app.platform.ok_or_else(|| CommandError("plataforma no soportada".into()))?;
    let pkg = app.manifest.find(tool, &version, platform)?.clone();

    let key = (tool, version.clone());
    if !app.installing.lock().expect("installing lock").insert(key.clone()) {
        return Err(CommandError(format!("{} {version} ya se está instalando", tool.as_str())));
    }
    let result = installer::install(&app.paths, &pkg, |p| {
        let _ = handle.emit("install-progress", p);
    })
    .await;
    app.installing.lock().expect("installing lock").remove(&key);

    result?;
    app.update(|s| {
        s.mark_installed(tool, &version);
        Ok(())
    })
}

#[tauri::command]
async fn uninstall_package(app: tauri::State<'_, AppState>, tool: Tool, version: String) -> CommandResult<()> {
    if version == HOMEBREW {
        return Err(ampp_core::Error::SystemInstall.into());
    }
    let in_use = app.snapshot().active.get(&tool) == Some(&version)
        && app.supervisor.status().await.iter().any(|s| s.tool == tool && s.running);
    if in_use {
        return Err(CommandError("detén el servicio antes de desinstalar la versión en uso".into()));
    }
    installer::uninstall(&app.paths, tool, &version)?;
    app.update(|s| {
        s.mark_uninstalled(tool, &version);
        Ok(())
    })
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

/// Devuelve los errores de los servicios que no pudieron arrancar.
#[tauri::command]
async fn start_all(app: tauri::State<'_, AppState>) -> CommandResult<Vec<String>> {
    let state = app.snapshot();
    let failures = app.supervisor.start_all(&state).await;
    Ok(failures.into_iter().map(|(_, e)| e.to_string()).collect())
}

#[tauri::command]
async fn stop_all(app: tauri::State<'_, AppState>) -> CommandResult<()> {
    app.supervisor.stop_all().await;
    Ok(())
}

#[tauri::command]
fn read_log(app: tauri::State<AppState>, tool: Tool) -> String {
    services::tail(&app.supervisor.log_file(tool), LOG_VIEW_BYTES)
}

/// Registra o quita las instalaciones de Homebrew según lo que haya en el sistema.
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
        .setup(|app| {
            let paths = AppPaths::from_system()?;
            paths.ensure()?;
            let mut state = State::load(&paths.state_file())?;
            sync_homebrew(&mut state);
            state.save(&paths.state_file())?;

            let supervisor = Supervisor::new(paths.clone());
            supervisor.cleanup_orphans();

            app.manage(AppState {
                paths,
                platform: Platform::current(),
                manifest: Manifest::bundled(),
                state: Mutex::new(state),
                supervisor,
                installing: Mutex::new(HashSet::new()),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_paths,
            get_platform,
            get_state,
            get_catalog,
            install_package,
            uninstall_package,
            set_active,
            service_status,
            start_service,
            stop_service,
            restart_service,
            start_all,
            stop_all,
            read_log,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|handle, event| {
            // Al salir, apagar los servicios de forma ordenada (MariaDB no debe morir a la fuerza).
            if let RunEvent::Exit = event {
                if let Some(app) = handle.try_state::<AppState>() {
                    tauri::async_runtime::block_on(app.supervisor.stop_all());
                }
            }
        });
}
