//! Resumable Hugging Face downloads (and the shared streaming helper the engine installer uses).
//!
//! Each download job holds one or more files from one repo; files run one after another and
//! at most two jobs run at once. Data streams into `<dest>.part`; resume sends `Range: bytes=N-`.

use super::hf;
use crate::jobs::{JobHandle, JobProgress, JobState};
use crate::state::AppState;
use futures_util::StreamExt;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};
use tokio::io::AsyncWriteExt;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub const HEADROOM: u64 = 1 << 30;
const MAX_CONCURRENT: usize = 2;
const EMIT_EVERY: Duration = Duration::from_millis(250);
const CHUNK_TIMEOUT: Duration = Duration::from_secs(60);

// ---------- shared helpers ----------

/// Free bytes on the drive holding `path` (or its nearest existing ancestor). Blocking.
pub fn free_space(path: &Path) -> Option<u64> {
    let mut p = path.to_path_buf();
    while !p.exists() {
        if !p.pop() {
            break;
        }
    }
    let canon = std::fs::canonicalize(&p).unwrap_or(p);
    let s = canon.to_string_lossy();
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s).to_lowercase();
    let target = PathBuf::from(s);
    let disks = sysinfo::Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .filter(|d| target.starts_with(PathBuf::from(d.mount_point().to_string_lossy().to_lowercase())))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.available_space())
}

pub fn fmt_gb(b: u64) -> String {
    format!("{:.1} GB", b as f64 / 1e9)
}

/// Error unless the drive holding `dir` has `need` bytes plus 1 GB headroom free.
pub async fn check_space(dir: &Path, need: u64) -> Result<(), String> {
    let d = dir.to_path_buf();
    let free = tokio::task::spawn_blocking(move || free_space(&d)).await.ok().flatten();
    match free {
        Some(free) if free < need.saturating_add(HEADROOM) => Err(format!(
            "Not enough disk space: this needs {} plus 1 GB headroom, but only {} is free on the drive holding {}.",
            fmt_gb(need),
            fmt_gb(free),
            dir.display()
        )),
        _ => Ok(()),
    }
}

pub fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_os_string();
    s.push(".part");
    PathBuf::from(s)
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Done,
    Paused,
    Cancelled,
}

/// Stream `url` into `part`, resuming from its current length. `progress` gets the bytes of
/// this file on disk so far. Returns when finished, paused or cancelled; never deletes `part`.
pub async fn fetch_resumable(
    http: &reqwest::Client,
    url: &str,
    token: Option<&str>,
    part: &Path,
    expected: Option<u64>,
    handle: &JobHandle,
    progress: &mut (dyn FnMut(u64) + Send),
) -> Result<Outcome, String> {
    if let Some(dir) = part.parent() {
        tokio::fs::create_dir_all(dir).await.map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    }
    let mut have = tokio::fs::metadata(part).await.map(|m| m.len()).unwrap_or(0);
    if let Some(exp) = expected {
        if have > exp {
            let _ = tokio::fs::remove_file(part).await;
            have = 0;
        }
        if have == exp && exp > 0 {
            progress(have);
            return Ok(Outcome::Done);
        }
    }
    let mut req = http.get(url);
    if have > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let resp = hf::with_auth(req, token).send().await.map_err(|e| format!("Download failed: {e}"))?;
    let status = resp.status();
    if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
        // Our partial file doesn't fit the remote one: start over next time.
        let _ = tokio::fs::remove_file(part).await;
        return Err("The server rejected the resume point; the partial file was discarded. Try again.".into());
    }
    if !status.is_success() {
        return Err(if url.contains("huggingface.co") {
            hf::status_error(status, url.rsplit('/').next().unwrap_or(url))
        } else {
            format!("Download failed: HTTP {status}")
        });
    }
    let append = if have > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT {
        let start = resp
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_content_range_start);
        if start.is_some_and(|s| s != have) {
            let _ = tokio::fs::remove_file(part).await;
            return Err("The server resumed at the wrong offset; the partial file was discarded. Try again.".into());
        }
        true
    } else {
        // 200 OK: the server ignored Range (or there was nothing to resume) → from zero.
        have = 0;
        false
    };
    let file = if append {
        tokio::fs::OpenOptions::new().append(true).open(part).await
    } else {
        tokio::fs::File::create(part).await
    }
    .map_err(|e| format!("Could not write {}: {e}", part.display()))?;
    let mut out = tokio::io::BufWriter::with_capacity(1 << 20, file);
    progress(have);
    let mut stream = resp.bytes_stream();
    let outcome = loop {
        if handle.cancelled() {
            break Outcome::Cancelled;
        }
        if handle.paused() {
            break Outcome::Paused;
        }
        let next = tokio::time::timeout(CHUNK_TIMEOUT, stream.next())
            .await
            .map_err(|_| "Download stalled (no data for 60 s). Resume to continue.".to_string())?;
        match next {
            None => break Outcome::Done,
            Some(Err(e)) => {
                let _ = out.flush().await;
                return Err(format!("Download interrupted: {e}. Resume to continue."));
            }
            Some(Ok(bytes)) => {
                out.write_all(&bytes).await.map_err(|e| format!("Could not write {}: {e}", part.display()))?;
                have += bytes.len() as u64;
                progress(have);
            }
        }
    };
    out.flush().await.map_err(|e| format!("Could not write {}: {e}", part.display()))?;
    Ok(outcome)
}

