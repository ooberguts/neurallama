//! llama-server telemetry: `/metrics` (Prometheus text), `/slots` and `/props`.
//!
//! Endpoint names and fields differ between llama.cpp builds, so everything is parsed
//! generically and every value is optional. A missing or slow endpoint only blanks the
//! values it would have provided.

use super::ServerMetrics;
use serde_json::Value;
use std::collections::HashMap;
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_millis(800);
const PROPS_REFRESH: Duration = Duration::from_secs(30);

/// Prometheus text → metric name → value. Labels are dropped (values for the same name are
/// summed); each metric is stored under its full name and under the part after the last ':'
/// (`llamacpp:prompt_tokens_total` → also `prompt_tokens_total`).
pub fn parse_prometheus(text: &str) -> HashMap<String, f64> {
    let mut out: HashMap<String, f64> = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, rest) = match line.find('{') {
            Some(i) => {
                let close = line[i..].find('}').map(|j| i + j + 1).unwrap_or(line.len());
                (&line[..i], &line[close..])
            }
            None => match line.split_once(char::is_whitespace) {
                Some((n, r)) => (n, r),
                None => continue,
            },
        };
        let Some(val) = rest.split_whitespace().next().and_then(|v| v.parse::<f64>().ok()) else {
            continue;
        };
        if !val.is_finite() {
            continue;
        }
        *out.entry(name.to_string()).or_insert(0.0) += val;
        if let Some((_, short)) = name.rsplit_once(':') {
            *out.entry(short.to_string()).or_insert(0.0) += val;
        }
    }
    out
}

fn pick(m: &HashMap<String, f64>, names: &[&str]) -> Option<f64> {
    names.iter().find_map(|n| m.get(*n).copied())
}

/// One slot from `/slots`, normalized across builds.
#[derive(Debug, Clone, PartialEq)]
pub struct SlotInfo {
    pub id: i64,
    pub task: Option<i64>,
    pub processing: bool,
    pub n_decoded: Option<u64>,
    pub n_ctx: Option<u32>,
}

pub fn parse_slots(v: &Value) -> Option<Vec<SlotInfo>> {
    let arr = v.as_array()?;
    Some(
        arr.iter()
            .enumerate()
            .map(|(i, s)| {
                let processing = s
                    .get("is_processing")
                    .and_then(Value::as_bool)
                    .or_else(|| s.get("state").and_then(Value::as_i64).map(|st| st != 0))
                    .unwrap_or(false);
                let nt = s.get("next_token");
                let nt = nt.and_then(|n| n.as_array().and_then(|a| a.first())).or(nt);
                let n_decoded = nt
                    .and_then(|n| n.get("n_decoded"))
                    .or_else(|| s.get("n_decoded"))
                    .and_then(Value::as_u64);
                SlotInfo {
                    id: s.get("id").and_then(Value::as_i64).unwrap_or(i as i64),
                    task: s.get("id_task").and_then(Value::as_i64),
                    processing,
                    n_decoded,
                    n_ctx: s.get("n_ctx").and_then(Value::as_u64).map(|n| n as u32),
                }
            })
            .collect(),
    )
}

#[derive(Debug, Clone, Default)]
pub struct Props {
    pub model: Option<String>,
    pub build: Option<String>,
    pub n_ctx: Option<u32>,
    pub total_slots: Option<u32>,
}

pub fn parse_props(v: &Value) -> Props {
    let s = |k: &str| v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from);
    let file_name = |p: String| {
        p.rsplit(['\\', '/']).next().map(String::from).unwrap_or(p)
    };
    let model = s("model_alias").or_else(|| s("model_path").map(file_name)).or_else(|| s("model"));
    let build = s("build_info").or_else(|| {
        let n = v.get("build_number").and_then(Value::as_i64)?;
        Some(format!("b{n}"))
    });
    let n_ctx = v
        .get("default_generation_settings")
        .and_then(|d| d.get("n_ctx"))
        .or_else(|| v.get("n_ctx"))
        .and_then(Value::as_u64)
        .map(|n| n as u32);
    Props {
        model,
        build,
        n_ctx,
        total_slots: v.get("total_slots").and_then(Value::as_u64).map(|n| n as u32),
    }
}

/// Counter delta that tolerates a counter reset (server restart): a smaller value means the
/// counter started over, so everything it shows is new.
pub fn counter_delta(prev: Option<u64>, cur: u64) -> u64 {
    match prev {
        Some(p) if cur >= p => cur - p,
        Some(_) => cur,
        None => 0,
    }
}

/// What one poll adds on top of the snapshot values, for lifetime stats.
#[derive(Debug, Clone, Default)]
pub struct PollDelta {
    pub gen_tokens: u64,
    pub requests: u64,
    /// A slot produced tokens since the last poll.
    pub generating: bool,
}

