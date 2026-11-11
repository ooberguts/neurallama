//! #[tauri::command]s for the server module, plus app start/exit hooks.

use super::{args, process, resolve_server_exe, CommandPreview, ServerStatus, SERVER_EXE_NAME};
use crate::settings::Profile;
use crate::state::{AppState, CmdResult};
use std::path::PathBuf;
use tauri::{AppHandle, Manager, State};

const NO_ACTIVE: &str = "No active profile. Pick a model to serve first.";

/// Called once at startup: attach to a running server, or auto-start if enabled.
pub fn on_app_start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        state.server.set_app(&app);
        let settings = state.settings();
        if state.server.attach_if_running(&app, &settings).await.is_some() {
            return;
        }
        if settings.autostart_server {
            if let Some(p) = settings.active().filter(|p| !p.model_path.trim().is_empty()) {
                if let Err(e) = state.server.start(&app, p, &settings).await {
                    log::warn!("auto-start failed: {e}");
                }
            }
        }
    });
}

/// Call from `RunEvent::Exit`: stops the llama-server we spawned (blocks up to ~7 s).
/// On Windows the Job Object also kills it if the app dies without getting here.
pub fn on_exit(app: &AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        state.server.shutdown_blocking();
    }
}

/// Start the active profile (used by tray and model_serve).
pub async fn start_active(app: &AppHandle) -> Result<super::ServerStatus, String> {
    let state = app.state::<AppState>();
    let _work = state.claim_server_work()?;
    let settings = state.settings();
    let profile = settings.active().cloned().ok_or(NO_ACTIVE)?;
    state.server.start(app, &profile, &settings).await
}

#[tauri::command]
pub fn server_status(state: State<'_, AppState>) -> ServerStatus {
    let mut s = state.server.status();
    if s.port == 0 {
        s.port = state.settings().port;
    }
    s
}

#[tauri::command]
pub async fn server_start(app: AppHandle, profile_id: Option<String>) -> CmdResult<ServerStatus> {
    let state = app.state::<AppState>();
    let _work = state.claim_server_work()?;
    let settings = state.settings();
    let profile = match profile_id {
        Some(id) => settings.profile(&id).cloned().ok_or_else(|| format!("Profile not found: {id}"))?,
        None => settings.active().cloned().ok_or(NO_ACTIVE)?,
    };
    state.server.start(&app, &profile, &settings).await
}

#[tauri::command]
pub async fn server_stop(app: AppHandle) -> CmdResult<ServerStatus> {
    let state = app.state::<AppState>();
    state.jobs.cancel_server_work();
    state.server.stop(&app).await
}

/// Stop + start with the same profile (latest saved settings for it).
#[tauri::command]
pub async fn server_restart(app: AppHandle) -> CmdResult<ServerStatus> {
    let state = app.state::<AppState>();
    let _work = state.claim_server_work()?;
    let settings = state.settings();
    let id = state.server.last_profile_id().or_else(|| settings.active_profile.clone());
    let profile = id
        .and_then(|id| settings.profile(&id).cloned())
        .or_else(|| settings.active().cloned())
        .ok_or(NO_ACTIVE)?;
    state.server.start(&app, &profile, &settings).await
}

#[tauri::command]
pub fn server_log_tail(state: State<'_, AppState>, lines: usize) -> Vec<String> {
    state.server.log_tail(lines)
}

#[tauri::command]
pub fn server_command_preview(state: State<'_, AppState>, profile: Profile) -> CommandPreview {
    let settings = state.settings();
    let exe = resolve_server_exe(&state.paths, &settings).unwrap_or_else(|_| PathBuf::from(SERVER_EXE_NAME));
    args::build(&profile, &settings, &exe)
}

#[tauri::command]
pub async fn server_next_free_port(app: AppHandle) -> CmdResult<u16> {
    let settings = app.state::<AppState>().settings();
    let host = settings.bind_host().to_string();
    let from = settings.port;
    tauri::async_runtime::spawn_blocking(move || process::next_free_port(&host, from))
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "No free port found.".to_string())
}
