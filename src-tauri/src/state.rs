//! Shared application state, managed by Tauri (`tauri::State<'_, AppState>`).

use crate::settings::{Paths, Settings};
use parking_lot::RwLock;

pub struct AppState {
    pub paths: Paths,
    settings: RwLock<Settings>,
    pub server: crate::server::ServerManager,
    pub telemetry: crate::telemetry::Telemetry,
    pub jobs: crate::jobs::Jobs,
    server_work: std::sync::Arc<tokio::sync::Mutex<()>>,
    pub http: reqwest::Client,
}

impl AppState {
    pub fn new(paths: Paths) -> Self {
        let _ = paths.ensure();
        let mut settings = Settings::load(&paths);
        if settings.profiles.is_empty() {
            settings.profiles = crate::settings::presets();
        }
        let _ = settings.save(&paths);
        let http = reqwest::Client::builder()
            .user_agent(concat!("neurallama/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("http client");
        Self {
            server: crate::server::ServerManager::new(paths.clone()),
            telemetry: crate::telemetry::Telemetry::new(paths.clone()),
            jobs: crate::jobs::Jobs::default(),
            server_work: Default::default(),
            settings: RwLock::new(settings),
            http,
            paths,
        }
    }

    /// A snapshot copy of the current settings.
    pub fn settings(&self) -> Settings {
        self.settings.read().clone()
    }

    /// Keep auto-tune, benchmarks and manual launches from replacing one another's server.
    pub fn claim_server_work(&self) -> Result<tokio::sync::OwnedMutexGuard<()>, String> {
        self.server_work.clone().try_lock_owned()
            .map_err(|_| "The server is busy with another operation. Cancel it or wait for it to finish.".into())
    }

    /// Mutate settings and persist them. Returns the new settings.
    pub fn update_settings<F: FnOnce(&mut Settings)>(&self, f: F) -> Result<Settings, String> {
        let mut guard = self.settings.write();
        let mut next = guard.clone();
        f(&mut next);
        next.save(&self.paths).map_err(|e| format!("Could not save settings: {e}"))?;
        *guard = next;
        Ok(guard.clone())
    }
}

/// Result type for every #[tauri::command]: errors are plain-English strings for the UI.
pub type CmdResult<T> = Result<T, String>;


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_save_keeps_previous_settings() {
        let root = std::env::temp_dir().join(format!("nl-settings-failure-{}", crate::settings::new_id()));
        let paths = Paths::from_root(root.clone());
        let state = AppState::new(paths.clone());
        let port = state.settings().port;
        std::fs::remove_file(paths.settings_file()).unwrap();
        std::fs::create_dir(paths.settings_file()).unwrap();
        assert!(state.update_settings(|s| s.port = port + 1).is_err());
        assert_eq!(state.settings().port, port);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn server_work_is_exclusive_and_released() {
        let root = std::env::temp_dir().join(format!("nl-server-work-{}", crate::settings::new_id()));
        let state = AppState::new(Paths::from_root(root.clone()));
        let first = state.claim_server_work().unwrap();
        assert!(state.claim_server_work().is_err());
        drop(first);
        assert!(state.claim_server_work().is_ok());
        std::fs::remove_dir_all(root).ok();
    }
}
