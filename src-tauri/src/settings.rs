//! App settings, per-model launch profiles and on-disk paths.
//!
//! Layout (root defaults to `%USERPROFILE%\AI`, override with `NEURALLAMA_HOME`):
//! ```text
//! AI\models\<org>\<name>\*.gguf
//! AI\llama.cpp\<tag>\           one folder per installed llama.cpp build
//! AI\logs\                      server logs
//! AI\neurallama\settings.json
//! AI\neurallama\stats.json
//! ```

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const HOME_ENV: &str = "NEURALLAMA_HOME";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Paths {
    pub root: PathBuf,
    pub models: PathBuf,
    pub engines: PathBuf,
    pub logs: PathBuf,
    pub data: PathBuf,
}

impl Paths {
    pub fn resolve() -> Self {
        let root = std::env::var_os(HOME_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join("AI"));
        Self::from_root(root)
    }

    pub fn from_root(root: PathBuf) -> Self {
        Self {
            models: root.join("models"),
            engines: root.join("llama.cpp"),
            logs: root.join("logs"),
            data: root.join("neurallama"),
            root,
        }
    }

    pub fn ensure(&self) -> std::io::Result<()> {
        for p in [&self.root, &self.models, &self.engines, &self.logs, &self.data] {
            std::fs::create_dir_all(p)?;
        }
        Ok(())
    }

    pub fn settings_file(&self) -> PathBuf {
        self.data.join("settings.json")
    }

    pub fn stats_file(&self) -> PathBuf {
        self.data.join("stats.json")
    }
}

/// One saved launch configuration. Turned into llama-server CLI args by `server::args`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Profile {
    pub id: String,
    pub name: String,
    /// Absolute path to the .gguf (first shard for split models).
    pub model_path: String,
    pub mmproj_path: Option<String>,
    /// `-ngl`; 99 means "all layers".
    pub ngl: u32,
    /// `--n-cpu-moe N`; None = flag omitted.
    pub n_cpu_moe: Option<u32>,
    /// `--cpu-moe` (all experts on CPU). Wins over `n_cpu_moe`.
    pub cpu_moe_all: bool,
    pub ctx_size: u32,
    /// "f16" | "q8_0" | "q4_0"
    pub cache_type_k: String,
    pub cache_type_v: String,
    pub flash_attn: bool,
    pub threads: Option<u32>,
    pub threads_batch: Option<u32>,
    pub batch: Option<u32>,
    pub ubatch: Option<u32>,
    pub parallel: u32,
    pub jinja: bool,
    pub temp: Option<f32>,
    pub top_p: Option<f32>,
    pub top_k: Option<u32>,
    pub min_p: Option<f32>,
    /// Free text appended verbatim (split shell-style).
    pub extra_args: String,
    /// Last benchmark result for comparison, if any.
    pub bench: Option<BenchRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BenchRecord {
    pub at: String,
    pub prompt_tps: f64,
    pub gen_tps: f64,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            id: new_id(),
            name: "New profile".into(),
            model_path: String::new(),
            mmproj_path: None,
            ngl: 99,
            n_cpu_moe: None,
            cpu_moe_all: false,
            ctx_size: 32768,
            cache_type_k: "f16".into(),
            cache_type_v: "f16".into(),
            flash_attn: true,
            threads: None,
            threads_batch: None,
            batch: None,
            ubatch: None,
            parallel: 1,
            jinja: true,
            temp: None,
            top_p: None,
            top_k: None,
            min_p: None,
            extra_args: String::new(),
            bench: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub host: String,
    pub port: u16,
    pub api_key: String,
    /// Model id clients use (`--alias`).
    pub alias: String,
    /// Bind 0.0.0.0. Requires a non-empty api_key.
    pub lan_mode: bool,
    /// Folder name under `engines` of the active llama.cpp build.
    pub active_engine: Option<String>,
    /// Use this llama-server.exe instead of a managed build.
    pub custom_server_exe: Option<String>,
    /// "stable" | "prerelease"
    pub engine_channel: String,
    pub active_profile: Option<String>,
    pub profiles: Vec<Profile>,
    pub start_with_windows: bool,
    pub autostart_server: bool,
    pub tray: bool,
    pub close_to_tray: bool,
    pub always_on_top: bool,
    pub auto_restart: bool,
    pub dismissed_tips: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 8080,
            api_key: generate_api_key(),
            alias: "local".into(),
            lan_mode: false,
            active_engine: None,
            custom_server_exe: None,
            engine_channel: "stable".into(),
            active_profile: None,
            profiles: Vec::new(),
            start_with_windows: false,
            autostart_server: false,
            tray: true,
            close_to_tray: false,
            always_on_top: false,
            auto_restart: true,
            dismissed_tips: Vec::new(),
        }
    }
}

