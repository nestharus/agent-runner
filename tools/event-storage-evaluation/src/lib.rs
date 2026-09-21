mod framed;
mod lsm;
mod model;
mod sqlite_candidates;

use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::Instant;

pub type EvalResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct EvaluationConfig {
    pub records: usize,
    pub producers: usize,
    pub batch_size: usize,
    pub repetitions: usize,
    pub payload_bytes: usize,
}

impl Default for EvaluationConfig {
    fn default() -> Self {
        Self {
            records: 4_096,
            producers: 4,
            batch_size: 32,
            repetitions: 5,
            payload_bytes: 256,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct QueryResult {
    pub records: usize,
    pub body_bytes: u64,
    pub checksum: u64,
}

impl QueryResult {
    pub fn observe(&mut self, body: &[u8]) {
        self.records += 1;
        self.body_bytes = self.body_bytes.saturating_add(body.len() as u64);
        self.checksum = self.checksum.wrapping_add(fnv1a(body));
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CandidateObservation {
    pub candidate: String,
    pub append_ms: f64,
    pub reopen_ms: f64,
    pub full_scan_ms: f64,
    pub time_query_ms: f64,
    pub trace_query_ms: f64,
    pub full_scan: QueryResult,
    pub time_query: QueryResult,
    pub trace_query: QueryResult,
    pub bytes_on_disk: u64,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CandidateSummary {
    pub candidate: String,
    pub append_ms_median: f64,
    pub append_events_per_second_median: f64,
    pub reopen_ms_median: f64,
    pub full_scan_ms_median: f64,
    pub time_query_ms_median: f64,
    pub trace_query_ms_median: f64,
    pub bytes_on_disk_median: u64,
}

#[derive(Debug, Serialize)]
pub struct EvaluationReport {
    pub schema: String,
    pub warning: String,
    pub environment: Environment,
    pub config: EvaluationConfig,
    pub expected: ExpectedResults,
    pub observations: Vec<CandidateObservation>,
    pub summaries: Vec<CandidateSummary>,
}

#[derive(Debug, Serialize)]
pub struct Environment {
    pub os: String,
    pub architecture: String,
    pub kernel: Option<String>,
    pub fjall_version: String,
    pub rusqlite_version: String,
}

#[derive(Debug, Serialize)]
pub struct ExpectedResults {
    pub full_scan_records: usize,
    pub time_query_records: usize,
    pub trace_query_records: usize,
    pub full_scan_checksum: u64,
    pub time_query_checksum: u64,
    pub trace_query_checksum: u64,
}

pub fn run_evaluation(root: &Path, config: EvaluationConfig) -> EvalResult<EvaluationReport> {
    validate_config(config)?;
    if root.exists() {
        return Err(format!(
            "evaluation root already exists; refusing to reuse or remove {}",
            root.display()
        )
        .into());
    }
    fs::create_dir_all(root)?;
    make_private(root)?;

    let events = model::fixture_events(config.records, config.producers, config.payload_bytes);
    let time_bounds = model::time_bounds(config.records);
    let trace_id = model::target_trace();
    let expected = expected_results(&events, time_bounds, trace_id)?;
    let candidates = [
        "partitioned_sqlite",
        "wal_write_broker",
        "framed_shards_side_index",
        "embedded_lsm_fjall",
    ];
    let mut observations = Vec::new();
    for repetition in 0..config.repetitions {
        for offset in 0..candidates.len() {
            let candidate = candidates[(offset + repetition) % candidates.len()];
            let candidate_root = root.join(format!("run-{repetition:02}-{candidate}"));
            let observation = match candidate {
                "partitioned_sqlite" => sqlite_candidates::partitioned_sqlite(
                    &candidate_root,
                    &events,
                    config.producers,
                    config.batch_size,
                    time_bounds,
                    trace_id,
                )?,
                "wal_write_broker" => sqlite_candidates::wal_broker_sqlite(
                    &candidate_root,
                    &events,
                    config.producers,
                    config.batch_size,
                    time_bounds,
                    trace_id,
                )?,
                "framed_shards_side_index" => framed::framed_shards(
                    &candidate_root,
                    &events,
                    config.producers,
                    config.batch_size,
                    time_bounds,
                    trace_id,
                )?,
                "embedded_lsm_fjall" => lsm::embedded_lsm(
                    &candidate_root,
                    &events,
                    config.producers,
                    config.batch_size,
                    time_bounds,
                    trace_id,
                )?,
                _ => unreachable!(),
            };
            validate_observation(&observation, &expected)?;
            observations.push(observation);
        }
    }
    let summaries = summarize(&observations, config.records);
    Ok(EvaluationReport {
        schema: "age375.event-storage-evaluation.v1".to_string(),
        warning: "bounded synthetic smoke fixture; not AGE-353 stress or a production SLO"
            .to_string(),
        environment: Environment {
            os: std::env::consts::OS.to_string(),
            architecture: std::env::consts::ARCH.to_string(),
            kernel: kernel_version(),
            fjall_version: "3.1.10".to_string(),
            rusqlite_version: rusqlite::version().to_string(),
        },
        config,
        expected,
        observations,
        summaries,
    })
}

fn validate_config(config: EvaluationConfig) -> EvalResult<()> {
    if config.records == 0
        || config.producers == 0
        || config.batch_size == 0
        || config.repetitions == 0
    {
        return Err("records, producers, batch size, and repetitions must be positive".into());
    }
    if config.records < config.producers {
        return Err("records must be at least the number of producers".into());
    }
    if config.producers > 64 || config.records > 100_000 || config.payload_bytes > 65_536 {
        return Err("fixture exceeds the bounded smoke-scale guardrails".into());
    }
    Ok(())
}

fn expected_results(
    events: &[model::Event],
    time_bounds: (u64, u64),
    trace_id: [u8; 16],
) -> EvalResult<ExpectedResults> {
    let mut full = QueryResult::default();
    let mut time = QueryResult::default();
    let mut trace = QueryResult::default();
    for event in events {
        let body = event.encoded()?;
        full.observe(&body);
        if event.timestamp_micros >= time_bounds.0 && event.timestamp_micros < time_bounds.1 {
            time.observe(&body);
        }
        if event.trace_id == trace_id {
            trace.observe(&body);
        }
    }
    Ok(ExpectedResults {
        full_scan_records: full.records,
        time_query_records: time.records,
        trace_query_records: trace.records,
        full_scan_checksum: full.checksum,
        time_query_checksum: time.checksum,
        trace_query_checksum: trace.checksum,
    })
}

fn validate_observation(
    observation: &CandidateObservation,
    expected: &ExpectedResults,
) -> EvalResult<()> {
    let checks = [
        (
            "full scan",
            &observation.full_scan,
            expected.full_scan_records,
            expected.full_scan_checksum,
        ),
        (
            "time query",
            &observation.time_query,
            expected.time_query_records,
            expected.time_query_checksum,
        ),
        (
            "trace query",
            &observation.trace_query,
            expected.trace_query_records,
            expected.trace_query_checksum,
        ),
    ];
    for (label, actual, expected_records, expected_checksum) in checks {
        if actual.records != expected_records || actual.checksum != expected_checksum {
            return Err(format!(
                "{} returned incorrect {label}: records {} != {expected_records} or checksum {} != {expected_checksum}",
                observation.candidate, actual.records, actual.checksum
            )
            .into());
        }
    }
    Ok(())
}

fn summarize(observations: &[CandidateObservation], records: usize) -> Vec<CandidateSummary> {
    let mut grouped: BTreeMap<&str, Vec<&CandidateObservation>> = BTreeMap::new();
    for observation in observations {
        grouped
            .entry(&observation.candidate)
            .or_default()
            .push(observation);
    }
    grouped
        .into_iter()
        .map(|(candidate, group)| {
            let append_ms = median(group.iter().map(|item| item.append_ms).collect());
            CandidateSummary {
                candidate: candidate.to_string(),
                append_ms_median: append_ms,
                append_events_per_second_median: records as f64 / (append_ms / 1_000.0),
                reopen_ms_median: median(group.iter().map(|item| item.reopen_ms).collect()),
                full_scan_ms_median: median(group.iter().map(|item| item.full_scan_ms).collect()),
                time_query_ms_median: median(group.iter().map(|item| item.time_query_ms).collect()),
                trace_query_ms_median: median(
                    group.iter().map(|item| item.trace_query_ms).collect(),
                ),
                bytes_on_disk_median: median_u64(
                    group.iter().map(|item| item.bytes_on_disk).collect(),
                ),
            }
        })
        .collect()
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

fn median_u64(mut values: Vec<u64>) -> u64 {
    values.sort_unstable();
    values[values.len() / 2]
}

pub(crate) fn timed<T>(operation: impl FnOnce() -> EvalResult<T>) -> EvalResult<(f64, T)> {
    let started = Instant::now();
    let value = operation()?;
    Ok((started.elapsed().as_secs_f64() * 1_000.0, value))
}

pub(crate) fn directory_size(path: &Path) -> EvalResult<u64> {
    let mut total = 0_u64;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            total = total.saturating_add(directory_size(&entry.path())?);
        } else {
            total = total.saturating_add(metadata.len());
        }
    }
    Ok(total)
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn kernel_version() -> Option<String> {
    let output = std::process::Command::new("uname")
        .args(["-s", "-r"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(unix)]
fn make_private(path: &Path) -> EvalResult<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn make_private(_path: &Path) -> EvalResult<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_evaluation_returns_identical_result_sets() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("evaluation");
        let report = run_evaluation(
            &root,
            EvaluationConfig {
                records: 128,
                producers: 4,
                batch_size: 16,
                repetitions: 1,
                payload_bytes: 32,
            },
        )
        .unwrap();
        assert_eq!(report.observations.len(), 4);
        assert_eq!(report.summaries.len(), 4);
        assert!(report.observations.iter().all(|observation| {
            observation.full_scan.records == report.expected.full_scan_records
                && observation.time_query.records == report.expected.time_query_records
                && observation.trace_query.records == report.expected.trace_query_records
        }));
    }

    #[test]
    fn evaluator_refuses_to_reuse_a_root() {
        let root = tempfile::tempdir().unwrap();
        assert!(run_evaluation(root.path(), EvaluationConfig::default()).is_err());
    }
}
