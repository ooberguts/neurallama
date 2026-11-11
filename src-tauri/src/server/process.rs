//! llama-server process control: spawn, readiness, stop, attach, crash handling.
//!
//! State machine: Stopped → Starting → Loading → Ready; Crashed on unexpected exit;
//! Attached when an llama-server we didn't spawn already answers on the port.
//! Every change is emitted as `server-status`; log lines go out as `server-log` (≤4 Hz).

use super::logs::{LineInfo, LogSink};
use super::{args, resolve_server_exe, ServerState, ServerStatus, LOG_EVENT, STATUS_EVENT};
use crate::settings::{Paths, Profile, Settings};
use crate::state::AppState;
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

type BoxFut<T> = Pin<Box<dyn Future<Output = T> + Send>>;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CRASH_WINDOW: Duration = Duration::from_secs(60);
const MAX_CRASHES: usize = 3;
const STOP_WAIT: Duration = Duration::from_secs(5);
const HEALTH_INTERVAL: Duration = Duration::from_millis(500);
const LOG_FLUSH_INTERVAL: Duration = Duration::from_millis(250);
const NO_KEY_LAN: &str =
    "LAN mode needs an API key. Set one in Settings before starting, or turn LAN mode off.";

/// Where the server listens, captured at start/attach.
#[derive(Debug, Clone)]
struct Endpoint {
    base_url: String,
    api_key: String,
    bind_host: String,
    port: u16,
}

impl Endpoint {
    fn from_settings(s: &Settings) -> Self {
        Self {
            base_url: s.base_url(),
            api_key: s.api_key.trim().to_string(),
            bind_host: s.bind_host().to_string(),
            port: s.port,
        }
    }
}

/// One spawned llama-server process.
struct Run {
    id: u64,
    pid: Option<u32>,
    kill_tx: mpsc::Sender<()>,
    exited: watch::Receiver<bool>,
    stopping: Arc<AtomicBool>,
}

enum Probe {
    Nothing,
    Llama,
    Other,
}

struct Inner {
    paths: Paths,
    http: reqwest::Client,
    logs: LogSink,
    status: Mutex<ServerStatus>,
    status_tx: watch::Sender<ServerStatus>,
    app: Mutex<Option<AppHandle>>,
    run: Mutex<Option<Run>>,
    /// Serializes start/stop/attach.
    op: tokio::sync::Mutex<()>,
    next_run_id: AtomicU64,
    /// Bumped by every user-initiated start/stop; a pending auto-restart checks it.
    generation: AtomicU64,
    crashes: Mutex<VecDeque<Instant>>,
    last_launch: Mutex<Option<(Profile, Settings)>>,
    endpoint: Mutex<Option<Endpoint>>,
    shutting_down: AtomicBool,
    #[cfg(windows)]
    job: Mutex<Option<super::job_object::JobObject>>,
}

pub struct ServerManager {
    inner: Arc<Inner>,
}

fn stopped_status() -> ServerStatus {
    ServerStatus {
        state: ServerState::Stopped,
        pid: None,
        port: 0,
        profile_id: None,
        model_path: None,
        started_at: None,
        last_error: None,
        load_progress: None,
        command_line: None,
        crash_count: 0,
    }
}

