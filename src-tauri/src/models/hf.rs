//! Hugging Face Hub API: search GGUF repos, list repo files, optional token in the OS keyring.

use super::{gguf, scan, HfFile, HfModel};
use serde::Deserialize;

pub const API: &str = "https://huggingface.co";
const KEYRING_SERVICE: &str = "neurallama";
const KEYRING_USER: &str = "huggingface";

fn entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).map_err(|e| format!("Credential store unavailable: {e}"))
}

/// The stored token, if any. Blocking (OS credential store); call from spawn_blocking.
pub fn token_get() -> Option<String> {
    entry().ok()?.get_password().ok().filter(|t| !t.trim().is_empty())
}

pub fn token_set(token: Option<String>) -> Result<(), String> {
    let e = entry()?;
    match token.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
        Some(t) => e.set_password(&t).map_err(|e| format!("Could not save the token: {e}")),
        None => match e.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(format!("Could not delete the token: {err}")),
        },
    }
}

pub async fn token() -> Option<String> {
    tokio::task::spawn_blocking(token_get).await.ok().flatten()
}

/// Attach the bearer token when one is stored. reqwest drops it on cross-host redirects.
pub fn with_auth(req: reqwest::RequestBuilder, token: Option<&str>) -> reqwest::RequestBuilder {
    match token {
        Some(t) => req.bearer_auth(t),
        None => req,
    }
}

pub fn status_error(status: reqwest::StatusCode, what: &str) -> String {
    match status.as_u16() {
        401 | 403 => "Gated or private repo — add a Hugging Face token in Settings (and accept the model's terms on huggingface.co).".into(),
        404 => format!("{what} not found on Hugging Face."),
        429 => "Hugging Face rate limit reached. Wait a minute or add a token in Settings.".into(),
        s => format!("Hugging Face returned HTTP {s} for {what}."),
    }
}

