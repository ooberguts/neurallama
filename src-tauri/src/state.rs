//! Shared application state, managed by Tauri (`tauri::State<'_, AppState>`).

use crate::settings::{Paths, Settings};
use parking_lot::RwLock;

pub struct AppState {
    pub paths: Paths,
    settings: RwLock<Settings>,
    pub server: crate::server::ServerManager,
    pub telemetry: crate::telemetry::Telemetry,
    pub jobs: crate::jobs::Jobs,
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
            settings: RwLock::new(settings),
            http,
            paths,
        }
    }

    /// A snapshot copy of the current settings.
    pub fn settings(&self) -> Settings {
        self.settings.read().clone()
    }

    /// Mutate settings and persist them. Returns the new settings.
    pub fn update_settings<F: FnOnce(&mut Settings)>(&self, f: F) -> Result<Settings, String> {
        let mut guard = self.settings.write();
        f(&mut guard);
        guard.save(&self.paths).map_err(|e| format!("Could not save settings: {e}"))?;
        Ok(guard.clone())
    }
}

/// Result type for every #[tauri::command]: errors are plain-English strings for the UI.
pub type CmdResult<T> = Result<T, String>;

pub fn err<E: std::fmt::Display>(context: &str) -> impl FnOnce(E) -> String + '_ {
    move |e| format!("{context}: {e}")
}
