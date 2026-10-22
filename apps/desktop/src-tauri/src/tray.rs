//! System tray icon: quick access to the services while the window is closed.

use std::time::Duration;

use ampp_core::{Extra, tr};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Wry};
use tauri_plugin_opener::OpenerExt;

use crate::AppState;

/// How often the menu's service summary is refreshed.
const REFRESH: Duration = Duration::from_secs(3);

pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let summary = MenuItem::with_id(app, "summary", tr!("Servicios detenidos", "Services stopped"), false, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &summary,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "show", tr!("Mostrar Open AMPP", "Show Open AMPP"), true, None::<&str>)?,
            &MenuItem::with_id(app, "start", tr!("Iniciar todo", "Start all"), true, None::<&str>)?,
            &MenuItem::with_id(app, "stop", tr!("Detener todo", "Stop all"), true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "site", tr!("Abrir localhost", "Open localhost"), true, None::<&str>)?,
            &MenuItem::with_id(app, "phpmyadmin", tr!("Abrir phpMyAdmin", "Open phpMyAdmin"), true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "quit", tr!("Salir (detiene los servicios)", "Quit (stops the services)"), true, None::<&str>)?,
        ],
    )?;

    // On macOS, a monochrome template icon that adapts to the light/dark menu bar.
    let (icon, template) = if cfg!(target_os = "macos") {
        (tauri::image::Image::from_bytes(include_bytes!("../icons/tray-template.png"))?, true)
    } else {
        (app.default_window_icon().cloned().expect("app icon"), false)
    };

    let tray = TrayIconBuilder::with_id("main")
        .icon(icon)
        .icon_as_template(template)
        .tooltip("Open AMPP")
        .menu(&menu)
        .on_menu_event(|app, event| handle_menu(app, event.id.as_ref()))
        .build(app)?;

    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            refresh(&handle, &tray, &summary).await;
            tokio::time::sleep(REFRESH).await;
        }
    });
    Ok(())
}

async fn refresh(app: &AppHandle, tray: &tauri::tray::TrayIcon<Wry>, summary: &MenuItem<Wry>) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let statuses = state.supervisor.status().await;
    let running = statuses.iter().filter(|s| s.running).count();
    let text = match running {
        0 => tr!("Servicios detenidos", "Services stopped"),
        n if n == statuses.len() => tr!("Todos los servicios en ejecución", "All services running"),
        n => {
            let total = statuses.len();
            tr!("{n} de {total} servicios en ejecución", "{n} of {total} services running")
        }
    };
    let _ = summary.set_text(&text);
    let _ = tray.set_tooltip(Some(format!("Open AMPP · {text}")));
}

fn handle_menu(app: &AppHandle, id: &str) {
    let app = app.clone();
    match id {
        "show" => show_main_window(&app),
        "start" => {
            tauri::async_runtime::spawn(async move {
                let state = app.state::<AppState>();
                let snapshot = state.snapshot();
                state.supervisor.start_all(&snapshot).await;
            });
        }
        "stop" => {
            tauri::async_runtime::spawn(async move {
                app.state::<AppState>().supervisor.stop_all().await;
            });
        }
        "site" | "phpmyadmin" => {
            let state = app.state::<AppState>();
            let port = state.snapshot().ports.http;
            let url = if id == "site" {
                format!("http://localhost:{port}")
            } else if state.snapshot().extras.contains_key(&Extra::Phpmyadmin) {
                format!("http://localhost:{port}/phpmyadmin/")
            } else {
                // Without phpMyAdmin installed, show the app so it can be installed from Tools.
                show_main_window(&app);
                return;
            };
            let _ = app.opener().open_url(url, None::<&str>);
        }
        "quit" => app.exit(0),
        _ => {}
    }
}
