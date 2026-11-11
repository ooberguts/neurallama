//! Download, verify and extract llama.cpp builds into `AI\llama.cpp\<tag>-<flavor>[-cu<ver>]\`.

use super::{releases, EngineAsset, InstalledEngine};
use crate::jobs::JobState;
use crate::models::download::{self, Outcome, Throttle};
use crate::settings::{Paths, Settings};
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

pub const MARKER: &str = "neurallama-engine.json";

#[cfg(windows)]
pub const SERVER_EXE: &str = "llama-server.exe";
#[cfg(not(windows))]
pub const SERVER_EXE: &str = "llama-server";

/// Written next to the binaries so the listing knows what was installed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Marker {
    pub tag: String,
    pub asset: String,
    pub flavor: String,
    pub cuda_version: Option<String>,
}

/// Folder name for an asset: `b12292-cuda-cu13.5`, `b12292-vulkan`, `b12292-cpu`, `b12292-sycl`.
pub fn dir_name(tag: &str, asset: &EngineAsset) -> String {
    match (asset.flavor.as_str(), &asset.cuda_version) {
        ("cuda", Some(v)) => format!("{tag}-cuda-cu{v}"),
        ("other", _) => format!("{tag}-{}", releases::asset_variant(&asset.name).unwrap_or_else(|| "other".into())),
        (f, _) => format!("{tag}-{f}"),
    }
}

/// Guess flavor + CUDA version from a folder name when there is no marker file.
pub fn flavor_from_dir(name: &str) -> (String, Option<String>) {
    let lower = name.to_lowercase();
    if let Some(i) = lower.find("-cuda") {
        let rest = lower[i + 5..].trim_start_matches('-').trim_start_matches("cu");
        let v: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        return ("cuda".into(), (!v.is_empty()).then_some(v));
    }
    for f in ["vulkan", "cpu"] {
        if lower.contains(&format!("-{f}")) {
            return (f.into(), None);
        }
    }
    ("other".into(), None)
}

pub fn read_marker(dir: &Path) -> Option<Marker> {
    serde_json::from_str(&std::fs::read_to_string(dir.join(MARKER)).ok()?).ok()
}

/// llama-server(.exe) at the folder root or up to two levels down.
pub fn find_server_exe(dir: &Path) -> Option<PathBuf> {
    fn go(dir: &Path, depth: u32) -> Option<PathBuf> {
        for name in [SERVER_EXE, "llama-server.exe", "llama-server"] {
            let p = dir.join(name);
            if p.is_file() {
                return Some(p);
            }
        }
        if depth == 0 {
            return None;
        }
        let mut subs: Vec<PathBuf> =
            std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        subs.sort();
        subs.into_iter().find_map(|d| go(&d, depth - 1))
    }
    go(dir, 2)
}

/// CUDA version of an installed engine folder, if it is a CUDA build.
pub fn engine_cuda_version(paths: &Paths, tag: &str) -> Option<String> {
    let dir = paths.engines.join(tag);
    match read_marker(&dir) {
        Some(m) => m.cuda_version,
        None => flavor_from_dir(tag).1,
    }
}

fn build_number(tag: &str) -> u64 {
    tag.trim_start_matches(['b', 'B']).chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap_or(0)
}

/// Installed builds (folders under `engines` containing llama-server), newest first. Blocking.
pub fn list_installed(paths: &Paths, settings: &Settings) -> Vec<InstalledEngine> {
    let Ok(rd) = std::fs::read_dir(&paths.engines) else { return vec![] };
    let mut out: Vec<InstalledEngine> = rd
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let tag = e.file_name().to_string_lossy().into_owned();
            if tag.starts_with('.') {
                return None;
            }
            let dir = e.path();
            let exe = find_server_exe(&dir)?;
            let flavor = read_marker(&dir).map(|m| m.flavor).unwrap_or_else(|| flavor_from_dir(&tag).0);
            Some(InstalledEngine {
                active: settings.active_engine.as_deref() == Some(tag.as_str()),
                flavor,
                dir: dir.to_string_lossy().into_owned(),
                server_exe: exe.to_string_lossy().into_owned(),
                tag,
            })
        })
        .collect();
    out.sort_by(|a, b| build_number(&b.tag).cmp(&build_number(&a.tag)).then_with(|| a.tag.cmp(&b.tag)));
    out
}