fn parse_content_range_start(v: &str) -> Option<u64> {
    // "bytes 100-199/200"
    v.trim().strip_prefix("bytes ")?.split('-').next()?.trim().parse().ok()
}

/// Check `part` has exactly `expected` bytes and move it to `dest`.
pub async fn finalize(part: &Path, dest: &Path, expected: Option<u64>) -> Result<(), String> {
    let len = tokio::fs::metadata(part).await.map(|m| m.len()).map_err(|e| format!("Missing {}: {e}", part.display()))?;
    if let Some(exp) = expected {
        if len < exp {
            return Err(format!("Download ended early ({} of {}). Resume to continue.", fmt_gb(len), fmt_gb(exp)));
        }
        if len > exp {
            let _ = tokio::fs::remove_file(part).await;
            return Err(format!("Size check failed for {} (got {len} bytes, expected {exp}). The file was discarded.", dest.display()));
        }
    }
    if tokio::fs::metadata(dest).await.is_ok() {
        let _ = tokio::fs::remove_file(dest).await;
    }
    tokio::fs::rename(part, dest).await.map_err(|e| format!("Could not move the finished file into place: {e}"))
}

/// Update a job's stored progress and emit it.
pub fn set_job(app: &AppHandle, id: &str, f: impl FnOnce(&mut JobProgress)) {
    let state = app.state::<AppState>();
    if let Some(mut p) = state.jobs.get(id) {
        f(&mut p);
        state.jobs.update(app, p);
    }
}

