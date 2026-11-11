//! #[tauri::command]s for the llama.cpp engine manager.

use super::{install, releases, DriverInfo, InstalledEngine, Release};
use crate::server::ServerState;
use crate::settings::Settings;
use crate::state::{AppState, CmdResult};
use std::time::Duration;
use tauri::{AppHandle, State};

async fn installed(state: &AppState) -> CmdResult<Vec<InstalledEngine>> {
    let (paths, settings) = (state.paths.clone(), state.settings());
    tokio::task::spawn_blocking(move || install::list_installed(&paths, &settings))
        .await
        .map_err(|e| format!("Could not list engines: {e}"))
}

#[tauri::command]
pub async fn engine_installed(state: State<'_, AppState>) -> CmdResult<Vec<InstalledEngine>> {
    installed(&state).await
}

#[tauri::command]
pub async fn engine_releases(state: State<'_, AppState>) -> CmdResult<Vec<Release>> {
    let channel = state.settings().engine_channel;
    releases::list(&state.http, &channel).await
}

#[tauri::command]
pub async fn engine_install(app: AppHandle, tag: String, asset_name: String) -> CmdResult<String> {
    install::install(&app, tag.trim(), asset_name.trim()).await
}

fn valid_tag(tag: &str) -> CmdResult<()> {
    if tag.is_empty() || tag.starts_with('.') || tag.contains(['/', '\\', ':']) || tag.contains("..") {
        return Err(format!("Invalid engine name {tag}"));
    }
    Ok(())
}

#[tauri::command]
pub async fn engine_set_active(state: State<'_, AppState>, tag: String) -> CmdResult<Settings> {
    valid_tag(&tag)?;
    if !installed(&state).await?.iter().any(|e| e.tag == tag) {
        return Err(format!("{tag} is not installed."));
    }
    state.update_settings(|s| s.active_engine = Some(tag))
}

#[tauri::command]
pub async fn engine_remove(state: State<'_, AppState>, tag: String) -> CmdResult<Vec<InstalledEngine>> {
    valid_tag(&tag)?;
    let is_active = state.settings().active_engine.as_deref() == Some(tag.as_str());
    let running = matches!(
        state.server.status().state,
        ServerState::Starting | ServerState::Loading | ServerState::Ready
    );
    if is_active && running {
        return Err("This is the active engine and the server is running. Stop the server first.".into());
    }
    let dir = state.paths.engines.join(&tag);
    if !dir.is_dir() {
        return Err(format!("{tag} is not installed."));
    }
    tokio::task::spawn_blocking(move || std::fs::remove_dir_all(&dir))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("Could not remove {tag}: {e} (is a file in use?)"))?;
    if is_active {
        let remaining = installed(&state).await?;
        let next = remaining.first().map(|e| e.tag.clone());
        state.update_settings(|s| s.active_engine = next)?;
    }
    installed(&state).await
}

/// NVIDIA driver / CUDA compatibility. `cuda_version` optionally names a CUDA build being
/// considered (e.g. "13.5"); the active engine's CUDA version is always checked.
#[tauri::command]
pub async fn engine_driver_info(state: State<'_, AppState>, cuda_version: Option<String>) -> CmdResult<DriverInfo> {
    let mut info = query_driver().await;
    let active_cuda = state
        .settings()
        .active_engine
        .and_then(|tag| install::engine_cuda_version(&state.paths, &tag));
    let mut builds: Vec<String> = active_cuda.into_iter().collect();
    if let Some(v) = cuda_version.filter(|v| !v.trim().is_empty()) {
        if !builds.contains(&v) {
            builds.push(v);
        }
    }
    let extra = compat_warnings(&info, &builds);
    info.warnings.extend(extra);
    Ok(info)
}

