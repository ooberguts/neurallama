//! Pure VRAM/RAM forecast for a model + launch config. No I/O: everything it needs is passed
//! in, so it is cheap to call repeatedly (suggestion search, auto-tune guesses) and easy to test.
//!
//! Model (all sizes in bytes):
//! ```text
//! weights      = GGUF file size (all shards)
//!   cpu_layers = block_count - min(ngl, block_count)          (first layers stay on CPU)
//!   weights_cpu = weights * cpu_layers / block_count
//!               + expert_bytes * max(0, moe_cpu_layers - cpu_layers) / block_count
//!     moe_cpu_layers = block_count with --cpu-moe, else --n-cpu-moe N (both count from layer 0)
//!   weights_gpu = weights - weights_cpu
//! kv           = kv_layers * ctx_size * head_count_kv * (key_len * bytes(k) + value_len * bytes(v))
//!                (ctx_size is the total across --parallel slots; the GPU share scales with
//!                 the offloaded layer fraction)
//! recurrent    = recurrent_state_bytes * parallel                 (hybrid / SSM layers)
//! overhead     = measured (from the running server), else the heuristic
//!                600 MiB CUDA context
//!              + ubatch * 600 KiB            activations + logits for one micro-batch
//!              + ctx_size * ubatch * 2       f16 KQ mask
//!              + n_head * ctx_size * ubatch * 4   only without flash attention (KQ scores)
//! reserve      = 1.75 GiB Windows display / other apps
//! vram_needed  = weights_gpu + kv_gpu + recurrent_gpu + overhead + reserve
//! verdict      = fits <= 92 % of VRAM, tight <= 100 %, else won't fit
//! ram_needed   = weights_cpu + kv_cpu + 2 GiB
//! ```
use super::{Accuracy, FitInput, FitResult, Suggestion, Verdict};
use crate::models::GgufSummary;
use crate::telemetry::{GpuProcess, GpuStats};
use serde_json::json;

pub const MIB: u64 = 1 << 20;
pub const GIB: u64 = 1 << 30;
pub const CUDA_CONTEXT: u64 = 600 * MIB;
pub const DISPLAY_RESERVE: u64 = 1792 * MIB; // 1.75 GiB
pub const RAM_HEADROOM: u64 = 2 * GIB;
pub const DEFAULT_VRAM: u64 = 24 * GIB;
pub const DEFAULT_UBATCH: u32 = 512;
pub const FIT_RATIO: f64 = 0.92;
/// Fraction of the file assumed to be routed experts when the GGUF reader could not say.
const ASSUMED_EXPERT_FRACTION: f64 = 0.85;

/// Bytes per element of a KV cache type (block-quantized types include their scale).
pub fn kv_type_bytes(t: &str) -> Option<f64> {
    Some(match t.trim().to_ascii_lowercase().as_str() {
        "f32" => 4.0,
        "f16" | "bf16" => 2.0,
        "q8_0" => 34.0 / 32.0,
        "q5_1" => 24.0 / 32.0,
        "q5_0" => 22.0 / 32.0,
        "q4_1" => 20.0 / 32.0,
        "q4_0" | "iq4_nl" => 18.0 / 32.0,
        _ => return None,
    })
}

/// Everything except suggestions; recomputed for every candidate config.
#[derive(Debug, Clone)]
struct Parts {
    weights_gpu: u64,
    weights_cpu: u64,
    kv_gpu: u64,
    kv_cpu: u64,
    kv_accuracy: Accuracy,
    recurrent_gpu: u64,
    recurrent_cpu: u64,
    overhead: u64,
    overhead_accuracy: Accuracy,
    cpu_layers: u32,
    moe_cpu_layers: u32,
    needed: u64,
    notes: Vec<String>,
}

struct Env<'a> {
    summary: &'a GgufSummary,
    file_bytes: u64,
    measured_overhead: Option<u64>,
}