impl ServerManager {
    pub fn new(paths: Paths) -> Self {
        let initial = stopped_status();
        let (status_tx, _) = watch::channel(initial.clone());
        let http = reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(3))
            .build()
            .unwrap_or_default();
        let inner = Inner {
            logs: LogSink::new(paths.logs.clone()),
            paths,
            http,
            status: Mutex::new(initial),
            status_tx,
            app: Mutex::new(None),
            run: Mutex::new(None),
            op: tokio::sync::Mutex::new(()),
            next_run_id: AtomicU64::new(0),
            generation: AtomicU64::new(0),
            crashes: Mutex::new(VecDeque::new()),
            last_launch: Mutex::new(None),
            endpoint: Mutex::new(None),
            shutting_down: AtomicBool::new(false),
            #[cfg(windows)]
            job: Mutex::new(super::job_object::JobObject::new().ok()),
        };
        Self { inner: Arc::new(inner) }
    }

    /// Start `profile`. Stops whatever runs first. If an llama-server already answers on
    /// the port, attaches instead. Returns once spawned; readiness arrives via events.
    pub async fn start(&self, app: &AppHandle, p: &Profile, s: &Settings) -> Result<ServerStatus, String> {
        self.inner.clone().start_inner(Some(app.clone()), p.clone(), s.clone(), true, None).await
    }

    pub async fn stop(&self, app: &AppHandle) -> Result<ServerStatus, String> {
        self.inner.clone().stop_public(Some(app.clone())).await
    }

    pub fn status(&self) -> ServerStatus {
        self.inner.status.lock().clone()
    }

    /// Wait until the server answers /health with 200. Errors if it stops or crashes.
    pub async fn wait_ready(&self, timeout: Duration) -> Result<(), String> {
        self.inner.wait_ready(timeout).await
    }

    pub fn log_tail(&self, n: usize) -> Vec<String> {
        self.inner.logs.tail(n)
    }

    /// Endpoint captured at launch, even when the user has saved new settings for next time.
    pub fn connection(&self) -> Option<(String, String)> {
        self.inner.endpoint.lock().as_ref().map(|ep| (ep.base_url.clone(), ep.api_key.clone()))
    }

    pub fn pid(&self) -> Option<u32> {
        self.inner.run.lock().as_ref().and_then(|r| r.pid)
    }

    /// CUDA buffer total (model + KV + compute) parsed from the log since the last start.
    pub fn vram_allocations_mib(&self) -> Option<f64> {
        self.inner.logs.parse_state().cuda_mib
    }

    // ---- additions beyond the contract ----

    /// Remember the AppHandle for status/log events.
    pub fn set_app(&self, app: &AppHandle) {
        *self.inner.app.lock() = Some(app.clone());
    }

    /// If an llama-server already answers on the configured port (and we aren't running
    /// one), go to Attached. Returns the new status when attached.
    pub async fn attach_if_running(&self, app: &AppHandle, s: &Settings) -> Option<ServerStatus> {
        self.inner.clone().attach_if_running(Some(app.clone()), s.clone()).await
    }

    /// Profile id of the last launch (for restart).
    pub fn last_profile_id(&self) -> Option<String> {
        self.inner.last_launch.lock().as_ref().map(|(p, _)| p.id.clone())
    }

    /// Stop our child synchronously (app exit). Safe to call from any thread. An attached
    /// server we didn't spawn is left running.
    pub fn shutdown_blocking(&self) {
        let inner = self.inner.clone();
        inner.shutting_down.store(true, Ordering::SeqCst);
        let run = inner.run.lock().take();
        if let Some(run) = run {
            run.stopping.store(true, Ordering::SeqCst);
            let _ = run.kill_tx.try_send(());
            let mut exited = run.exited.clone();
            // Own thread: block_on would panic if we happened to be inside the runtime.
            let _ = std::thread::spawn(move || {
                tauri::async_runtime::block_on(async move {
                    let _ = tokio::time::timeout(Duration::from_secs(7), exited.wait_for(|v| *v)).await;
                })
            })
            .join();
        }
        inner.logs.drain_pending();
    }
}

/// True when nothing is listening on `port` (checked on the bind host, all interfaces and
/// loopback, which catches listeners on any of them on Windows and Unix).
pub fn port_free(host: &str, port: u16) -> bool {
    let mut hosts = vec![host, "0.0.0.0", "127.0.0.1"];
    hosts.dedup();
    hosts.iter().all(|h| std::net::TcpListener::bind((*h, port)).is_ok())
}

/// First free port at or above `from`.
pub fn next_free_port(host: &str, from: u16) -> Option<u16> {
    (from.max(1)..=u16::MAX).find(|p| port_free(host, *p))
}

fn port_busy_msg(ep: &Endpoint) -> String {
    match next_free_port(&ep.bind_host, ep.port.saturating_add(1)) {
        Some(n) => format!(
            "Port {} is already in use by another program. Port {n} is free — change it in Settings.",
            ep.port
        ),
        None => format!("Port {} is already in use by another program. Pick another port in Settings.", ep.port),
    }
}

fn describe_exit(r: &std::io::Result<ExitStatus>) -> (String, Option<String>) {
    match r {
        Ok(s) => match s.code() {
            Some(c) => {
                let u = c as u32;
                let hint = match u {
                    0xC000_0135 | 0xC000_007B => Some(
                        "llama-server couldn't start: a required DLL is missing or mismatched. Reinstall the engine in Settings → Engine (CUDA builds also need the matching cudart package).".to_string(),
                    ),
                    0xC000_001D => Some(
                        "llama-server crashed with an illegal instruction: this build needs CPU features this machine lacks. Try a different build in Settings → Engine.".to_string(),
                    ),
                    _ => None,
                };
                let text = if !(0..=0xFFFF).contains(&c) { format!("exit code 0x{u:08X}") } else { format!("exit code {c}") };
                (text, hint)
            }
            None => ("terminated by a signal".into(), None),
        },
        Err(e) => (format!("could not wait for the process: {e}"), None),
    }
}

impl Inner {
    // ---- status ----

    fn set_status<F: FnOnce(&mut ServerStatus)>(&self, f: F) -> ServerStatus {
        let (new, changed) = {
            let mut g = self.status.lock();
            let old = g.clone();
            f(&mut g);
            (g.clone(), *g != old)
        };
        if changed {
            self.status_tx.send_replace(new.clone());
            if let Some(app) = self.app.lock().clone() {
                let _ = app.emit(STATUS_EVENT, &new);
            }
        }
        self.emit_pending_logs();
        new
    }

    fn emit_pending_logs(&self) {
        let lines = self.logs.drain_pending();
        if lines.is_empty() {
            return;
        }
        if let Some(app) = self.app.lock().clone() {
            let _ = app.emit(LOG_EVENT, &lines);
        }
    }

