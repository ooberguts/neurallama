//! #[tauri::command]s for the fit calculator, auto-tune and benchmark.

use super::{calc, BenchResult, FitInput, FitResult};
use crate::models::GgufSummary;
use crate::server::{ServerState, ServerStatus};
use crate::settings::Profile;
use crate::state::{AppState, CmdResult};
use crate::telemetry::gpu;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, State};

/// All shards of a split model (`name-00001-of-00003.gguf` → every `name-NNNNN-of-00003.gguf`
/// in the same folder), or just the file itself.
pub fn shard_files(path: &Path) -> Vec<PathBuf> {
    let Some((prefix, total)) = path.file_name().and_then(|n| n.to_str()).and_then(split_name) else {
        return vec![path.to_path_buf()];
    };
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .and_then(split_name)
                        .is_some_and(|(pf, t)| pf == prefix && t == total)
                })
                .collect()
        })
        .unwrap_or_default();
    if v.is_empty() {
        v.push(path.to_path_buf());
    }
    v.sort();
    v
}

/// `"Model-Q4-00002-of-00003.gguf"` → `("Model-Q4", "00003")`.
fn split_name(name: &str) -> Option<(&str, &str)> {
    let stem = name.strip_suffix(".gguf").or_else(|| name.strip_suffix(".GGUF"))?;
    let (rest, total) = stem.rsplit_once("-of-")?;
    let (prefix, idx) = rest.rsplit_once('-')?;
    let digits = |s: &str| s.len() == 5 && s.bytes().all(|b| b.is_ascii_digit());
    (digits(idx) && digits(total)).then_some((prefix, total))
}

pub fn model_bytes(path: &Path) -> Result<u64, String> {
    let files = shard_files(path);
    let mut total = 0;
    for f in files {
        total += std::fs::metadata(&f).map_err(|e| format!("Could not read {}: {e}", f.display()))?.len();
    }
    Ok(total)
}

pub fn read_model(path: &str) -> Result<(GgufSummary, u64), String> {
    let p = Path::new(path);
    if path.trim().is_empty() {
        return Err("No model file set for this profile.".into());
    }
    if !p.exists() {
        return Err(format!("Model file not found: {path}"));
    }
    let summary = crate::models::gguf::read_summary(p).map_err(|e| format!("Could not read model metadata: {e}"))?;
    Ok((summary, model_bytes(p)?))
}

pub fn input_from_profile(p: &Profile) -> FitInput {
    FitInput {
        model_path: p.model_path.clone(),
        ctx_size: p.ctx_size,
        cache_type_k: p.cache_type_k.clone(),
        cache_type_v: p.cache_type_v.clone(),
        ngl: p.ngl,
        n_cpu_moe: p.n_cpu_moe,
        cpu_moe_all: p.cpu_moe_all,
        parallel: p.parallel,
        vram_total_override: None,
        ubatch: p.ubatch,
        flash_attn: Some(p.flash_attn),
    }
}

fn same_path(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.replace('\\', "/").to_lowercase();
    norm(a) == norm(b)
}

fn ram_total(state: &AppState) -> u64 {
    match state.telemetry.latest().map(|s| s.mem.ram_total).filter(|t| *t > 0) {
        Some(t) => t,
        None => {
            let mut sys = sysinfo::System::new();
            sys.refresh_memory();
            sys.total_memory()
        }
    }
}

/// Is the server currently serving `model_path`?
fn running_model(status: &ServerStatus, model_path: &str) -> bool {
    matches!(status.state, ServerState::Ready | ServerState::Attached)
        && status.model_path.as_deref().is_some_and(|m| same_path(m, model_path))
}

