//! Rolling log file + in-memory tail + log parsing.
//!
//! - `server.log` in `paths.logs`, rotated at 10 MB, keeping `server.log.1..3`.
//! - Ring of the last 2000 lines for `server_log_tail`.
//! - Pending lines are drained by a ≤4 Hz flusher that emits `server-log`.
//! - [`parse_line`] extracts load progress, readiness, CUDA allocations and errors.

use parking_lot::Mutex;
use regex::Regex;
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const RING_LINES: usize = 2000;
pub const ROTATE_BYTES: u64 = 10 * 1024 * 1024;
pub const KEEP_ROTATED: usize = 3;
const MAX_LINE_CHARS: usize = 4000;

pub const OOM_MSG: &str = "Out of GPU memory while loading. Try the Limits tab: lower context, quantize KV, or move more experts to CPU.";

/// Something interesting found in one log line.
#[derive(Debug, Clone, PartialEq)]
pub enum LineInfo {
    /// Approximate 0..1 load progress.
    Progress(f32),
    /// A CUDA device buffer allocation, in MiB.
    CudaAlloc(f64),
    /// "server is listening" / "model loaded".
    Listening,
    /// A plain-English error, with a priority (higher wins over earlier lower ones).
    Error { priority: u8, message: String },
}

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).expect("valid regex"))
}

/// Parse one llama-server log line.
pub fn parse_line(line: &str) -> Option<LineInfo> {
    static CUDA: OnceLock<Regex> = OnceLock::new();
    static UNKNOWN_ARG: OnceLock<Regex> = OnceLock::new();
    static BIND: OnceLock<Regex> = OnceLock::new();
    let lower = line.to_ascii_lowercase();

    // Errors first.
    if (lower.contains("out of memory") && (lower.contains("cuda") || lower.contains("alloc")))
        || lower.contains("cudamalloc failed")
        || lower.contains("failed to allocate")
        || (lower.contains("alloc_buffer") && lower.contains("failed"))
        || lower.contains("unable to allocate")
    {
        return Some(LineInfo::Error { priority: 90, message: OOM_MSG.into() });
    }
    if lower.contains("unknown argument") || lower.contains("invalid argument") || lower.contains("unrecognized argument") {
        let arg = re(&UNKNOWN_ARG, r"(?i)(?:unknown|invalid|unrecognized) argument:?\s*(\S+)")
            .captures(line)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().trim_matches(|c| c == '\'' || c == '"').to_string());
        let message = match arg {
            Some(a) => format!(
                "This llama.cpp build doesn't accept the argument {a}. Check the profile's extra args, or update the engine in Settings → Engine."
            ),
            None => "This llama.cpp build rejected one of the launch arguments. Check the profile's extra args, or update the engine in Settings → Engine.".into(),
        };
        return Some(LineInfo::Error { priority: 80, message });
    }
    if lower.contains("unknown model architecture") {
        return Some(LineInfo::Error {
            priority: 75,
            message: "This llama.cpp build doesn't support the model's architecture. Update the engine in Settings → Engine.".into(),
        });
    }
    if lower.contains("couldn't bind") || lower.contains("address already in use") || re(&BIND, r"(?i)failed to (?:bind|listen)").is_match(line) {
        return Some(LineInfo::Error {
            priority: 70,
            message: "llama-server couldn't open its port; another program is probably using it. Pick another port in Settings.".into(),
        });
    }
    if lower.contains("error loading model") || lower.contains("failed to load model") || lower.contains("unable to load model") {
        return Some(LineInfo::Error {
            priority: 60,
            message: "llama-server couldn't load the model. The file may be incomplete or corrupt, or need a newer llama.cpp build.".into(),
        });
    }

    if let Some(c) = re(&CUDA, r"\bCUDA\d+\s+(?:[A-Za-z_]+\s+)?buffer size\s*=\s*([0-9]+(?:\.[0-9]+)?)\s*MiB").captures(line) {
        if let Ok(v) = c[1].parse::<f64>() {
            return Some(LineInfo::CudaAlloc(v));
        }
    }

    if lower.contains("server is listening") || lower.contains("model loaded") || lower.contains("all slots are idle") {
        return Some(LineInfo::Listening);
    }

    // Coarse load stages (llama.cpp prints no numeric progress to the log).
    let stage = if lower.contains("warming up") {
        0.9
    } else if lower.contains("kv buffer size") || lower.contains("kv_cache") || lower.contains("compute buffer size") {
        0.8
    } else if lower.contains("llama_context:") || lower.contains("llama_init_from_model") || lower.contains("llama_new_context") {
        0.7
    } else if lower.contains("load_tensors:") || lower.contains("llm_load_tensors") {
        0.4
    } else if lower.contains("print_info:") || lower.contains("llm_load_print_meta") {
        0.2
    } else if lower.contains("llama_model_loader:") || lower.contains("loading model") {
        0.1
    } else {
        return None;
    };
    Some(LineInfo::Progress(stage))
}