    fn is_current(&self, run_id: u64) -> bool {
        self.run.lock().as_ref().map(|r| r.id) == Some(run_id)
    }

    fn crash_count(&self) -> u32 {
        let mut c = self.crashes.lock();
        while c.front().is_some_and(|t| t.elapsed() > CRASH_WINDOW) {
            c.pop_front();
        }
        c.len() as u32
    }

    fn fresh_settings(&self) -> Option<Settings> {
        let app = self.app.lock().clone()?;
        app.try_state::<AppState>().map(|s| s.settings())
    }

    // ---- start ----

    fn start_inner(
        self: Arc<Self>,
        app: Option<AppHandle>,
        profile: Profile,
        settings: Settings,
        manual: bool,
        expect_gen: Option<u64>,
    ) -> BoxFut<Result<ServerStatus, String>> {
        Box::pin(async move {
            let _op = self.op.lock().await;
            if self.shutting_down.load(Ordering::SeqCst) {
                return Err("Neurallama is shutting down.".into());
            }
            if let Some(g) = expect_gen {
                if self.generation.load(Ordering::SeqCst) != g {
                    return Ok(self.status.lock().clone());
                }
            }
            if let Some(app) = app {
                *self.app.lock() = Some(app);
            }

            // Validate before touching whatever is running now.
            if settings.lan_mode && settings.api_key.trim().is_empty() {
                return Err(NO_KEY_LAN.into());
            }
            if profile.model_path.trim().is_empty() {
                return Err(format!(
                    "The profile \"{}\" has no model file. Pick a model on the Models tab.",
                    profile.name
                ));
            }
            if !Path::new(&profile.model_path).is_file() {
                return Err(format!("Model file not found: {}", profile.model_path));
            }
            if let Some(mm) = profile.mmproj_path.as_deref().filter(|m| !m.trim().is_empty()) {
                if !Path::new(mm).is_file() {
                    return Err(format!("Vision projector (mmproj) file not found: {mm}"));
                }
            }
            let exe = resolve_server_exe(&self.paths, &settings)?;

            if manual {
                self.generation.fetch_add(1, Ordering::SeqCst);
                self.crashes.lock().clear();
            }
            self.stop_locked().await?;

            let ep = Endpoint::from_settings(&settings);
            if !port_free(&ep.bind_host, ep.port) {
                return match self.probe(&ep).await {
                    Probe::Llama => Ok(self.clone().mark_attached(ep).await),
                    Probe::Other | Probe::Nothing => Err(port_busy_msg(&ep)),
                };
            }
            *self.endpoint.lock() = Some(ep.clone());

            let preview = args::build(&profile, &settings, &exe);
            let shown = args::redacted_display(&preview.program, &preview.args);
            let mut secrets = vec![settings.api_key.trim().to_string()];
            for (i, arg) in preview.args.iter().enumerate() {
                if arg == "--api-key" {
                    secrets.extend(preview.args.get(i + 1).cloned());
                } else if let Some(key) = arg.strip_prefix("--api-key=") {
                    secrets.push(key.to_string());
                }
            }
            self.logs.begin_run(secrets);
            self.logs.note(&format!("Starting: {shown}"));

            let mut cmd = Command::new(&exe);
            cmd.args(&preview.args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            if let Some(dir) = exe.parent() {
                cmd.current_dir(dir);
            }
            #[cfg(windows)]
            cmd.creation_flags(CREATE_NO_WINDOW);

            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    let msg = format!("Could not start llama-server ({}): {e}", exe.display());
                    self.logs.note(&msg);
                    let m = msg.clone();
                    self.set_status(|s| {
                        *s = ServerStatus { port: ep.port, last_error: Some(m), ..stopped_status() };
                    });
                    return Err(msg);
                }
            };
            let pid = child.id();
            #[cfg(windows)]
            self.assign_job(&child);

            let run_id = self.next_run_id.fetch_add(1, Ordering::SeqCst) + 1;
            let (kill_tx, kill_rx) = mpsc::channel(1);
            let (exit_tx, exit_rx) = watch::channel(false);
            let stopping = Arc::new(AtomicBool::new(false));
            let ready = Arc::new(AtomicBool::new(false));
            *self.run.lock() =
                Some(Run { id: run_id, pid, kill_tx, exited: exit_rx.clone(), stopping: stopping.clone() });
            *self.last_launch.lock() = Some((profile.clone(), settings.clone()));

            let crash_count = self.crash_count();
            let status = self.set_status(|s| {
                *s = ServerStatus {
                    state: ServerState::Starting,
                    pid,
                    port: ep.port,
                    profile_id: Some(profile.id.clone()),
                    model_path: Some(profile.model_path.clone()),
                    started_at: Some(chrono::Local::now().to_rfc3339()),
                    last_error: None,
                    load_progress: None,
                    command_line: Some(shown),
                    crash_count,
                };
            });

            let mut readers = Vec::new();
            if let Some(out) = child.stdout.take() {
                readers.push(tokio::spawn(self.clone().read_lines(run_id, out)));
            }
            if let Some(err) = child.stderr.take() {
                readers.push(tokio::spawn(self.clone().read_lines(run_id, err)));
            }
            tokio::spawn(self.clone().flush_logs(exit_rx.clone()));
            tokio::spawn(self.clone().poll_health(run_id, ep, exit_rx, ready.clone()));
            tokio::spawn(self.clone().monitor(run_id, child, kill_rx, exit_tx, stopping, readers, ready, profile.id));
            Ok(status)
        })
    }

    #[cfg(windows)]
    fn assign_job(&self, child: &Child) {
        let mut job = self.job.lock();
        if job.is_none() {
            match super::job_object::JobObject::new() {
                Ok(j) => *job = Some(j),
                Err(e) => self.logs.note(&format!("Warning: {e}")),
            }
        }
        if let (Some(j), Some(h)) = (job.as_ref(), child.raw_handle()) {
            if let Err(e) = j.assign(h) {
                self.logs.note(&format!("Warning: {e} (llama-server may outlive the app if it crashes)"));
            }
        }
    }

    // ---- per-run tasks ----

    async fn read_lines<R: AsyncRead + Unpin + Send + 'static>(self: Arc<Self>, run_id: u64, r: R) {
        let mut reader = BufReader::new(r);
        let mut buf = Vec::with_capacity(512);
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let line = String::from_utf8_lossy(&buf);
                    if let Some(info) = self.logs.push(&line) {
                        self.on_line_info(run_id, info);
                    }
                }
            }
        }
    }

    fn on_line_info(&self, run_id: u64, info: LineInfo) {
        if !self.is_current(run_id) {
            return;
        }
        match info {
            LineInfo::Progress(_) => {
                let p = self.logs.parse_state().load_progress;
                self.set_status(|s| {
                    if matches!(s.state, ServerState::Starting | ServerState::Loading) {
                        s.state = ServerState::Loading;
                        s.load_progress = p;
                    }
                });
            }
            LineInfo::Error { .. } => {
                let e = self.logs.parse_state().error.map(|(_, m)| m);
                self.set_status(|s| s.last_error = e);
            }
            LineInfo::CudaAlloc(_) | LineInfo::Listening => {}
        }
    }

    async fn flush_logs(self: Arc<Self>, mut exited: watch::Receiver<bool>) {
        loop {
            let done = *exited.borrow();
            if !done {
                tokio::select! {
                    _ = tokio::time::sleep(LOG_FLUSH_INTERVAL) => {}
                    r = exited.changed() => {
                        if r.is_err() {
                            self.emit_pending_logs();
                            return;
                        }
                    }
                }
            }
            self.emit_pending_logs();
            if done {
                return;
            }
        }
    }

    async fn poll_health(
        self: Arc<Self>,
        run_id: u64,
        ep: Endpoint,
        mut exited: watch::Receiver<bool>,
        ready: Arc<AtomicBool>,
    ) {
        let url = format!("{}/health", ep.base_url);
        loop {
            if *exited.borrow() || !self.is_current(run_id) {
                return;
            }
            let mut req = self.http.get(&url).timeout(Duration::from_secs(2));
            if !ep.api_key.is_empty() {
                req = req.bearer_auth(&ep.api_key);
            }
            match req.send().await.map(|r| r.status().as_u16()) {
                Ok(200) if self.is_current(run_id) => {
                    ready.store(true, Ordering::SeqCst);
                    self.logs.note("llama-server is ready.");
                    self.set_status(|s| {
                        if matches!(s.state, ServerState::Starting | ServerState::Loading) {
                            s.state = ServerState::Ready;
                            s.load_progress = Some(1.0);
                        }
                    });
                    return;
                }
                Ok(503) if self.is_current(run_id) => {
                    self.set_status(|s| {
                        if s.state == ServerState::Starting {
                            s.state = ServerState::Loading;
                        }
                    });
                }
                _ => {}
            }
            tokio::select! {
                _ = tokio::time::sleep(HEALTH_INTERVAL) => {}
                r = exited.changed() => if r.is_err() { return; },
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn monitor(
        self: Arc<Self>,
        run_id: u64,
        mut child: Child,
        mut kill_rx: mpsc::Receiver<()>,
        exit_tx: watch::Sender<bool>,
        stopping: Arc<AtomicBool>,
        readers: Vec<JoinHandle<()>>,
        ready: Arc<AtomicBool>,
        profile_id: String,
    ) {
        let result = tokio::select! {
            r = child.wait() => r,
            _ = kill_rx.recv() => terminate(&mut child).await,
        };
        for h in readers {
            // A grandchild holding the pipe open must not hang us.
            let _ = tokio::time::timeout(Duration::from_secs(2), h).await;
        }
        {
            let mut r = self.run.lock();
            if r.as_ref().map(|r| r.id) == Some(run_id) {
                *r = None;
            }
        }
        if !stopping.load(Ordering::SeqCst) {
            self.clone().handle_crash(result, ready.load(Ordering::SeqCst), profile_id);
        }
        exit_tx.send_replace(true);
    }

    fn handle_crash(self: Arc<Self>, result: std::io::Result<ExitStatus>, was_ready: bool, profile_id: String) {
        let (code, hint) = describe_exit(&result);
        self.logs.note(&format!("llama-server exited unexpectedly ({code})."));
        let mut err = self
            .logs
            .parse_state()
            .error
            .map(|(_, m)| m)
            .or(hint)
            .unwrap_or_else(|| format!("llama-server exited unexpectedly ({code}). See the log for details."));
        let count = {
            self.crashes.lock().push_back(Instant::now());
            self.crash_count()
        };
        let settings = self.fresh_settings().or_else(|| self.last_launch.lock().as_ref().map(|(_, s)| s.clone()));
        let auto = settings.as_ref().is_some_and(|s| s.auto_restart);
        // Only restart a server that had come up: a failure during load (OOM, bad model,
        // bad args) would just fail again.
        let restart = auto && was_ready && (count as usize) < MAX_CRASHES && !self.shutting_down.load(Ordering::SeqCst);
        if auto && count as usize >= MAX_CRASHES {
            err = format!("{err} Auto-restart stopped after {MAX_CRASHES} crashes in 60 s.");
        }
        self.set_status(|s| {
            s.state = ServerState::Crashed;
            s.pid = None;
            s.load_progress = None;
            s.last_error = Some(err);
            s.crash_count = count;
        });
        if !restart {
            return;
        }
        let gen = self.generation.load(Ordering::SeqCst);
        let inner = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            if inner.generation.load(Ordering::SeqCst) != gen || inner.status.lock().state != ServerState::Crashed {
                return;
            }
            let fresh = inner.fresh_settings();
            let last = inner.last_launch.lock().clone();
            let (profile, settings) = match (fresh, last) {
                (Some(s), last) => match s.profile(&profile_id).cloned().or(last.map(|(p, _)| p)) {
                    Some(p) => (p, s),
                    None => return,
                },
                (None, Some(last)) => last,
                (None, None) => return,
            };
            inner.logs.note("Auto-restarting llama-server…");
            if let Err(e) = inner.clone().start_inner(None, profile, settings, false, Some(gen)).await {
                inner.logs.note(&format!("Auto-restart failed: {e}"));
                inner.set_status(|s| s.last_error = Some(e));
            }
        });
    }

    // ---- stop ----

    async fn stop_public(self: Arc<Self>, app: Option<AppHandle>) -> Result<ServerStatus, String> {
        let _op = self.op.lock().await;
        if let Some(app) = app {
            *self.app.lock() = Some(app);
        }
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.stop_locked().await?;
        Ok(self.set_status(|s| {
            if s.state == ServerState::Crashed {
                s.state = ServerState::Stopped;
            }
        }))
    }

    /// Stop our process (or the attached one). Caller holds `op`.
    async fn stop_locked(&self) -> Result<(), String> {
        let run = self.run.lock().take();
        let ep = self.endpoint.lock().clone();
        if let Some(run) = run {
            run.stopping.store(true, Ordering::SeqCst);
            self.logs.note("Stopping llama-server…");
            let _ = run.kill_tx.try_send(());
            let mut exited = run.exited.clone();
            if tokio::time::timeout(STOP_WAIT * 2 + Duration::from_secs(2), exited.wait_for(|v| *v)).await.is_err() {
                self.logs.note("llama-server did not exit in time.");
            } else {
                self.logs.note("llama-server stopped.");
            }
            self.set_status(|s| {
                s.state = ServerState::Stopped;
                s.pid = None;
                s.load_progress = None;
            });
        } else if self.status.lock().state == ServerState::Attached {
            let port = ep.as_ref().map(|e| e.port).unwrap_or(self.status.lock().port);
            let killed = tokio::task::spawn_blocking(move || kill_external(port)).await.unwrap_or(0);
            if killed == 0 {
                return Err(format!(
                    "The llama-server on port {port} wasn't started by Neurallama and its process couldn't be found. Close it yourself (e.g. Task Manager), then try again."
                ));
            }
            self.logs.note(&format!("Stopped the external llama-server on port {port}."));
            self.set_status(|s| {
                s.state = ServerState::Stopped;
                s.pid = None;
                s.load_progress = None;
            });
        } else {
            return Ok(());
        }
        if let Some(ep) = ep {
            let deadline = Instant::now() + STOP_WAIT;
            while !port_free(&ep.bind_host, ep.port) && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
        Ok(())
    }

    // ---- attach ----

    async fn attach_if_running(self: Arc<Self>, app: Option<AppHandle>, settings: Settings) -> Option<ServerStatus> {
        let _op = self.op.lock().await;
        if let Some(app) = app {
            *self.app.lock() = Some(app);
        }
        if self.run.lock().is_some() {
            return None;
        }
        let ep = Endpoint::from_settings(&settings);
        if port_free(&ep.bind_host, ep.port) {
            self.set_status(|s| {
                if s.port == 0 {
                    s.port = ep.port;
                }
            });
            return None;
        }
        match self.probe(&ep).await {
            Probe::Llama => Some(self.clone().mark_attached(ep).await),
            _ => None,
        }
    }

    async fn mark_attached(self: Arc<Self>, ep: Endpoint) -> ServerStatus {
        *self.endpoint.lock() = Some(ep.clone());
        let model_path = self.props(&ep).await.and_then(|v| {
            v.get("model_path").and_then(|m| m.as_str()).map(str::to_string)
        });
        self.logs.note(&format!("Attached to the llama-server already running on port {}.", ep.port));
        let status = self.set_status(|s| {
            *s = ServerStatus {
                state: ServerState::Attached,
                port: ep.port,
                model_path,
                crash_count: s.crash_count,
                ..stopped_status()
            };
        });
        let gen = self.generation.load(Ordering::SeqCst);
        tokio::spawn(self.clone().watch_attached(ep, gen));
        status
    }

    /// While Attached, notice when the external server goes away.
    async fn watch_attached(self: Arc<Self>, ep: Endpoint, gen: u64) {
        let mut misses = 0;
        loop {
            tokio::time::sleep(Duration::from_secs(3)).await;
            if self.status.lock().state != ServerState::Attached || self.generation.load(Ordering::SeqCst) != gen {
                return;
            }
            let alive = self
                .http
                .get(format!("{}/health", ep.base_url))
                .timeout(Duration::from_secs(3))
                .send()
                .await
                .is_ok();
            misses = if alive { 0 } else { misses + 1 };
            if misses >= 2 {
                self.logs.note("The attached llama-server is no longer answering.");
                self.set_status(|s| {
                    if s.state == ServerState::Attached {
                        s.state = ServerState::Stopped;
                    }
                });
                return;
            }
        }
    }

    async fn props(&self, ep: &Endpoint) -> Option<serde_json::Value> {
        let mut req = self.http.get(format!("{}/props", ep.base_url)).timeout(Duration::from_secs(2));
        if !ep.api_key.is_empty() {
            req = req.bearer_auth(&ep.api_key);
        }
        let r = req.send().await.ok()?;
        if !r.status().is_success() {
            return None;
        }
        r.json().await.ok()
    }

    /// Does whatever holds the port look like llama-server?
    async fn probe(&self, ep: &Endpoint) -> Probe {
        let mut req = self.http.get(format!("{}/health", ep.base_url)).timeout(Duration::from_secs(2));
        if !ep.api_key.is_empty() {
            req = req.bearer_auth(&ep.api_key);
        }
        match req.send().await {
            Err(e) if e.is_connect() => Probe::Nothing,
            Err(_) => Probe::Other,
            Ok(resp) => {
                let code = resp.status().as_u16();
                let body: serde_json::Value = resp.json().await.unwrap_or_default();
                if looks_like_llama_health(code, &body) {
                    return Probe::Llama;
                }
                match self.props(ep).await {
                    Some(v) if ["default_generation_settings", "build_info", "total_slots"].iter().any(|k| v.get(k).is_some()) => {
                        Probe::Llama
                    }
                    _ => Probe::Other,
                }
            }
        }
    }

    // ---- readiness ----

    async fn wait_ready(&self, timeout: Duration) -> Result<(), String> {
        let mut rx = self.status_tx.subscribe();
        let wait = async {
            loop {
                let s = rx.borrow_and_update().clone();
                match s.state {
                    ServerState::Ready => return Ok(()),
                    ServerState::Crashed => {
                        return Err(s.last_error.unwrap_or_else(|| "llama-server crashed.".into()))
                    }
                    ServerState::Stopped => {
                        return Err(s.last_error.unwrap_or_else(|| "llama-server is not running.".into()))
                    }
                    ServerState::Attached => {
                        let ep = self.endpoint.lock().clone();
                        if let Some(ep) = ep {
                            let mut req = self.http.get(format!("{}/health", ep.base_url)).timeout(Duration::from_secs(2));
                            if !ep.api_key.is_empty() {
                                req = req.bearer_auth(&ep.api_key);
                            }
                            if req.send().await.is_ok_and(|r| r.status().as_u16() == 200) {
                                return Ok(());
                            }
                        }
                        tokio::select! {
                            _ = tokio::time::sleep(HEALTH_INTERVAL) => {}
                            r = rx.changed() => if r.is_err() { return Err("llama-server status unavailable.".to_string()); },
                        }
                        continue;
                    }
                    ServerState::Starting | ServerState::Loading => {}
                }
                if rx.changed().await.is_err() {
                    return Err("llama-server status unavailable.".to_string());
                }
            }
        };
        tokio::time::timeout(timeout, wait)
            .await
            .unwrap_or_else(|_| Err("Timed out waiting for llama-server to become ready.".into()))
    }
}

fn looks_like_llama_health(code: u16, body: &serde_json::Value) -> bool {
    match code {
        200 => body.get("status").and_then(|s| s.as_str()) == Some("ok"),
        503 => body
            .get("error")
            .map(|e| {
                e.get("type").and_then(|t| t.as_str()) == Some("unavailable_error")
                    || e.get("message").and_then(|m| m.as_str()).is_some_and(|m| m.contains("Loading"))
            })
            .unwrap_or(false),
        _ => false,
    }
}

/// Ask nicely, then force. On Windows a windowless child has no console to signal, so
/// TerminateProcess (`start_kill`) is the only reliable option.
async fn terminate(child: &mut Child) -> std::io::Result<ExitStatus> {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        let _ = Command::new("kill").arg("-TERM").arg(pid.to_string()).status().await;
        if let Ok(r) = tokio::time::timeout(STOP_WAIT, child.wait()).await {
            return r;
        }
    }
    let _ = child.start_kill();
    match tokio::time::timeout(STOP_WAIT, child.wait()).await {
        Ok(r) => r,
        Err(_) => Err(std::io::Error::other("llama-server did not exit after being killed")),
    }
}