impl Settings {
    pub fn load(paths: &Paths) -> Self {
        let file = paths.settings_file();
        let mut s: Settings = std::fs::read_to_string(&file)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        if s.api_key.trim().is_empty() {
            s.api_key = generate_api_key();
        }
        s
    }

    pub fn save(&self, paths: &Paths) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        write_atomic(&paths.settings_file(), text.as_bytes())
    }

    /// The host llama-server binds to: 0.0.0.0 in LAN mode.
    pub fn bind_host(&self) -> &str {
        if self.lan_mode {
            "0.0.0.0"
        } else {
            &self.host
        }
    }

    /// Base URL a client on this machine uses.
    pub fn base_url(&self) -> String {
        let host = if self.lan_mode || self.host == "0.0.0.0" { "127.0.0.1" } else { &self.host };
        format!("http://{}:{}", host, self.port)
    }

    pub fn profile(&self, id: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    pub fn active(&self) -> Option<&Profile> {
        self.active_profile.as_deref().and_then(|id| self.profile(id))
    }
}

/// Preset profiles for the target machine (RTX 5090 Laptop 24 GB). `model_path` is filled
/// in when the matching model is on disk.
pub fn presets() -> Vec<Profile> {
    vec![
        Profile {
            id: "preset-qwen3.8-27b-q4km-64k".into(),
            name: "Qwen3.8-27B Q4_K_M, 64K".into(),
            ctx_size: 65536,
            cache_type_k: "q8_0".into(),
            cache_type_v: "q8_0".into(),
            ..Profile::default()
        },
        Profile {
            id: "preset-qwen3.6-35b-a3b-q5km-128k".into(),
            name: "Qwen3.6-35B-A3B Q5_K_M, 128K".into(),
            ctx_size: 131072,
            n_cpu_moe: Some(8),
            cache_type_k: "q8_0".into(),
            cache_type_v: "q8_0".into(),
            ..Profile::default()
        },
    ]
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..12].to_string()
}

pub fn generate_api_key() -> String {
    format!("nl-{}", uuid::Uuid::new_v4().simple())
}

/// Write to a temp file then rename, so a crash never leaves a half-written file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Copy `path` to `path.<timestamp>.bak` if it exists. Returns the backup path.
pub fn backup_file(path: &Path) -> std::io::Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let name = format!(
        "{}.{}.bak",
        path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        stamp
    );
    let dest = path.with_file_name(name);
    std::fs::copy(path, &dest)?;
    Ok(Some(dest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_roundtrip_and_defaults() {
        let dir = std::env::temp_dir().join(format!("nl-test-{}", new_id()));
        let paths = Paths::from_root(dir.clone());
        paths.ensure().unwrap();
        let mut s = Settings::default();
        s.profiles = presets();
        s.save(&paths).unwrap();
        let back = Settings::load(&paths);
        assert_eq!(back.profiles, s.profiles);
        assert!(back.api_key.starts_with("nl-"));
        // Missing fields fall back to defaults.
        std::fs::write(paths.settings_file(), r#"{"port": 9000}"#).unwrap();
        let partial = Settings::load(&paths);
        assert_eq!(partial.port, 9000);
        assert_eq!(partial.alias, "local");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn lan_mode_binds_all_interfaces() {
        let s = Settings { lan_mode: true, ..Settings::default() };
        assert_eq!(s.bind_host(), "0.0.0.0");
        assert_eq!(s.base_url(), "http://127.0.0.1:8080");
    }
}
