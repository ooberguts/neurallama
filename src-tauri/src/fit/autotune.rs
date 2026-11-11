//! `--n-cpu-moe` auto-tune: binary search for the smallest N (most experts on the GPU) whose
//! real VRAM use after load stays at or under ~92 %. Starts at the calculator's guess, runs
//! as a cancellable background job (kind "autotune"), saves the winner to the profile and
//! leaves the server running with it.

use super::calc;
use super::commands::{input_from_profile, read_model};
use crate::jobs::{JobHandle, JobProgress, JobState};
use crate::server::ServerState;
use crate::settings::Profile;
use crate::state::AppState;
use std::time::Duration;
use tauri::{AppHandle, Manager};

pub const MAX_ITERATIONS: u32 = 8;
const TARGET: f64 = 0.92;
const LOW: f64 = 0.85;
const LOAD_TIMEOUT: Duration = Duration::from_secs(600);
const SETTLE: Duration = Duration::from_secs(3);

enum Probe {
    /// Loaded; VRAM used / total.
    Loaded(u64, u64),
    /// Crashed during load (usually out of memory).
    Crashed,
    Cancelled,
}

/// Validate, register the job and spawn it. Returns the job id.
pub async fn start(app: AppHandle, profile_id: String) -> Result<String, String> {
    let state = app.state::<AppState>();
    let settings = state.settings();
    let profile = settings.profile(&profile_id).cloned().ok_or("Profile not found.")?;
    let path = profile.model_path.clone();
    let (summary, bytes) = tauri::async_runtime::spawn_blocking(move || read_model(&path))
        .await
        .map_err(|e| format!("Auto-tune failed: {e}"))??;
    if !summary.is_moe {
        return Err("auto-tune is for MoE models".into());
    }
    let input = input_from_profile(&profile);
    let gpu = state.telemetry.gpu_now();
    let (vram_total, _) = calc::vram_total_for(&input, gpu.as_ref());
    let guess = calc::smallest_fitting_cpu_moe(&input, &summary, bytes, vram_total, None, 0)
        .unwrap_or(summary.block_count);

    let (job_id, handle) = state.jobs.create("autotune", &format!("Auto-tune --n-cpu-moe: {}", profile.name));
    let id = job_id.clone();
    let block_count = summary.block_count;
    tauri::async_runtime::spawn(async move {
        let mut p = Progress { app: app.clone(), id, label: format!("Auto-tune --n-cpu-moe: {}", profile.name) };
        p.send(0, JobState::Running, format!("Starting at the calculator's guess: --n-cpu-moe {guess}"));
        match run(&app, &handle, &mut p, profile, guess, block_count).await {
            Ok(msg) => p.send(MAX_ITERATIONS as u64, JobState::Done, msg),
            Err(e) if handle.cancelled() => p.send(0, JobState::Cancelled, e),
            Err(e) => p.send(0, JobState::Error, e),
        }
    });
    Ok(job_id)
}

struct Progress {
    app: AppHandle,
    id: String,
    label: String,
}

impl Progress {
    fn send(&mut self, done: u64, state: JobState, msg: String) {
        let st = self.app.state::<AppState>();
        st.jobs.update(
            &self.app,
            JobProgress {
                job_id: self.id.clone(),
                kind: "autotune".into(),
                label: self.label.clone(),
                done,
                total: Some(MAX_ITERATIONS as u64),
                state,
                message: Some(msg),
            },
        );
    }
}

fn with_n(base: &Profile, n: u32) -> Profile {
    Profile { n_cpu_moe: Some(n), cpu_moe_all: false, ..base.clone() }
}

