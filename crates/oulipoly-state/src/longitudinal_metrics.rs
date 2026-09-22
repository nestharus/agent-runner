//! Bounded, non-authoritative longitudinal metrics over diagnostic event partitions.
//!
//! Metric emission is best effort and never shares a correctness-database
//! writer. Queries are bounded over the sharded event-store discovery journal;
//! missing coverage remains explicit rather than being rendered as zero.

use crate::diagnostic_recorder::{
    DiagnosticEvent, DiagnosticPhase, OutcomeCertainty, SqliteDatabaseRole,
};
use crate::event_store::{
    BoundedRead, CoverageIssue, EventCorrelations, EventFamily, EventFilter, EventId, EventKind,
    ReadLimits, SpanId, TraceId, discover_generation_read_targets, read_discovered_generations,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

pub const METRIC_SCHEMA_VERSION: u32 = 1;
pub const MAX_METRIC_LABELS: usize = 6;
pub const MAX_METRIC_LABEL_VALUE_BYTES: usize = 96;
pub const MAX_PROCESS_METRIC_SERIES: usize = 256;
pub const DEFAULT_DISCOVERY_NODE_LIMIT: usize = 16_384;
pub const DEFAULT_DISCOVERY_ENTRY_LIMIT: usize = 256;
pub const DEFAULT_METRIC_RECORD_LIMIT: usize = 20_000;
pub const DEFAULT_METRIC_PAYLOAD_LIMIT: usize = 64 * 1024 * 1024;
pub const MAX_EXEMPLARS_PER_SERIES: usize = 3;
pub const RETENTION_WINDOW_MICROS: i64 = 30 * 24 * 60 * 60 * 1_000_000;
const CLOCK_DISCONTINUITY_TOLERANCE_MICROS: i64 = 5 * 60 * 1_000_000;
const HISTOGRAM_UPPER_BOUNDS: [u64; 7] =
    [100, 1_000, 10_000, 100_000, 1_000_000, 10_000_000, u64::MAX];

static SERIES: OnceLock<Mutex<SeriesRegistry>> = OnceLock::new();
static DROPPED_SAMPLES: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricName {
    LaunchLatency,
    RegistrationLatency,
    SqliteWriterWait,
    SqliteWriterAcquisition,
    SqliteStatementLatency,
    QueueDepth,
    QueueAge,
    UnresolvedObligations,
    SupervisorLifecycle,
    WorkerLifecycle,
    RotationLatency,
    PartitionSize,
    RetentionLag,
    DroppedDiagnosticEvents,
    FailureCertainty,
    RuntimeCapPrecursor,
    RuntimeCapTerminal,
}

impl MetricName {
    pub const ALL: [Self; 17] = [
        Self::LaunchLatency,
        Self::RegistrationLatency,
        Self::SqliteWriterWait,
        Self::SqliteWriterAcquisition,
        Self::SqliteStatementLatency,
        Self::QueueDepth,
        Self::QueueAge,
        Self::UnresolvedObligations,
        Self::SupervisorLifecycle,
        Self::WorkerLifecycle,
        Self::RotationLatency,
        Self::PartitionSize,
        Self::RetentionLag,
        Self::DroppedDiagnosticEvents,
        Self::FailureCertainty,
        Self::RuntimeCapPrecursor,
        Self::RuntimeCapTerminal,
    ];

    pub const fn unit(self) -> MetricUnit {
        match self {
            Self::LaunchLatency
            | Self::RegistrationLatency
            | Self::SqliteWriterWait
            | Self::SqliteWriterAcquisition
            | Self::SqliteStatementLatency
            | Self::QueueAge
            | Self::RotationLatency
            | Self::RetentionLag => MetricUnit::Microseconds,
            Self::PartitionSize => MetricUnit::Bytes,
            Self::SupervisorLifecycle
            | Self::WorkerLifecycle
            | Self::FailureCertainty
            | Self::RuntimeCapPrecursor
            | Self::RuntimeCapTerminal => MetricUnit::State,
            Self::QueueDepth | Self::UnresolvedObligations | Self::DroppedDiagnosticEvents => {
                MetricUnit::Count
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricUnit {
    Microseconds,
    Count,
    Bytes,
    State,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricOutcome {
    Observed,
    Success,
    Contention,
    Failed,
    Unknown,
    Dropped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricLabelKey {
    QueryFamily,
    DatabaseRole,
    LifecyclePhase,
    CapId,
    CapClass,
    CapPhase,
    Certainty,
    Reason,
    WorkerKind,
    State,
    Measurement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleState {
    Starting,
    Running,
    Stopping,
    Completed,
    Failed,
}

impl LifecycleState {
    const fn label(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    const fn outcome(self) -> MetricOutcome {
        match self {
            Self::Completed => MetricOutcome::Success,
            Self::Failed => MetricOutcome::Failed,
            _ => MetricOutcome::Observed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerKind {
    EventWriter,
    Maintenance,
    Provider,
    Completion,
    LiveSessionBinding,
}

impl WorkerKind {
    const fn label(self) -> &'static str {
        match self {
            Self::EventWriter => "event_writer",
            Self::Maintenance => "maintenance",
            Self::Provider => "provider",
            Self::Completion => "completion",
            Self::LiveSessionBinding => "live_session_binding",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricLabel {
    pub key: MetricLabelKey,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricSample {
    pub schema_version: u32,
    pub name: MetricName,
    pub unit: MetricUnit,
    pub outcome: MetricOutcome,
    pub value: u64,
    pub labels: Vec<MetricLabel>,
}

impl MetricSample {
    pub fn new(name: MetricName, outcome: MetricOutcome, value: u64) -> Self {
        Self {
            schema_version: METRIC_SCHEMA_VERSION,
            name,
            unit: name.unit(),
            outcome,
            value,
            labels: Vec::new(),
        }
    }

    /// Public label values must be repository-owned static labels. Dynamic
    /// diagnostic values use the crate-private path after their own typed
    /// boundary has already normalized them.
    pub fn with_label(mut self, key: MetricLabelKey, value: &'static str) -> Self {
        self.push_label(key, value);
        self
    }

    fn with_normalized_label(mut self, key: MetricLabelKey, value: &str) -> Self {
        self.push_label(key, value);
        self
    }

    fn push_label(&mut self, key: MetricLabelKey, value: &str) {
        if self.labels.len() >= MAX_METRIC_LABELS
            || self.labels.iter().any(|label| label.key == key)
        {
            return;
        }
        let value = normalize_label_value(value);
        self.labels.push(MetricLabel { key, value });
        self.labels.sort();
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.schema_version != METRIC_SCHEMA_VERSION || self.unit != self.name.unit() {
            return Err("metric schema or unit mismatch");
        }
        if self.labels.len() > MAX_METRIC_LABELS {
            return Err("metric label bound exceeded");
        }
        let mut keys = BTreeSet::new();
        for label in &self.labels {
            if !keys.insert(label.key)
                || label.value.is_empty()
                || label.value.len() > MAX_METRIC_LABEL_VALUE_BYTES
                || !label.value.bytes().all(valid_label_byte)
            {
                return Err("invalid metric label");
            }
        }
        Ok(())
    }

    fn series_key(&self) -> String {
        let mut key = format!("{:?}|{:?}|{:?}", self.name, self.unit, self.outcome);
        for label in &self.labels {
            key.push('|');
            key.push_str(&format!("{:?}={}", label.key, label.value));
        }
        key
    }
}

fn normalize_label_value(value: &str) -> String {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty()
        || value.len() > MAX_METRIC_LABEL_VALUE_BYTES
        || !value.bytes().all(valid_label_byte)
    {
        "other".to_string()
    } else {
        value
    }
}

fn valid_label_byte(byte: u8) -> bool {
    byte.is_ascii_lowercase()
        || byte.is_ascii_digit()
        || matches!(byte, b'.' | b'_' | b'-' | b':' | b'/')
}

fn admit_sample(sample: MetricSample) -> MetricSample {
    if sample.name == MetricName::DroppedDiagnosticEvents {
        return sample;
    }
    let key = sample.series_key();
    let admitted = SERIES
        .get_or_init(|| Mutex::new(SeriesRegistry::new(MAX_PROCESS_METRIC_SERIES)))
        .try_lock()
        .ok()
        .is_some_and(|mut series| series.admit(key));
    if admitted {
        sample
    } else {
        DROPPED_SAMPLES.fetch_add(1, Ordering::Relaxed);
        MetricSample::new(
            MetricName::DroppedDiagnosticEvents,
            MetricOutcome::Dropped,
            1,
        )
        .with_label(MetricLabelKey::Reason, "metric_series_capacity")
    }
}

struct SeriesRegistry {
    keys: HashSet<String>,
    limit: usize,
}

impl SeriesRegistry {
    fn new(limit: usize) -> Self {
        Self {
            keys: HashSet::new(),
            limit,
        }
    }

    fn admit(&mut self, key: String) -> bool {
        self.keys.contains(&key) || (self.keys.len() < self.limit && self.keys.insert(key))
    }
}

pub fn dropped_metric_samples() -> u64 {
    DROPPED_SAMPLES.load(Ordering::Relaxed)
}

/// Emit one best-effort sample without touching a correctness database.
pub fn emit_metric(sample: MetricSample) {
    if sample.validate().is_err() {
        DROPPED_SAMPLES.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let sample = admit_sample(sample);
    let status = crate::diagnostic_recorder::process_recorder().record_metric_sample(
        &sample,
        Utc::now().timestamp_micros().max(0),
        EventCorrelations::default(),
    );
    observe_emit_status(&status);
}

pub fn observe_queue(depth: u64, oldest_age_micros: u64) {
    emit_metric(MetricSample::new(
        MetricName::QueueDepth,
        MetricOutcome::Observed,
        depth,
    ));
    emit_metric(MetricSample::new(
        MetricName::QueueAge,
        MetricOutcome::Observed,
        oldest_age_micros,
    ));
}

pub fn observe_unresolved_obligations(count: u64) {
    emit_metric(MetricSample::new(
        MetricName::UnresolvedObligations,
        MetricOutcome::Observed,
        count,
    ));
}

pub fn observe_supervisor_lifecycle(state: LifecycleState) {
    emit_metric(
        MetricSample::new(MetricName::SupervisorLifecycle, state.outcome(), 1)
            .with_label(MetricLabelKey::State, state.label()),
    );
}

pub fn observe_worker_lifecycle(kind: WorkerKind, state: LifecycleState) {
    emit_metric(
        MetricSample::new(MetricName::WorkerLifecycle, state.outcome(), 1)
            .with_label(MetricLabelKey::WorkerKind, kind.label())
            .with_label(MetricLabelKey::State, state.label()),
    );
}

pub fn observe_rotation(elapsed_micros: u64, partition_bytes: u64) {
    emit_metric(MetricSample::new(
        MetricName::RotationLatency,
        MetricOutcome::Success,
        elapsed_micros,
    ));
    emit_metric(MetricSample::new(
        MetricName::PartitionSize,
        MetricOutcome::Observed,
        partition_bytes,
    ));
}

pub fn observe_retention_lag(lag_micros: u64) {
    emit_metric(MetricSample::new(
        MetricName::RetentionLag,
        MetricOutcome::Observed,
        lag_micros,
    ));
}

pub fn observe_dropped_diagnostic_events(count: u64, reason: &'static str) {
    emit_metric(
        MetricSample::new(
            MetricName::DroppedDiagnosticEvents,
            MetricOutcome::Dropped,
            count,
        )
        .with_label(MetricLabelKey::Reason, reason),
    );
}

fn observe_emit_status(status: &crate::diagnostic_recorder::RecordStatus) {
    if matches!(
        status,
        crate::diagnostic_recorder::RecordStatus::Disabled
            | crate::diagnostic_recorder::RecordStatus::Failed { .. }
    ) {
        DROPPED_SAMPLES.fetch_add(1, Ordering::Relaxed);
    }
}

pub(crate) fn emit_diagnostic_metrics(
    recorder: &crate::diagnostic_recorder::FlightRecorder,
    event: &DiagnosticEvent,
) {
    let correlations = diagnostic_correlations(event).unwrap_or_default();
    let recorded_at = chrono::DateTime::parse_from_rfc3339(&event.recorded_at)
        .map(|value| value.timestamp_micros().max(0))
        .unwrap_or_else(|_| Utc::now().timestamp_micros().max(0));
    for sample in diagnostic_samples(event) {
        let sample = admit_sample(sample);
        let status = recorder.record_metric_sample(&sample, recorded_at, correlations.clone());
        observe_emit_status(&status);
    }
}

fn diagnostic_correlations(event: &DiagnosticEvent) -> Result<EventCorrelations, uuid::Error> {
    let trace = uuid::Uuid::parse_str(&event.diagnostic_id.to_string())?;
    let span = uuid::Uuid::parse_str(&event.span_id.to_string())?;
    let parent = event
        .parent_span_id
        .as_ref()
        .map(|value| uuid::Uuid::parse_str(&value.to_string()).map(SpanId::from))
        .transpose()?;
    Ok(EventCorrelations {
        trace_id: Some(TraceId::from(trace)),
        span_id: Some(SpanId::from(span)),
        parent_span_id: parent,
        invocation_uuid: None,
        session_correlation_sha256: None,
    })
}

pub fn diagnostic_samples(event: &DiagnosticEvent) -> Vec<MetricSample> {
    let mut samples = Vec::new();
    let terminal = !matches!(
        event.phase,
        DiagnosticPhase::Requested | DiagnosticPhase::Acquired
    );
    let outcome = if event.phase == DiagnosticPhase::Contention {
        MetricOutcome::Contention
    } else if event.phase == DiagnosticPhase::Failed {
        MetricOutcome::Failed
    } else if terminal {
        MetricOutcome::Success
    } else {
        MetricOutcome::Observed
    };
    let operation = event.operation.to_ascii_lowercase();
    if terminal && operation.contains("launch") {
        samples.push(MetricSample::new(
            MetricName::LaunchLatency,
            outcome,
            event.elapsed_micros,
        ));
    }
    if terminal && (operation.contains("registration") || operation.contains("admission")) {
        samples.push(MetricSample::new(
            MetricName::RegistrationLatency,
            outcome,
            event.elapsed_micros,
        ));
    }
    if terminal
        || matches!(
            event.phase,
            DiagnosticPhase::Contention | DiagnosticPhase::Failed
        )
    {
        samples.push(
            MetricSample::new(
                MetricName::FailureCertainty,
                outcome,
                certainty_value(event.observation.certainty),
            )
            .with_normalized_label(
                MetricLabelKey::Certainty,
                certainty_label(event.observation.certainty),
            ),
        );
    }
    if let (Some(identity), Some(sqlite)) = (&event.sqlite, &event.observation.sqlite) {
        let role = database_role_label(identity.database_role);
        if let Some(wait) = sqlite.writer_authority_wait_micros {
            samples.push(
                MetricSample::new(MetricName::SqliteWriterWait, outcome, wait)
                    .with_normalized_label(MetricLabelKey::DatabaseRole, role)
                    .with_normalized_label(MetricLabelKey::QueryFamily, &identity.query_family),
            );
        }
        if let Some(acquisition) = sqlite.writer_authority_acquisition_micros {
            samples.push(
                MetricSample::new(MetricName::SqliteWriterAcquisition, outcome, acquisition)
                    .with_normalized_label(MetricLabelKey::DatabaseRole, role)
                    .with_normalized_label(MetricLabelKey::QueryFamily, &identity.query_family)
                    .with_label(MetricLabelKey::Measurement, "acquisition_not_exact_wait"),
            );
        }
        if let Some(total) = sqlite
            .statement_total_micros
            .or(sqlite.total_elapsed_micros)
        {
            samples.push(
                MetricSample::new(MetricName::SqliteStatementLatency, outcome, total)
                    .with_normalized_label(MetricLabelKey::DatabaseRole, role)
                    .with_normalized_label(MetricLabelKey::QueryFamily, &identity.query_family),
            );
        }
    }
    if let Some(cap) = &event.observation.runtime_cap {
        let name = if cap.outcome == oulipoly_core::runtime_cap::ProgressWaitOutcome::Pending {
            MetricName::RuntimeCapPrecursor
        } else {
            MetricName::RuntimeCapTerminal
        };
        samples.push(
            MetricSample::new(name, outcome, 1)
                .with_normalized_label(MetricLabelKey::CapId, &cap.cap_id)
                .with_normalized_label(MetricLabelKey::CapClass, &format!("{:?}", cap.class))
                .with_normalized_label(MetricLabelKey::CapPhase, &cap.phase),
        );
    }
    samples
}

fn certainty_value(certainty: OutcomeCertainty) -> u64 {
    match certainty {
        OutcomeCertainty::NotStarted => 0,
        OutcomeCertainty::StartedUnknown => 1,
        OutcomeCertainty::EffectsPossible => 2,
        OutcomeCertainty::Committed => 3,
        OutcomeCertainty::Terminal => 4,
    }
}

fn certainty_label(certainty: OutcomeCertainty) -> &'static str {
    match certainty {
        OutcomeCertainty::NotStarted => "not_started",
        OutcomeCertainty::StartedUnknown => "started_unknown",
        OutcomeCertainty::EffectsPossible => "effects_possible",
        OutcomeCertainty::Committed => "committed",
        OutcomeCertainty::Terminal => "terminal",
    }
}

fn database_role_label(role: SqliteDatabaseRole) -> &'static str {
    match role {
        SqliteDatabaseRole::State => "state",
        SqliteDatabaseRole::PidMailbox => "pid_mailbox",
        SqliteDatabaseRole::PidIdentity => "pid_identity",
    }
}

#[derive(Debug, Clone)]
pub struct MetricQuery {
    pub recorded_at_or_after_unix_micros: i64,
    pub recorded_before_unix_micros: i64,
    pub discovery_max_nodes: usize,
    pub discovery_max_entries: usize,
    pub read_limits: ReadLimits,
}

impl MetricQuery {
    pub fn recent(minutes: u64) -> Result<Self, String> {
        let end = Utc::now().timestamp_micros().max(0);
        let span = i64::try_from(minutes)
            .ok()
            .and_then(|value| value.checked_mul(60 * 1_000_000))
            .ok_or_else(|| "metric query window is too large".to_string())?;
        Ok(Self {
            recorded_at_or_after_unix_micros: end.saturating_sub(span),
            recorded_before_unix_micros: end,
            discovery_max_nodes: DEFAULT_DISCOVERY_NODE_LIMIT,
            discovery_max_entries: DEFAULT_DISCOVERY_ENTRY_LIMIT,
            read_limits: ReadLimits::new(
                DEFAULT_DISCOVERY_ENTRY_LIMIT,
                DEFAULT_METRIC_RECORD_LIMIT,
                DEFAULT_METRIC_PAYLOAD_LIMIT,
            )
            .map_err(str::to_string)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MetricSeriesKey {
    pub name: MetricName,
    pub unit: MetricUnit,
    pub outcome: MetricOutcome,
    pub labels: Vec<MetricLabel>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricExemplar {
    pub event_id: EventId,
    pub trace_id: Option<TraceId>,
    pub span_id: Option<SpanId>,
    pub recorded_at_unix_micros: i64,
    pub value: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistogramBucket {
    pub upper_bound: u64,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricSeries {
    pub key: MetricSeriesKey,
    pub count: u64,
    pub sum: u64,
    pub sum_overflowed: bool,
    pub min: u64,
    pub max: u64,
    pub histogram: Vec<HistogramBucket>,
    pub exemplars: Vec<MetricExemplar>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricQueryReport {
    pub window_start_unix_micros: i64,
    pub window_end_unix_micros: i64,
    pub series: Vec<MetricSeries>,
    pub missing_metric_names: Vec<MetricName>,
    pub dropped_sample_total: u64,
    pub clock_discontinuities: u64,
    pub retention_window_truncated: bool,
    pub coverage_complete: bool,
    pub discovery_issues: Vec<String>,
    pub read_issues: Vec<CoverageIssue>,
    pub partitions_examined: usize,
    pub partitions_omitted: usize,
    pub records_examined: usize,
}

struct Aggregate {
    count: u64,
    sum: u64,
    sum_overflowed: bool,
    min: u64,
    max: u64,
    buckets: [u64; HISTOGRAM_UPPER_BOUNDS.len()],
    exemplars: Vec<MetricExemplar>,
}

impl Aggregate {
    fn new() -> Self {
        Self {
            count: 0,
            sum: 0,
            sum_overflowed: false,
            min: u64::MAX,
            max: 0,
            buckets: [0; HISTOGRAM_UPPER_BOUNDS.len()],
            exemplars: Vec::new(),
        }
    }

    fn observe(&mut self, value: u64, exemplar: MetricExemplar) {
        self.count = self.count.saturating_add(1);
        let (sum, overflowed) = self.sum.overflowing_add(value);
        self.sum = if overflowed { u64::MAX } else { sum };
        self.sum_overflowed |= overflowed;
        self.min = self.min.min(value);
        self.max = self.max.max(value);
        if let Some(index) = HISTOGRAM_UPPER_BOUNDS
            .iter()
            .position(|bound| value <= *bound)
        {
            self.buckets[index] = self.buckets[index].saturating_add(1);
        }
        self.exemplars.push(exemplar);
        self.exemplars
            .sort_by(|left, right| right.value.cmp(&left.value));
        self.exemplars.truncate(MAX_EXEMPLARS_PER_SERIES);
    }
}

pub fn default_event_store_root() -> Result<PathBuf, String> {
    crate::paths::data_dir().map(|root| root.join("diagnostics/event-store-v1"))
}

pub fn query_metrics(
    event_store_root: &Path,
    query: &MetricQuery,
) -> Result<MetricQueryReport, String> {
    if query.recorded_at_or_after_unix_micros < 0
        || query.recorded_before_unix_micros < query.recorded_at_or_after_unix_micros
    {
        return Err("invalid metric query window".to_string());
    }
    let discovery = discover_generation_read_targets(
        event_store_root,
        query.discovery_max_nodes,
        query.discovery_max_entries,
    )?;
    let filter = EventFilter {
        recorded_at_or_after_unix_micros: Some(query.recorded_at_or_after_unix_micros),
        recorded_before_unix_micros: Some(query.recorded_before_unix_micros),
        family: Some(EventFamily::Metric),
        kind: Some(EventKind::registered("metric.sample").map_err(|error| error.to_string())?),
        ..EventFilter::default()
    };
    let read = read_discovered_generations(
        &discovery.targets,
        &discovery.coverage,
        &filter,
        &query.read_limits,
    );
    Ok(aggregate_read(query, discovery.issues, read))
}

fn aggregate_read(
    query: &MetricQuery,
    discovery_issues: Vec<String>,
    read: BoundedRead,
) -> MetricQueryReport {
    let now = Utc::now().timestamp_micros().max(0);
    let mut aggregates: BTreeMap<MetricSeriesKey, Aggregate> = BTreeMap::new();
    let mut present = BTreeSet::new();
    let mut dropped_sample_total = 0_u64;
    let mut clock_discontinuities = 0_u64;
    let records_examined = read.records.len();
    for record in &read.records {
        let Ok(sample) = serde_json::from_str::<MetricSample>(&record.envelope.payload) else {
            continue;
        };
        if sample.validate().is_err() {
            continue;
        }
        present.insert(sample.name);
        if sample.name == MetricName::DroppedDiagnosticEvents
            || sample.outcome == MetricOutcome::Dropped
        {
            dropped_sample_total = dropped_sample_total.saturating_add(sample.value);
        }
        if record.envelope.recorded_at_unix_micros
            > record
                .envelope
                .ingested_at_unix_micros
                .saturating_add(CLOCK_DISCONTINUITY_TOLERANCE_MICROS)
        {
            clock_discontinuities = clock_discontinuities.saturating_add(1);
        }
        let key = MetricSeriesKey {
            name: sample.name,
            unit: sample.unit,
            outcome: sample.outcome,
            labels: sample.labels,
        };
        aggregates
            .entry(key)
            .or_insert_with(Aggregate::new)
            .observe(
                sample.value,
                MetricExemplar {
                    event_id: record.envelope.event_id,
                    trace_id: record.envelope.correlations.trace_id,
                    span_id: record.envelope.correlations.span_id,
                    recorded_at_unix_micros: record.envelope.recorded_at_unix_micros,
                    value: sample.value,
                },
            );
    }
    let series = aggregates
        .into_iter()
        .map(|(key, value)| MetricSeries {
            key,
            count: value.count,
            sum: value.sum,
            sum_overflowed: value.sum_overflowed,
            min: value.min,
            max: value.max,
            histogram: HISTOGRAM_UPPER_BOUNDS
                .iter()
                .zip(value.buckets)
                .map(|(upper_bound, count)| HistogramBucket {
                    upper_bound: *upper_bound,
                    count,
                })
                .collect(),
            exemplars: value.exemplars,
        })
        .collect();
    MetricQueryReport {
        window_start_unix_micros: query.recorded_at_or_after_unix_micros,
        window_end_unix_micros: query.recorded_before_unix_micros,
        series,
        missing_metric_names: MetricName::ALL
            .into_iter()
            .filter(|name| !present.contains(name))
            .collect(),
        dropped_sample_total,
        clock_discontinuities,
        retention_window_truncated: query.recorded_at_or_after_unix_micros
            < now.saturating_sub(RETENTION_WINDOW_MICROS),
        coverage_complete: read.coverage_complete && discovery_issues.is_empty(),
        discovery_issues,
        read_issues: read.issues,
        partitions_examined: read.partitions_examined,
        partitions_omitted: read.partitions_omitted,
        records_examined,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceQueryReport {
    pub trace_id: TraceId,
    pub read: BoundedRead,
    pub discovery_issues: Vec<String>,
}

pub fn query_trace(
    event_store_root: &Path,
    trace_id: TraceId,
    limits: &ReadLimits,
) -> Result<TraceQueryReport, String> {
    let discovery = discover_generation_read_targets(
        event_store_root,
        DEFAULT_DISCOVERY_NODE_LIMIT,
        DEFAULT_DISCOVERY_ENTRY_LIMIT,
    )?;
    let read = read_discovered_generations(
        &discovery.targets,
        &discovery.coverage,
        &EventFilter {
            trace_id: Some(trace_id),
            ..EventFilter::default()
        },
        limits,
    );
    Ok(TraceQueryReport {
        trace_id,
        read,
        discovery_issues: discovery.issues,
    })
}

pub(crate) fn metric_payload(sample: &MetricSample) -> Result<serde_json::Value, String> {
    sample.validate().map_err(str::to_string)?;
    serde_json::to_value(sample).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_store::{
        Digest32, EnqueueErrorKind, EventEnvelopeV1, EventWriterConfig, GenerationId,
        NativeProcessIdentity, NewEventV1, PayloadNormalizationPolicy, ProcessEventWriter,
        ProcessInstanceId, ProducerIdentity, ReadRecord, WriterInstanceId,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    fn producer(byte: u8) -> ProducerIdentity {
        ProducerIdentity {
            writer_instance_id: WriterInstanceId::from_bytes([byte; 16]),
            process_instance_id: ProcessInstanceId::from_bytes([byte; 16]),
            process_root_id: ProcessInstanceId::from_bytes([byte; 16]),
            parent_process_instance_id: None,
            supervisor_authority_id: None,
            native_process: Some(NativeProcessIdentity {
                os_pid: i64::from(byte) + 100,
                os_boot_id_sha256: Digest32::sha256(&[byte]),
                os_pid_starttime_ticks: i64::from(byte) + 200,
            }),
        }
    }

    fn metric_event(
        producer: &ProducerIdentity,
        id: u8,
        sequence: i64,
        recorded_at: i64,
        ingested_at: i64,
        sample: MetricSample,
        trace: Option<TraceId>,
    ) -> EventEnvelopeV1 {
        EventEnvelopeV1::normalize(
            NewEventV1 {
                event_id: EventId::from_bytes([id; 16]),
                family: EventFamily::Metric,
                kind: EventKind::registered("metric.sample").unwrap(),
                recorded_at_unix_micros: recorded_at,
                producer_sequence: sequence,
                producer: producer.clone(),
                correlations: trace
                    .map(|trace_id| EventCorrelations {
                        trace_id: Some(trace_id),
                        span_id: Some(SpanId::from_bytes([id.saturating_add(1); 16])),
                        ..EventCorrelations::default()
                    })
                    .unwrap_or_default(),
                payload: serde_json::to_value(sample).unwrap(),
                legacy_provenance: None,
                retry_of_generation_id: None,
            },
            &PayloadNormalizationPolicy::registered(&[
                "schema_version",
                "name",
                "unit",
                "outcome",
                "value",
                "labels",
            ])
            .unwrap(),
            ingested_at,
        )
        .unwrap()
    }

    #[test]
    fn labels_are_bounded_and_invalid_values_collapse() {
        let sample = MetricSample::new(MetricName::QueueDepth, MetricOutcome::Observed, 7)
            .with_normalized_label(MetricLabelKey::State, "../../raw path with spaces");
        assert_eq!(sample.labels[0].value, "other");
        assert!(sample.validate().is_ok());
    }

    #[test]
    fn diagnostic_sqlite_measurements_do_not_call_acquisition_exact_wait() {
        let source = include_str!("sqlite_observability.rs");
        assert!(source.contains("WriterWaitNotExposedByApi"));
        assert_ne!(
            MetricName::SqliteWriterWait,
            MetricName::SqliteWriterAcquisition
        );
    }

    #[test]
    fn cardinality_registry_admits_existing_series_and_rejects_new_overflow() {
        let mut registry = SeriesRegistry::new(2);
        assert!(registry.admit("one".to_string()));
        assert!(registry.admit("two".to_string()));
        assert!(registry.admit("one".to_string()));
        assert!(!registry.admit("three".to_string()));
        assert_eq!(registry.keys.len(), 2);
    }

    #[test]
    fn terminal_sink_loss_is_counted_without_retrying_a_correctness_writer() {
        let before = dropped_metric_samples();
        observe_emit_status(&crate::diagnostic_recorder::RecordStatus::Failed {
            stage: "fixture_sink_outage".to_string(),
        });
        assert_eq!(dropped_metric_samples(), before.saturating_add(1));
    }

    #[test]
    fn aggregation_exposes_missing_drop_clock_retention_and_trace_exemplars() {
        let identity = producer(41);
        let trace = TraceId::from_bytes([91; 16]);
        let normal = metric_event(
            &identity,
            1,
            1,
            900_000_000,
            1,
            MetricSample::new(MetricName::QueueDepth, MetricOutcome::Observed, 7),
            Some(trace),
        );
        let dropped = metric_event(
            &identity,
            2,
            2,
            900_000_001,
            900_000_002,
            MetricSample::new(
                MetricName::DroppedDiagnosticEvents,
                MetricOutcome::Dropped,
                3,
            ),
            None,
        );
        let records = [normal, dropped]
            .into_iter()
            .enumerate()
            .map(|(index, envelope)| ReadRecord {
                generation_id: GenerationId::from_bytes([5; 16]),
                local_sequence: index as i64 + 1,
                immutable_sha256: envelope.immutable_digest().unwrap(),
                envelope,
            })
            .collect::<Vec<_>>();
        let read = BoundedRead {
            records,
            watermarks: Vec::new(),
            issues: Vec::new(),
            coverage_complete: true,
            partitions_examined: 1,
            partitions_omitted: 0,
            records_returned: 2,
            payload_bytes_returned: 0,
            payload_bytes_examined: 0,
            discovery_watermark: None,
        };
        let report = aggregate_read(
            &MetricQuery {
                recorded_at_or_after_unix_micros: 0,
                recorded_before_unix_micros: 1_000_000_000,
                discovery_max_nodes: 1,
                discovery_max_entries: 1,
                read_limits: ReadLimits::new(1, 10, 10_000).unwrap(),
            },
            Vec::new(),
            read,
        );
        assert_eq!(report.dropped_sample_total, 3);
        assert_eq!(report.clock_discontinuities, 1);
        assert!(report.retention_window_truncated);
        assert!(
            report
                .missing_metric_names
                .contains(&MetricName::LaunchLatency)
        );
        let queue = report
            .series
            .iter()
            .find(|series| series.key.name == MetricName::QueueDepth)
            .unwrap();
        assert_eq!(queue.count, 1);
        assert_eq!(queue.exemplars[0].trace_id, Some(trace));
    }

    #[test]
    fn bounded_query_crosses_rotation_and_process_restart() {
        let root = tempfile::tempdir().unwrap();
        let first_identity = producer(51);
        let mut first_config = EventWriterConfig::native(root.path(), first_identity.clone());
        first_config.rotation_soft_bytes = 1;
        let first = ProcessEventWriter::start(first_config).unwrap();
        first
            .append(metric_event(
                &first_identity,
                11,
                1,
                100,
                101,
                MetricSample::new(MetricName::QueueDepth, MetricOutcome::Observed, 1),
                Some(TraceId::from_bytes([77; 16])),
            ))
            .unwrap();
        first.request_rotation().unwrap();
        first
            .append(metric_event(
                &first_identity,
                12,
                2,
                200,
                201,
                MetricSample::new(MetricName::QueueDepth, MetricOutcome::Observed, 2),
                Some(TraceId::from_bytes([77; 16])),
            ))
            .unwrap();
        first.shutdown().unwrap();

        let second_identity = producer(61);
        let second = ProcessEventWriter::start(EventWriterConfig::native(
            root.path(),
            second_identity.clone(),
        ))
        .unwrap();
        second
            .append(metric_event(
                &second_identity,
                13,
                1,
                300,
                301,
                MetricSample::new(MetricName::QueueDepth, MetricOutcome::Observed, 3),
                Some(TraceId::from_bytes([77; 16])),
            ))
            .unwrap();
        second.shutdown().unwrap();

        let report = query_metrics(
            root.path(),
            &MetricQuery {
                recorded_at_or_after_unix_micros: 0,
                recorded_before_unix_micros: 1_000,
                discovery_max_nodes: 16_384,
                discovery_max_entries: 32,
                read_limits: ReadLimits::new(32, 64, 1024 * 1024).unwrap(),
            },
        )
        .unwrap();
        let queue = report
            .series
            .iter()
            .find(|series| series.key.name == MetricName::QueueDepth)
            .unwrap();
        assert_eq!(queue.count, 3);
        assert_eq!(queue.sum, 6);
        assert!(report.partitions_examined >= 3);

        let trace = query_trace(
            root.path(),
            TraceId::from_bytes([77; 16]),
            &ReadLimits::new(32, 64, 1024 * 1024).unwrap(),
        )
        .unwrap();
        assert_eq!(trace.read.records.len(), 3);
    }

    #[test]
    fn concurrent_metric_appends_share_only_the_process_local_writer() {
        let root = tempfile::tempdir().unwrap();
        let identity = producer(71);
        let writer = Arc::new(
            ProcessEventWriter::start(EventWriterConfig::native(root.path(), identity.clone()))
                .unwrap(),
        );
        let accepted = Arc::new(AtomicUsize::new(0));
        let dropped = Arc::new(AtomicUsize::new(0));
        let workers = (0_u8..16)
            .map(|ordinal| {
                let writer = Arc::clone(&writer);
                let identity = identity.clone();
                let accepted = Arc::clone(&accepted);
                let dropped = Arc::clone(&dropped);
                std::thread::spawn(move || {
                    match writer.try_append(metric_event(
                        &identity,
                        ordinal.saturating_add(20),
                        i64::from(ordinal) + 1,
                        1_000 + i64::from(ordinal),
                        2_000 + i64::from(ordinal),
                        MetricSample::new(
                            MetricName::UnresolvedObligations,
                            MetricOutcome::Observed,
                            u64::from(ordinal),
                        ),
                        None,
                    )) {
                        Ok(pending) => {
                            pending.wait().unwrap();
                            accepted.fetch_add(1, AtomicOrdering::Relaxed);
                        }
                        Err(error) => {
                            assert_eq!(error.kind, EnqueueErrorKind::Full);
                            dropped.fetch_add(1, AtomicOrdering::Relaxed);
                        }
                    }
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            worker.join().unwrap();
        }
        let accepted = accepted.load(AtomicOrdering::Relaxed);
        let dropped = dropped.load(AtomicOrdering::Relaxed);
        assert_eq!(accepted + dropped, 16);
        if dropped > 0 {
            writer
                .append(metric_event(
                    &identity,
                    99,
                    99,
                    3_000,
                    3_001,
                    MetricSample::new(
                        MetricName::DroppedDiagnosticEvents,
                        MetricOutcome::Dropped,
                        dropped as u64,
                    )
                    .with_label(MetricLabelKey::Reason, "writer_backpressure"),
                    None,
                ))
                .unwrap();
        }
        Arc::try_unwrap(writer).ok().unwrap().shutdown().unwrap();
        let report = query_metrics(
            root.path(),
            &MetricQuery {
                recorded_at_or_after_unix_micros: 0,
                recorded_before_unix_micros: 10_000,
                discovery_max_nodes: 16_384,
                discovery_max_entries: 8,
                read_limits: ReadLimits::new(8, 64, 1024 * 1024).unwrap(),
            },
        )
        .unwrap();
        assert_eq!(
            report
                .series
                .iter()
                .find(|series| series.key.name == MetricName::UnresolvedObligations)
                .unwrap()
                .count,
            accepted as u64
        );
        assert_eq!(report.dropped_sample_total, dropped as u64);
    }
}