fn estimate_blocking(app: &AppHandle, input: FitInput) -> Result<FitResult, String> {
    let state = app.state::<AppState>();
    let (summary, bytes) = read_model(&input.model_path)?;
    let gpu_now = state.telemetry.gpu_now();
    let ram = ram_total(&state);
    let status = state.server.status();
    let server_pid = state.server.pid();

    // Measured overhead + forecast error, only when the server runs this very model.
    let mut measured_overhead = None;
    let mut measured_vram = None;
    let mut error_pct = None;
    let mut extra_notes = Vec::new();
    if running_model(&status, &input.model_path) {
        let settings = state.settings();
        let running = status
            .profile_id
            .as_deref()
            .and_then(|id| settings.profile(id))
            .filter(|p| same_path(&p.model_path, &input.model_path))
            .map(input_from_profile);
        if let Some(rin) = running {
            let forecast = calc::estimate(&rin, &summary, bytes, gpu_now.as_ref(), ram, None);
            let known = forecast.weights_gpu + forecast.kv_bytes + forecast.recurrent_bytes;
            let alloc = state.server.vram_allocations_mib().map(|m| (m * calc::MIB as f64) as u64);
            if let Some(a) = alloc {
                measured_overhead = Some(calc::CUDA_CONTEXT + a.saturating_sub(known));
            }
            let process = server_pid.and_then(gpu::process_vram);
            measured_vram = process.or(alloc.map(|a| a + calc::CUDA_CONTEXT));
            if let Some(m) = measured_vram.filter(|m| *m > 0) {
                // Forecast for the process alone (no display reserve), heuristic overhead.
                let predicted = forecast.vram_needed.saturating_sub(forecast.display_reserve);
                error_pct = Some((predicted as f64 - m as f64) * 100.0 / m as f64);
            }
            if rin.ctx_size != input.ctx_size
                || rin.cache_type_k != input.cache_type_k
                || rin.cache_type_v != input.cache_type_v
                || rin.n_cpu_moe != input.n_cpu_moe
                || rin.cpu_moe_all != input.cpu_moe_all
                || rin.ngl != input.ngl
            {
                extra_notes.push("Measured VRAM and error are for the config the server is running now.".into());
            }
            if process.is_none() && measured_vram.is_some() {
                extra_notes.push("Measured VRAM comes from the server log's CUDA buffers plus ~0.6 GB context.".into());
            }
        }
    }

    let mut result = calc::estimate(&input, &summary, bytes, gpu_now.as_ref(), ram, measured_overhead);
    result.measured_vram = measured_vram;
    result.error_pct = error_pct;
    result.notes.extend(extra_notes);
    let others: Vec<_> = gpu::processes().into_iter().filter(|p| Some(p.pid) != server_pid).collect();
    calc::add_close_apps(&mut result, &others);
    Ok(result)
}

#[tauri::command]
pub async fn fit_estimate(app: AppHandle, input: FitInput) -> CmdResult<FitResult> {
    tauri::async_runtime::spawn_blocking(move || estimate_blocking(&app, input))
        .await
        .map_err(|e| format!("Fit estimate failed: {e}"))?
}

#[tauri::command]
pub async fn autotune_start(app: AppHandle, profile_id: String) -> CmdResult<String> {
    super::autotune::start(app, profile_id).await
}

#[tauri::command]
pub fn autotune_cancel(state: State<'_, AppState>, job_id: String) -> CmdResult<()> {
    if state.jobs.cancel(&job_id) {
        Ok(())
    } else {
        Err("That auto-tune job is not running.".into())
    }
}

#[tauri::command]
pub async fn benchmark_run(app: AppHandle, profile_id: Option<String>) -> CmdResult<BenchResult> {
    super::bench::run(&app, profile_id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_names() {
        assert_eq!(split_name("Qwen-UD-Q5_K_M-00001-of-00002.gguf"), Some(("Qwen-UD-Q5_K_M", "00002")));
        assert_eq!(split_name("model.gguf"), None);
        assert_eq!(split_name("a-1-of-2.gguf"), None);
    }

    #[test]
    fn shard_sizes_are_summed() {
        let dir = std::env::temp_dir().join(format!("nl-shards-{}", crate::settings::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("M-00001-of-00002.gguf"), vec![0u8; 100]).unwrap();
        std::fs::write(dir.join("M-00002-of-00002.gguf"), vec![0u8; 50]).unwrap();
        std::fs::write(dir.join("M-00001-of-00003.gguf"), vec![0u8; 7]).unwrap();
        std::fs::write(dir.join("single.gguf"), vec![0u8; 9]).unwrap();
        assert_eq!(model_bytes(&dir.join("M-00001-of-00002.gguf")).unwrap(), 150);
        assert_eq!(model_bytes(&dir.join("single.gguf")).unwrap(), 9);
        assert!(model_bytes(&dir.join("missing.gguf")).is_err());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn path_compare() {
        assert!(same_path("C:\\AI\\Models\\x.gguf", "c:/ai/models/X.gguf"));
    }
}