async fn run_nvidia_smi(args: &[&str]) -> Option<String> {
    let mut cmd = tokio::process::Command::new("nvidia-smi");
    cmd.args(args).kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = tokio::time::timeout(Duration::from_secs(10), cmd.output()).await.ok()?.ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

async fn query_driver() -> DriverInfo {
    let mut info = DriverInfo::default();
    let Some(csv) = run_nvidia_smi(&["--query-gpu=name,driver_version,compute_cap", "--format=csv,noheader"]).await
    else {
        info.warnings.push(
            "nvidia-smi was not found, so the NVIDIA driver could not be checked. CUDA builds need an NVIDIA GPU and driver; use the Vulkan or CPU build otherwise."
                .into(),
        );
        return info;
    };
    parse_query(&csv, &mut info);
    if let Some(plain) = run_nvidia_smi(&[]).await {
        info.cuda_driver_version = parse_cuda_version(&plain);
    }
    info
}

fn parse_query(csv: &str, info: &mut DriverInfo) {
    if let Some(line) = csv.lines().map(str::trim).find(|l| !l.is_empty()) {
        let cols: Vec<&str> = line.split(',').map(str::trim).collect();
        let col = |i: usize| cols.get(i).filter(|s| !s.is_empty() && !s.contains("N/A")).map(|s| s.to_string());
        info.gpu_name = col(0);
        info.driver_version = col(1);
        info.compute_cap = col(2);
    }
}

fn parse_cuda_version(text: &str) -> Option<String> {
    let i = text.find("CUDA Version:")?;
    let v: String = text[i + 13..].trim_start().chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    (!v.is_empty()).then_some(v)
}

fn compat_warnings(info: &DriverInfo, builds: &[String]) -> Vec<String> {
    let mut w = Vec::new();
    let blackwell = info.compute_cap.as_deref().is_some_and(|c| c.starts_with("12"));
    let drv = info.cuda_driver_version.as_deref();
    let lt = |a: &str, b: &str| releases::version_cmp(Some(a), Some(b)) == std::cmp::Ordering::Less;
    if blackwell {
        if let Some(d) = drv.filter(|d| lt(d, "12.8")) {
            w.push(format!(
                "Your RTX 50-series (Blackwell) GPU needs CUDA 12.8 or newer, but the driver only supports CUDA {d}. Update the NVIDIA driver."
            ));
        }
    }
    for b in builds {
        if let Some(d) = drv.filter(|d| lt(d, b)) {
            w.push(format!(
                "The CUDA {b} build needs a newer NVIDIA driver (yours supports up to CUDA {d}). Update the driver or pick an older CUDA build."
            ));
        }
        if blackwell && lt(b, "12.8") {
            w.push(format!(
                "The CUDA {b} build predates Blackwell support; RTX 50-series GPUs need a CUDA 12.8+ build."
            ));
        }
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nvidia_smi() {
        let mut info = DriverInfo::default();
        parse_query("NVIDIA GeForce RTX 5090 Laptop GPU, 576.02, 12.0\n", &mut info);
        assert_eq!(info.gpu_name.as_deref(), Some("NVIDIA GeForce RTX 5090 Laptop GPU"));
        assert_eq!(info.driver_version.as_deref(), Some("576.02"));
        assert_eq!(info.compute_cap.as_deref(), Some("12.0"));
        let plain = "| NVIDIA-SMI 576.02     Driver Version: 576.02     CUDA Version: 12.9     |";
        assert_eq!(parse_cuda_version(plain).as_deref(), Some("12.9"));
    }

    #[test]
    fn warnings() {
        let info = DriverInfo {
            compute_cap: Some("12.0".into()),
            cuda_driver_version: Some("12.6".into()),
            ..Default::default()
        };
        let w = compat_warnings(&info, &["13.5".into(), "12.4".into()]);
        assert_eq!(w.len(), 3, "{w:?}");
        let ok = DriverInfo { compute_cap: Some("8.9".into()), cuda_driver_version: Some("13.5".into()), ..Default::default() };
        assert!(compat_warnings(&ok, &["13.5".into(), "12.4".into()]).is_empty());
    }
}
