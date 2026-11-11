//! Settings, profiles and app-level commands. OWNER: main session.

use crate::settings::{Paths, Profile, Settings};
use crate::state::{AppState, CmdResult};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

#[tauri::command]
pub fn settings_get(state: State<'_, AppState>) -> Settings {
    state.settings()
}

#[tauri::command]
pub fn settings_save(app: AppHandle, state: State<'_, AppState>, settings: Settings) -> CmdResult<Settings> {
    if settings.lan_mode && settings.api_key.trim().is_empty() {
        return Err("LAN mode needs an API key. Generate one before binding to 0.0.0.0.".into());
    }
    let prev = state.settings();
    if prev.start_with_windows != settings.start_with_windows {
        set_autostart(&app, settings.start_with_windows)?;
    }
    if settings.tray && app.tray_by_id("main").is_none() {
        crate::tray::create(&app).map_err(|e| format!("Could not create tray icon: {e}"))?;
    }
    let saved = state.update_settings(|s| *s = settings)?;
    if !saved.tray {
        app.remove_tray_by_id("main");
    }
    if prev.always_on_top != saved.always_on_top {
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.set_always_on_top(saved.always_on_top);
        }
    }
    Ok(saved)
}

#[tauri::command]
pub fn paths_get(state: State<'_, AppState>) -> Paths {
    state.paths.clone()
}

#[tauri::command]
pub fn profile_upsert(state: State<'_, AppState>, profile: Profile) -> CmdResult<Settings> {
    state.update_settings(|s| match s.profiles.iter_mut().find(|p| p.id == profile.id) {
        Some(existing) => *existing = profile,
        None => s.profiles.push(profile),
    })
}

#[tauri::command]
pub fn profile_delete(state: State<'_, AppState>, id: String) -> CmdResult<Settings> {
    state.update_settings(|s| {
        s.profiles.retain(|p| p.id != id);
        if s.active_profile.as_deref() == Some(id.as_str()) {
            s.active_profile = None;
        }
    })
}

#[tauri::command]
pub fn profile_set_active(state: State<'_, AppState>, id: String) -> CmdResult<Settings> {
    if state.settings().profile(&id).is_none() {
        return Err(format!("No profile with id {id}"));
    }
    state.update_settings(|s| s.active_profile = Some(id))
}

#[tauri::command]
pub fn api_key_regenerate(state: State<'_, AppState>) -> CmdResult<Settings> {
    state.update_settings(|s| s.api_key = crate::settings::generate_api_key())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub os: String,
}

#[tauri::command]
pub fn app_info() -> AppInfo {
    AppInfo { version: env!("CARGO_PKG_VERSION").into(), os: std::env::consts::OS.into() }
}

#[tauri::command]
pub fn tip_dismiss(state: State<'_, AppState>, id: String) -> CmdResult<Settings> {
    state.update_settings(|s| {
        if !s.dismissed_tips.contains(&id) {
            s.dismissed_tips.push(id)
        }
    })
}

#[tauri::command]
pub fn open_path(path: String) -> CmdResult<()> {
    #[cfg(windows)]
    let cmd = ("explorer", vec![path.clone()]);
    #[cfg(target_os = "macos")]
    let cmd = ("open", vec![path.clone()]);
    #[cfg(all(not(windows), not(target_os = "macos")))]
    let cmd = ("xdg-open", vec![path.clone()]);
    std::process::Command::new(cmd.0)
        .args(cmd.1)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not open {path}: {e}"))
}

pub fn set_autostart(app: &AppHandle, enabled: bool) -> CmdResult<()> {
    use tauri_plugin_autostart::ManagerExt;
    let al = app.autolaunch();
    let res = if enabled { al.enable() } else { al.disable() };
    res.map_err(|e| format!("Could not change start-with-Windows: {e}"))
}
