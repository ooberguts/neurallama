//! GPU (NVML / nvidia-smi), RAM, llama-server /metrics + /slots, ring buffers, lifetime
//! stats. OWNER: telemetry+fit agent.
//!
//! Contract used by other modules:
//! - `Telemetry::new(paths)`
//! - `Telemetry::latest(&self) -> Option<Snapshot>`
//! - `Telemetry::gpu_now(&self) -> Option<GpuStats>` (fresh read, not cached)
//! - `spawn_poller(app)` started once from lib.rs setup
//! - `Telemetry::flush(&self)` persists lifetime stats (call on exit; also runs on drop)
pub mod commands;
pub mod gpu;
pub mod metrics;
pub mod stats;

use crate::server::ServerState;
use crate::settings::Paths;
use crate::state::AppState;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::time::Duration;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use tauri::{AppHandle, Emitter, Manager};

const GIB: f32 = 1024.0 * 1024.0 * 1024.0;

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

pub const HISTORY_LEN: usize = 90;

pub struct Telemetry {
    latest: RwLock<Option<Snapshot>>,
    stats: Mutex<stats::StatsStore>,
}

impl Telemetry {
    pub fn new(paths: Paths) -> Self {
        let stats = stats::StatsStore::load(paths.stats_file());
        Self { latest: RwLock::new(None), stats: Mutex::new(stats) }
    }

    /// The last snapshot the poller produced.
    pub fn latest(&self) -> Option<Snapshot> {
        self.latest.read().clone()
    }

    /// A fresh GPU read (blocking; NVML is fast, the nvidia-smi fallback ~100 ms).
    pub fn gpu_now(&self) -> Option<GpuStats> {
        gpu::read()
    }

    pub fn stats_list(&self) -> Vec<ModelStats> {
        self.stats.lock().list()
    }

    pub fn stats_reset(&self, model: Option<&str>) -> Vec<ModelStats> {
        let mut s = self.stats.lock();
        s.reset(model);
        s.list()
    }

    /// Persist lifetime stats now. Call on app exit.
    pub fn flush(&self) {
        self.stats.lock().flush();
    }
}

impl Drop for Telemetry {
    fn drop(&mut self) {
        self.flush();
    }
}

/// Fixed-length history, oldest first.
struct Ring {
    buf: VecDeque<f32>,
}

impl Ring {
    fn new() -> Self {
        Self { buf: VecDeque::with_capacity(HISTORY_LEN) }
    }
    fn push(&mut self, v: f32) {
        if self.buf.len() == HISTORY_LEN {
            self.buf.pop_front();
        }
        self.buf.push_back(v);
    }
    fn to_vec(&self) -> Vec<f32> {
        self.buf.iter().copied().collect()
    }
}

/// Model key for lifetime stats: the served model's file name.
fn model_key(model_path: Option<&str>, fallback: Option<&str>) -> Option<String> {
    let p = model_path.filter(|p| !p.is_empty()).or(fallback)?;
    Some(p.rsplit(['\\', '/']).next().unwrap_or(p).to_string())
}

/// Start the 1 Hz telemetry loop. Called once from setup.
pub fn spawn_poller(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut sys = System::new();
        let mut poller = metrics::MetricsPoller::new();
        let (mut h_gpu, mut h_vram, mut h_tps) = (Ring::new(), Ring::new(), Ring::new());
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let state = app.state::<AppState>();

            let gpu = tauri::async_runtime::spawn_blocking(gpu::read).await.ok().flatten();

            sys.refresh_memory();
            let pid = state.server.pid();
            let server_rss = pid.and_then(|p| {
                let pid = Pid::from_u32(p);
                sys.refresh_processes_specifics(
                    ProcessesToUpdate::Some(&[pid]),
                    true,
                    ProcessRefreshKind::nothing().with_memory(),
                );
                sys.process(pid).map(|p| p.memory())
            });
            let mem = MemStats { ram_used: sys.used_memory(), ram_total: sys.total_memory(), server_rss };

            let status = state.server.status();
            let metrics = if matches!(status.state, ServerState::Ready | ServerState::Attached) {
                let settings = state.settings();
                let session = format!("{:?}|{:?}|{}", status.pid, status.started_at, status.port);
                let (m, delta) =
                    poller.poll(&state.http, &settings.base_url(), &settings.api_key, &session).await;
                if let Some(key) = model_key(status.model_path.as_deref(), m.model.as_deref()) {
                    let sample = if delta.generating { m.gen_tps } else { None };
                    state.telemetry.stats.lock().record(&key, delta.gen_tokens, delta.requests, sample);
                }
                Some(m)
            } else {
                poller.reset();
                None
            };
            state.telemetry.stats.lock().maybe_save();

            h_gpu.push(gpu.as_ref().map(|g| g.util_pct).unwrap_or(0.0));
            h_vram.push(gpu.as_ref().map(|g| g.vram_used as f32 / GIB).unwrap_or(0.0));
            h_tps.push(metrics.as_ref().and_then(|m| m.gen_tps).unwrap_or(0.0) as f32);

            let snap = Snapshot {
                at: chrono::Local::now().to_rfc3339(),
                gpu,
                mem,
                metrics,
                history: History { gpu_util: h_gpu.to_vec(), vram_used: h_vram.to_vec(), gen_tps: h_tps.to_vec() },
            };
            *state.telemetry.latest.write() = Some(snap.clone());
            let _ = app.emit(EVENT, &snap);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_keeps_last_90() {
        let mut r = Ring::new();
        for i in 0..100 {
            r.push(i as f32);
        }
        let v = r.to_vec();
        assert_eq!(v.len(), HISTORY_LEN);
        assert_eq!(v[0], 10.0);
        assert_eq!(v[89], 99.0);
    }

    #[test]
    fn model_keys() {
        assert_eq!(model_key(Some("C:\\AI\\models\\a\\b\\M-Q4.gguf"), None).as_deref(), Some("M-Q4.gguf"));
        assert_eq!(model_key(Some(""), Some("local")).as_deref(), Some("local"));
        assert_eq!(model_key(None, None), None);
    }
}