fn parts(input: &FitInput, env: &Env) -> Parts {
    let s = env.summary;
    let mut notes = Vec::new();
    let bc = s.block_count.max(1) as u64;
    let gpu_layers = (input.ngl as u64).min(bc);
    let cpu_layers = bc - gpu_layers;
    let total = env.file_bytes;

    // --- weights ---
    let expert = if s.is_moe {
        match s.expert_tensor_bytes {
            Some(b) => b.min(total),
            None => {
                notes.push("Expert tensor size unknown; assumed 85% of the file.".into());
                (total as f64 * ASSUMED_EXPERT_FRACTION) as u64
            }
        }
    } else {
        0
    };
    let moe_cpu_layers = if !s.is_moe {
        0
    } else if input.cpu_moe_all {
        bc
    } else {
        (input.n_cpu_moe.unwrap_or(0) as u64).min(bc)
    };
    let extra_moe_layers = moe_cpu_layers.saturating_sub(cpu_layers);
    let weights_cpu = (total as u128 * cpu_layers as u128 / bc as u128) as u64
        + (expert as u128 * extra_moe_layers as u128 / bc as u128) as u64;
    let weights_cpu = weights_cpu.min(total);
    let weights_gpu = total - weights_cpu;

    // --- KV cache ---
    let (kv_total, kv_accuracy) = kv_bytes(input, s, &mut notes);
    let kv_gpu = (kv_total as u128 * gpu_layers as u128 / bc as u128) as u64;
    let kv_cpu = kv_total - kv_gpu;

    // --- recurrent state (hybrid / SSM) ---
    let rec_total = s.recurrent_state_bytes.unwrap_or(0) * input.parallel.max(1) as u64;
    let recurrent_gpu = (rec_total as u128 * gpu_layers as u128 / bc as u128) as u64;
    let recurrent_cpu = rec_total - recurrent_gpu;

    // --- compute buffers + CUDA context ---
    let (overhead, overhead_accuracy) = match env.measured_overhead {
        Some(m) => (m, Accuracy::Measured),
        None => (heuristic_overhead(input, s), Accuracy::Estimate),
    };

    let needed = weights_gpu + kv_gpu + recurrent_gpu + overhead + DISPLAY_RESERVE;
    Parts {
        weights_gpu,
        weights_cpu,
        kv_gpu,
        kv_cpu,
        kv_accuracy,
        recurrent_gpu,
        recurrent_cpu,
        overhead,
        overhead_accuracy,
        cpu_layers: cpu_layers as u32,
        moe_cpu_layers: moe_cpu_layers as u32,
        needed,
        notes,
    }
}

fn kv_bytes(input: &FitInput, s: &GgufSummary, notes: &mut Vec<String>) -> (u64, Accuracy) {
    let head_count = s.head_count.unwrap_or(0);
    let hkv = s.head_count_kv.or(s.head_count).unwrap_or(0) as f64;
    let per_head = match (s.embedding_length, head_count) {
        (Some(e), h) if h > 0 => Some(e / h),
        _ => None,
    };
    let kl = s.key_length.or(per_head).unwrap_or(0) as f64;
    let vl = s.value_length.or(per_head).unwrap_or(0) as f64;
    if hkv == 0.0 || kl == 0.0 {
        notes.push("Model metadata has no attention head info; KV cache not counted.".into());
        return (0, Accuracy::UpperBound);
    }
    let bk = kv_type_bytes(&input.cache_type_k).unwrap_or_else(|| {
        notes.push(format!("Unknown K cache type \"{}\"; counted as f16.", input.cache_type_k));
        2.0
    });
    let bv = kv_type_bytes(&input.cache_type_v).unwrap_or_else(|| {
        notes.push(format!("Unknown V cache type \"{}\"; counted as f16.", input.cache_type_v));
        2.0
    });
    let (layers, accuracy) = match (s.kv_layers, s.kv_layers_source.as_str()) {
        (Some(l), "exact") => (l, Accuracy::Estimate),
        (l, _) => {
            notes.push(
                "Attention layer pattern not in metadata; KV assumes every layer keeps a cache \
                 (upper bound)."
                    .into(),
            );
            (l.unwrap_or(s.block_count), Accuracy::UpperBound)
        }
    };
    let bytes = layers as f64 * input.ctx_size as f64 * hkv * (kl * bk + vl * bv);
    (bytes as u64, accuracy)
}

/// See the module docs for the formula.
fn heuristic_overhead(input: &FitInput, s: &GgufSummary) -> u64 {
    let ub = input.ubatch.unwrap_or(DEFAULT_UBATCH).max(1) as u64;
    let ctx = input.ctx_size as u64;
    let mut o = CUDA_CONTEXT + ub * 600 * 1024 + ctx * ub * 2;
    if input.flash_attn == Some(false) {
        o += s.head_count.unwrap_or(32) as u64 * ctx * ub * 4;
    }
    o
}

fn fits(p: &Parts, vram_total: u64) -> bool {
    p.needed as f64 <= vram_total as f64 * FIT_RATIO
}

fn verdict(needed: u64, total: u64) -> Verdict {
    let r = needed as f64 / total.max(1) as f64;
    if r <= FIT_RATIO {
        Verdict::Fits
    } else if r <= 1.0 {
        Verdict::Tight
    } else {
        Verdict::WontFit
    }
}