/// Size-rotated log file.
struct RollingFile {
    path: PathBuf,
    writer: Option<BufWriter<File>>,
    size: u64,
}

impl RollingFile {
    fn open(path: PathBuf) -> Self {
        let mut f = RollingFile { path, writer: None, size: 0 };
        f.reopen();
        f
    }

    fn reopen(&mut self) {
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match OpenOptions::new().create(true).append(true).open(&self.path) {
            Ok(file) => {
                self.size = file.metadata().map(|m| m.len()).unwrap_or(0);
                self.writer = Some(BufWriter::new(file));
            }
            Err(_) => self.writer = None,
        }
    }

    fn write_line(&mut self, line: &str) {
        if self.size >= ROTATE_BYTES {
            self.rotate();
        }
        if let Some(w) = self.writer.as_mut() {
            if w.write_all(line.as_bytes()).and_then(|_| w.write_all(b"\n")).is_ok() {
                self.size += line.len() as u64 + 1;
            }
        }
    }

    fn rotate(&mut self) {
        if let Some(mut w) = self.writer.take() {
            let _ = w.flush();
        }
        rotate_files(&self.path, KEEP_ROTATED);
        self.reopen();
    }

    fn flush(&mut self) {
        if let Some(w) = self.writer.as_mut() {
            let _ = w.flush();
        }
    }
}

fn rotated(path: &Path, i: usize) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(format!(".{i}"));
    PathBuf::from(s)
}

/// server.log → server.log.1 → … → server.log.{keep}; the oldest is dropped.
fn rotate_files(path: &Path, keep: usize) {
    let _ = std::fs::remove_file(rotated(path, keep));
    for i in (1..keep).rev() {
        let _ = std::fs::rename(rotated(path, i), rotated(path, i + 1));
    }
    let _ = std::fs::rename(path, rotated(path, 1));
}

/// Parsed facts since the last start.
#[derive(Debug, Default, Clone)]
pub struct ParseState {
    pub cuda_mib: Option<f64>,
    pub load_progress: Option<f32>,
    pub listening: bool,
    pub error: Option<(u8, String)>,
}

impl ParseState {
    /// Fold one parsed line in. Returns true if `load_progress` or `error` changed.
    pub fn apply(&mut self, info: LineInfo) -> bool {
        match info {
            LineInfo::Progress(p) => {
                if self.load_progress.is_none_or(|old| p > old) {
                    self.load_progress = Some(p);
                    return true;
                }
                false
            }
            LineInfo::CudaAlloc(v) => {
                self.cuda_mib = Some(self.cuda_mib.unwrap_or(0.0) + v);
                false
            }
            LineInfo::Listening => {
                self.listening = true;
                false
            }
            LineInfo::Error { priority, message } => {
                if self.error.as_ref().is_none_or(|(p, _)| priority > *p) {
                    self.error = Some((priority, message));
                    return true;
                }
                false
            }
        }
    }
}

struct Inner {
    ring: VecDeque<String>,
    pending: Vec<String>,
    file: Option<RollingFile>,
    parse: ParseState,
    redact: Vec<String>,
}

pub struct LogSink {
    dir: PathBuf,
    inner: Mutex<Inner>,
}

