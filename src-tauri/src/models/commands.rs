//! #[tauri::command]s for the Models tab.

use super::{download, gguf, hf, scan, GgufSummary, HfFile, HfModel, LocalModel};
use crate::jobs::JobProgress;
use crate::server::{ServerState, ServerStatus};
use crate::settings::Profile;
use crate::state::{AppState, CmdResult};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, State};

const DEFAULT_MAX_CTX: u32 = 32768;

async fn scan_models(state: &AppState, with_meta: bool) -> CmdResult<Vec<LocalModel>> {
    let paths = state.paths.clone();
    tokio::task::spawn_blocking(move || scan::scan(&paths, with_meta))
        .await
        .map_err(|e| format!("Model scan failed: {e}"))
}

#[tauri::command]
pub async fn models_scan(state: State<'_, AppState>, with_meta: bool) -> CmdResult<Vec<LocalModel>> {
    scan_models(&state, with_meta).await
}

#[tauri::command]
pub async fn model_meta(path: String) -> CmdResult<GgufSummary> {
    tokio::task::spawn_blocking(move || gguf::read_summary(Path::new(&path)))
        .await
        .map_err(|e| format!("Could not read model metadata: {e}"))?
}

fn same_path(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        let s = s.replace('\\', "/");
        if cfg!(windows) {
            s.to_lowercase()
        } else {
            s
        }
    };
    !a.is_empty() && norm(a) == norm(b)
}

fn server_busy(state: &AppState) -> Option<ServerStatus> {
    let st = state.server.status();
    matches!(st.state, ServerState::Starting | ServerState::Loading | ServerState::Ready).then_some(st)
}

#[tauri::command]
pub async fn model_delete(state: State<'_, AppState>, id: String) -> CmdResult<Vec<LocalModel>> {
    let models = scan_models(&state, false).await?;
    let model = models.iter().find(|m| m.id == id).ok_or("That model is no longer on disk.")?.clone();
    if let Some(st) = server_busy(&state) {
        if st.model_path.as_deref().is_some_and(|p| model.files.iter().any(|f| same_path(f, p))) {
            return Err("This model is being served. Stop the server before deleting it.".into());
        }
    }
    let mut targets: Vec<PathBuf> = model.files.iter().map(PathBuf::from).collect();
    if let Some(mm) = &model.mmproj_path {
        // Keep a shared projector that another model in the folder still uses.
        let shared = models.iter().any(|m| m.id != model.id && m.mmproj_path.as_deref() == Some(mm.as_str()));
        if !shared {
            targets.push(PathBuf::from(mm));
        }
    }
    let models_dir = state.paths.models.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        for t in &targets {
            if !t.starts_with(&models_dir) {
                return Err(format!("Refusing to delete {} (outside the models folder).", t.display()));
            }
        }
        for t in &targets {
            match std::fs::remove_file(t) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("Could not delete {}: {e}", t.display())),
            }
            let _ = std::fs::remove_file(download::part_path(t));
        }
        for t in &targets {
            remove_empty_parents(t, &models_dir);
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("Delete failed: {e}"))??;
    scan_models(&state, false).await
}

/// Remove empty folders from `file`'s parent up to (not including) `stop`.
fn remove_empty_parents(file: &Path, stop: &Path) {
    let mut dir = file.parent();
    while let Some(d) = dir {
        if d == stop || !d.starts_with(stop) {
            break;
        }
        if std::fs::remove_dir(d).is_err() {
            break; // not empty (or in use)
        }
        dir = d.parent();
    }
}

#[tauri::command]
pub async fn model_serve(app: AppHandle, state: State<'_, AppState>, id: String) -> CmdResult<ServerStatus> {
    let models = scan_models(&state, false).await?;
    let model = models.into_iter().find(|m| m.id == id).ok_or("That model is no longer on disk.")?;
    let existing = state.settings().profiles.iter().find(|p| same_path(&p.model_path, &model.primary_path)).cloned();
    let profile = match existing {
        Some(p) => p,
        None => {
            let path = model.primary_path.clone();
            let meta = tokio::task::spawn_blocking(move || gguf::read_summary(Path::new(&path)).ok())
                .await
                .ok()
                .flatten();
            new_profile_for(&model, meta.as_ref())
        }
    };
    let pid = profile.id.clone();
    let settings = state.update_settings(|s| {
        match s.profiles.iter_mut().find(|p| p.id == profile.id) {
            Some(p) => *p = profile.clone(),
            None => s.profiles.push(profile.clone()),
        }
        s.active_profile = Some(pid);
    })?;
    state.server.stop(&app).await?;
    state.server.start(&app, &profile, &settings).await
}

