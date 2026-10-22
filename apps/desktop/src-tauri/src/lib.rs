use std::sync::Mutex;

use ampp_core::{AppPaths, State};
use serde::Serialize;
use tauri::Manager;

struct AppState {
    paths: AppPaths,
    state: Mutex<State>,
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

#[tauri::command]
fn get_paths(app: tauri::State<AppState>) -> AppPaths {
    app.paths.clone()
}

#[tauri::command]
fn get_state(app: tauri::State<AppState>) -> CommandResult<State> {
    Ok(app.state.lock().expect("state lock").clone())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let paths = AppPaths::from_system()?;
            paths.ensure()?;
            let state = State::load(&paths.state_file())?;
            app.manage(AppState { paths, state: Mutex::new(state) });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![get_paths, get_state])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
