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
            server::commands::server_status,
            server::commands::server_start,
            server::commands::server_stop,
            server::commands::server_restart,
            server::commands::server_log_tail,
            server::commands::server_command_preview,
            server::commands::server_next_free_port,
            // --- end server ---
            // --- engine + models (models agent) ---
            engine::commands::engine_installed,
            engine::commands::engine_releases,
            engine::commands::engine_install,
            engine::commands::engine_set_active,
            engine::commands::engine_remove,
            engine::commands::engine_driver_info,
            models::commands::models_scan,
            models::commands::model_meta,
            models::commands::model_delete,
            models::commands::model_serve,
            models::commands::hf_search,
            models::commands::hf_files,
            models::commands::download_start,
            models::commands::download_pause,
            models::commands::download_resume,
            models::commands::download_cancel,
            models::commands::downloads_list,
            models::commands::hf_token_set,
            models::commands::hf_token_status,
            models::commands::disk_free,
            // --- end models ---
            // --- telemetry + fit (telemetry agent) ---
            telemetry::commands::telemetry_snapshot,
            telemetry::commands::stats_list,
            telemetry::commands::stats_reset,
            telemetry::commands::gpu_processes,
            fit::commands::fit_estimate,
            fit::commands::autotune_start,
            fit::commands::autotune_cancel,
            fit::commands::benchmark_run,
            // --- end telemetry ---
            // --- clients (main session) ---
            clients::commands::connect_info,
            clients::commands::connect_test,
            clients::commands::hermes_targets,
            clients::commands::hermes_push,
            clients::commands::continue_push,
            // --- end clients ---
        ])
        .build(tauri::generate_context!())
        .expect("error while building Neurallama")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                // Stop llama-server (the Job Object is the backstop) and save stats.
                server::commands::on_exit(app);
                app.state::<AppState>().telemetry.flush();
            }
        });
}
