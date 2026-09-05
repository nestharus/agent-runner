//! Opt-in aggregate timings for the PTY TUI, without recording terminal contents.
//!
//! Set `OULIPOLY_TUI_PROFILE` to an unused absolute path. The final shared owner
//! writes one JSON summary there on shutdown; existing files are never replaced.

use serde::Serialize;
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const BUCKET_UPPER_NS: [u64; 6] = [
    100_000,
    1_000_000,
    5_000_000,
    16_000_000,
    50_000_000,
    100_000_000,
];

#[derive(Clone, Default)]
pub(super) struct Profile(Option<Arc<Shared>>);

struct Shared {
    path: PathBuf,
    metrics: Mutex<Metrics>,
}

#[derive(Default, Serialize)]
struct Metrics {
    timings: BTreeMap<&'static str, Timing>,
    counters: BTreeMap<&'static str, u64>,
}

#[derive(Default, Serialize)]
struct Timing {
    count: u64,
    total_ns: u64,
    max_ns: u64,
    /// Noncumulative counts: six exclusive upper bounds, then an overflow bin.
    buckets: [u64; 7],
}

pub(super) struct MeasureGuard(Option<Measurement>);

struct Measurement {
    shared: Arc<Shared>,
    stage: &'static str,
    started: Instant,
}

impl Profile {
    pub(super) fn from_env() -> Self {
        Self::from_path(std::env::var_os("OULIPOLY_TUI_PROFILE").map(PathBuf::from))
    }

    fn from_path(path: Option<PathBuf>) -> Self {
        Self(path.filter(|path| path.is_absolute()).map(|path| {
            Arc::new(Shared {
                path,
                metrics: Mutex::new(Metrics::default()),
            })
        }))
    }

    /// Only static stage names are accepted; never pass terminal/user content.
    pub(super) fn measure(&self, stage: &'static str) -> MeasureGuard {
        MeasureGuard(self.0.as_ref().map(|shared| Measurement {
            shared: Arc::clone(shared),
            stage,
            started: Instant::now(),
        }))
    }

    pub(super) fn record_count(&self, name: &'static str, count: u64) {
        if let Some(shared) = &self.0 {
            if let Ok(mut metrics) = shared.metrics.lock() {
                let total = metrics.counters.entry(name).or_default();
                *total = total.saturating_add(count);
            }
        }
    }
}

impl Shared {
    fn record_duration(&self, stage: &'static str, duration: Duration) {
        let ns = u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
        if let Ok(mut metrics) = self.metrics.lock() {
            let timing = metrics.timings.entry(stage).or_default();
            timing.count = timing.count.saturating_add(1);
            timing.total_ns = timing.total_ns.saturating_add(ns);
            timing.max_ns = timing.max_ns.max(ns);
            let bucket = BUCKET_UPPER_NS.partition_point(|upper| ns >= *upper);
            timing.buckets[bucket] = timing.buckets[bucket].saturating_add(1);
        }
    }
}

impl Drop for MeasureGuard {
    fn drop(&mut self) {
        if let Some(measurement) = &self.0 {
            measurement
                .shared
                .record_duration(measurement.stage, measurement.started.elapsed());
        }
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        let metrics = self
            .metrics
            .get_mut()
            .unwrap_or_else(|error| error.into_inner());
        let summary = serde_json::json!({
            "schema_version": 1,
            "bucket_upper_ns_exclusive": BUCKET_UPPER_NS,
            "timings": metrics.timings,
            "counters": metrics.counters,
        });
        // Serialize first so a serialization failure cannot leave an empty file.
        // Profiling must never emit errors or any extra bytes to the terminal.
        let Ok(mut bytes) = serde_json::to_vec_pretty(&summary) else {
            return;
        };
        bytes.push(b'\n');
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        if let Ok(mut file) = options.open(&self.path) {
            let _ = file.write_all(&bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_accumulate_only_aggregate_metrics_and_write_on_last_drop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profile.json");
        let profile = Profile::from_path(Some(path.clone()));
        let worker_profile = profile.clone();
        std::thread::spawn(move || {
            worker_profile.record_count("pty_bytes", 10);
            worker_profile
                .0
                .as_ref()
                .unwrap()
                .record_duration("parse", Duration::from_micros(99));
        })
        .join()
        .unwrap();
        profile.record_count("pty_bytes", 20);
        profile
            .0
            .as_ref()
            .unwrap()
            .record_duration("parse", Duration::from_millis(16));

        let guard = profile.measure("draw");
        drop(profile);
        assert!(!path.exists(), "an active guard retains shared metrics");
        drop(guard);

        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(json["counters"], serde_json::json!({ "pty_bytes": 30 }));
        assert_eq!(
            json["timings"]["parse"],
            serde_json::json!({
                "count": 2,
                "total_ns": 16_099_000,
                "max_ns": 16_000_000,
                "buckets": [1, 0, 0, 0, 1, 0, 0],
            })
        );
        assert_eq!(json["timings"]["draw"]["count"], 1);
        // The schema permits aggregate numbers and static stage/counter labels
        // only: no prompt, command, provider output, environment or session data.
        let object = json.as_object().unwrap();
        assert_eq!(object.len(), 4);
        for key in [
            "bucket_upper_ns_exclusive",
            "counters",
            "schema_version",
            "timings",
        ] {
            assert!(object.contains_key(key));
        }
    }

    #[test]
    fn existing_output_is_never_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profile.json");
        std::fs::write(&path, b"previous profile").unwrap();
        let profile = Profile::from_path(Some(path.clone()));
        profile.record_count("pty_bytes", 42);
        drop(profile);
        assert_eq!(std::fs::read(path).unwrap(), b"previous profile");
    }

    #[test]
    fn disabled_and_relative_paths_do_not_create_measurements() {
        for profile in [
            Profile::default(),
            Profile::from_path(Some(PathBuf::from("relative.json"))),
        ] {
            assert!(profile.measure("draw").0.is_none());
            profile.record_count("pty_bytes", 42);
            assert!(profile.0.is_none());
        }
    }
}