/// Wait while the job is paused. Returns false if it was cancelled.
pub async fn wait_unpaused(handle: &JobHandle) -> bool {
    while handle.paused() && !handle.cancelled() {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    !handle.cancelled()
}

/// Rate-limited progress emitter: at most 4 Hz.
pub struct Throttle {
    last: Option<Instant>,
}

impl Throttle {
    pub fn new() -> Self {
        Self { last: None }
    }
    pub fn ready(&mut self) -> bool {
        if self.last.is_none_or(|t| t.elapsed() >= EMIT_EVERY) {
            self.last = Some(Instant::now());
            true
        } else {
            false
        }
    }
}

// ---------- Hugging Face download jobs ----------

#[derive(Clone)]
struct Spec {
    repo_id: String,
    dir: PathBuf,
    files: Vec<(String, u64)>,
    running: bool,
}

fn registry() -> &'static Mutex<HashMap<String, Spec>> {
    static R: OnceLock<Mutex<HashMap<String, Spec>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

fn semaphore() -> Arc<Semaphore> {
    static S: OnceLock<Arc<Semaphore>> = OnceLock::new();
    S.get_or_init(|| Arc::new(Semaphore::new(MAX_CONCURRENT))).clone()
}

fn validate_rel_path(p: &str) -> Result<(), String> {
    let bad = p.is_empty()
        || p.starts_with('/')
        || p.starts_with('\\')
        || p.contains(':')
        || p.split(['/', '\\']).any(|c| c.is_empty() || c == "." || c == "..");
    if bad {
        Err(format!("Refusing to download to an unsafe path: {p}"))
    } else {
        Ok(())
    }
}

pub fn resolve_url(repo_id: &str, path: &str) -> String {
    let enc: Vec<String> = path.split('/').map(urlencode).collect();
    format!("{}/{repo_id}/resolve/main/{}", hf::API, enc.join("/"))
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Start a download job for `files` (repo-relative paths) of `repo_id`. Returns the job id.
pub async fn start(app: &AppHandle, repo_id: &str, files: Vec<String>) -> Result<String, String> {
    hf::validate_repo_id(repo_id)?;
    if files.is_empty() {
        return Err("Pick at least one file to download.".into());
    }
    for f in &files {
        validate_rel_path(f)?;
    }
    let state = app.state::<AppState>();
    let listing = hf::files(&state.http, repo_id).await?;
    let mut sized = Vec::new();
    for f in &files {
        let entry = listing
            .iter()
            .find(|x| &x.path == f)
            .ok_or_else(|| format!("{f} is not a GGUF file in {repo_id}."))?;
        sized.push((f.clone(), entry.size));
    }
    let (org, name) = repo_id.split_once('/').unwrap_or((repo_id, ""));
    let dir = state.paths.models.join(org).join(name);

    let mut need = 0u64;
    for (f, size) in &sized {
        let dest = dir.join(f);
        if tokio::fs::metadata(&dest).await.map(|m| m.len()).ok() == Some(*size) {
            continue;
        }
        let have = tokio::fs::metadata(part_path(&dest)).await.map(|m| m.len()).unwrap_or(0).min(*size);
        need += size - have;
    }
    check_space(&state.paths.models, need).await?;

    let label = match sized.len() {
        1 => format!("{repo_id} · {}", sized[0].0),
        n => format!("{repo_id} · {} (+{} more)", sized[0].0, n - 1),
    };
    let (id, _handle) = state.jobs.create("download", &label);
    let total: u64 = sized.iter().map(|(_, s)| s).sum();
    set_job(app, &id, |p| p.total = Some(total));
    registry().lock().insert(id.clone(), Spec { repo_id: repo_id.into(), dir, files: sized, running: false });
    spawn(app.clone(), id.clone());
    Ok(id)
}

fn spawn(app: AppHandle, id: String) {
    {
        let mut reg = registry().lock();
        match reg.get_mut(&id) {
            Some(s) if !s.running => s.running = true,
            _ => return,
        }
    }
    tauri::async_runtime::spawn(async move {
        let result = run(&app, &id).await;
        let spec = {
            let mut reg = registry().lock();
            if let Some(s) = reg.get_mut(&id) {
                s.running = false;
            }
            reg.get(&id).cloned()
        };
        match result {
            Ok(Outcome::Done) => {
                registry().lock().remove(&id);
                set_job(&app, &id, |p| {
                    p.state = JobState::Done;
                    p.done = p.total.unwrap_or(p.done);
                    p.message = spec.map(|s| format!("Saved to {}", s.dir.display()));
                });
            }
            Ok(Outcome::Cancelled) => {
                if let Some(s) = spec {
                    delete_parts(&s).await;
                }
                registry().lock().remove(&id);
                set_job(&app, &id, |p| {
                    p.state = JobState::Cancelled;
                    p.message = Some("Cancelled; partial files deleted.".into());
                });
            }
            Ok(Outcome::Paused) => {}
            Err(e) => set_job(&app, &id, |p| {
                p.state = JobState::Error;
                p.message = Some(e);
            }),
        }
    });
}

async fn delete_parts(spec: &Spec) {
    for (f, _) in &spec.files {
        let _ = tokio::fs::remove_file(part_path(&spec.dir.join(f))).await;
    }
}

async fn run(app: &AppHandle, id: &str) -> Result<Outcome, String> {
    let state = app.state::<AppState>();
    let spec = registry().lock().get(id).cloned().ok_or("Unknown download")?;
    let handle = state.jobs.handle(id).ok_or("Unknown download")?;
    let token = hf::token().await;
    let sem = semaphore();
    let mut permit: Option<OwnedSemaphorePermit> = None;
    let mut done_before = 0u64;

    for (path, size) in &spec.files {
        let dest = spec.dir.join(path);
        let part = part_path(&dest);
        if tokio::fs::metadata(&dest).await.map(|m| m.len()).ok() == Some(*size) {
            done_before += size;
            continue;
        }
        let url = resolve_url(&spec.repo_id, path);
        loop {
            if handle.cancelled() {
                return Ok(Outcome::Cancelled);
            }
            if handle.paused() {
                permit = None; // let another queued job run meanwhile
                set_job(app, id, |p| p.state = JobState::Paused);
                if !wait_unpaused(&handle).await {
                    return Ok(Outcome::Cancelled);
                }
                continue;
            }
            if permit.is_none() {
                set_job(app, id, |p| p.state = JobState::Queued);
                match tokio::time::timeout(Duration::from_millis(250), sem.clone().acquire_owned()).await {
                    Ok(Ok(p)) => permit = Some(p),
                    Ok(Err(_)) => return Err("Download queue closed".into()),
                    Err(_) => continue,
                }
            }
            set_job(app, id, |p| {
                p.state = JobState::Running;
                p.message = Some(path.clone());
            });
            let mut throttle = Throttle::new();
            let app2 = app.clone();
            let id2 = id.to_string();
            let base = done_before;
            let mut on_progress = move |bytes: u64| {
                if throttle.ready() {
                    set_job(&app2, &id2, |p| p.done = base + bytes);
                }
            };
            match fetch_resumable(&state.http, &url, token.as_deref(), &part, Some(*size), &handle, &mut on_progress).await? {
                Outcome::Done => {
                    finalize(&part, &dest, Some(*size)).await?;
                    done_before += size;
                    set_job(app, id, |p| p.done = done_before);
                    break;
                }
                Outcome::Paused => continue,
                Outcome::Cancelled => return Ok(Outcome::Cancelled),
            }
        }
    }
    drop(permit);
    Ok(Outcome::Done)
}

pub fn pause(app: &AppHandle, id: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    if !state.jobs.set_paused(id, true) {
        return Err("No such download.".into());
    }
    if registry().lock().get(id).is_some_and(|s| !s.running) {
        set_job(app, id, |p| p.state = JobState::Paused);
    }
    Ok(())
}

/// Resume a paused job, or restart one that stopped with an error (keeps the .part files).
pub fn resume(app: &AppHandle, id: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    if !state.jobs.set_paused(id, false) {
        return Err("No such download.".into());
    }
    if registry().lock().contains_key(id) {
        spawn(app.clone(), id.to_string());
    }
    Ok(())
}

/// Cancel any job. For downloads, the .part files are deleted.
pub async fn cancel(app: &AppHandle, id: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    if !state.jobs.cancel(id) {
        return Err("No such job.".into());
    }
    let idle = registry().lock().get(id).filter(|s| !s.running).cloned();
    if let Some(spec) = idle {
        delete_parts(&spec).await;
        registry().lock().remove(id);
        set_job(app, id, |p| {
            p.state = JobState::Cancelled;
            p.message = Some("Cancelled; partial files deleted.".into());
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(parse_content_range_start("bytes 100-199/200"), Some(100));
        assert_eq!(part_path(Path::new("/a/b.gguf")), PathBuf::from("/a/b.gguf.part"));
        assert!(validate_rel_path("BF16/x.gguf").is_ok());
        assert!(validate_rel_path("../x.gguf").is_err());
        assert!(validate_rel_path("C:/x.gguf").is_err());
        assert_eq!(
            resolve_url("unsloth/Q-GGUF", "BF16/a b.gguf"),
            "https://huggingface.co/unsloth/Q-GGUF/resolve/main/BF16/a%20b.gguf"
        );
        assert!(free_space(&std::env::temp_dir()).is_some());
    }

    /// Serve one file over a raw TCP socket, optionally ignoring Range, and check resume.
    async fn serve(body: Vec<u8>, honor_range: bool) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { return };
                let body = body.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_lowercase();
                    let start = req
                        .lines()
                        .find_map(|l| l.strip_prefix("range: bytes="))
                        .and_then(|r| r.trim_end_matches('-').trim().trim_end_matches('-').parse::<usize>().ok());
                    let (status, slice, extra) = match start {
                        Some(s) if honor_range => (
                            "206 Partial Content",
                            &body[s..],
                            format!("Content-Range: bytes {s}-{}/{}\r\n", body.len() - 1, body.len()),
                        ),
                        _ => ("200 OK", &body[..], String::new()),
                    };
                    let head = format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n",
                        slice.len()
                    );
                    let _ = sock.write_all(head.as_bytes()).await;
                    let _ = sock.write_all(slice).await;
                });
            }
        });
        format!("http://{addr}/file")
    }

    #[tokio::test]
    async fn resumes_and_restarts() {
        let body: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        let dir = crate::models::gguf::tests::tmp_dir("dl");
        let http = reqwest::Client::new();
        let handle = JobHandle::default();
        for honor in [true, false] {
            let url = serve(body.clone(), honor).await;
            let dest = dir.join(format!("f-{honor}.bin"));
            let part = part_path(&dest);
            std::fs::write(&part, &body[..30_000]).unwrap();
            let mut last = 0;
            let mut cb = |b: u64| last = b;
            let out = fetch_resumable(&http, &url, None, &part, Some(body.len() as u64), &handle, &mut cb)
                .await
                .unwrap();
            assert_eq!(out, Outcome::Done);
            assert_eq!(last, body.len() as u64);
            finalize(&part, &dest, Some(body.len() as u64)).await.unwrap();
            assert_eq!(std::fs::read(&dest).unwrap(), body);
            assert!(!part.exists());
        }
        // Size mismatch is caught.
        let url = serve(body.clone(), true).await;
        let dest = dir.join("short.bin");
        let part = part_path(&dest);
        let mut cb = |_b: u64| {};
        fetch_resumable(&http, &url, None, &part, None, &handle, &mut cb).await.unwrap();
        assert!(finalize(&part, &dest, Some(body.len() as u64 + 5)).await.is_err());
        std::fs::remove_dir_all(dir).ok();
    }
}