fn gb(b: u64) -> String {
    format!("{:.1} GB", b as f64 / GIB as f64)
}

fn ctx_label(c: u32) -> String {
    if c.is_multiple_of(1024) {
        format!("{}K", c / 1024)
    } else {
        c.to_string()
    }
}

fn pct(p: &Parts, total: u64) -> String {
    format!("{:.0}%", p.needed as f64 * 100.0 / total.max(1) as f64)
}

/// The VRAM the calculation is checked against: override, else the GPU's, else 24 GiB.
pub fn vram_total_for(input: &FitInput, gpu: Option<&GpuStats>) -> (u64, Option<String>) {
    if let Some(v) = input.vram_total_override.filter(|v| *v > 0) {
        return (v, None);
    }
    match gpu.map(|g| g.vram_total).filter(|v| *v > 0) {
        Some(v) => (v, None),
        None => (
            DEFAULT_VRAM,
            Some("No NVIDIA GPU detected; assuming 24 GB of VRAM.".into()),
        ),
    }
}

/// Forecast VRAM/RAM for `input`. Pure: no I/O.
pub fn estimate(
    input: &FitInput,
    summary: &GgufSummary,
    file_bytes: u64,
    gpu: Option<&GpuStats>,
    ram_total: u64,
    measured_overhead: Option<u64>,
) -> FitResult {
    let env = Env { summary, file_bytes, measured_overhead };
    let (vram_total, vram_note) = vram_total_for(input, gpu);
    let p = parts(input, &env);
    let mut notes = p.notes.clone();
    if let Some(n) = vram_note {
        notes.push(n);
    }
    if p.kv_cpu + p.recurrent_cpu > 0 {
        notes.push(format!(
            "{} of KV cache / state sits in RAM with the CPU layers.",
            gb(p.kv_cpu + p.recurrent_cpu)
        ));
    }
    let v = verdict(p.needed, vram_total);

    let ram_needed = p.weights_cpu + p.kv_cpu + p.recurrent_cpu + RAM_HEADROOM;
    if ram_total > 0 {
        if ram_needed > ram_total {
            notes.push(format!(
                "Needs about {} of system RAM but only {} is installed.",
                gb(ram_needed),
                gb(ram_total)
            ));
        } else if ram_needed as f64 > ram_total as f64 * 0.85 {
            notes.push(format!(
                "RAM is tight: about {} of {} needed; close other apps.",
                gb(ram_needed),
                gb(ram_total)
            ));
        }
    }

    let suggestions = if v == Verdict::Fits {
        Vec::new()
    } else {
        suggestions(input, &env, vram_total)
    };

    FitResult {
        weights_gpu: p.weights_gpu,
        weights_cpu: p.weights_cpu,
        kv_bytes: p.kv_gpu,
        kv_accuracy: p.kv_accuracy.clone(),
        recurrent_bytes: p.recurrent_gpu,
        compute_overhead: p.overhead,
        overhead_accuracy: p.overhead_accuracy.clone(),
        display_reserve: DISPLAY_RESERVE,
        vram_needed: p.needed,
        vram_total,
        vram_free_now: gpu.map(|g| g.vram_total.saturating_sub(g.vram_used)),
        ram_needed,
        ram_total,
        verdict: v,
        suggestions,
        speed_hint: speed_hint(&p, summary),
        measured_vram: None,
        error_pct: None,
        notes,
    }
}

fn speed_hint(p: &Parts, s: &GgufSummary) -> Option<String> {
    let bc = s.block_count.max(1);
    if p.cpu_layers > 0 {
        let kind = if s.is_moe { "models" } else { "dense models" };
        return Some(format!(
            "Warning: {} of {} layers run on the CPU. Partially offloaded {kind} generate \
             much slower (often 2-5x). Prefer a smaller quant or a shorter context.",
            p.cpu_layers, bc
        ));
    }
    if s.is_moe && p.moe_cpu_layers > 0 {
        return Some(format!(
            "Experts of {} of {} layers run from system RAM. MoE models only touch a few experts \
             per token, so expect a modest slowdown.",
            p.moe_cpu_layers, bc
        ));
    }
    None
}

