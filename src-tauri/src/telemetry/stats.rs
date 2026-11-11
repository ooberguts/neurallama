//! Per-model lifetime stats, persisted to `stats.json`.
//!
//! Model key = the served model's file name. Tokens are generated tokens (from counter
//! deltas, or slot progress when the build has no counters); tok/s stats use only samples
//! taken while a slot was generating.

use super::ModelStats;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub const SAVE_EVERY: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Record {
    pub tokens: u64,
    pub requests: u64,
    pub tps_sum: f64,
    pub tps_samples: u64,
    pub peak_tps: f64,
    pub low_tps: f64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct FileFormat {
    version: u32,
    models: BTreeMap<String, Record>,
}

pub struct StatsStore {
    file: PathBuf,
    models: BTreeMap<String, Record>,
    dirty: bool,
    last_save: Instant,
}

impl StatsStore {
    pub fn load(file: PathBuf) -> Self {
        let models = std::fs::read_to_string(&file)
            .ok()
            .and_then(|t| serde_json::from_str::<FileFormat>(&t).ok())
            .map(|f| f.models)
            .unwrap_or_default();
        Self { file, models, dirty: false, last_save: Instant::now() }
    }

    /// Add one poll's worth of activity. `tps` is a sample only when generating.
    pub fn record(&mut self, model: &str, tokens: u64, requests: u64, tps: Option<f64>) {
        let tps = tps.filter(|t| t.is_finite() && *t > 0.0);
        if tokens == 0 && requests == 0 && tps.is_none() {
            return;
        }
        let r = self.models.entry(model.to_string()).or_default();
        r.tokens += tokens;
        r.requests += requests;
        if let Some(t) = tps {
            r.tps_sum += t;
            r.tps_samples += 1;
            r.peak_tps = r.peak_tps.max(t);
            r.low_tps = if r.low_tps <= 0.0 { t } else { r.low_tps.min(t) };
        }
        self.dirty = true;
    }

    pub fn list(&self) -> Vec<ModelStats> {
        self.models
            .iter()
            .map(|(model, r)| ModelStats {
                model: model.clone(),
                tokens: r.tokens,
                requests: r.requests,
                avg_tps: if r.tps_samples > 0 { r.tps_sum / r.tps_samples as f64 } else { 0.0 },
                peak_tps: r.peak_tps,
                low_tps: r.low_tps,
            })
            .collect()
    }

    /// Reset one model, or everything with None.
    pub fn reset(&mut self, model: Option<&str>) {
        match model {
            Some(m) => {
                self.models.remove(m);
            }
            None => self.models.clear(),
        }
        self.dirty = true;
        self.flush();
    }

    /// Save if dirty and the save interval has passed.
    pub fn maybe_save(&mut self) {
        if self.dirty && self.last_save.elapsed() >= SAVE_EVERY {
            self.flush();
        }
    }

    /// Save now if anything changed.
    pub fn flush(&mut self) {
        if !self.dirty {
            return;
        }
        let f = FileFormat { version: 1, models: self.models.clone() };
        match serde_json::to_vec_pretty(&f) {
            Ok(bytes) => match crate::settings::write_atomic(&self.file, &bytes) {
                Ok(()) => self.dirty = false,
                Err(e) => log::warn!("could not save stats: {e}"),
            },
            Err(e) => log::warn!("could not serialize stats: {e}"),
        }
        self.last_save = Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_persists() {
        let dir = std::env::temp_dir().join(format!("nl-stats-{}", crate::settings::new_id()));
        let file = dir.join("stats.json");
        let mut s = StatsStore::load(file.clone());
        assert!(s.list().is_empty());
        s.record("a.gguf", 0, 0, None); // no-op
        assert!(s.list().is_empty());
        s.record("a.gguf", 100, 1, Some(50.0));
        s.record("a.gguf", 50, 0, Some(30.0));
        s.record("a.gguf", 10, 0, Some(f64::NAN));
        s.record("b.gguf", 5, 1, None);
        let l = s.list();
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].tokens, 160);
        assert_eq!(l[0].requests, 1);
        assert_eq!(l[0].avg_tps, 40.0);
        assert_eq!(l[0].peak_tps, 50.0);
        assert_eq!(l[0].low_tps, 30.0);
        assert_eq!(l[1].avg_tps, 0.0);
        s.maybe_save(); // interval not reached
        assert!(!file.exists());
        s.flush();
        let back = StatsStore::load(file.clone());
        assert_eq!(back.list().len(), 2);
        s.reset(Some("b.gguf"));
        assert_eq!(StatsStore::load(file.clone()).list().len(), 1);
        s.reset(None);
        assert!(StatsStore::load(file).list().is_empty());
        std::fs::remove_dir_all(dir).ok();
    }
}
