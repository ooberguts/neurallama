//! #[tauri::command]s for telemetry.

use super::{gpu, GpuProcess, ModelStats, Snapshot};
use crate::state::{AppState, CmdResult};
use tauri::State;

/// The latest 1 Hz snapshot (an empty one before the first tick).
#[tauri::command]
pub fn telemetry_snapshot(state: State<'_, AppState>) -> CmdResult<Snapshot> {
    Ok(state.telemetry.latest().unwrap_or_default())
}

#[tauri::command]
pub fn stats_list(state: State<'_, AppState>) -> CmdResult<Vec<ModelStats>> {
    Ok(state.telemetry.stats_list())
}

#[tauri::command]
pub fn stats_reset(state: State<'_, AppState>, model: Option<String>) -> CmdResult<Vec<ModelStats>> {
    Ok(state.telemetry.stats_reset(model.as_deref()))
}

/// Top VRAM consumers (largest first).
#[tauri::command]
pub async fn gpu_processes() -> CmdResult<Vec<GpuProcess>> {
    tauri::async_runtime::spawn_blocking(gpu::processes)
        .await
        .map_err(|e| format!("Could not list GPU processes: {e}"))
}
