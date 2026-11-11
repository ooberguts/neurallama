//! #[tauri::command]s for the server module.

use crate::state::AppState;
use tauri::{AppHandle, Manager};

/// Called once at startup: attach to a running server, or auto-start if enabled.
pub fn on_app_start(_app: AppHandle) {}

/// Start the active profile (used by tray and model_serve).
pub async fn start_active(app: &AppHandle) -> Result<super::ServerStatus, String> {
    let state = app.state::<AppState>();
    let settings = state.settings();
    let profile = settings.active().cloned().ok_or("No active profile. Pick a model to serve first.")?;
    state.server.start(app, &profile, &settings).await
}
