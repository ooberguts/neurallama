//! GitHub releases of ggml-org/llama.cpp and classification of the Windows x64 assets.
//!
//! Real asset names (b12292, Nov 2026):
//! `llama-b12292-bin-win-cuda-13.5-x64.zip` + `cudart-llama-bin-win-cuda-13.5-x64.zip`,
//! `llama-b12292-bin-win-vulkan-x64.zip`, `llama-b12292-bin-win-cpu-x64.zip`,
//! `llama-b12292-bin-win-rocm-10.0-x64.zip`, `llama-b12292-bin-win-sycl-x64.zip`, ...
//! Every `b*` build is published as a GitHub prerelease. Stable releases are tagged `v*`
//! and only carry `nightly-tag.txt`, naming the `b*` build they promote.

use super::{EngineAsset, Release};
use parking_lot::Mutex;
use regex::Regex;
use serde::Deserialize;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

pub const API: &str = "https://api.github.com/repos/ggml-org/llama.cpp";
const CACHE_TTL: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Deserialize)]
pub struct GhRelease {
    pub tag_name: String,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub html_url: String,
    #[serde(default)]
    pub assets: Vec<GhAsset>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhAsset {
    pub name: String,
    pub size: u64,
    pub browser_download_url: String,
}

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).unwrap())
}

