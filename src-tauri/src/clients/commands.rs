//! Connect tab commands.

use super::hermes::{self, DiscoverCtx, PushSpec};
use super::{ClientWriteResult, ConnectInfo, HermesPushResult, HermesTarget, TestResult};
use crate::settings::Settings;
use crate::state::{AppState, CmdResult};
use serde_yaml::{Mapping, Value};
use std::time::{Duration, Instant};
use tauri::State;

fn base_v1(s: &Settings) -> String {
    format!("{}/v1", s.base_url())
}

/// Best-effort primary LAN address (no packets are sent).
fn lan_ip() -> Option<std::net::IpAddr> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("10.255.255.255:1").ok()?;
    let ip = sock.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

#[tauri::command]
pub fn connect_info(state: State<'_, AppState>) -> ConnectInfo {
    let s = state.settings();
    let base = base_v1(&s);
    let lan_urls = if s.lan_mode {
        lan_ip().map(|ip| vec![format!("http://{ip}:{}/v1", s.port)]).unwrap_or_default()
    } else {
        Vec::new()
    };
    ConnectInfo {
        snippets: super::snippets::build(&base, &s.api_key, &s.alias),
        base_url: base,
        api_key: s.api_key.clone(),
        model_id: s.alias.clone(),
        lan_urls,
    }
}

/// Send a real chat completion and report the server's own words.
#[tauri::command]
pub async fn connect_test(state: State<'_, AppState>) -> CmdResult<TestResult> {
    let s = state.settings();
    let body = serde_json::json!({
        "model": s.alias,
        "messages": [{"role": "user", "content": "Reply with exactly: Neurallama is connected."}],
        "max_tokens": 24,
        "temperature": 0
    });
    let started = Instant::now();
    let mut req = state
        .http
        .post(format!("{}/chat/completions", base_v1(&s)))
        .timeout(Duration::from_secs(120))
        .json(&body);
    if !s.api_key.is_empty() {
        req = req.bearer_auth(&s.api_key);
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            let message = if e.is_connect() {
                format!("Nothing is listening at {}. Start the server first.", s.base_url())
            } else if e.is_timeout() {
                "The server didn't answer within 2 minutes (is the model still loading?).".into()
            } else {
                e.to_string()
            };
            return Ok(TestResult { ok: false, status: None, message, latency_ms: started.elapsed().as_millis() as u64 });
        }
    };
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    let latency_ms = started.elapsed().as_millis() as u64;
    if !status.is_success() {
        let message = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
            .unwrap_or(text);
        return Ok(TestResult { ok: false, status: Some(status.as_u16()), message, latency_ms });
    }
    let reply = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v["choices"][0]["message"]["content"].as_str().map(|s| s.trim().to_string()))
        .unwrap_or_else(|| "(empty reply)".into());
    Ok(TestResult { ok: true, status: Some(status.as_u16()), message: reply, latency_ms })
}

fn discover_ctx(s: &Settings) -> DiscoverCtx {
    DiscoverCtx { base_url_v1: base_v1(s), port: s.port, lan_mode: s.lan_mode }
}

#[tauri::command]
pub async fn hermes_targets(state: State<'_, AppState>) -> CmdResult<Vec<HermesTarget>> {
    let ctx = discover_ctx(&state.settings());
    tokio::task::spawn_blocking(move || hermes::discover(&ctx).into_iter().map(|f| f.target).collect())
        .await
        .map_err(|e| e.to_string())
}

