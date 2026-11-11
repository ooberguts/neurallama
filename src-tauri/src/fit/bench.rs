//! Benchmark: one fixed ~500-token prompt through `/completion` (no prompt cache), 128
//! generated tokens; reads the server's own `timings`. The result is saved to the profile.
//!
//! Note: `llama-bench.exe` (shipped next to llama-server.exe) is not used yet; the HTTP path
//! measures the configuration that is actually being served.

use super::BenchResult;
use crate::server::ServerState;
use crate::settings::BenchRecord;
use crate::state::AppState;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

const N_PREDICT: u32 = 128;
const LOAD_TIMEOUT: Duration = Duration::from_secs(600);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

const PARAGRAPH: &str = "A lighthouse keeper on a small northern island kept a careful log of \
every ship that passed, the direction of the wind, the height of the waves and the colour of \
the evening sky. Over many years the notebooks filled a whole shelf, and visiting scientists \
later used them to study how storms had changed along the coast. The keeper never thought of \
the work as research; it was simply a habit, done each night after the lamp was lit and the \
kettle had boiled.";

/// The fixed benchmark prompt (~500 tokens with most tokenizers).
pub fn prompt() -> String {
    let mut s = String::from("Read the following notes, then continue the story in your own words.\n\n");
    for i in 1..=5 {
        s.push_str(&format!("Note {i}. {PARAGRAPH}\n\n"));
    }
    s.push_str("Continue the story:");
    s
}

/// Pull the numbers out of a `/completion` response.
pub fn parse_timings(v: &Value) -> Option<(u32, f64, u32, f64)> {
    let t = v.get("timings")?;
    let f = |k: &str| t.get(k).and_then(Value::as_f64);
    Some((
        f("prompt_n")? as u32,
        f("prompt_per_second").unwrap_or(0.0),
        f("predicted_n")? as u32,
        f("predicted_per_second").unwrap_or(0.0),
    ))
}

pub async fn run(app: &AppHandle, profile_id: Option<String>) -> Result<BenchResult, String> {
    let state = app.state::<AppState>();
    let settings = state.settings();
    let status = state.server.status();
    let ready = matches!(status.state, ServerState::Ready | ServerState::Attached);
    let wrong_profile = profile_id.is_some() && profile_id != status.profile_id;

    if !ready || wrong_profile {
        let id = profile_id
            .clone()
            .or_else(|| settings.active_profile.clone())
            .ok_or("No profile to benchmark. Pick a model to serve first.")?;
        let profile = settings.profile(&id).cloned().ok_or("Profile not found.")?;
        if !matches!(status.state, ServerState::Stopped | ServerState::Crashed) {
            state.server.stop(app).await?;
        }
        state.server.start(app, &profile, &settings).await?;
        state.server.wait_ready(LOAD_TIMEOUT).await.map_err(|e| format!("Server did not become ready: {e}"))?;
    }
    let target = profile_id.or(state.server.status().profile_id).or(settings.active_profile.clone());

    let body = json!({
        "prompt": prompt(),
        "n_predict": N_PREDICT,
        "cache_prompt": false,
        "ignore_eos": true,
        "temperature": 0.0,
        "stream": false,
    });
    let mut req = state.http.post(format!("{}/completion", settings.base_url())).timeout(REQUEST_TIMEOUT).json(&body);
    if !settings.api_key.is_empty() {
        req = req.bearer_auth(&settings.api_key);
    }
    let t0 = Instant::now();
    let resp = req.send().await.map_err(|e| format!("Benchmark request failed: {e}"))?;
    let code = resp.status();
    let text = resp.text().await.map_err(|e| format!("Benchmark response failed: {e}"))?;
    let total_ms = t0.elapsed().as_millis() as u64;
    if !code.is_success() {
        return Err(format!("Server returned {code}: {}", text.chars().take(300).collect::<String>()));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("Unexpected benchmark response: {e}"))?;
    let (prompt_tokens, prompt_tps, gen_tokens, gen_tps) =
        parse_timings(&v).ok_or("The server's response has no timings; is this llama-server?")?;

    let result = BenchResult { prompt_tokens, gen_tokens, prompt_tps, gen_tps, total_ms };
    if let Some(id) = target {
        let rec = BenchRecord { at: chrono::Local::now().to_rfc3339(), prompt_tps, gen_tps };
        state.update_settings(|s| {
            if let Some(p) = s.profiles.iter_mut().find(|p| p.id == id) {
                p.bench = Some(rec);
            }
        })?;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_is_about_500_tokens() {
        let words = prompt().split_whitespace().count();
        assert!((350..=520).contains(&words), "{words} words");
    }

    #[test]
    fn timings_parse() {
        let v = json!({"content": "x", "timings": {
            "prompt_n": 498, "prompt_ms": 120.5, "prompt_per_second": 4132.8,
            "predicted_n": 128, "predicted_ms": 900.0, "predicted_per_second": 142.2}});
        assert_eq!(parse_timings(&v), Some((498, 4132.8, 128, 142.2)));
        assert_eq!(parse_timings(&json!({"content": ""})), None);
    }
}
