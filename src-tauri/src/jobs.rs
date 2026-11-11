//! Registry of long-running, cancellable background jobs (downloads, engine installs,
//! auto-tune, benchmark). Every job reports through one event: `job-progress`.

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

pub const EVENT: &str = "job-progress";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    Queued,
    Running,
    Paused,
    Done,
    Error,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobProgress {
    pub job_id: String,
    /// "download" | "engine" | "autotune" | "benchmark"
    pub kind: String,
    pub label: String,
    pub done: u64,
    pub total: Option<u64>,
    pub state: JobState,
    pub message: Option<String>,
}

#[derive(Clone, Default)]
pub struct JobHandle {
    pub cancel: Arc<AtomicBool>,
    pub pause: Arc<AtomicBool>,
}

impl JobHandle {
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    pub fn paused(&self) -> bool {
        self.pause.load(Ordering::Relaxed)
    }
}

#[derive(Default)]
pub struct Jobs {
    inner: Mutex<HashMap<String, (JobHandle, JobProgress)>>,
}

impl Jobs {
    /// Register a new job and return its id + handle.
    pub fn create(&self, kind: &str, label: &str) -> (String, JobHandle) {
        let id = crate::settings::new_id();
        let handle = JobHandle::default();
        let progress = JobProgress {
            job_id: id.clone(),
            kind: kind.into(),
            label: label.into(),
            done: 0,
            total: None,
            state: JobState::Queued,
            message: None,
        };
        self.inner.lock().insert(id.clone(), (handle.clone(), progress));
        (id, handle)
    }

    pub fn handle(&self, id: &str) -> Option<JobHandle> {
        self.inner.lock().get(id).map(|(h, _)| h.clone())
    }

    /// Store and emit a progress update.
    pub fn update(&self, app: &AppHandle, p: JobProgress) {
        if let Some(entry) = self.inner.lock().get_mut(&p.job_id) {
            entry.1 = p.clone();
        }
        let _ = app.emit(EVENT, &p);
    }

    pub fn get(&self, id: &str) -> Option<JobProgress> {
        self.inner.lock().get(id).map(|(_, p)| p.clone())
    }

    pub fn list(&self, kind: Option<&str>) -> Vec<JobProgress> {
        self.inner
            .lock()
            .values()
            .filter(|(_, p)| kind.is_none_or(|k| p.kind == k))
            .map(|(_, p)| p.clone())
            .collect()
    }

    pub fn cancel(&self, id: &str) -> bool {
        match self.inner.lock().get(id) {
            Some((h, _)) => {
                h.cancel.store(true, Ordering::Relaxed);
                true
            }
            None => false,
        }
    }

    pub fn set_paused(&self, id: &str, paused: bool) -> bool {
        match self.inner.lock().get(id) {
            Some((h, _)) => {
                h.pause.store(paused, Ordering::Relaxed);
                true
            }
            None => false,
        }
    }
}