/// Smallest `--n-cpu-moe` (from `start`) that fits at <= 92 %, MoE only.
pub fn smallest_fitting_cpu_moe(
    input: &FitInput,
    summary: &GgufSummary,
    file_bytes: u64,
    vram_total: u64,
    measured_overhead: Option<u64>,
    start: u32,
) -> Option<u32> {
    if !summary.is_moe {
        return None;
    }
    let env = Env { summary, file_bytes, measured_overhead };
    (start..=summary.block_count).find(|n| {
        let cand = FitInput { n_cpu_moe: Some(*n), cpu_moe_all: false, ..input.clone() };
        fits(&parts(&cand, &env), vram_total)
    })
}

fn suggestions(input: &FitInput, env: &Env, vram_total: u64) -> Vec<Suggestion> {
    let mut out = Vec::new();
    let s = env.summary;
    let base = parts(input, env);

    // Lower context: largest multiple of 4096 below the current one that fits.
    let mut c = (input.ctx_size / 4096) * 4096;
    if c == input.ctx_size {
        c = c.saturating_sub(4096);
    }
    while c >= 4096 {
        let p = parts(&FitInput { ctx_size: c, ..input.clone() }, env);
        if fits(&p, vram_total) {
            out.push(Suggestion {
                kind: "lower_ctx".into(),
                label: format!("Lower context to {} (fits at {})", ctx_label(c), pct(&p, vram_total)),
                patch: json!({ "ctxSize": c }),
            });
            break;
        }
        c -= 4096;
    }

    // Quantize the KV cache.
    let cur_k = kv_type_bytes(&input.cache_type_k).unwrap_or(2.0);
    let cur_v = kv_type_bytes(&input.cache_type_v).unwrap_or(2.0);
    for (kind, ty) in [("kv_q8", "q8_0"), ("kv_q4", "q4_0")] {
        let b = kv_type_bytes(ty).unwrap_or(2.0);
        if cur_k <= b && cur_v <= b {
            continue;
        }
        let cand = FitInput { cache_type_k: ty.into(), cache_type_v: ty.into(), ..input.clone() };
        let p = parts(&cand, env);
        let saved = base.needed.saturating_sub(p.needed);
        if saved == 0 {
            continue;
        }
        let outcome = if fits(&p, vram_total) {
            format!("fits at {}", pct(&p, vram_total))
        } else {
            format!("{} of VRAM", pct(&p, vram_total))
        };
        out.push(Suggestion {
            kind: kind.into(),
            label: format!("Quantize KV cache to {ty} (saves {}, {outcome})", gb(saved)),
            patch: json!({ "cacheTypeK": ty, "cacheTypeV": ty, "flashAttn": true }),
        });
    }

    // Move more experts to RAM.
    if s.is_moe && !input.cpu_moe_all {
        let cur = input.n_cpu_moe.unwrap_or(0);
        let start = cur.saturating_add(1);
        match smallest_fitting_cpu_moe(input, s, env.file_bytes, vram_total, env.measured_overhead, start) {
            Some(n) => {
                let p = parts(&FitInput { n_cpu_moe: Some(n), ..input.clone() }, env);
                out.push(Suggestion {
                    kind: "more_cpu_moe".into(),
                    label: format!(
                        "Keep experts of {n} layers in RAM (--n-cpu-moe {n}, fits at {})",
                        pct(&p, vram_total)
                    ),
                    patch: json!({ "nCpuMoe": n, "cpuMoeAll": false }),
                });
            }
            None => {
                let p = parts(&FitInput { cpu_moe_all: true, ..input.clone() }, env);
                if fits(&p, vram_total) {
                    out.push(Suggestion {
                        kind: "more_cpu_moe".into(),
                        label: format!("Keep all experts in RAM (--cpu-moe, fits at {})", pct(&p, vram_total)),
                        patch: json!({ "cpuMoeAll": true }),
                    });
                }
            }
        }
    }

    // Smaller quant: advisory only.
    let quant = s.file_type.as_deref().unwrap_or("this quant");
    out.push(Suggestion {
        kind: "smaller_quant".into(),
        label: format!("Pick a smaller quant than {quant} (e.g. Q4_K_M or IQ4_XS)"),
        patch: json!({}),
    });
    out
}

