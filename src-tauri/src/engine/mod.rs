//! llama.cpp binary manager: GitHub releases, install, activate, rollback, driver check.
//! OWNER: models+engine agent.
pub mod commands;
pub mod install;
pub mod releases;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledEngine {
    /// Folder name under AI\llama.cpp (e.g. "b6500-cuda-12.8").
    pub tag: String,
    /// "cuda" | "vulkan" | "cpu" | "other"
    pub flavor: String,
    pub dir: String,
    pub server_exe: String,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineAsset {
    pub name: String,
    pub url: String,
    pub size: u64,
    pub flavor: String,
    pub cuda_version: Option<String>,
    /// Matching cudart zip, when the CUDA runtime ships separately.
    pub cudart_url: Option<String>,
    pub cudart_size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub tag: String,
    pub published_at: String,
    pub prerelease: bool,
    pub html_url: String,
    pub assets: Vec<EngineAsset>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriverInfo {
    pub gpu_name: Option<String>,
    pub driver_version: Option<String>,
    /// Max CUDA version the driver supports, e.g. "12.8".
    pub cuda_driver_version: Option<String>,
    /// e.g. "12.0" for Blackwell sm_120.
    pub compute_cap: Option<String>,
    pub warnings: Vec<String>,
}