fn new_profile_for(model: &LocalModel, meta: Option<&GgufSummary>) -> Profile {
    let ctx = meta
        .and_then(|m| m.context_length)
        .filter(|&c| c > 0)
        .map(|c| c.min(DEFAULT_MAX_CTX))
        .unwrap_or(DEFAULT_MAX_CTX);
    let name = match &model.quant {
        Some(q) if !model.name.to_uppercase().contains(&q.to_uppercase()) => format!("{} {q}", model.name),
        _ => model.name.clone(),
    };
    Profile {
        name,
        model_path: model.primary_path.clone(),
        mmproj_path: model.mmproj_path.clone(),
        ctx_size: ctx,
        n_cpu_moe: None,
        ..Profile::default()
    }
}

#[tauri::command]
pub async fn hf_search(state: State<'_, AppState>, query: String) -> CmdResult<Vec<HfModel>> {
    hf::search(&state.http, query.trim()).await
}

#[tauri::command]
pub async fn hf_files(state: State<'_, AppState>, repo_id: String) -> CmdResult<Vec<HfFile>> {
    hf::files(&state.http, repo_id.trim()).await
}

#[tauri::command]
pub async fn download_start(app: AppHandle, repo_id: String, files: Vec<String>) -> CmdResult<String> {
    download::start(&app, repo_id.trim(), files).await
}

#[tauri::command]
pub fn download_pause(app: AppHandle, job_id: String) -> CmdResult<()> {
    download::pause(&app, &job_id)
}

#[tauri::command]
pub fn download_resume(app: AppHandle, job_id: String) -> CmdResult<()> {
    download::resume(&app, &job_id)
}

/// Cancels any job id (downloads delete their .part files; engine installs clean up too).
#[tauri::command]
pub async fn download_cancel(app: AppHandle, job_id: String) -> CmdResult<()> {
    download::cancel(&app, &job_id).await
}

#[tauri::command]
pub fn downloads_list(state: State<'_, AppState>) -> Vec<JobProgress> {
    let mut v = state.jobs.list(Some("download"));
    v.sort_by(|a, b| a.label.cmp(&b.label));
    v
}

#[tauri::command]
pub async fn hf_token_set(token: Option<String>) -> CmdResult<()> {
    tokio::task::spawn_blocking(move || hf::token_set(token))
        .await
        .map_err(|e| format!("Credential store error: {e}"))?
}

#[tauri::command]
pub async fn hf_token_status() -> CmdResult<bool> {
    Ok(hf::token().await.is_some())
}

#[tauri::command]
pub async fn disk_free(state: State<'_, AppState>) -> CmdResult<u64> {
    let dir = state.paths.models.clone();
    tokio::task::spawn_blocking(move || download::free_space(&dir))
        .await
        .map_err(|e| format!("Disk check failed: {e}"))?
        .ok_or_else(|| "Could not determine free disk space for the models folder.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_from_model() {
        let m = LocalModel {
            id: "u/r/x-Q4_K_M.gguf".into(),
            org: Some("u".into()),
            name: "x-Q4_K_M".into(),
            primary_path: "/m/u/r/x-Q4_K_M.gguf".into(),
            files: vec![],
            size_bytes: 0,
            mmproj_path: Some("/m/u/r/mmproj-F16.gguf".into()),
            quant: Some("Q4_K_M".into()),
            source_known: true,
            repo_id: Some("u/r".into()),
            meta: None,
        };
        let meta = GgufSummary { context_length: Some(262144), ..Default::default() };
        let p = new_profile_for(&m, Some(&meta));
        assert_eq!(p.ctx_size, 32768);
        assert_eq!(p.name, "x-Q4_K_M");
        assert_eq!(p.mmproj_path, m.mmproj_path);
        assert_eq!(p.n_cpu_moe, None);
        let small = GgufSummary { context_length: Some(8192), ..Default::default() };
        assert_eq!(new_profile_for(&m, Some(&small)).ctx_size, 8192);
    }

    #[test]
    fn removes_empty_parents_only() {
        let root = crate::models::gguf::tests::tmp_dir("del");
        let f = root.join("org").join("name").join("sub").join("a.gguf");
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(root.join("org").join("keep.txt"), b"x").unwrap();
        remove_empty_parents(&f, &root);
        assert!(!root.join("org").join("name").exists());
        assert!(root.join("org").exists());
        assert!(root.exists());
        std::fs::remove_dir_all(root).ok();
    }
}
