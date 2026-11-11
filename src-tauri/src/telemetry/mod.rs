//! GPU (NVML / nvidia-smi), RAM, llama-server /metrics + /slots, ring buffers, lifetime
//! stats. OWNER: telemetry+fit agent.
//!
//! Contract used by other modules:
//! - `Telemetry::new(paths)`
//! - `Telemetry::latest(&self) -> Option<Snapshot>`
//! - `Telemetry::gpu_now(&self) -> Option<GpuStats>` (fresh read, not cached)
//! - `spawn_poller(app)` started once from lib.rs setup
pub mod commands;
pub mod gpu;
pub mod metrics;
pub mod stats;

use crate::settings::Paths;
use serde::{Deserialize, Serialize};

pub const EVENT: &str = "telemetry";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuStats {
    pub name: String,
    pub util_pct: f32,
    pub vram_used: u64,
    pub vram_total: u64,
    pub temp_c: Option<f32>,
    pub power_w: Option<f32>,
    pub clock_mhz: Option<u32>,
    /// "nvml" | "nvidia-smi"
    pub source: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemStats {
    pub ram_used: u64,
    pub ram_total: u64,
    pub server_rss: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerMetrics {
    pub prompt_tokens_total: Option<u64>,
    pub gen_tokens_total: Option<u64>,
    pub prompt_tps: Option<f64>,
    /// Live generation speed (slot-derived while a request is in flight).
    pub gen_tps: Option<f64>,
    pub requests_processing: Option<u32>,
    pub requests_deferred: Option<u32>,
    /// 0..1
    pub kv_usage: Option<f64>,
    pub slots_busy: Option<u32>,
    pub slots_total: Option<u32>,
    pub ctx_per_slot: Option<u32>,
    pub model: Option<String>,
    pub build: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct History {
    /// ~90 samples at 1 Hz, oldest first.
    pub gpu_util: Vec<f32>,
    pub vram_used: Vec<f32>,
    pub gen_tps: Vec<f32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub at: String,
    pub gpu: Option<GpuStats>,
    pub mem: MemStats,
    pub metrics: Option<ServerMetrics>,
    pub history: History,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStats {
    pub model: String,
    pub tokens: u64,
    pub requests: u64,
    pub avg_tps: f64,
    pub peak_tps: f64,
    pub low_tps: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuProcess {
    pub pid: u32,
    pub name: String,
    pub vram_bytes: Option<u64>,
}

pub struct Telemetry {
    paths: Paths,
}

impl Telemetry {
    pub fn new(paths: Paths) -> Self {
        Self { paths }
    }
    pub fn latest(&self) -> Option<Snapshot> {
        let _ = &self.paths;
        None
    }
    pub fn gpu_now(&self) -> Option<GpuStats> {
        None
    }
}

pub fn spawn_poller(_app: tauri::AppHandle) {}