async fn probe(app: &AppHandle, handle: &JobHandle, profile: &Profile) -> Result<Probe, String> {
    let state = app.state::<AppState>();
    let settings = state.settings();
    if !matches!(state.server.status().state, ServerState::Stopped | ServerState::Crashed) {
        let _ = state.server.stop(app).await;
    }
    state.server.start(app, profile, &settings).await?;
    let ready = state.server.wait_ready(LOAD_TIMEOUT);
    tokio::pin!(ready);
    let res = loop {
        tokio::select! {
            r = &mut ready => break r,
            _ = tokio::time::sleep(Duration::from_millis(250)) => {
                if handle.cancelled() {
                    let _ = state.server.stop(app).await;
                    return Ok(Probe::Cancelled);
                }
            }
        }
    };
    if let Err(e) = res {
        return if state.server.status().state == ServerState::Crashed {
            Ok(Probe::Crashed)
        } else {
            Err(format!("Server did not become ready: {e}"))
        };
    }
    tokio::time::sleep(SETTLE).await;
    let gpu = tauri::async_runtime::spawn_blocking(crate::telemetry::gpu::read)
        .await
        .ok()
        .flatten()
        .ok_or("No GPU readings (NVML / nvidia-smi); auto-tune needs them.")?;
    Ok(Probe::Loaded(gpu.vram_used, gpu.vram_total))
}

fn gib(b: u64) -> f64 {
    b as f64 / calc::GIB as f64
}

async fn run(
    app: &AppHandle,
    handle: &JobHandle,
    p: &mut Progress,
    profile: Profile,
    guess: u32,
    block_count: u32,
) -> Result<String, String> {
    let (mut lo, mut hi) = (0u32, block_count);
    let mut n = guess.min(block_count);
    let mut best: Option<(u32, f64)> = None;
    let mut last_tried = None;

    for iter in 1..=MAX_ITERATIONS {
        if handle.cancelled() {
            return Err("Cancelled.".into());
        }
        p.send(iter as u64 - 1, JobState::Running, format!("Try {iter}/{MAX_ITERATIONS}: loading with --n-cpu-moe {n}…"));
        let outcome = probe(app, handle, &with_n(&profile, n)).await?;
        last_tried = Some(n);
        let msg = match outcome {
            Probe::Cancelled => return Err("Cancelled.".into()),
            Probe::Crashed => {
                lo = n + 1;
                format!("Try {iter}/{MAX_ITERATIONS}: --n-cpu-moe {n} crashed while loading (out of memory?); raising N")
            }
            Probe::Loaded(used, total) => {
                let r = used as f64 / total.max(1) as f64;
                let pct = format!("{:.1}/{:.1} GB ({:.0}%)", gib(used), gib(total), r * 100.0);
                if r > TARGET {
                    lo = n + 1;
                    format!("Try {iter}/{MAX_ITERATIONS}: --n-cpu-moe {n} → {pct}, over 92%; raising N")
                } else {
                    if best.is_none_or(|(b, _)| n < b) {
                        best = Some((n, r));
                    }
                    if r >= LOW {
                        p.send(iter as u64, JobState::Running, format!("Try {iter}/{MAX_ITERATIONS}: --n-cpu-moe {n} → {pct}, in range"));
                        break;
                    }
                    hi = n.saturating_sub(1);
                    if n == 0 {
                        p.send(iter as u64, JobState::Running, format!("Try {iter}/{MAX_ITERATIONS}: --n-cpu-moe 0 → {pct}"));
                        break;
                    }
                    format!("Try {iter}/{MAX_ITERATIONS}: --n-cpu-moe {n} → {pct}, under 85%; lowering N")
                }
            }
        };
        p.send(iter as u64, JobState::Running, msg);
        if lo > hi || lo > block_count {
            break;
        }
        // Next probe: midpoint of the remaining range (never one we already know about).
        n = lo + (hi - lo) / 2;
        if best.is_some_and(|(b, _)| n >= b) {
            break;
        }
    }

    let (n, ratio) = best.ok_or(format!(
        "Even --n-cpu-moe {} did not fit in VRAM. Lower the context or pick a smaller quant.",
        lo.min(block_count)
    ))?;
    let state = app.state::<AppState>();
    let id = profile.id.clone();
    state.update_settings(|s| {
        if let Some(pr) = s.profiles.iter_mut().find(|pr| pr.id == id) {
            pr.n_cpu_moe = Some(n);
            pr.cpu_moe_all = false;
        }
    })?;
    if last_tried != Some(n) {
        p.send(MAX_ITERATIONS as u64 - 1, JobState::Running, format!("Restarting with the best value, --n-cpu-moe {n}…"));
        if let Probe::Cancelled = probe(app, handle, &with_n(&profile, n)).await? {
            return Err("Cancelled.".into());
        }
    }
    Ok(format!("Saved --n-cpu-moe {n} ({:.0}% VRAM). Server is running with it.", ratio * 100.0))
}