/// Push this server into one Hermes profile. `context_length` defaults to the active
/// profile's per-slot context.
#[tauri::command]
pub async fn hermes_push(
    state: State<'_, AppState>,
    target_id: String,
    context_length: Option<u32>,
) -> CmdResult<HermesPushResult> {
    let s = state.settings();
    let ctx = discover_ctx(&s);
    let context_length =
        context_length.or_else(|| s.active().map(|p| p.ctx_size / p.parallel.max(1)));
    let api_key = s.api_key.clone();
    let model = s.alias.clone();
    tokio::task::spawn_blocking(move || {
        let found = hermes::discover(&ctx)
            .into_iter()
            .find(|f| f.target.id == target_id)
            .ok_or_else(|| format!("Hermes profile {target_id} was not found. Refresh and try again."))?;
        let spec = PushSpec { base_url_v1: found.target.base_url.clone(), api_key, model, context_length };
        Ok(hermes::push(&found, &spec))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Add or replace a "Neurallama" model entry in Continue's config.yaml
/// (`%USERPROFILE%\.continue\config.yaml`), with a timestamped backup.
#[tauri::command]
pub fn continue_push(state: State<'_, AppState>) -> CmdResult<ClientWriteResult> {
    let s = state.settings();
    let dir = dirs::home_dir().ok_or("No home folder")?.join(".continue");
    let path = dir.join("config.yaml");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let new_text = continue_apply(&existing, &base_v1(&s), &s.api_key, &s.alias)?;
    let backup = crate::settings::backup_file(&path).map_err(|e| format!("Could not back up Continue config: {e}"))?;
    crate::settings::write_atomic(&path, new_text.as_bytes())
        .map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    Ok(ClientWriteResult {
        ok: true,
        path: path.display().to_string(),
        backup_path: backup.map(|b| b.display().to_string()),
        message: "Continue now lists \"Neurallama (local)\". Reload VS Code to pick it up.".into(),
    })
}

const CONTINUE_NAME: &str = "Neurallama (local)";

pub fn continue_apply(existing: &str, base_v1: &str, api_key: &str, model: &str) -> Result<String, String> {
    let text = existing.trim_start_matches('\u{feff}');
    let mut root = if text.trim().is_empty() {
        let mut m = Mapping::new();
        m.insert("name".into(), "Local config".into());
        m.insert("version".into(), "1.0.0".into());
        m.insert("schema".into(), "v1".into());
        m
    } else {
        match serde_yaml::from_str::<Value>(text) {
            Ok(Value::Mapping(m)) => m,
            _ => return Err("Continue's config.yaml isn't valid YAML, so it was left unchanged.".into()),
        }
    };
    let mut entry = Mapping::new();
    entry.insert("name".into(), CONTINUE_NAME.into());
    entry.insert("provider".into(), "openai".into());
    entry.insert("model".into(), model.into());
    entry.insert("apiBase".into(), base_v1.into());
    entry.insert("apiKey".into(), api_key.into());
    entry.insert(
        "roles".into(),
        Value::Sequence(vec!["chat".into(), "edit".into(), "apply".into()]),
    );
    let models = root.entry("models".into()).or_insert_with(|| Value::Sequence(vec![]));
    if !models.is_sequence() {
        *models = Value::Sequence(vec![]);
    }
    let seq = models.as_sequence_mut().unwrap();
    seq.retain(|m| m.get("name").and_then(Value::as_str) != Some(CONTINUE_NAME));
    seq.push(Value::Mapping(entry));
    serde_yaml::to_string(&Value::Mapping(root)).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continue_replaces_own_entry_only() {
        let existing = "name: Mine\nversion: 1.0.0\nschema: v1\nmodels:\n  - name: GPT\n    provider: openai\n    model: gpt-4o\n";
        let once = continue_apply(existing, "http://127.0.0.1:8080/v1", "k", "local").unwrap();
        let twice = continue_apply(&once, "http://127.0.0.1:9090/v1", "k", "local").unwrap();
        let v: Value = serde_yaml::from_str(&twice).unwrap();
        let models = v["models"].as_sequence().unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0]["name"], Value::from("GPT"));
        assert_eq!(models[1]["apiBase"], Value::from("http://127.0.0.1:9090/v1"));
        assert_eq!(v["name"], Value::from("Mine"));
    }

    #[test]
    fn continue_creates_fresh_config() {
        let out = continue_apply("", "http://127.0.0.1:8080/v1", "k", "local").unwrap();
        let v: Value = serde_yaml::from_str(&out).unwrap();
        assert_eq!(v["schema"], Value::from("v1"));
        assert_eq!(v["models"][0]["provider"], Value::from("openai"));
    }
}