/// Keeps the previous poll to turn counters and slot progress into rates.
#[derive(Default)]
pub struct MetricsPoller {
    session: Option<String>,
    last_at: Option<Instant>,
    gen_total: Option<u64>,
    slots: HashMap<i64, (Option<i64>, bool, u64)>,
    slots_supported: bool,
    last_gen_tps: Option<f64>,
    props: Option<Props>,
    props_at: Option<Instant>,
}

impl MetricsPoller {
    pub fn new() -> Self {
        Self { slots_supported: true, ..Default::default() }
    }

    /// Forget everything (server stopped or replaced).
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Poll the server once. `session` identifies the server instance (pid + start time); a
    /// change resets the baselines.
    pub async fn poll(
        &mut self,
        http: &reqwest::Client,
        base: &str,
        api_key: &str,
        session: &str,
    ) -> (ServerMetrics, PollDelta) {
        if self.session.as_deref() != Some(session) {
            self.reset();
            self.session = Some(session.to_string());
        }
        let need_props = self.props.is_none()
            || self.props.as_ref().is_some_and(|p| p.model.is_none())
                && self.props_at.is_none_or(|t| t.elapsed() > PROPS_REFRESH);

        let get = |path: &'static str| {
            let mut req = http.get(format!("{base}{path}")).timeout(TIMEOUT);
            if !api_key.is_empty() {
                req = req.bearer_auth(api_key);
            }
            req.send()
        };
        let want_slots = self.slots_supported;
        let (metrics_r, slots_r, props_r) = tokio::join!(
            async { get("/metrics").await.ok().filter(|r| r.status().is_success())?.text().await.ok() },
            async {
                if !want_slots {
                    return Err(None);
                }
                let r = get("/slots").await.map_err(|_| None)?;
                let st = r.status().as_u16();
                if st == 404 || st == 501 {
                    return Err(Some(st));
                }
                if !r.status().is_success() {
                    return Err(None);
                }
                r.json::<Value>().await.map_err(|_| None)
            },
            async {
                if !need_props {
                    return None;
                }
                get("/props").await.ok().filter(|r| r.status().is_success())?.json::<Value>().await.ok()
            },
        );
        if need_props {
            self.props_at = Some(Instant::now());
            if let Some(v) = props_r {
                self.props = Some(parse_props(&v));
            }
        }

        let now = Instant::now();
        let dt = self.last_at.map(|t| now.duration_since(t).as_secs_f64()).filter(|d| *d > 0.05);
        let mut out = ServerMetrics::default();
        let mut delta = PollDelta::default();

        // --- /metrics ---
        let mut counter_gen_delta = None;
        if let Some(text) = metrics_r {
            let m = parse_prometheus(&text);
            let gen_total =
                pick(&m, &["tokens_predicted_total", "predicted_tokens_total", "n_tokens_predicted_total"])
                    .map(|v| v as u64);
            let prompt_total = pick(&m, &["prompt_tokens_total", "n_prompt_tokens_processed_total"]).map(|v| v as u64);
            out.gen_tokens_total = gen_total;
            out.prompt_tokens_total = prompt_total;
            out.prompt_tps = pick(&m, &["prompt_tokens_seconds"]).filter(|v| *v > 0.0);
            out.requests_processing = pick(&m, &["requests_processing"]).map(|v| v as u32);
            out.requests_deferred = pick(&m, &["requests_deferred"]).map(|v| v as u32);
            out.kv_usage = pick(&m, &["kv_cache_usage_ratio"]).map(|v| v.clamp(0.0, 1.0));
            if let Some(g) = gen_total {
                counter_gen_delta = Some(counter_delta(self.gen_total, g));
                self.gen_total = Some(g);
            }
            // Counter-based fallback rate (used only without /slots).
            if !self.slots_supported {
                let processing = out.requests_processing.unwrap_or(0) > 0;
                let moved = counter_gen_delta.unwrap_or(0) > 0;
                out.gen_tps = Some(if moved {
                    pick(&m, &["predicted_tokens_seconds"])
                        .filter(|v| *v > 0.0)
                        .or_else(|| dt.map(|d| counter_gen_delta.unwrap_or(0) as f64 / d))
                        .unwrap_or(0.0)
                } else if processing {
                    self.last_gen_tps.unwrap_or(0.0)
                } else {
                    0.0
                });
                delta.generating = moved;
                if moved {
                    delta.requests = 1;
                }
            }
        }

