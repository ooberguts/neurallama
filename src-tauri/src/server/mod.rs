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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// Find the llama-server executable: custom path, else the active managed build.
pub fn resolve_server_exe(
    paths: &crate::settings::Paths,
    settings: &crate::settings::Settings,
) -> Result<std::path::PathBuf, String> {
    let _ = (paths, settings);
    Err("not implemented".into())
}