/// Extract a zip into `dest`, refusing entries that would escape it (zip-slip). Blocking.
pub fn extract_zip(zip_path: &Path, dest: &Path) -> Result<(), String> {
    let f = std::fs::File::open(zip_path).map_err(|e| format!("Could not open {}: {e}", zip_path.display()))?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(f))
        .map_err(|e| format!("{} is not a valid zip: {e}", zip_path.display()))?;
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| format!("Corrupt zip entry: {e}"))?;
        let Some(rel) = entry.enclosed_name() else {
            return Err(format!("Refusing unsafe path in zip: {}", entry.name()));
        };
        let out = dest.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut w = std::fs::File::create(&out).map_err(|e| format!("Could not write {}: {e}", out.display()))?;
        std::io::copy(&mut entry, &mut w).map_err(|e| format!("Could not extract {}: {e}", out.display()))?;
    }
    Ok(())
}

/// Start installing `asset_name` from release `tag`. Returns the job id (kind "engine").
pub async fn install(app: &AppHandle, tag: &str, asset_name: &str) -> Result<String, String> {
    let state = app.state::<AppState>();
    let rel = releases::release_by_tag(&state.http, tag).await?;
    let asset = releases::classify(&rel.assets)
        .into_iter()
        .find(|a| a.name == asset_name)
        .ok_or_else(|| format!("{asset_name} is not a Windows x64 build in release {tag}."))?;
    let folder = dir_name(tag, &asset);
    let final_dir = state.paths.engines.join(&folder);
    if find_server_exe(&final_dir).is_some() {
        return Err(format!("{folder} is already installed."));
    }
    let mut files = vec![(asset.url.clone(), asset.name.clone(), asset.size)];
    if let (Some(u), Some(s)) = (&asset.cudart_url, asset.cudart_size) {
        let name = u.rsplit('/').next().unwrap_or("cudart.zip").to_string();
        files.push((u.clone(), name, s));
    }
    let total: u64 = files.iter().map(|f| f.2).sum();
    // Zips + extracted files (~2x) must fit.
    download::check_space(&state.paths.engines, total.saturating_mul(3)).await?;

    let label = match &asset.cuda_version {
        Some(v) => format!("llama.cpp {tag} (CUDA {v})"),
        None => format!("llama.cpp {tag} ({})", asset.flavor),
    };
    let (id, handle) = state.jobs.create("engine", &label);
    download::set_job(app, &id, |p| {
        p.total = Some(total);
        p.state = JobState::Running;
    });
    let marker = Marker {
        tag: tag.into(),
        asset: asset.name.clone(),
        flavor: asset.flavor.clone(),
        cuda_version: asset.cuda_version.clone(),
    };
    let app2 = app.clone();
    let id2 = id.clone();
    tauri::async_runtime::spawn(async move {
        let state = app2.state::<AppState>();
        let tmp = state.paths.engines.join(format!(".tmp-{id2}"));
        let result = run(&app2, &id2, &handle, &tmp, &files, &final_dir, &folder, &marker).await;
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        match result {
            Ok(Outcome::Done) => {
                let mut msg = format!("Installed to {}", final_dir.display());
                if state.settings().active_engine.is_none()
                    && state.update_settings(|s| s.active_engine = Some(folder.clone())).is_ok()
                {
                    msg.push_str(" and set as the active engine");
                }
                download::set_job(&app2, &id2, |p| {
                    p.state = JobState::Done;
                    p.done = p.total.unwrap_or(p.done);
                    p.message = Some(msg);
                });
            }
            Ok(_) => download::set_job(&app2, &id2, |p| {
                p.state = JobState::Cancelled;
                p.message = Some("Install cancelled.".into());
            }),
            Err(e) => download::set_job(&app2, &id2, |p| {
                p.state = JobState::Error;
                p.message = Some(e);
            }),
        }
    });
    Ok(id)
}