        // --- /slots ---
        let mut slot_tokens = 0u64;
        match slots_r {
            Ok(v) => {
                if let Some(slots) = parse_slots(&v) {
                    let mut decoded_now = 0u64;
                    let mut busy = 0u32;
                    let mut next = HashMap::new();
                    for s in &slots {
                        let dec = s.n_decoded.unwrap_or(0);
                        if s.processing {
                            busy += 1;
                            let prev = self.slots.get(&s.id);
                            let same_task = prev.is_some_and(|(t, was, _)| *was && *t == s.task);
                            let d = match prev {
                                Some((_, _, pd)) if same_task && dec >= *pd => dec - pd,
                                _ => dec,
                            };
                            if !same_task {
                                delta.requests += 1;
                            }
                            decoded_now += d;
                        }
                        next.insert(s.id, (s.task, s.processing, dec));
                    }
                    self.slots = next;
                    slot_tokens = decoded_now;
                    out.slots_busy = Some(busy);
                    out.slots_total = Some(slots.len() as u32);
                    out.ctx_per_slot = slots.iter().find_map(|s| s.n_ctx);
                    out.gen_tps = Some(match dt {
                        Some(d) if busy > 0 => decoded_now as f64 / d,
                        _ => 0.0,
                    });
                    delta.generating = decoded_now > 0;
                }
            }
            Err(Some(_)) => {
                // Endpoint disabled in this build: switch to counter deltas from now on.
                self.slots_supported = false;
            }
            Err(None) => {
                // Slow or failed this tick: keep showing the last rate while busy.
                if out.requests_processing.unwrap_or(0) > 0 {
                    out.gen_tps = self.last_gen_tps;
                }
            }
        }

        delta.gen_tokens = counter_gen_delta.unwrap_or(slot_tokens);

        if let Some(p) = &self.props {
            out.model = p.model.clone();
            out.build = p.build.clone();
            if out.ctx_per_slot.is_none() {
                out.ctx_per_slot = p.n_ctx;
            }
            if out.slots_total.is_none() {
                out.slots_total = p.total_slots;
            }
        }
        if out.slots_busy.is_none() {
            out.slots_busy = out.requests_processing;
        }

        self.last_gen_tps = out.gen_tps;
        self.last_at = Some(now);
        (out, delta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn prometheus_parsing() {
        let text = "# HELP llamacpp:prompt_tokens_total Number of prompt tokens processed.\n\
                    # TYPE llamacpp:prompt_tokens_total counter\n\
                    llamacpp:prompt_tokens_total 1234\n\
                    llamacpp:tokens_predicted_total 567\n\
                    llamacpp:predicted_tokens_seconds 42.5\n\
                    llamacpp:kv_cache_usage_ratio 0.25\n\
                    llamacpp:requests_processing{slot=\"0\"} 1 1700000000\n\
                    weird line\n\
                    nan_metric NaN\n";
        let m = parse_prometheus(text);
        assert_eq!(m["llamacpp:prompt_tokens_total"], 1234.0);
        assert_eq!(m["prompt_tokens_total"], 1234.0);
        assert_eq!(m["tokens_predicted_total"], 567.0);
        assert_eq!(m["predicted_tokens_seconds"], 42.5);
        assert_eq!(m["requests_processing"], 1.0);
        assert!(!m.contains_key("nan_metric"));
        assert_eq!(pick(&m, &["missing", "kv_cache_usage_ratio"]), Some(0.25));
    }

    #[test]
    fn slots_new_and_old_formats() {
        let new = json!([
            {"id": 0, "id_task": 7, "n_ctx": 65536, "is_processing": true,
             "next_token": [{"has_next_token": true, "n_decoded": 33}]},
            {"id": 1, "n_ctx": 65536, "is_processing": false, "next_token": [{"n_decoded": 0}]}
        ]);
        let s = parse_slots(&new).unwrap();
        assert_eq!(s[0], SlotInfo { id: 0, task: Some(7), processing: true, n_decoded: Some(33), n_ctx: Some(65536) });
        assert!(!s[1].processing);
        let old = json!([{"id": 0, "state": 1, "n_ctx": 4096, "next_token": {"n_decoded": 5}}]);
        let s = parse_slots(&old).unwrap();
        assert!(s[0].processing);
        assert_eq!(s[0].n_decoded, Some(5));
        let older = json!([{"id": 0, "state": 0, "n_decoded": 9}]);
        assert_eq!(parse_slots(&older).unwrap()[0].n_decoded, Some(9));
        assert!(parse_slots(&json!({"error": "x"})).is_none());
    }

    #[test]
    fn props_parsing() {
        let p = parse_props(&json!({
            "model_path": "C:\\AI\\models\\unsloth\\Q\\Qwen3.6-35B-A3B-UD-Q5_K_M.gguf",
            "build_info": "b6100-abcdef",
            "total_slots": 2,
            "default_generation_settings": {"n_ctx": 131072}
        }));
        assert_eq!(p.model.as_deref(), Some("Qwen3.6-35B-A3B-UD-Q5_K_M.gguf"));
        assert_eq!(p.build.as_deref(), Some("b6100-abcdef"));
        assert_eq!(p.n_ctx, Some(131072));
        assert_eq!(p.total_slots, Some(2));
        let p = parse_props(&json!({"model_alias": "local", "n_ctx": 4096, "build_number": 42}));
        assert_eq!(p.model.as_deref(), Some("local"));
        assert_eq!(p.build.as_deref(), Some("b42"));
        assert_eq!(p.n_ctx, Some(4096));
    }

    #[test]
    fn counter_reset_handling() {
        assert_eq!(counter_delta(None, 100), 0);
        assert_eq!(counter_delta(Some(100), 150), 50);
        assert_eq!(counter_delta(Some(500), 20), 20);
    }
}
