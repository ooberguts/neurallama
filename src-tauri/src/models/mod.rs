//! Local GGUF models: scan, metadata, Hugging Face search + resumable downloads.
//! OWNER: models+engine agent.
//!
//! Contract used by other modules:
//! - `gguf::read_summary(path) -> Result<GgufSummary, String>` (header only, fast)
//! - `scan::scan(&Paths, with_meta) -> Vec<LocalModel>`
pub mod commands;
pub mod download;
pub mod gguf;
pub mod hf;
pub mod scan;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalModel {
    /// Stable id: primary file path relative to the models dir, forward slashes.
    pub id: String,
    pub org: Option<String>,
    pub name: String,
    /// Absolute path of the first shard / single file.
    pub primary_path: String,
    pub files: Vec<String>,
    pub size_bytes: u64,
    pub mmproj_path: Option<String>,
    pub quant: Option<String>,
    /// false for flat folders ("source unknown").
    pub source_known: bool,
    /// "org/name" Hugging Face repo id when known.
    pub repo_id: Option<String>,
    pub meta: Option<GgufSummary>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GgufSummary {
    pub architecture: String,
    pub name: Option<String>,
    pub block_count: u32,
    pub context_length: Option<u32>,
    pub embedding_length: Option<u32>,
    pub head_count: Option<u32>,
    pub head_count_kv: Option<u32>,
    /// Per-head K/V dims (default embedding_length / head_count).
    pub key_length: Option<u32>,
    pub value_length: Option<u32>,
    pub expert_count: Option<u32>,
    pub expert_used_count: Option<u32>,
    /// Quant type name, e.g. "Q4_K_M".
    pub file_type: Option<String>,
    /// Layers that keep a full-attention KV cache. Equals block_count for classic
    /// transformers; fewer for hybrid (Gated DeltaNet / SSM) architectures.
    pub kv_layers: Option<u32>,
    /// "exact" (layer pattern read from metadata) | "assumed" (guessed)
    pub kv_layers_source: String,
    /// Fixed recurrent-state size for hybrid/SSM layers, if derivable.
    pub recurrent_state_bytes: Option<u64>,
    pub is_moe: bool,
    /// Total tensor bytes per class, used by the fit calculator.
    pub expert_tensor_bytes: Option<u64>,
    pub total_tensor_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HfModel {
    pub id: String,
    pub downloads: u64,
    pub likes: u64,
    pub last_modified: Option<String>,
    pub gated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HfFile {
    pub path: String,
    pub size: u64,
    pub quant: Option<String>,
    pub is_mmproj: bool,
    /// Split files share a group key so the UI can pick them together.
    pub shard_group: Option<String>,
}
