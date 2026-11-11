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
    /// Cancellation also interrupts requests that aren't producing any data.
    pub async fn until_cancelled(&self) {
        while !self.cancelled() {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }
    pub async fn until_paused(&self) {
        while !self.paused() {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
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

    pub fn cancel_server_work(&self) {
        for (handle, progress) in self.inner.lock().values() {
            if matches!(progress.kind.as_str(), "autotune" | "benchmark") {
                handle.cancel.store(true, Ordering::Relaxed);
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stopping_server_cancels_tuning_and_benchmarks_only() {
        let jobs = Jobs::default();
        let (_, tune) = jobs.create("autotune", "Tune");
        let (_, bench) = jobs.create("benchmark", "Benchmark");
        let (_, download) = jobs.create("download", "Model");
        jobs.cancel_server_work();
        assert!(tune.cancelled());
        assert!(bench.cancelled());
        assert!(!download.cancelled());
    }
}
