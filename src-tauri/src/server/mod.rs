//! llama-server process control. OWNER: server agent.
//!
//! Contract used by other modules (keep these signatures):
//! - `ServerManager::new(paths)`
//! - `ServerManager::start(&self, app, &Profile, &Settings) -> Result<ServerStatus, String>` (async)
//! - `ServerManager::stop(&self, app) -> Result<ServerStatus, String>` (async)
//! - `ServerManager::status(&self) -> ServerStatus`
//! - `ServerManager::wait_ready(&self, timeout) -> Result<(), String>` (async)
//! - `ServerManager::log_tail(&self, n) -> Vec<String>`
//! - `ServerManager::pid(&self) -> Option<u32>`
//! - `ServerManager::vram_allocations_mib(&self) -> Option<f64>` CUDA buffer total parsed from the log since last start
//! - `args::build(&Profile, &Settings, server_exe: &Path) -> CommandPreview`
//! - `resolve_server_exe(&Paths, &Settings) -> Result<PathBuf, String>`

pub mod args;
pub mod commands;
pub mod logs;
pub mod process;
#[cfg(windows)]
pub mod job_object;

pub use process::ServerManager;

use serde::{Deserialize, Serialize};

pub const STATUS_EVENT: &str = "server-status";
pub const LOG_EVENT: &str = "server-log";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ServerState {
    Stopped,
    Starting,
    Loading,
    Ready,
    Crashed,
    /// An llama-server we did not spawn is already listening on the port.
    Attached,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    pub state: ServerState,
    pub pid: Option<u32>,
    pub port: u16,
    pub profile_id: Option<String>,
    pub model_path: Option<String>,
    pub started_at: Option<String>,
    /// Plain-English last error (e.g. parsed out-of-memory).
    pub last_error: Option<String>,
    /// 0..1 while loading, if the log tells us.
    pub load_progress: Option<f32>,
    pub command_line: Option<String>,
    /// Crashes in the last 60 s (auto-restart stops at 3).
    pub crash_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandPreview {
    pub program: String,
    pub args: Vec<String>,
    /// Copyable, properly quoted command line.
    pub display: String,
}

/// Executable name of llama-server on this platform.
pub const SERVER_EXE_NAME: &str = if cfg!(windows) { "llama-server.exe" } else { "llama-server" };

/// Find the llama-server executable: custom path, else the active managed build
/// (`paths.engines/<active_engine>/`, searched up to 2 levels deep).
pub fn resolve_server_exe(
    paths: &crate::settings::Paths,
    settings: &crate::settings::Settings,
) -> Result<std::path::PathBuf, String> {
    use std::path::{Path, PathBuf};
    if let Some(custom) = settings.custom_server_exe.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let p = PathBuf::from(custom);
        return if p.is_file() {
            Ok(p)
        } else {
            Err(format!("The custom llama-server path doesn't exist: {custom}. Fix it in Settings → Engine."))
        };
    }
    const NONE: &str = "No llama.cpp build installed — open Settings → Engine";
    let tag = settings.active_engine.as_deref().filter(|t| !t.trim().is_empty()).ok_or(NONE)?;
    fn find(dir: &Path, depth: u32) -> Option<PathBuf> {
        let direct = dir.join(SERVER_EXE_NAME);
        if direct.is_file() {
            return Some(direct);
        }
        if depth == 0 {
            return None;
        }
        let mut subdirs: Vec<PathBuf> = std::fs::read_dir(dir)
            .ok()?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        subdirs.sort();
        subdirs.iter().find_map(|d| find(d, depth - 1))
    }
    find(&paths.engines.join(tag), 2).ok_or_else(|| {
        format!("No llama.cpp build installed — open Settings → Engine (llama-server not found in build \"{tag}\")")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{Paths, Settings};

    #[test]
    fn resolves_exe() {
        let root = std::env::temp_dir().join(format!("nl-exe-{}", crate::settings::new_id()));
        let paths = Paths::from_root(root.clone());
        paths.ensure().unwrap();
        let mut s = Settings::default();
        assert!(resolve_server_exe(&paths, &s).unwrap_err().starts_with("No llama.cpp build installed"));
        s.active_engine = Some("b9000".into());
        assert!(resolve_server_exe(&paths, &s).is_err());
        let nested = paths.engines.join("b9000").join("bin").join("Release");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join(SERVER_EXE_NAME), b"").unwrap();
        assert_eq!(resolve_server_exe(&paths, &s).unwrap(), nested.join(SERVER_EXE_NAME));
        s.custom_server_exe = Some(root.join("nope.exe").display().to_string());
        assert!(resolve_server_exe(&paths, &s).unwrap_err().contains("custom"));
        s.custom_server_exe = Some(nested.join(SERVER_EXE_NAME).display().to_string());
        assert!(resolve_server_exe(&paths, &s).is_ok());
        std::fs::remove_dir_all(root).ok();
    }
}