/// Add a "close other GPU apps" suggestion when the forecast is not a clean fit and other
/// processes hold VRAM. `others` must already exclude llama-server itself.
pub fn add_close_apps(result: &mut FitResult, others: &[GpuProcess]) {
    if result.verdict == Verdict::Fits {
        return;
    }
    let mut heavy: Vec<&GpuProcess> = others.iter().filter(|p| p.vram_bytes.unwrap_or(0) >= 64 * MIB).collect();
    if heavy.is_empty() {
        // WDDM often hides per-process usage; still list the apps by name.
        heavy = others.iter().take(3).collect();
    }
    if heavy.is_empty() {
        return;
    }
    let names: Vec<String> = heavy
        .iter()
        .take(3)
        .map(|p| match p.vram_bytes {
            Some(b) if b > 0 => format!("{} ({})", p.name, gb(b)),
            _ => p.name.clone(),
        })
        .collect();
    result.suggestions.push(Suggestion {
        kind: "close_apps".into(),
        label: format!("Close other GPU apps: {}", names.join(", ")),
        patch: json!({}),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1_000_000_000;

    fn input(ctx: u32, kv: &str) -> FitInput {
        FitInput {
            model_path: "m.gguf".into(),
            ctx_size: ctx,
            cache_type_k: kv.into(),
            cache_type_v: kv.into(),
            ngl: 99,
            n_cpu_moe: None,
            cpu_moe_all: false,
            parallel: 1,
            vram_total_override: None,
            ubatch: None,
            flash_attn: None,
        }
    }

    fn gpu24() -> GpuStats {
        GpuStats {
            name: "RTX 5090 Laptop".into(),
            vram_total: 24 * GIB,
            vram_used: 2 * GIB,
            source: "nvml".into(),
            ..Default::default()
        }
    }

    /// Qwen3.6-35B-A3B (hybrid Gated DeltaNet + gated attention, 256 experts):
    /// 40 blocks, 10 full-attention layers, 2 KV heads x 256 dims. UD-Q5_K_M ~24.9 GB,
    /// ~22.6 GB of it routed experts. 30 DeltaNet layers x ~2.2 MB recurrent state.
    fn qwen36_moe() -> (GgufSummary, u64) {
        (
            GgufSummary {
                architecture: "qwen35moe".into(),
                block_count: 40,
                context_length: Some(262144),
                embedding_length: Some(2048),
                head_count: Some(16),
                head_count_kv: Some(2),
                key_length: Some(256),
                value_length: Some(256),
                expert_count: Some(256),
                expert_used_count: Some(8),
                file_type: Some("Q5_K_M".into()),
                kv_layers: Some(10),
                kv_layers_source: "exact".into(),
                recurrent_state_bytes: Some(66_000_000),
                is_moe: true,
                expert_tensor_bytes: Some(22_600_000_000),
                total_tensor_bytes: Some(24_890_000_000),
                ..Default::default()
            },
            24_900_000_000,
        )
    }

    /// Qwen3.8-27B dense hybrid: 64 blocks, 16 full-attention layers, 4 KV heads x 256.
    /// Q4_K_M ~16.7 GB, 48 DeltaNet layers x ~3.1 MB state.
    fn qwen38_dense() -> (GgufSummary, u64) {
        (
            GgufSummary {
                architecture: "qwen35".into(),
                block_count: 64,
                embedding_length: Some(5120),
                head_count: Some(24),
                head_count_kv: Some(4),
                key_length: Some(256),
                value_length: Some(256),
                file_type: Some("Q4_K_M".into()),
                kv_layers: Some(16),
                kv_layers_source: "exact".into(),
                recurrent_state_bytes: Some(151_000_000),
                is_moe: false,
                ..Default::default()
            },
            16_700_000_000,
        )
    }

    #[test]
    fn kv_bytes_formula() {
        let (s, f) = qwen36_moe();
        let r = estimate(&input(131072, "q8_0"), &s, f, Some(&gpu24()), 64 * GIB, None);
        // 10 layers * 131072 * 2 heads * (256 + 256) * 34/32
        assert_eq!(r.kv_bytes, 1_426_063_360);
        assert_eq!(r.kv_accuracy, Accuracy::Estimate);
        let r = estimate(&input(131072, "f16"), &s, f, Some(&gpu24()), 64 * GIB, None);
        assert_eq!(r.kv_bytes, 10 * 131072 * 2 * 512 * 2);
        let mut i = input(131072, "f16");
        i.cache_type_v = "q4_0".into();
        let r = estimate(&i, &s, f, Some(&gpu24()), 64 * GIB, None);
        assert_eq!(r.kv_bytes, (10.0 * 131072.0 * 2.0 * (256.0 * 2.0 + 256.0 * 18.0 / 32.0)) as u64);
    }

    #[test]
    fn kv_not_multiplied_by_parallel_but_recurrent_is() {
        let (s, f) = qwen36_moe();
        let a = estimate(&input(65536, "q8_0"), &s, f, None, 0, None);
        let mut i = input(65536, "q8_0");
        i.parallel = 4;
        let b = estimate(&i, &s, f, None, 0, None);
        assert_eq!(a.kv_bytes, b.kv_bytes);
        assert_eq!(a.recurrent_bytes, 66_000_000);
        assert_eq!(b.recurrent_bytes, 4 * 66_000_000);
    }

    #[test]
    fn reference_moe_128k_on_24gb() {
        let (s, f) = qwen36_moe();
        // All experts on GPU: does not fit.
        let r = estimate(&input(131072, "q8_0"), &s, f, Some(&gpu24()), 64 * GIB, None);
        assert_eq!(r.verdict, Verdict::WontFit);
        let moe = r.suggestions.iter().find(|s| s.kind == "more_cpu_moe").expect("moe suggestion");
        let n = moe.patch["nCpuMoe"].as_u64().unwrap();
        assert!((8..=12).contains(&n), "suggested n_cpu_moe {n}");

        // The preset's --n-cpu-moe 8: on the edge (tight, under 100%).
        let mut i = input(131072, "q8_0");
        i.n_cpu_moe = Some(8);
        let r8 = estimate(&i, &s, f, Some(&gpu24()), 64 * GIB, None);
        assert_ne!(r8.verdict, Verdict::WontFit);
        assert_eq!(r8.weights_cpu, 22_600_000_000 * 8 / 40);
        assert_eq!(r8.weights_gpu + r8.weights_cpu, f);
        let ratio = r8.vram_needed as f64 / r8.vram_total as f64;
        assert!(ratio > 0.88 && ratio < 1.0, "ratio {ratio}");
        assert!(r8.speed_hint.as_deref().unwrap().contains("modest"));
        // Experts in RAM need RAM: 4.5 GB + 2 GB headroom.
        assert_eq!(r8.ram_needed, r8.weights_cpu + RAM_HEADROOM);

        i.n_cpu_moe = Some(n as u32);
        let rn = estimate(&i, &s, f, Some(&gpu24()), 64 * GIB, None);
        assert_eq!(rn.verdict, Verdict::Fits);
        assert!(rn.suggestions.is_empty());
        i.n_cpu_moe = Some(n as u32 - 1);
        assert_ne!(estimate(&i, &s, f, Some(&gpu24()), 64 * GIB, None).verdict, Verdict::Fits);
    }

    #[test]
    fn reference_dense_27b_64k_fits() {
        let (s, f) = qwen38_dense();
        let r = estimate(&input(65536, "q8_0"), &s, f, Some(&gpu24()), 64 * GIB, None);
        assert_eq!(r.verdict, Verdict::Fits, "needed {}", r.vram_needed);
        assert_eq!(r.weights_cpu, 0);
        assert!(r.speed_hint.is_none());
        let ratio = r.vram_needed as f64 / r.vram_total as f64;
        assert!(ratio > 0.75 && ratio < 0.92, "ratio {ratio}");
        // f16 KV doubles the cache but still fits.
        let r16 = estimate(&input(65536, "f16"), &s, f, Some(&gpu24()), 64 * GIB, None);
        assert!(r16.kv_bytes > r.kv_bytes * 18 / 10);
    }

    #[test]
    fn assumed_kv_layers_is_upper_bound() {
        let (mut s, f) = qwen38_dense();
        s.kv_layers = None;
        s.kv_layers_source = "assumed".into();
        let r = estimate(&input(65536, "q8_0"), &s, f, Some(&gpu24()), 64 * GIB, None);
        assert_eq!(r.kv_accuracy, Accuracy::UpperBound);
        assert!(r.notes.iter().any(|n| n.contains("upper bound")));
        // 4x the layers -> 4x the KV.
        assert_eq!(r.kv_bytes, 64 * 65536 * 4 * 544);
        assert_ne!(r.verdict, Verdict::Fits);
        // Suggests a lower context that fits, as a multiple of 4096.
        let lc = r.suggestions.iter().find(|s| s.kind == "lower_ctx").unwrap();
        let c = lc.patch["ctxSize"].as_u64().unwrap();
        assert_eq!(c % 4096, 0);
        assert!(c < 65536);
        let mut i = input(c as u32, "q8_0");
        assert_eq!(estimate(&i, &s, f, Some(&gpu24()), 0, None).verdict, Verdict::Fits);
        i.ctx_size = c as u32 + 4096;
        assert_ne!(estimate(&i, &s, f, Some(&gpu24()), 0, None).verdict, Verdict::Fits);
        // Dense: no MoE suggestion; q4 KV offered, q8 not (already q8).
        assert!(r.suggestions.iter().all(|s| s.kind != "more_cpu_moe" && s.kind != "kv_q8"));
        let q4 = r.suggestions.iter().find(|s| s.kind == "kv_q4").unwrap();
        assert_eq!(q4.patch["cacheTypeK"], "q4_0");
        assert_eq!(q4.patch["cacheTypeV"], "q4_0");
    }

    #[test]
    fn kv_q8_suggested_from_f16() {
        let (s, f) = qwen38_dense();
        let mut i = input(262144, "f16");
        i.vram_total_override = Some(24 * GIB);
        let r = estimate(&i, &s, f, None, 0, None);
        assert_ne!(r.verdict, Verdict::Fits);
        let kinds: Vec<&str> = r.suggestions.iter().map(|s| s.kind.as_str()).collect();
        assert!(kinds.contains(&"kv_q8") && kinds.contains(&"kv_q4") && kinds.contains(&"smaller_quant"));
        assert!(r.notes.is_empty(), "override means no 24 GB note: {:?}", r.notes);
    }

    #[test]
    fn cpu_moe_all_moves_all_experts() {
        let (s, f) = qwen36_moe();
        let mut i = input(32768, "q8_0");
        i.cpu_moe_all = true;
        i.n_cpu_moe = Some(3); // ignored
        let r = estimate(&i, &s, f, Some(&gpu24()), 64 * GIB, None);
        assert_eq!(r.weights_cpu, 22_600_000_000);
        assert_eq!(r.weights_gpu, f - 22_600_000_000);
    }

    #[test]
    fn partial_ngl_offloads_layers_and_warns() {
        let (s, f) = qwen38_dense();
        let mut i = input(65536, "q8_0");
        i.ngl = 48; // 16 of 64 layers on CPU
        let r = estimate(&i, &s, f, Some(&gpu24()), 64 * GIB, None);
        assert_eq!(r.weights_cpu, f / 4);
        assert_eq!(r.kv_bytes, 16 * 65536 * 4 * 544 * 3 / 4);
        assert!(r.speed_hint.as_deref().unwrap().contains("dense"));
        assert!(r.notes.iter().any(|n| n.contains("RAM")));
    }

    #[test]
    fn ngl_and_cpu_moe_overlap_on_first_layers() {
        let (s, f) = qwen36_moe();
        let mut i = input(32768, "q8_0");
        i.ngl = 36; // layers 0-3 on CPU
        i.n_cpu_moe = Some(6); // experts of layers 0-5; 0-3 already on CPU
        let r = estimate(&i, &s, f, Some(&gpu24()), 64 * GIB, None);
        assert_eq!(r.weights_cpu, f * 4 / 40 + 22_600_000_000 * 2 / 40);
    }

    #[test]
    fn measured_overhead_and_missing_gpu() {
        let (s, f) = qwen38_dense();
        let r = estimate(&input(65536, "q8_0"), &s, f, None, 0, Some(1500 * MIB));
        assert_eq!(r.overhead_accuracy, Accuracy::Measured);
        assert_eq!(r.compute_overhead, 1500 * MIB);
        assert_eq!(r.vram_total, DEFAULT_VRAM);
        assert!(r.notes.iter().any(|n| n.contains("24 GB")));
        assert!(r.vram_free_now.is_none());
        let r = estimate(&input(65536, "q8_0"), &s, f, Some(&gpu24()), 0, None);
        assert_eq!(r.overhead_accuracy, Accuracy::Estimate);
        assert_eq!(r.vram_free_now, Some(22 * GIB));
        // 600 MiB + 512 * 600 KiB + 64K * 512 * 2
        assert_eq!(r.compute_overhead, 600 * MIB + 300 * MIB + 64 * MIB);
    }

    #[test]
    fn overhead_grows_with_ubatch_and_without_flash_attn() {
        let (s, f) = qwen38_dense();
        let base = estimate(&input(65536, "q8_0"), &s, f, None, 0, None).compute_overhead;
        let mut i = input(65536, "q8_0");
        i.ubatch = Some(2048);
        let big = estimate(&i, &s, f, None, 0, None).compute_overhead;
        assert!(big > base + GIB / 2);
        let mut i = input(65536, "q8_0");
        i.flash_attn = Some(false);
        let nofa = estimate(&i, &s, f, None, 0, None).compute_overhead;
        assert_eq!(nofa - base, 24 * 65536 * 512 * 4);
    }

    #[test]
    fn verdict_thresholds() {
        assert_eq!(verdict(92, 100), Verdict::Fits);
        assert_eq!(verdict(93, 100), Verdict::Tight);
        assert_eq!(verdict(100, 100), Verdict::Tight);
        assert_eq!(verdict(101, 100), Verdict::WontFit);
    }

    #[test]
    fn ram_notes() {
        let (s, f) = qwen36_moe();
        let mut i = input(32768, "q8_0");
        i.cpu_moe_all = true;
        let r = estimate(&i, &s, f, Some(&gpu24()), 16 * GIB, None);
        assert!(r.notes.iter().any(|n| n.contains("only")), "{:?}", r.notes);
        let r = estimate(&i, &s, f, Some(&gpu24()), 26 * GIB, None);
        assert!(r.notes.iter().any(|n| n.contains("RAM is tight")), "{:?}", r.notes);
    }

    #[test]
    fn unknown_expert_bytes_assumed() {
        let (mut s, f) = qwen36_moe();
        s.expert_tensor_bytes = None;
        let mut i = input(32768, "q8_0");
        i.cpu_moe_all = true;
        let r = estimate(&i, &s, f, None, 0, None);
        assert_eq!(r.weights_cpu, (f as f64 * 0.85) as u64);
        assert!(r.notes.iter().any(|n| n.contains("assumed")));
    }

    #[test]
    fn smallest_fitting_and_close_apps() {
        let (s, f) = qwen36_moe();
        let i = input(131072, "q8_0");
        let n = smallest_fitting_cpu_moe(&i, &s, f, 24 * GIB, None, 0).unwrap();
        assert_eq!(n, 10);
        let (d, df) = qwen38_dense();
        assert!(smallest_fitting_cpu_moe(&i, &d, df, 24 * GIB, None, 0).is_none());

        let mut r = estimate(&i, &s, f, Some(&gpu24()), 0, None);
        let others = vec![
            GpuProcess { pid: 1, name: "chrome.exe".into(), vram_bytes: Some(900 * MIB) },
            GpuProcess { pid: 2, name: "tiny.exe".into(), vram_bytes: Some(MIB) },
        ];
        add_close_apps(&mut r, &others);
        let c = r.suggestions.last().unwrap();
        assert_eq!(c.kind, "close_apps");
        assert!(c.label.contains("chrome.exe") && !c.label.contains("tiny"));

        let mut ok = estimate(&input(4096, "q8_0"), &d, df, Some(&gpu24()), 0, None);
        assert_eq!(ok.verdict, Verdict::Fits);
        add_close_apps(&mut ok, &others);
        assert!(ok.suggestions.is_empty());
    }

    #[test]
    fn missing_head_info_counts_no_kv() {
        let s = GgufSummary { block_count: 10, kv_layers_source: "assumed".into(), ..Default::default() };
        let r = estimate(&input(4096, "f16"), &s, 4 * GB, None, 0, None);
        assert_eq!(r.kv_bytes, 0);
        assert_eq!(r.kv_accuracy, Accuracy::UpperBound);
    }

    #[test]
    fn print_reference_numbers() {
        let g = gpu24();
        let show = |label: &str, r: &FitResult| {
            println!(
                "{label}: wgpu {:.2} wcpu {:.2} kv {:.2} rec {:.3} ovh {:.2} need {:.2}/{:.2} GiB ({:.1}%) {:?}",
                r.weights_gpu as f64 / GIB as f64,
                r.weights_cpu as f64 / GIB as f64,
                r.kv_bytes as f64 / GIB as f64,
                r.recurrent_bytes as f64 / GIB as f64,
                r.compute_overhead as f64 / GIB as f64,
                r.vram_needed as f64 / GIB as f64,
                r.vram_total as f64 / GIB as f64,
                r.vram_needed as f64 * 100.0 / r.vram_total as f64,
                r.verdict
            )
        };
        let (s, f) = qwen36_moe();
        for n in [0, 8, 10] {
            let mut i = input(131072, "q8_0");
            i.n_cpu_moe = Some(n);
            show(&format!("35B-A3B 128K q8 n_cpu_moe={n}"), &estimate(&i, &s, f, Some(&g), 64 * GIB, None));
        }
        let (d, df) = qwen38_dense();
        show("27B 64K q8", &estimate(&input(65536, "q8_0"), &d, df, Some(&g), 64 * GIB, None));
        show("27B 64K f16", &estimate(&input(65536, "f16"), &d, df, Some(&g), 64 * GIB, None));
    }

    #[test]
    fn patches_are_camel_case() {
        let (s, f) = qwen36_moe();
        let r = estimate(&input(262144, "f16"), &s, f, Some(&gpu24()), 0, None);
        for sug in &r.suggestions {
            for k in sug.patch.as_object().unwrap().keys() {
                assert!(!k.contains('_'), "{k}");
            }
        }
    }
}