#[allow(clippy::too_many_arguments)]
async fn run(
    app: &AppHandle,
    id: &str,
    handle: &crate::jobs::JobHandle,
    tmp: &Path,
    files: &[(String, String, u64)],
    final_dir: &Path,
    folder: &str,
    marker: &Marker,
) -> Result<Outcome, String> {
    let state = app.state::<AppState>();
    tokio::fs::create_dir_all(tmp).await.map_err(|e| format!("Could not create {}: {e}", tmp.display()))?;
    let mut done_before = 0u64;
    let mut zips = Vec::new();
    for (url, name, size) in files {
        let dest = tmp.join(name);
        let part = download::part_path(&dest);
        loop {
            if handle.paused() {
                download::set_job(app, id, |p| p.state = JobState::Paused);
                if !download::wait_unpaused(handle).await {
                    return Ok(Outcome::Cancelled);
                }
            }
            if handle.cancelled() {
                return Ok(Outcome::Cancelled);
            }
            download::set_job(app, id, |p| {
                p.state = JobState::Running;
                p.message = Some(format!("Downloading {name}"));
            });
            let mut throttle = Throttle::new();
            let (app2, id2, base) = (app.clone(), id.to_string(), done_before);
            let mut cb = move |b: u64| {
                if throttle.ready() {
                    download::set_job(&app2, &id2, |p| p.done = base + b);
                }
            };
            match download::fetch_resumable(&state.http, url, None, &part, Some(*size), handle, &mut cb).await? {
                Outcome::Done => break,
                Outcome::Paused => continue,
                Outcome::Cancelled => return Ok(Outcome::Cancelled),
            }
        }
        download::finalize(&part, &dest, Some(*size)).await?;
        done_before += size;
        zips.push(dest);
    }

    download::set_job(app, id, |p| {
        p.done = done_before;
        p.message = Some("Extracting…".into());
    });
    let staging = tmp.join("out");
    let (staging2, final2, marker2) = (staging.clone(), final_dir.to_path_buf(), marker.clone());
    let folder = folder.to_string();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        for z in &zips {
            extract_zip(z, &staging2)?;
        }
        if find_server_exe(&staging2).is_none() {
            return Err(format!("The downloaded build has no {SERVER_EXE}; it was not installed."));
        }
        let text = serde_json::to_string_pretty(&marker2).map_err(|e| e.to_string())?;
        std::fs::write(staging2.join(MARKER), text).map_err(|e| e.to_string())?;
        if final2.exists() {
            std::fs::remove_dir_all(&final2).map_err(|e| format!("Could not replace {folder}: {e}"))?;
        }
        std::fs::rename(&staging2, &final2).map_err(|e| format!("Could not move {folder} into place: {e}"))
    })
    .await
    .map_err(|e| format!("Extraction failed: {e}"))??;
    Ok(Outcome::Done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn asset(name: &str, flavor: &str, cuda: Option<&str>) -> EngineAsset {
        EngineAsset {
            name: name.into(),
            url: String::new(),
            size: 0,
            flavor: flavor.into(),
            cuda_version: cuda.map(Into::into),
            cudart_url: None,
            cudart_size: None,
        }
    }

    #[test]
    fn names() {
        assert_eq!(dir_name("b12292", &asset("llama-b12292-bin-win-cuda-13.5-x64.zip", "cuda", Some("13.5"))), "b12292-cuda-cu13.5");
        assert_eq!(dir_name("b12292", &asset("llama-b12292-bin-win-vulkan-x64.zip", "vulkan", None)), "b12292-vulkan");
        assert_eq!(dir_name("b12292", &asset("llama-b12292-bin-win-sycl-x64.zip", "other", None)), "b12292-sycl");
        assert_eq!(flavor_from_dir("b12292-cuda-cu13.5"), ("cuda".into(), Some("13.5".into())));
        assert_eq!(flavor_from_dir("b6500-cuda-12.8"), ("cuda".into(), Some("12.8".into())));
        assert_eq!(flavor_from_dir("b1-vulkan").0, "vulkan");
        assert_eq!(flavor_from_dir("b1-rocm-10.0").0, "other");
    }

    #[test]
    fn extracts_and_blocks_zip_slip() {
        let dir = crate::models::gguf::tests::tmp_dir("zip");
        let good = dir.join("good.zip");
        {
            let mut z = zip::ZipWriter::new(std::fs::File::create(&good).unwrap());
            let opts = zip::write::SimpleFileOptions::default();
            z.start_file(SERVER_EXE, opts).unwrap();
            z.write_all(b"exe").unwrap();
            z.start_file("sub/ggml-cuda.dll", opts).unwrap();
            z.write_all(b"dll").unwrap();
            z.finish().unwrap();
        }
        let out = dir.join("out");
        extract_zip(&good, &out).unwrap();
        assert!(find_server_exe(&out).is_some());
        assert_eq!(std::fs::read(out.join("sub/ggml-cuda.dll")).unwrap(), b"dll");

        let bad = dir.join("bad.zip");
        {
            let mut z = zip::ZipWriter::new(std::fs::File::create(&bad).unwrap());
            z.start_file("../evil.txt", zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(b"x").unwrap();
            z.finish().unwrap();
        }
        assert!(extract_zip(&bad, &dir.join("out2")).is_err());
        assert!(!dir.join("evil.txt").exists());

        let paths = Paths::from_root(dir.clone());
        std::fs::create_dir_all(&paths.engines).unwrap();
        std::fs::rename(&out, paths.engines.join("b12292-cuda-cu13.5")).unwrap();
        let settings = Settings { active_engine: Some("b12292-cuda-cu13.5".into()), ..Settings::default() };
        let list = list_installed(&paths, &settings);
        assert_eq!(list.len(), 1);
        assert!(list[0].active);
        assert_eq!(list[0].flavor, "cuda");
        assert_eq!(engine_cuda_version(&paths, "b12292-cuda-cu13.5").as_deref(), Some("13.5"));
        std::fs::remove_dir_all(dir).ok();
    }
}
