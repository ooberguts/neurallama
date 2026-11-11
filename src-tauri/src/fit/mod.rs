//! Fit calculator, staged suggestions, --n-cpu-moe auto-tune, benchmark.
//! OWNER: telemetry+fit agent.
pub mod autotune;
pub mod bench;
pub mod calc;
pub mod commands;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FitInput {
    pub model_path: String,
    pub ctx_size: u32,
    pub cache_type_k: String,
    pub cache_type_v: String,
    pub ngl: u32,
    pub n_cpu_moe: Option<u32>,
    pub cpu_moe_all: bool,
    pub parallel: u32,
    /// Override detected VRAM (bytes) — for planning on another machine.
    pub vram_total_override: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Accuracy {
    Measured,
    Estimate,
    UpperBound,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Fits,
    Tight,
    WontFit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    /// "lower_ctx" | "kv_q8" | "kv_q4" | "more_cpu_moe" | "smaller_quant" | "close_apps"
    pub kind: String,
    pub label: String,
    /// Partial Profile (camelCase keys) to stage into the controls. Never auto-applied.
    pub patch: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FitResult {
    pub weights_gpu: u64,
    pub weights_cpu: u64,
    pub kv_bytes: u64,
    pub kv_accuracy: Accuracy,
    pub recurrent_bytes: u64,
    pub compute_overhead: u64,
    pub overhead_accuracy: Accuracy,
    pub display_reserve: u64,
    pub vram_needed: u64,
    pub vram_total: u64,
    pub vram_free_now: Option<u64>,
    pub ram_needed: u64,
    pub ram_total: u64,
    pub verdict: Verdict,
    pub suggestions: Vec<Suggestion>,
    /// Plain-English speed note (MoE offload vs dense offload).
    pub speed_hint: Option<String>,
    /// After a real load: measured VRAM and forecast error %.
    pub measured_vram: Option<u64>,
    pub error_pct: Option<f64>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchResult {
    pub prompt_tokens: u32,
    pub gen_tokens: u32,
    pub prompt_tps: f64,
    pub gen_tps: f64,
    pub total_ms: u64,
}
