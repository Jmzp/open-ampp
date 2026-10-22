use std::collections::HashSet;
use std::sync::Mutex;

use ampp_core::{AppPaths, Manifest, Platform, State, Tool, installer};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

struct AppState {
    paths: AppPaths,
    platform: Option<Platform>,
    manifest: Manifest,
    state: Mutex<State>,
    /// Instalaciones en curso, para no lanzar dos veces la misma.
    installing: Mutex<HashSet<(Tool, String)>>,
}

impl AppState {
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
    version: String,
    size: Option<u64>,
    installed: bool,
    active: bool,
}

#[tauri::command]
fn get_paths(app: tauri::State<AppState>) -> AppPaths {
    app.paths.clone()
}

#[tauri::command]
fn get_state(app: tauri::State<AppState>) -> State {
    app.state.lock().expect("state lock").clone()
}

/// Paquetes disponibles para esta plataforma más las versiones instaladas que ya no estén en el catálogo.
#[tauri::command]
fn get_catalog(app: tauri::State<AppState>) -> Vec<CatalogEntry> {
    let state = app.state.lock().expect("state lock");
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
            size: p.size,
            installed: is_installed(p.tool, &p.version),
            active: is_active(p.tool, &p.version),
        })
        .collect();

    for (tool, versions) in &state.installed {
        for v in versions {
            if !entries.iter().any(|e| e.tool == *tool && e.version == *v) {
                entries.push(CatalogEntry { tool: *tool, version: v.clone(), size: None, installed: true, active: is_active(*tool, v) });
            }
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
fn uninstall_package(app: tauri::State<AppState>, tool: Tool, version: String) -> CommandResult<()> {
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let paths = AppPaths::from_system()?;
            paths.ensure()?;
            let state = State::load(&paths.state_file())?;
            app.manage(AppState {
                paths,
                platform: Platform::current(),
                manifest: Manifest::bundled(),
                state: Mutex::new(state),
                installing: Mutex::new(HashSet::new()),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_paths,
            get_state,
            get_catalog,
            install_package,
            uninstall_package,
            set_active,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