impl LogSink {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            inner: Mutex::new(Inner {
                ring: VecDeque::with_capacity(RING_LINES),
                pending: Vec::new(),
                file: None,
                parse: ParseState::default(),
                redact: Vec::new(),
            }),
        }
    }

    pub fn file_path(&self) -> PathBuf {
        self.dir.join("server.log")
    }

    /// Reset parse state for a new run; secrets in `redact` are masked in every line.
    pub fn begin_run(&self, redact: Vec<String>) {
        let mut g = self.inner.lock();
        g.parse = ParseState::default();
        g.redact = redact.into_iter().filter(|s| s.len() >= 4).collect();
        if g.file.is_none() {
            g.file = Some(RollingFile::open(self.file_path()));
        }
    }

    /// Append a raw line (from the child). Returns what it parsed to, if anything.
    pub fn push(&self, raw: &str) -> Option<LineInfo> {
        let mut line: String = raw.trim_end_matches(['\r', '\n']).to_string();
        if line.chars().count() > MAX_LINE_CHARS {
            line = line.chars().take(MAX_LINE_CHARS).collect::<String>() + " …";
        }
        let mut g = self.inner.lock();
        for secret in &g.redact {
            if line.contains(secret.as_str()) {
                line = line.replace(secret.as_str(), "***");
            }
        }
        let info = parse_line(&line);
        if let Some(i) = info.clone() {
            g.parse.apply(i);
        }
        if g.file.is_none() {
            g.file = Some(RollingFile::open(self.file_path()));
        }
        if let Some(f) = g.file.as_mut() {
            f.write_line(&line);
        }
        if g.ring.len() >= RING_LINES {
            g.ring.pop_front();
        }
        g.ring.push_back(line.clone());
        g.pending.push(line);
        info
    }

    /// Lines written by Neurallama itself (prefixed so they stand out). Never parsed.
    pub fn note(&self, msg: &str) {
        let line = format!("[neurallama {}] {msg}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
        let mut g = self.inner.lock();
        if let Some(f) = g.file.as_mut() {
            f.write_line(&line);
        }
        if g.ring.len() >= RING_LINES {
            g.ring.pop_front();
        }
        g.ring.push_back(line.clone());
        g.pending.push(line);
    }

    /// Take pending lines for the `server-log` event and flush the file.
    pub fn drain_pending(&self) -> Vec<String> {
        let mut g = self.inner.lock();
        if let Some(f) = g.file.as_mut() {
            f.flush();
        }
        std::mem::take(&mut g.pending)
    }

    pub fn tail(&self, n: usize) -> Vec<String> {
        let g = self.inner.lock();
        let skip = g.ring.len().saturating_sub(n);
        g.ring.iter().skip(skip).cloned().collect()
    }

    pub fn parse_state(&self) -> ParseState {
        self.inner.lock().parse.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cuda_allocations() {
        assert_eq!(parse_line("load_tensors:        CUDA0 model buffer size = 15123.50 MiB"), Some(LineInfo::CudaAlloc(15123.5)));
        assert_eq!(parse_line("llama_kv_cache_unified:      CUDA0 KV buffer size =  2048.00 MiB"), Some(LineInfo::CudaAlloc(2048.0)));
        assert_eq!(parse_line("llama_context:      CUDA1 compute buffer size =   300.01 MiB"), Some(LineInfo::CudaAlloc(300.01)));
        assert_eq!(parse_line("llama_memory_recurrent:      CUDA0 RS buffer size =    62.81 MiB"), Some(LineInfo::CudaAlloc(62.81)));
        // Host buffers are system RAM, not VRAM.
        assert_ne!(parse_line("load_tensors:   CPU_Mapped model buffer size =  4000.00 MiB").map(|i| matches!(i, LineInfo::CudaAlloc(_))), Some(true));
        assert_ne!(parse_line("llama_context:  CUDA_Host  output buffer size =     0.58 MiB").map(|i| matches!(i, LineInfo::CudaAlloc(_))), Some(true));
    }

    #[test]
    fn parses_errors() {
        let oom = [
            "ggml_backend_cuda_buffer_type_alloc_buffer: allocating 20000.00 MiB on device 0: cudaMalloc failed: out of memory",
            "alloc_tensor_range: failed to allocate CUDA0 buffer of size 2097152000",
            "llama_init_from_model: failed to allocate buffer for kv cache",
            "CUDA error: out of memory",
        ];
        for l in oom {
            assert_eq!(parse_line(l), Some(LineInfo::Error { priority: 90, message: OOM_MSG.into() }), "{l}");
        }
        match parse_line("error: unknown argument: --bogus-flag") {
            Some(LineInfo::Error { priority: 80, message }) => assert!(message.contains("--bogus-flag")),
            other => panic!("{other:?}"),
        }
        assert!(matches!(parse_line("llama_model_load: error loading model: tensor 'blk.0' data is not within the file bounds"), Some(LineInfo::Error { priority: 60, .. })));
        assert!(matches!(parse_line("couldn't bind HTTP server socket, hostname: 127.0.0.1, port: 8080"), Some(LineInfo::Error { priority: 70, .. })));
    }

    #[test]
    fn parses_ready_and_progress() {
        assert_eq!(parse_line("main: server is listening on http://127.0.0.1:8080 - starting the main loop"), Some(LineInfo::Listening));
        assert_eq!(parse_line("srv  load_model: model loaded"), Some(LineInfo::Listening));
        assert_eq!(parse_line("llama_model_loader: loaded meta data with 40 key-value pairs"), Some(LineInfo::Progress(0.1)));
        assert_eq!(parse_line("load_tensors: offloaded 65/65 layers to GPU"), Some(LineInfo::Progress(0.4)));
        assert_eq!(parse_line("common_init_from_params: warming up the model with an empty run"), Some(LineInfo::Progress(0.9)));
        assert_eq!(parse_line("srv  update_slots: all good"), None);
    }

    #[test]
    fn parse_state_priority_and_sum() {
        let mut s = ParseState::default();
        s.apply(LineInfo::CudaAlloc(100.0));
        s.apply(LineInfo::CudaAlloc(50.5));
        assert_eq!(s.cuda_mib, Some(150.5));
        assert!(s.apply(LineInfo::Error { priority: 90, message: "oom".into() }));
        assert!(!s.apply(LineInfo::Error { priority: 60, message: "load".into() }));
        assert_eq!(s.error.as_ref().unwrap().1, "oom");
        assert!(s.apply(LineInfo::Progress(0.4)));
        assert!(!s.apply(LineInfo::Progress(0.1)));
    }

    #[test]
    fn sink_ring_redaction_and_rotation() {
        let dir = std::env::temp_dir().join(format!("nl-logs-{}", crate::settings::new_id()));
        let sink = LogSink::new(dir.clone());
        sink.begin_run(vec!["nl-supersecret".into()]);
        sink.push("Authorization: Bearer nl-supersecret\r\n");
        assert_eq!(sink.tail(1), ["Authorization: Bearer ***"]);
        for i in 0..(RING_LINES + 10) {
            sink.push(&format!("line {i}"));
        }
        assert_eq!(sink.tail(5000).len(), RING_LINES);
        assert_eq!(sink.tail(1), [format!("line {}", RING_LINES + 9)]);
        assert_eq!(sink.drain_pending().len(), RING_LINES + 11);
        assert!(sink.drain_pending().is_empty());
        let text = std::fs::read_to_string(sink.file_path()).unwrap();
        assert!(!text.contains("supersecret"));

        // Rotation keeps at most KEEP_ROTATED old files.
        let p = dir.join("r.log");
        for i in 0..5 {
            std::fs::write(&p, format!("gen {i}")).unwrap();
            rotate_files(&p, KEEP_ROTATED);
        }
        assert_eq!(std::fs::read_to_string(rotated(&p, 1)).unwrap(), "gen 4");
        assert_eq!(std::fs::read_to_string(rotated(&p, 3)).unwrap(), "gen 2");
        assert!(!rotated(&p, 4).exists());
        std::fs::remove_dir_all(dir).ok();
    }
}
