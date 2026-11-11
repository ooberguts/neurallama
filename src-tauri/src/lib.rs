//! Neurallama — a Windows control panel for llama.cpp's llama-server.

mod app_commands;
mod clients;
mod engine;
mod fit;
mod jobs;
mod models;
mod server;
pub mod settings;
mod state;
mod telemetry;
mod tray;

use state::AppState;
use tauri::Manager;

pub fn run() {
    let paths = settings::Paths::resolve();
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .manage(AppState::new(paths))
        .setup(|app| {
            let handle = app.handle().clone();
            let settings = app.state::<AppState>().settings();
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.set_always_on_top(settings.always_on_top);
                if std::env::args().any(|a| a == "--minimized") && settings.tray {
                    let _ = w.hide();
                }
            }
            if settings.tray {
                tray::create(&handle)?;
            }
            telemetry::spawn_poller(handle.clone());
            server::commands::on_app_start(handle.clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let s = window.state::<AppState>().settings();
                if s.tray && s.close_to_tray {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            // --- app (main session) ---
            app_commands::settings_get,
            app_commands::settings_save,
            app_commands::paths_get,
            app_commands::profile_upsert,
            app_commands::profile_delete,
            app_commands::profile_set_active,
            app_commands::api_key_regenerate,
            app_commands::app_info,
            app_commands::tip_dismiss,
            app_commands::open_path,
            // --- server (server agent) ---
            // --- end server ---
            // --- engine + models (models agent) ---
            // --- end models ---
            // --- telemetry + fit (telemetry agent) ---
            // --- end telemetry ---
            // --- clients (main session) ---
            // --- end clients ---
        ])
        .run(tauri::generate_context!())
        .expect("error while running Neurallama");
}
