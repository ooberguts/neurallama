use super::{ServerState, ServerStatus};
use crate::settings::{Paths, Profile, Settings};
use std::time::Duration;
use tauri::AppHandle;

pub struct ServerManager {
    paths: Paths,
}

impl ServerManager {
    pub fn new(paths: Paths) -> Self {
        Self { paths }
    }
    pub async fn start(&self, _app: &AppHandle, _p: &Profile, s: &Settings) -> Result<ServerStatus, String> {
        let _ = &self.paths;
        Err(format!("not implemented (port {})", s.port))
    }
    pub async fn stop(&self, _app: &AppHandle) -> Result<ServerStatus, String> {
        Ok(self.status())
    }
    pub fn status(&self) -> ServerStatus {
        ServerStatus {
            state: ServerState::Stopped,
            pid: None,
            port: 0,
            profile_id: None,
            model_path: None,
            started_at: None,
            last_error: None,
            load_progress: None,
            command_line: None,
            crash_count: 0,
        }
    }
    pub async fn wait_ready(&self, _timeout: Duration) -> Result<(), String> {
        Err("not implemented".into())
    }
    pub fn log_tail(&self, _n: usize) -> Vec<String> {
        vec![]
    }
    pub fn pid(&self) -> Option<u32> {
        None
    }
    pub fn vram_allocations_mib(&self) -> Option<f64> {
        None
    }
}