/// Kill llama-server processes listening on `port` that we didn't spawn. Returns how many.
fn kill_external(port: u16) -> usize {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
    );
    let mut killed = 0;
    for p in sys.processes().values() {
        let name = p.name().to_string_lossy().to_ascii_lowercase();
        if !name.starts_with("llama-server") {
            continue;
        }
        let cmd: Vec<String> = p.cmd().iter().map(|a| a.to_string_lossy().into_owned()).collect();
        if process_port(&cmd) == port && p.kill() {
            killed += 1;
        }
    }
    killed
}

/// The port an llama-server command line listens on (default 8080).
fn process_port(cmd: &[String]) -> u16 {
    for (i, a) in cmd.iter().enumerate() {
        if let Some(v) = a.strip_prefix("--port=") {
            return v.parse().unwrap_or(8080);
        }
        if a == "--port" {
            return cmd.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or(8080);
        }
    }
    8080
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_detection() {
        assert!(looks_like_llama_health(200, &serde_json::json!({"status": "ok"})));
        assert!(looks_like_llama_health(
            503,
            &serde_json::json!({"error": {"code": 503, "message": "Loading model", "type": "unavailable_error"}})
        ));
        assert!(!looks_like_llama_health(200, &serde_json::json!({"healthy": true})));
        assert!(!looks_like_llama_health(404, &serde_json::Value::Null));
    }

    #[test]
    fn parses_process_port() {
        let v = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(process_port(&v(&["llama-server", "-m", "x", "--port", "9001"])), 9001);
        assert_eq!(process_port(&v(&["llama-server", "--port=9002"])), 9002);
        assert_eq!(process_port(&v(&["llama-server"])), 8080);
    }

    #[test]
    fn ports() {
        let l = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let busy = l.local_addr().unwrap().port();
        assert!(!port_free("127.0.0.1", busy));
        let next = next_free_port("127.0.0.1", busy).unwrap();
        assert!(next > busy);
        let ep = Endpoint { base_url: String::new(), api_key: String::new(), bind_host: "127.0.0.1".into(), port: busy };
        assert!(port_busy_msg(&ep).contains(&format!("Port {busy}")));
    }

    #[test]
    fn exit_descriptions() {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            let (t, h) = describe_exit(&Ok(ExitStatus::from_raw(1 << 8)));
            assert_eq!(t, "exit code 1");
            assert!(h.is_none());
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::ExitStatusExt;
            let (t, h) = describe_exit(&Ok(ExitStatus::from_raw(0xC000_0135)));
            assert_eq!(t, "exit code 0xC0000135");
            assert!(h.unwrap().contains("DLL"));
        }
    }

    /// End-to-end with a fake llama-server (python3). Unix only; skipped without python3.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fake_server_lifecycle() {
        use std::os::unix::fs::PermissionsExt;
        if std::process::Command::new("python3").arg("--version").output().is_err() {
            eprintln!("python3 not found; skipping");
            return;
        }
        let root = std::env::temp_dir().join(format!("nl-proc-{}", crate::settings::new_id()));
        let paths = Paths::from_root(root.clone());
        paths.ensure().unwrap();
        let exe = root.join("llama-server");
        std::fs::write(&exe, FAKE_SERVER).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let model = root.join("m.gguf");
        std::fs::write(&model, b"GGUF").unwrap();

        let port = next_free_port("127.0.0.1", 18080).unwrap();
        let settings = Settings {
            port,
            custom_server_exe: Some(exe.display().to_string()),
            api_key: "nl-testkey".into(),
            auto_restart: false,
            ..Settings::default()
        };
        let profile = Profile { model_path: model.display().to_string(), ..Profile::default() };

        let mgr = ServerManager::new(paths.clone());
        let st = mgr.inner.clone().start_inner(None, profile.clone(), settings.clone(), true, None).await.unwrap();
        assert_eq!(st.state, ServerState::Starting);
        assert!(mgr.pid().is_some());
        assert!(!st.command_line.unwrap().contains("nl-testkey"));
        mgr.wait_ready(Duration::from_secs(15)).await.unwrap();
        assert_eq!(mgr.status().state, ServerState::Ready);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(mgr.vram_allocations_mib(), Some(1100.0));
        assert!(mgr.log_tail(100).iter().any(|l| l.contains("server is listening")));
        assert!(!mgr.log_tail(1000).iter().any(|l| l.contains("nl-testkey")));

        let pid_before = mgr.pid();
        let missing = Profile { model_path: root.join("missing.gguf").display().to_string(), ..profile.clone() };
        assert!(mgr.inner.clone().start_inner(None, missing, settings.clone(), true, None).await.is_err());
        assert_eq!(mgr.pid(), pid_before);
        assert_eq!(mgr.status().state, ServerState::Ready);
        assert_eq!(mgr.connection(), Some((settings.base_url(), settings.api_key.clone())));

        // A second manager sees the running server and attaches instead of spawning.
        let other = ServerManager::new(paths.clone());
        let st2 = other.inner.clone().start_inner(None, profile.clone(), settings.clone(), true, None).await.unwrap();
        assert_eq!(st2.state, ServerState::Attached);
        assert!(st2.pid.is_none());
        other.wait_ready(Duration::from_secs(5)).await.unwrap();

        // Stop frees the port.
        let st = mgr.inner.clone().stop_public(None).await.unwrap();
        assert_eq!(st.state, ServerState::Stopped);
        assert!(mgr.pid().is_none());
        assert!(port_free("127.0.0.1", port));

        // A load-time OOM is surfaced as plain English and the state is Crashed.
        let crash = Profile { extra_args: "--fake-oom".into(), ..profile.clone() };
        mgr.inner.clone().start_inner(None, crash, settings.clone(), true, None).await.unwrap();
        let err = mgr.wait_ready(Duration::from_secs(15)).await.unwrap_err();
        assert!(err.starts_with("Out of GPU memory"), "{err}");
        assert_eq!(mgr.status().state, ServerState::Crashed);
        assert_eq!(mgr.status().crash_count, 1);

        // LAN mode without a key is refused.
        let lan = Settings { lan_mode: true, api_key: String::new(), ..settings.clone() };
        assert_eq!(
            mgr.inner.clone().start_inner(None, profile.clone(), lan, true, None).await.unwrap_err(),
            NO_KEY_LAN
        );

        // Something that isn't llama-server holding the port → error naming a free port.
        let squat = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
        let e = mgr.inner.clone().start_inner(None, profile.clone(), settings.clone(), true, None).await.unwrap_err();
        assert!(e.contains("already in use"), "{e}");
        drop(squat);

        // Shutdown kills the child.
        mgr.inner.clone().start_inner(None, profile.clone(), settings.clone(), true, None).await.unwrap();
        let pid = mgr.pid().unwrap();
        tokio::task::spawn_blocking({
            let inner = mgr.inner.clone();
            move || ServerManager { inner }.shutdown_blocking()
        })
        .await
        .unwrap();
        let alive = std::process::Command::new("kill").arg("-0").arg(pid.to_string()).status().unwrap().success();
        assert!(!alive, "child should be gone after shutdown");
        std::fs::remove_dir_all(root).ok();
    }

    #[cfg(unix)]
    const FAKE_SERVER: &str = r#"#!/usr/bin/env python3