/// Repo ids look like "org/name"; reject anything that could escape the models folder.
pub fn validate_repo_id(repo: &str) -> Result<(), String> {
    let ok = repo.split('/').count() == 2
        && repo.split('/').all(|p| {
            !p.is_empty() && p != "." && p != ".." && p.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        });
    if ok {
        Ok(())
    } else {
        Err(format!("\"{repo}\" is not a valid Hugging Face repo id (expected org/name)."))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiModel {
    id: String,
    #[serde(default)]
    downloads: u64,
    #[serde(default)]
    likes: u64,
    last_modified: Option<String>,
    /// false | "auto" | "manual"
    #[serde(default)]
    gated: serde_json::Value,
}

#[derive(Deserialize)]
struct ApiTreeEntry {
    #[serde(rename = "type")]
    kind: String,
    path: String,
    #[serde(default)]
    size: u64,
    lfs: Option<ApiLfs>,
}

#[derive(Deserialize)]
struct ApiLfs {
    size: u64,
}

pub async fn search(http: &reqwest::Client, query: &str) -> Result<Vec<HfModel>, String> {
    let token = token().await;
    let req = http.get(format!("{API}/api/models")).query(&[
        ("search", query),
        ("filter", "gguf"),
        ("sort", "downloads"),
        ("direction", "-1"),
        ("limit", "30"),
        ("expand[]", "downloads"),
        ("expand[]", "likes"),
        ("expand[]", "lastModified"),
        ("expand[]", "gated"),
    ]);
    let resp = with_auth(req, token.as_deref())
        .send()
        .await
        .map_err(|e| format!("Could not reach Hugging Face: {e}"))?;
    if !resp.status().is_success() {
        return Err(status_error(resp.status(), "search"));
    }
    let items: Vec<ApiModel> = resp.json().await.map_err(|e| format!("Unexpected search response: {e}"))?;
    Ok(items
        .into_iter()
        .map(|m| HfModel {
            gated: !matches!(m.gated, serde_json::Value::Bool(false) | serde_json::Value::Null),
            id: m.id,
            downloads: m.downloads,
            likes: m.likes,
            last_modified: m.last_modified,
        })
        .collect())
}

pub async fn files(http: &reqwest::Client, repo_id: &str) -> Result<Vec<HfFile>, String> {
    validate_repo_id(repo_id)?;
    let token = token().await;
    let req = http.get(format!("{API}/api/models/{repo_id}/tree/main")).query(&[("recursive", "true")]);
    let resp = with_auth(req, token.as_deref())
        .send()
        .await
        .map_err(|e| format!("Could not reach Hugging Face: {e}"))?;
    if !resp.status().is_success() {
        return Err(status_error(resp.status(), repo_id));
    }
    let entries: Vec<ApiTreeEntry> = resp.json().await.map_err(|e| format!("Unexpected file list: {e}"))?;
    Ok(tree_to_files(entries))
}

fn tree_to_files(entries: Vec<ApiTreeEntry>) -> Vec<HfFile> {
    let mut out: Vec<HfFile> = entries
        .into_iter()
        .filter(|e| e.kind == "file" && e.path.to_lowercase().ends_with(".gguf"))
        .map(|e| {
            let file_name = e.path.rsplit('/').next().unwrap_or(&e.path).to_string();
            let dir = e.path.rsplit_once('/').map(|(d, _)| d.to_string());
            let shard_group = gguf::split_parts(&file_name).map(|(prefix, _, total)| match &dir {
                Some(d) => format!("{d}/{prefix}-of-{total:05}"),
                None => format!("{prefix}-of-{total:05}"),
            });
            HfFile {
                size: e.lfs.map(|l| l.size).unwrap_or(e.size),
                quant: scan::quant_from_name(&file_name)
                    .or_else(|| dir.as_deref().and_then(|d| scan::quant_from_name(d.rsplit('/').next().unwrap_or(d)))),
                is_mmproj: scan::is_mmproj(&file_name),
                shard_group,
                path: e.path,
            }
        })
        .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tree_shape() {
        // Shape captured from /api/models/unsloth/Qwen3.6-35B-A3B-GGUF/tree/main?recursive=true
        let json = r#"[
          {"type":"directory","oid":"b7","size":0,"path":"BF16"},
          {"type":"file","oid":"71","size":3502,"path":".gitattributes"},
          {"type":"file","oid":"a1","size":49913715424,"path":"BF16/Qwen3.6-35B-A3B-BF16-00001-of-00002.gguf","lfs":{"oid":"x","size":49913715424,"pointerSize":135}},
          {"type":"file","oid":"a2","size":19462922752,"path":"BF16/Qwen3.6-35B-A3B-BF16-00002-of-00002.gguf","lfs":{"oid":"y","size":19462922752,"pointerSize":135}},
          {"type":"file","oid":"a3","size":22134528992,"path":"Qwen3.6-35B-A3B-UD-Q4_K_M.gguf"},
          {"type":"file","oid":"a4","size":899283680,"path":"mmproj-F16.gguf"},
          {"type":"file","oid":"a5","size":192223904,"path":"imatrix_unsloth.gguf_file"}
        ]"#;
        let entries: Vec<ApiTreeEntry> = serde_json::from_str(json).unwrap();
        let files = tree_to_files(entries);
        assert_eq!(files.len(), 4);
        assert_eq!(files[0].shard_group, files[1].shard_group);
        assert_eq!(files[0].shard_group.as_deref(), Some("BF16/Qwen3.6-35B-A3B-BF16-of-00002"));
        assert_eq!(files[0].quant.as_deref(), Some("BF16"));
        assert_eq!(files[2].quant.as_deref(), Some("UD-Q4_K_M"));
        assert!(files[3].is_mmproj);
        assert_eq!(files[2].size, 22134528992);
    }

    #[test]
    fn parses_search_shape() {
        let json = r#"[{"_id":"6a","id":"unsloth/Qwen3.6-35B-A3B-MTP-GGUF","gated":false,"lastModified":"2026-05-20T08:48:17.000Z","likes":1049,"downloads":2250643},
                       {"_id":"6b","id":"meta/x","gated":"manual","likes":1,"downloads":2}]"#;
        let items: Vec<ApiModel> = serde_json::from_str(json).unwrap();
        assert_eq!(items.len(), 2);
        assert!(matches!(items[0].gated, serde_json::Value::Bool(false)));
        assert!(items[1].gated.is_string());
    }

    #[test]
    fn repo_ids() {
        assert!(validate_repo_id("unsloth/Qwen3.6-35B-A3B-GGUF").is_ok());
        assert!(validate_repo_id("../etc").is_err());
        assert!(validate_repo_id("a/b/c").is_err());
        assert!(validate_repo_id("a/..").is_err());
    }
}
