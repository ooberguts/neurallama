//! Connect tab: snippets, connection test, client config wiring (Hermes Agent, Continue).
//! OWNER: main session.
pub mod commands;
pub mod hermes;
pub mod snippets;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snippets {
    pub curl: String,
    pub python: String,
    pub node: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectInfo {
    /// e.g. http://127.0.0.1:8080/v1
    pub base_url: String,
    pub api_key: String,
    /// The --alias clients send as `model`.
    pub model_id: String,
    /// http://<lan-ip>:<port>/v1 for each LAN address, when LAN mode is on.
    pub lan_urls: Vec<String>,
    pub snippets: Snippets,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    pub ok: bool,
    pub status: Option<u16>,
    /// The model's reply on success, or the server's own error text.
    pub message: String,
    pub latency_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesTarget {
    /// Stable id to pass back to hermes_push.
    pub id: String,
    /// "native" | "wsl"
    pub kind: String,
    pub distro: Option<String>,
    /// "default" or the named profile.
    pub profile: String,
    pub is_default: bool,
    /// Display path of the Hermes home (e.g. C:\Users\me\AppData\Local\hermes).
    pub home: String,
    pub config_path: String,
    pub config_exists: bool,
    pub current_provider: Option<String>,
    pub current_model: Option<String>,
    /// Already configured for this server.
    pub points_here: bool,
    /// The base_url this target would be given.
    pub base_url: String,
    /// Warnings such as "WSL in NAT mode: turn on LAN mode".
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesPushResult {
    pub target_id: String,
    pub ok: bool,
    pub backup_path: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientWriteResult {
    pub ok: bool,
    pub path: String,
    pub backup_path: Option<String>,
    pub message: String,
}