/// Classify the Windows x64 assets of one release. Sorted: CUDA (newest first), Vulkan, CPU, other.
pub fn classify(assets: &[GhAsset]) -> Vec<EngineAsset> {
    static WIN: OnceLock<Regex> = OnceLock::new();
    static CUDART: OnceLock<Regex> = OnceLock::new();
    let win = re(&WIN, r"(?i)^llama-[^-]+-bin-win-(?P<variant>.+)-x64\.zip$");
    let cudart = re(&CUDART, r"(?i)^cudart-llama-(?:[^-]+-)?bin-win-(?:cuda-?)?(?:cu)?(?P<ver>\d+(?:\.\d+)*)-x64\.zip$");

    let mut out = Vec::new();
    for a in assets {
        let Some(c) = win.captures(&a.name) else { continue };
        let variant = c["variant"].to_lowercase();
        let (flavor, cuda_version) = if let Some(v) = variant.strip_prefix("cuda-") {
            ("cuda", Some(v.trim_start_matches("cu").to_string()))
        } else if variant == "vulkan" {
            ("vulkan", None)
        } else if variant == "cpu" || variant == "avx2" {
            ("cpu", None)
        } else {
            ("other", None)
        };
        let rt = cuda_version.as_ref().and_then(|v| {
            assets.iter().find(|r| cudart.captures(&r.name).is_some_and(|rc| &rc["ver"] == v.as_str()))
        });
        out.push(EngineAsset {
            name: a.name.clone(),
            url: a.browser_download_url.clone(),
            size: a.size,
            flavor: flavor.into(),
            cuda_version,
            cudart_url: rt.map(|r| r.browser_download_url.clone()),
            cudart_size: rt.map(|r| r.size),
        });
    }
    let rank = |f: &str| match f {
        "cuda" => 0,
        "vulkan" => 1,
        "cpu" => 2,
        _ => 3,
    };
    out.sort_by(|a, b| {
        rank(&a.flavor)
            .cmp(&rank(&b.flavor))
            .then_with(|| version_cmp(b.cuda_version.as_deref(), a.cuda_version.as_deref()))
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

/// The variant part of a Windows asset name, e.g. "cuda-13.5", "vulkan", "sycl", "rocm-10.0".
pub fn asset_variant(name: &str) -> Option<String> {
    static WIN: OnceLock<Regex> = OnceLock::new();
    let win = re(&WIN, r"(?i)^llama-[^-]+-bin-win-(?P<variant>.+)-x64\.zip$");
    win.captures(name).map(|c| c["variant"].to_lowercase())
}

/// Compare dotted versions ("12.4" < "12.10"). None sorts first.
pub fn version_cmp(a: Option<&str>, b: Option<&str>) -> std::cmp::Ordering {
    let parse = |s: &str| s.split('.').map(|p| p.parse::<u32>().unwrap_or(0)).collect::<Vec<_>>();
    match (a, b) {
        (Some(a), Some(b)) => parse(a).cmp(&parse(b)),
        (a, b) => a.is_some().cmp(&b.is_some()),
    }
}

pub fn to_release(r: &GhRelease) -> Release {
    Release {
        tag: r.tag_name.clone(),
        published_at: r.published_at.clone().unwrap_or_default(),
        prerelease: r.prerelease,
        html_url: r.html_url.clone(),
        assets: classify(&r.assets),
        stable_tag: None,
    }
}

fn gh_error(status: reqwest::StatusCode) -> String {
    match status.as_u16() {
        403 | 429 => "GitHub rate limit reached (60 requests/hour without sign-in). Try again later.".into(),
        404 => "That llama.cpp release was not found on GitHub.".into(),
        s => format!("GitHub returned HTTP {s}."),
    }
}

async fn get_json<T: serde::de::DeserializeOwned>(http: &reqwest::Client, url: &str) -> Result<T, String> {
    let resp = http
        .get(url)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("Could not reach GitHub: {e}"))?;
    if !resp.status().is_success() {
        return Err(gh_error(resp.status()));
    }
    resp.json().await.map_err(|e| format!("Unexpected GitHub response: {e}"))
}

pub async fn release_by_tag(http: &reqwest::Client, tag: &str) -> Result<GhRelease, String> {
    if !tag.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) {
        return Err(format!("Invalid release tag {tag}"));
    }
    get_json(http, &format!("{API}/releases/tags/{tag}")).await
}

/// A `v*` stable release with no binaries: follow nightly-tag.txt to the build it promotes.
async fn resolve_stable(http: &reqwest::Client, r: &GhRelease) -> Option<Release> {
    let marker = r.assets.iter().find(|a| a.name == "nightly-tag.txt")?;
    if marker.size > 256 {
        return None;
    }
    let text = http.get(&marker.browser_download_url).send().await.ok()?.text().await.ok()?;
    let build = release_by_tag(http, text.trim()).await.ok()?;
    let mut rel = to_release(&build);
    if rel.assets.is_empty() {
        return None;
    }
    rel.stable_tag = Some(r.tag_name.clone());
    rel.prerelease = false;
    rel.html_url = r.html_url.clone();
    if let Some(p) = &r.published_at {
        rel.published_at = p.clone();
    }
    Some(rel)
}

type Cache = Mutex<Option<(Instant, String, Vec<Release>)>>;

fn cache() -> &'static Cache {
    static C: OnceLock<Cache> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// Releases for the channel ("stable" | "prerelease"), newest first; cached for 10 minutes.
pub async fn list(http: &reqwest::Client, channel: &str) -> Result<Vec<Release>, String> {
    if let Some((at, ch, v)) = cache().lock().as_ref() {
        if ch == channel && at.elapsed() < CACHE_TTL {
            return Ok(v.clone());
        }
    }
    let all: Vec<GhRelease> = get_json(http, &format!("{API}/releases?per_page=15")).await?;
    let mut out = Vec::new();
    let mut stables: Vec<GhRelease> = all.iter().filter(|r| !r.prerelease && !r.draft).cloned().collect();
    if stables.is_empty() {
        if let Ok(latest) = get_json::<GhRelease>(http, &format!("{API}/releases/latest")).await {
            stables.push(latest);
        }
    }
    for r in stables.iter().take(2) {
        let rel = to_release(r);
        if !rel.assets.is_empty() {
            out.push(rel);
        } else if let Some(rel) = resolve_stable(http, r).await {
            out.push(rel);
        }
    }
    if channel == "prerelease" {
        for r in all.iter().filter(|r| r.prerelease && !r.draft) {
            let rel = to_release(r);
            if !rel.assets.is_empty() && !out.iter().any(|o| o.tag == rel.tag) {
                out.push(rel);
            }
        }
        out.sort_by(|a, b| b.published_at.cmp(&a.published_at));
    }
    *cache().lock() = Some((Instant::now(), channel.to_string(), out.clone()));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str, size: u64) -> GhAsset {
        GhAsset { name: name.into(), size, browser_download_url: format!("https://github.com/dl/{name}") }
    }

    #[test]
    fn classifies_real_assets() {
        // Windows + a sample of other assets from release b12292.
        let names = [
            ("cudart-llama-b12292-bin-ubuntu-cuda-12.8-x64.tar.gz", 594371462),
            ("cudart-llama-bin-win-cuda-12.4-x64.zip", 391443627),
            ("cudart-llama-bin-win-cuda-13.5-arm64.zip", 278821062),
            ("cudart-llama-bin-win-cuda-13.5-x64.zip", 426314199),
            ("llama-b12292-bin-macos-arm64.tar.gz", 16870580),
            ("llama-b12292-bin-ubuntu-cuda-12.8-x64.tar.gz", 197931082),
            ("llama-b12292-bin-win-cpu-arm64.zip", 13408732),
            ("llama-b12292-bin-win-cpu-x64.zip", 25057882),
            ("llama-b12292-bin-win-cuda-12.4-x64.zip", 317119579),
            ("llama-b12292-bin-win-cuda-13.5-arm64.zip", 165610684),
            ("llama-b12292-bin-win-cuda-13.5-x64.zip", 178009744),
            ("llama-b12292-bin-win-opencl-adreno-arm64.zip", 14419693),
            ("llama-b12292-bin-win-openvino-2026.4.1-x64.zip", 105054722),
            ("llama-b12292-bin-win-rocm-10.0-x64.zip", 280016185),
            ("llama-b12292-bin-win-sycl-x64.zip", 160179218),
            ("llama-b12292-bin-win-vulkan-arm64.zip", 25954390),
            ("llama-b12292-bin-win-vulkan-x64.zip", 38023711),
            ("llama-b12292-ui.tar.gz", 3377209),
            ("llama-b12292-xcframework.zip", 65452134),
            // Older naming (cu prefix) still classifies.
            ("llama-b5000-bin-win-cuda-cu12.4-x64.zip", 1),
        ];
        let assets: Vec<GhAsset> = names.iter().map(|(n, s)| asset(n, *s)).collect();
        let c = classify(&assets);
        let summary: Vec<(&str, &str, Option<&str>)> =
            c.iter().map(|a| (a.name.as_str(), a.flavor.as_str(), a.cuda_version.as_deref())).collect();
        assert_eq!(
            summary,
            vec![
                ("llama-b12292-bin-win-cuda-13.5-x64.zip", "cuda", Some("13.5")),
                ("llama-b12292-bin-win-cuda-12.4-x64.zip", "cuda", Some("12.4")),
                ("llama-b5000-bin-win-cuda-cu12.4-x64.zip", "cuda", Some("12.4")),
                ("llama-b12292-bin-win-vulkan-x64.zip", "vulkan", None),
                ("llama-b12292-bin-win-cpu-x64.zip", "cpu", None),
                ("llama-b12292-bin-win-openvino-2026.4.1-x64.zip", "other", None),
                ("llama-b12292-bin-win-rocm-10.0-x64.zip", "other", None),
                ("llama-b12292-bin-win-sycl-x64.zip", "other", None),
            ]
        );
        assert_eq!(c[0].cudart_size, Some(426314199));
        assert!(c[0].cudart_url.as_deref().unwrap().ends_with("cudart-llama-bin-win-cuda-13.5-x64.zip"));
        assert_eq!(c[1].cudart_size, Some(391443627));
        assert_eq!(c[3].cudart_url, None);
        assert_eq!(asset_variant("llama-b12292-bin-win-rocm-10.0-x64.zip").as_deref(), Some("rocm-10.0"));
    }

    #[test]
    fn versions() {
        use std::cmp::Ordering::*;
        assert_eq!(version_cmp(Some("12.4"), Some("12.10")), Less);
        assert_eq!(version_cmp(Some("13.5"), Some("12.8")), Greater);
        assert_eq!(version_cmp(None, Some("1")), Less);
    }

    #[test]
    fn parses_release_json() {
        let json = r#"[{"tag_name":"v0.9.0","prerelease":false,"draft":false,"published_at":"2026-11-04T13:08:06Z",
            "html_url":"https://github.com/ggml-org/llama.cpp/releases/tag/v0.9.0",
            "assets":[{"name":"nightly-tag.txt","size":7,"browser_download_url":"https://x/nightly-tag.txt"}]}]"#;
        let v: Vec<GhRelease> = serde_json::from_str(json).unwrap();
        assert!(to_release(&v[0]).assets.is_empty());
    }
}