import sys, time, json, http.server, threading
args = sys.argv[1:]
port = int(args[args.index("--port") + 1])
key = args[args.index("--api-key") + 1] if "--api-key" in args else ""
print("llama_model_loader: loaded meta data with 40 key-value pairs", flush=True)
if "--fake-oom" in args:
    print("ggml_backend_cuda_buffer_type_alloc_buffer: allocating 99999.00 MiB on device 0: cudaMalloc failed: out of memory", file=sys.stderr, flush=True)
    print("llama_model_load: error loading model: unable to allocate CUDA0 buffer", file=sys.stderr, flush=True)
    sys.exit(1)
start = time.time()
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        if self.path == "/health":
            if time.time() - start < 1.0:
                self.send_response(503); body = {"error": {"code": 503, "message": "Loading model", "type": "unavailable_error"}}
            else:
                self.send_response(200); body = {"status": "ok"}
        elif self.path == "/props":
            self.send_response(200); body = {"model_path": "/m.gguf", "total_slots": 1}
        else:
            self.send_response(404); body = {}
        self.send_header("Content-Type", "application/json"); self.end_headers()
        self.wfile.write(json.dumps(body).encode())
srv = http.server.ThreadingHTTPServer(("127.0.0.1", port), H)
print("load_tensors:        CUDA0 model buffer size =  1000.00 MiB", flush=True)
print("llama_context:      CUDA0 compute buffer size =   100.00 MiB", flush=True)
print("auth key is " + key, flush=True)
print("main: server is listening on http://127.0.0.1:%d - starting the main loop" % port, file=sys.stderr, flush=True)
srv.serve_forever()
"#;
}
