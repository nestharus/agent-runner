//! Database-independent, fail-open control-plane flight recorder.
//!
//! Each process writes only its own bounded JSONL shard. Recorder failures are
//! deliberately not returned through the observed operation's result.

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{
    Arc, Mutex, OnceLock,
    mpsc::{self, Receiver, SyncSender, TrySendError},
};
use std::time::SystemTime;
use std::time::{Duration, Instant};
use uuid::Uuid;

pub const DIAGNOSTIC_SCHEMA_VERSION: u32 = 1;
const RECORDER_DIRECTORY: &str = "diagnostics/flight-recorder-v1";
const MAX_TEXT_BYTES: usize = 1_024;
const MAX_CORRELATIONS: usize = 24;
const MAX_READ_RECORD_BYTES: usize = 1024 * 1024;
pub const MAX_INSPECTION_SHARDS: usize = 256;
pub const MAX_INSPECTION_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_INSPECTION_DIRECTORY_ENTRIES: usize = 1_024;
pub const MAX_RETENTION_STATUS_READ_BYTES: u64 = 64 * 1024;
pub const MAX_CLEANUP_DIRECTORY_ENTRIES: usize = 1_024;
pub const MAX_CLEANUP_ISSUES: usize = 128;
pub const MAX_CLEANUP_ISSUE_BYTES: usize = 64;
pub const DEFAULT_DEFERRED_QUEUE_CAPACITY: usize = 1_024;
pub const GAP_REPORTER_QUEUE_CAPACITY: usize = 16;

static DIAGNOSTIC_GAP_STAGES: AtomicU64 = AtomicU64::new(0);
static PENDING_DIAGNOSTIC_GAP_STAGES: AtomicU64 = AtomicU64::new(0);
static GAP_REPORTER: OnceLock<Option<SyncSender<&'static str>>> = OnceLock::new();

macro_rules! uuid_id {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(value).map(Self)
            }
        }
    };
}

uuid_id!(DiagnosticId);
uuid_id!(EventId);
uuid_id!(SpanId);
uuid_id!(ProducerInstanceId);

impl ProducerInstanceId {
    fn unknown() -> Self {
        Self(Uuid::nil())
    }
}

#[derive(Debug, Clone)]
pub struct RecorderConfig {
    pub max_shard_bytes: u64,
    /// Total files retained for this recorder instance, including the active file.
    pub max_shards: usize,
    /// Aggregate retained flight shards after an opportunistic stale-process sweep.
    pub max_total_shards: usize,
    /// Inactive process shards older than this are eligible for retirement.
    pub stale_shard_age: Duration,
    /// Later phases wait in this bounded per-recorder queue. Producers never
    /// wait for capacity in this queue.
    pub deferred_queue_capacity: usize,
}

impl Default for RecorderConfig {
    fn default() -> Self {
        Self {
            max_shard_bytes: 1024 * 1024,
            max_shards: 4,
            max_total_shards: 64,
            stale_shard_age: Duration::from_secs(7 * 24 * 60 * 60),
            deferred_queue_capacity: DEFAULT_DEFERRED_QUEUE_CAPACITY,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecorderProcessIdentity {
    pub os_pid: i64,
    #[serde(default)]
    pub parent_pid: Option<i64>,
    #[serde(default)]
    pub os_boot_id: Option<String>,
    #[serde(default)]
    pub os_pid_starttime_ticks: Option<i64>,
    #[serde(default = "ProducerInstanceId::unknown")]
    pub producer_instance: ProducerInstanceId,
}

impl RecorderProcessIdentity {
    fn current(producer_instance: ProducerInstanceId) -> Self {
        let os_pid = i64::from(std::process::id());
        let parent_pid = parent_pid();
        match crate::pid_identity::read_live_process_identity(os_pid) {
            Ok(Some(identity)) => Self {
                os_pid,
                parent_pid,
                os_boot_id: Some(identity.os_boot_id),
                os_pid_starttime_ticks: Some(identity.os_pid_starttime_ticks),
                producer_instance,
            },
            Ok(None) | Err(_) => Self {
                os_pid,
                parent_pid,
                os_boot_id: None,
                os_pid_starttime_ticks: None,
                producer_instance,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticPhase {
    Requested,
    Acquired,
    Contention,
    CommitStarted,
    Committed,
    Released,
    Failed,
}

pub type Phase = DiagnosticPhase;

impl DiagnosticPhase {
    fn is_failure(self) -> bool {
        matches!(self, Self::Contention | Self::Failed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeCertainty {
    NotStarted,
    StartedUnknown,
    EffectsPossible,
    Committed,
    Terminal,
}

/// Stable, non-path database classification. Producers must never attach a
/// filesystem path, SQLite URI, SQL text, or bound value to this identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SqliteDatabaseRole {
    State,
    PidMailbox,
    PidIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SqlitePathClass {
    ManagedFile,
    ReadOnlySnapshot,
    Memory,
    ExternalFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SqliteTransactionMode {
    Autocommit,
    Deferred,
    Immediate,
    Exclusive,
    ReadOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SqliteTransactionPhase {
    ConnectionOpen,
    Requested,
    WriterAuthority,
    StatementExecution,
    Commit,
    PostCommit,
    Released,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SqliteMeasurementGap {
    WriterAuthorityNotApplicable,
    WriterAuthorityNotReached,
    WriterWaitNotExposedByApi,
    WriterWaitAndExecutionNotSeparable,
    ExecutionNotApplicable,
    ExecutionNotReached,
    ExecutionNotExposedByApi,
    CommitNotApplicable,
    CommitNotReached,
    CommitNotExposedByApi,
    PostCommitNotApplicable,
    PostCommitNotReached,
    RowsExaminedNotExposed,
    RowsExaminedNotApplicable,
    RowsChangedNotReported,
    RowsChangedNotApplicable,
    PostCommitOutsideBoundary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SqliteEventIdentity {
    pub database_role: SqliteDatabaseRole,
    pub path_class: SqlitePathClass,
    pub query_family: String,
    #[serde(default)]
    pub transaction_mode: Option<SqliteTransactionMode>,
}

impl SqliteEventIdentity {
    pub fn new(
        database_role: SqliteDatabaseRole,
        path_class: SqlitePathClass,
        query_family: &'static str,
    ) -> Self {
        Self {
            database_role,
            path_class,
            query_family: stable_sqlite_label(query_family),
            transaction_mode: None,
        }
    }

    pub fn with_transaction_mode(mut self, mode: SqliteTransactionMode) -> Self {
        self.transaction_mode = Some(mode);
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SqlitePhaseEvidence {
    pub transaction_phase: Option<SqliteTransactionPhase>,
    /// End-to-end observed operation time. For early recorder spans the event's
    /// `elapsed_micros` also carries this; late-filtered observations use this
    /// field because recorder emission intentionally begins after the DB call.
    pub total_elapsed_micros: Option<u64>,
    /// Total time inside a single SQLite statement API when acquisition/busy
    /// wait cannot be separated from VM execution.
    pub statement_total_micros: Option<u64>,
    /// Total duration of the SQLite transaction-begin API call. SQLite does
    /// not expose the subset spent in its busy handler, so this must not be
    /// interpreted as exact writer-lock wait time.
    pub writer_authority_acquisition_micros: Option<u64>,
    /// Exact time spent waiting for writer authority, when exposed by the
    /// adapter. rusqlite does not expose it for `BEGIN IMMEDIATE`, so current
    /// producers leave this absent and report `WriterWaitNotExposedByApi`.
    pub writer_authority_wait_micros: Option<u64>,
    /// Time from explicit transaction authority acquisition until commit was
    /// requested, or time inside a directly observed read/open API. For a
    /// multi-statement transaction this is a transaction-body phase duration,
    /// not a claim about SQLite VM-only execution time.
    pub execution_micros: Option<u64>,
    pub commit_micros: Option<u64>,
    pub post_commit_micros: Option<u64>,
    pub rows_examined: Option<u64>,
    pub rows_changed: Option<u64>,
    pub rows_returned: Option<u64>,
    pub measurement_gaps: Vec<SqliteMeasurementGap>,
    pub query_plan: Option<SqliteQueryPlanEvidence>,
}

impl SqlitePhaseEvidence {
    pub fn for_phase(transaction_phase: SqliteTransactionPhase) -> Self {
        Self {
            transaction_phase: Some(transaction_phase),
            ..Self::default()
        }
    }

    pub fn with_writer_acquisition(mut self, acquisition: Duration) -> Self {
        self.writer_authority_acquisition_micros = Some(saturating_micros(acquisition));
        self
    }

    pub fn with_statement_total(mut self, elapsed: Duration) -> Self {
        self.statement_total_micros = Some(saturating_micros(elapsed));
        self
    }

    pub fn with_total_elapsed(mut self, elapsed: Duration) -> Self {
        self.total_elapsed_micros = Some(saturating_micros(elapsed));
        self
    }

    pub fn with_execution(mut self, execution: Duration) -> Self {
        self.execution_micros = Some(saturating_micros(execution));
        self
    }

    pub fn with_commit(mut self, commit: Duration) -> Self {
        self.commit_micros = Some(saturating_micros(commit));
        self
    }

    pub fn with_post_commit(mut self, post_commit: Duration) -> Self {
        self.post_commit_micros = Some(saturating_micros(post_commit));
        self
    }

    pub fn with_rows_changed(mut self, rows: u64) -> Self {
        self.rows_changed = Some(rows);
        self
    }

    pub fn with_rows_returned(mut self, rows: u64) -> Self {
        self.rows_returned = Some(rows);
        self
    }

    pub fn with_gap(mut self, gap: SqliteMeasurementGap) -> Self {
        if !self.measurement_gaps.contains(&gap) && self.measurement_gaps.len() < 8 {
            self.measurement_gaps.push(gap);
        }
        self
    }

    pub fn with_query_plan(mut self, plan: SqliteQueryPlanEvidence) -> Self {
        self.query_plan = Some(plan);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SqliteQueryPlanOperator {
    Scan,
    Search,
    TemporaryBTree,
    Compound,
    Coroutine,
    Materialize,
    MultiIndex,
    BloomFilter,
    Subquery,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SqliteQueryPlanOperatorCount {
    pub operator: SqliteQueryPlanOperator,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SqliteQueryPlanEvidence {
    Disabled,
    Captured {
        nodes_seen: u32,
        truncated: bool,
        operators: Vec<SqliteQueryPlanOperatorCount>,
    },
    Unavailable {
        primary_code: Option<String>,
        extended_code: Option<i32>,
    },
}

impl Default for OutcomeCertainty {
    fn default() -> Self {
        Self::NotStarted
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SqliteFailure {
    pub primary_code: Option<String>,
    pub extended_code: Option<i32>,
    pub message: String,
    pub contention: bool,
}

impl SqliteFailure {
    pub fn from_error(error: &rusqlite::Error) -> Self {
        let sqlite = error.sqlite_error();
        let primary_code = sqlite.map(|failure| format!("{:?}", failure.code));
        let extended_code = sqlite.map(|failure| failure.extended_code);
        let contention = matches!(
            error.sqlite_error_code(),
            Some(rusqlite::ffi::ErrorCode::DatabaseBusy)
                | Some(rusqlite::ffi::ErrorCode::DatabaseLocked)
        );
        Self {
            primary_code,
            extended_code,
            // SQLite error strings are not a safe telemetry boundary: virtual
            // tables, user functions, and future adapters may include input in
            // them. Typed codes carry the useful classification without SQL,
            // paths, credentials, or bound values.
            message: if error.sqlite_error().is_some() {
                "sqlite operation failed".to_string()
            } else {
                "sqlite adapter operation failed".to_string()
            },
            contention,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PhaseObservation {
    pub certainty: OutcomeCertainty,
    pub wait_micros: Option<u64>,
    pub busy_timeout_millis: Option<u64>,
    pub retry_count: Option<u32>,
    pub sqlite_failure: Option<SqliteFailure>,
    pub sqlite: Option<SqlitePhaseEvidence>,
    pub causes: Vec<String>,
}

impl PhaseObservation {
    pub fn not_started() -> Self {
        Self::default()
    }

    pub fn started_unknown() -> Self {
        Self {
            certainty: OutcomeCertainty::StartedUnknown,
            ..Self::default()
        }
    }

    pub fn effects_possible() -> Self {
        Self {
            certainty: OutcomeCertainty::EffectsPossible,
            ..Self::default()
        }
    }

    pub fn committed() -> Self {
        Self {
            certainty: OutcomeCertainty::Committed,
            ..Self::default()
        }
    }

    pub fn terminal() -> Self {
        Self {
            certainty: OutcomeCertainty::Terminal,
            ..Self::default()
        }
    }

    pub fn with_wait(mut self, wait: Duration) -> Self {
        self.wait_micros = Some(saturating_micros(wait));
        self
    }

    pub fn with_busy_timeout(mut self, timeout: Duration) -> Self {
        self.busy_timeout_millis = Some(saturating_millis(timeout));
        self
    }

    pub fn with_retry_count(mut self, retry_count: u32) -> Self {
        self.retry_count = Some(retry_count);
        self
    }

    pub fn with_sqlite_failure(mut self, error: &rusqlite::Error) -> Self {
        self.sqlite_failure = Some(SqliteFailure::from_error(error));
        self
    }

    pub fn with_sqlite_evidence(mut self, evidence: SqlitePhaseEvidence) -> Self {
        self.sqlite = Some(evidence);
        self
    }

    pub fn with_cause(mut self, cause: impl AsRef<str>) -> Self {
        if self.causes.len() < 8 {
            self.causes.push(redact_text(cause.as_ref()));
        }
        self
    }
}

#[derive(Debug, Clone)]
pub struct SpanStart {
    diagnostic_id: DiagnosticId,
    parent_span_id: Option<SpanId>,
    operation: String,
    resource: String,
    lifecycle_phase: Option<String>,
    correlations: BTreeMap<String, String>,
    busy_timeout_millis: Option<u64>,
    retry_count: u32,
    sqlite: Option<SqliteEventIdentity>,
}

impl SpanStart {
    pub fn new(operation: impl AsRef<str>, resource: impl AsRef<str>) -> Self {
        Self {
            diagnostic_id: DiagnosticId::new(),
            parent_span_id: None,
            operation: bounded_text(operation.as_ref(), 128),
            resource: bounded_text(resource.as_ref(), 128),
            lifecycle_phase: None,
            correlations: BTreeMap::new(),
            busy_timeout_millis: None,
            retry_count: 0,
            sqlite: None,
        }
    }

    pub fn with_diagnostic_id(mut self, diagnostic_id: DiagnosticId) -> Self {
        self.diagnostic_id = diagnostic_id;
        self
    }

    pub fn with_parent_span_id(mut self, parent_span_id: SpanId) -> Self {
        self.parent_span_id = Some(parent_span_id);
        self
    }

    pub fn with_lifecycle_phase(mut self, lifecycle_phase: impl AsRef<str>) -> Self {
        self.lifecycle_phase = Some(bounded_text(lifecycle_phase.as_ref(), 128));
        self
    }

    /// Adds a bounded identifier. Values attached to secret-bearing keys are
    /// replaced, even if a caller accidentally selects this builder.
    pub fn with_identifier(mut self, key: impl AsRef<str>, value: impl AsRef<str>) -> Self {
        if self.correlations.len() < MAX_CORRELATIONS {
            let key = safe_key(key.as_ref());
            let value = if sensitive_key(&key) {
                "[REDACTED]".to_string()
            } else {
                bounded_text(value.as_ref(), 256)
            };
            self.correlations.insert(key, value);
        }
        self
    }

    /// Arbitrary external correlations are represented only by a stable digest.
    pub fn with_hashed_correlation(
        mut self,
        key: impl AsRef<str>,
        value: impl AsRef<[u8]>,
    ) -> Self {
        if self.correlations.len() < MAX_CORRELATIONS {
            let key = safe_key(key.as_ref());
            let value = if sensitive_key(&key) {
                "[REDACTED]".to_string()
            } else {
                let digest = Sha256::digest(value.as_ref());
                format!("sha256:{digest:x}")
            };
            self.correlations.insert(key, value);
        }
        self
    }

    pub fn with_busy_timeout(mut self, timeout: Duration) -> Self {
        self.busy_timeout_millis = Some(saturating_millis(timeout));
        self
    }

    pub fn with_retry_count(mut self, retry_count: u32) -> Self {
        self.retry_count = retry_count;
        self
    }

    pub fn with_sqlite_identity(mut self, identity: SqliteEventIdentity) -> Self {
        self.sqlite = Some(identity);
        self
    }

    pub fn diagnostic_id(&self) -> &DiagnosticId {
        &self.diagnostic_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticEvent {
    pub schema_version: u32,
    pub event_id: EventId,
    pub diagnostic_id: DiagnosticId,
    pub span_id: SpanId,
    #[serde(default)]
    pub parent_span_id: Option<SpanId>,
    pub recorded_at: String,
    pub elapsed_micros: u64,
    pub process: RecorderProcessIdentity,
    pub operation: String,
    pub resource: String,
    #[serde(default)]
    pub lifecycle_phase: Option<String>,
    pub phase: DiagnosticPhase,
    #[serde(default)]
    pub observation: PhaseObservation,
    #[serde(default)]
    pub correlations: BTreeMap<String, String>,
    #[serde(default)]
    pub sqlite: Option<SqliteEventIdentity>,
}

#[derive(Clone)]
pub struct FlightRecorder {
    inner: Arc<RecorderInner>,
}

struct RecorderInner {
    process: RecorderProcessIdentity,
    #[cfg(test)]
    active_path: Option<PathBuf>,
    max_shard_bytes: u64,
    writer: Option<SyncSender<WriterCommand>>,
}

struct WriterState {
    root: PathBuf,
    active_path: PathBuf,
    active_file: fs::File,
    shard_prefix: String,
    config: RecorderConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppendCoordination {
    Synchronous,
    Deferred,
}

enum WriterCommand {
    Append {
        encoded: Vec<u8>,
        completion: Option<mpsc::Sender<RecordStatus>>,
    },
    Cleanup {
        completion: mpsc::Sender<CleanupReport>,
    },
    #[cfg(test)]
    Drain { completion: mpsc::Sender<()> },
    #[cfg(test)]
    Block {
        entered: mpsc::Sender<()>,
        release: mpsc::Receiver<()>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RecordStatus {
    Appended,
    /// Accepted by the in-process writer, but not yet appended or crash-durable.
    Queued,
    Disabled,
    Failed {
        stage: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CleanupReport {
    /// Raw directory entries examined by this cleanup generation.
    pub directory_entries_examined: usize,
    /// Directory entries that could not be classified as shards or non-shards.
    pub directory_entries_unreadable: usize,
    /// True when cleanup stopped at its hard directory enumeration bound.
    pub directory_limit_reached: bool,
    pub scanned_shards: usize,
    pub retired_shards: usize,
    pub retained_live_shards: usize,
    pub retained_uncertain_shards: usize,
    pub aggregate_limit_satisfied: bool,
    /// True when per-entry issue details reached their persisted bound.
    pub issue_limit_reached: bool,
    /// Issue details omitted after `MAX_CLEANUP_ISSUES` was reached.
    pub issues_omitted: usize,
    pub issues: Vec<String>,
}

impl FlightRecorder {
    pub fn open(root: impl AsRef<Path>, config: RecorderConfig) -> Result<Self, String> {
        // Reporter startup is attempted at most once during recorder
        // initialization. Operation and writer paths only observe the resulting
        // sender and never start or wait for the reporter.
        ensure_gap_reporter();
        let root = root.as_ref().to_path_buf();
        create_private_directory(&root)?;
        let instance = ProducerInstanceId::new();
        let process = RecorderProcessIdentity::current(instance.clone());
        let boot_fingerprint = process
            .os_boot_id
            .as_deref()
            .map(fingerprint)
            .unwrap_or_else(|| "unknown".to_string());
        let shard_prefix = format!(
            "flight-{}-{}-{boot_fingerprint}-{instance}",
            process.os_pid,
            process.os_pid_starttime_ticks.unwrap_or(0)
        );
        let active_path = root.join(format!("{shard_prefix}.jsonl"));
        let active_file = open_leased_private_append_file(&active_path)
            .map_err(|error| format!("Failed to lease diagnostic recorder shard: {error}"))?;
        let config = RecorderConfig {
            max_shard_bytes: config.max_shard_bytes.max(256),
            max_shards: config.max_shards.max(1),
            max_total_shards: config.max_total_shards.max(config.max_shards.max(1)),
            stale_shard_age: config.stale_shard_age,
            deferred_queue_capacity: config.deferred_queue_capacity.max(1),
        };
        let max_shard_bytes = config.max_shard_bytes;
        let queue_capacity = config.deferred_queue_capacity;
        let writer = WriterState {
            root,
            active_path: active_path.clone(),
            active_file,
            shard_prefix,
            config,
        };
        // Opening is not an observed database operation. Complete the
        // opportunistic sweep before the writer becomes solely thread-owned.
        let _ = cleanup_stale_shards(&writer);
        let (sender, receiver) = mpsc::sync_channel(queue_capacity);
        std::thread::Builder::new()
            .name("oulipoly-flight-recorder".to_string())
            .spawn(move || writer_loop(writer, receiver))
            .map_err(|error| format!("Failed to start diagnostic recorder writer: {error}"))?;
        let recorder = Self {
            inner: Arc::new(RecorderInner {
                process,
                #[cfg(test)]
                active_path: Some(active_path),
                max_shard_bytes,
                writer: Some(sender),
            }),
        };
        Ok(recorder)
    }

    fn disabled() -> Self {
        let instance = ProducerInstanceId::new();
        Self {
            inner: Arc::new(RecorderInner {
                process: RecorderProcessIdentity::current(instance),
                #[cfg(test)]
                active_path: None,
                max_shard_bytes: RecorderConfig::default().max_shard_bytes,
                writer: None,
            }),
        }
    }

    /// Emits Requested synchronously before invoking the supplied operation.
    /// Recorder failures are swallowed and the closure result is returned unchanged.
    pub fn with_requested_span<T>(
        &self,
        start: SpanStart,
        operation: impl FnOnce(&DiagnosticSpan) -> T,
    ) -> T {
        // This boundary precedes the protected database attempt, so it is safe
        // to surface gaps deferred by database-held producer paths.
        emit_pending_gaps();
        self.with_span(start, AppendCoordination::Synchronous, operation)
    }

    /// Synchronously retains one observation made after the protected work has
    /// finished. This avoids inventing a post-hoc `Requested` event and is safe
    /// only when the producer no longer holds the observed database authority.
    pub(crate) fn record_completed_observation(
        &self,
        start: SpanStart,
        elapsed: Duration,
        phase: DiagnosticPhase,
        observation: PhaseObservation,
    ) -> RecordStatus {
        emit_pending_gaps();
        self.record_completed_observation_with_coordination(
            start,
            elapsed,
            phase,
            observation,
            AppendCoordination::Synchronous,
        )
    }

    fn record_completed_observation_with_coordination(
        &self,
        start: SpanStart,
        elapsed: Duration,
        phase: DiagnosticPhase,
        mut observation: PhaseObservation,
        coordination: AppendCoordination,
    ) -> RecordStatus {
        if observation.busy_timeout_millis.is_none() {
            observation.busy_timeout_millis = start.busy_timeout_millis;
        }
        if observation.retry_count.is_none() {
            observation.retry_count = Some(start.retry_count);
        }
        for cause in &mut observation.causes {
            *cause = redact_text(cause);
        }
        self.append(
            &DiagnosticEvent {
                schema_version: DIAGNOSTIC_SCHEMA_VERSION,
                event_id: EventId::new(),
                diagnostic_id: start.diagnostic_id,
                span_id: SpanId::new(),
                parent_span_id: start.parent_span_id,
                recorded_at: now(),
                elapsed_micros: saturating_micros(elapsed),
                process: self.inner.process.clone(),
                operation: start.operation,
                resource: start.resource,
                lifecycle_phase: start.lifecycle_phase,
                phase,
                observation,
                correlations: start.correlations,
                sqlite: start.sqlite,
            },
            coordination,
        )
    }

    /// Emits a child Requested event without waiting for recorder file I/O.
    /// This is for a nested resource span entered while its parent database
    /// transaction is already held.
    fn with_deferred_requested_span<T>(
        &self,
        start: SpanStart,
        operation: impl FnOnce(&DiagnosticSpan) -> T,
    ) -> T {
        self.with_span(start, AppendCoordination::Deferred, operation)
    }

    fn with_span<T>(
        &self,
        start: SpanStart,
        requested_coordination: AppendCoordination,
        operation: impl FnOnce(&DiagnosticSpan) -> T,
    ) -> T {
        let span = DiagnosticSpan {
            recorder: self.clone(),
            started: Instant::now(),
            span_id: SpanId::new(),
            start,
            requested_coordination,
            requested_status: OnceLock::new(),
        };
        let mut requested = PhaseObservation::not_started()
            .with_retry_count(span.start.retry_count)
            .with_optional_busy_timeout(span.start.busy_timeout_millis);
        if span.start.sqlite.is_some() {
            requested = requested.with_sqlite_evidence(
                SqlitePhaseEvidence::for_phase(SqliteTransactionPhase::Requested)
                    .with_gap(SqliteMeasurementGap::WriterAuthorityNotReached)
                    .with_gap(SqliteMeasurementGap::ExecutionNotReached)
                    .with_gap(SqliteMeasurementGap::CommitNotReached)
                    .with_gap(SqliteMeasurementGap::PostCommitNotReached)
                    .with_gap(SqliteMeasurementGap::RowsExaminedNotExposed)
                    .with_gap(SqliteMeasurementGap::RowsChangedNotReported),
            );
        }
        let requested_status = span.record(DiagnosticPhase::Requested, requested);
        let _ = span.requested_status.set(requested_status);
        operation(&span)
    }

    /// Opportunistically retires only shards whose exact producer process is no
    /// longer live. The recorder writer performs the filesystem work.
    pub fn cleanup_stale_shards(&self) -> CleanupReport {
        let Some(writer) = self.inner.writer.as_ref() else {
            return CleanupReport {
                issues: vec!["recorder_disabled".to_string()],
                ..CleanupReport::default()
            };
        };
        let (completion, result) = mpsc::channel();
        if writer.send(WriterCommand::Cleanup { completion }).is_err() {
            return CleanupReport {
                issues: vec!["writer_disconnected".to_string()],
                ..CleanupReport::default()
            };
        }
        result.recv().unwrap_or_else(|_| CleanupReport {
            issues: vec!["writer_disconnected".to_string()],
            ..CleanupReport::default()
        })
    }

    fn append(&self, event: &DiagnosticEvent, coordination: AppendCoordination) -> RecordStatus {
        let Ok(mut encoded) = serde_json::to_vec(event) else {
            return coordinated_record_failure(coordination, "serialize");
        };
        encoded.push(b'\n');
        if encoded.len() as u64 > self.inner.max_shard_bytes {
            return coordinated_record_failure(coordination, "record_too_large");
        }
        let Some(writer) = self.inner.writer.as_ref() else {
            coordinated_gap(coordination, "recorder_disabled");
            return RecordStatus::Disabled;
        };
        match coordination {
            AppendCoordination::Synchronous => {
                let (completion, result) = mpsc::channel();
                if writer
                    .send(WriterCommand::Append {
                        encoded,
                        completion: Some(completion),
                    })
                    .is_err()
                {
                    return record_failure("writer_disconnected");
                }
                result
                    .recv()
                    .unwrap_or_else(|_| record_failure("writer_disconnected"))
            }
            AppendCoordination::Deferred => match writer.try_send(WriterCommand::Append {
                encoded,
                completion: None,
            }) {
                Ok(()) => RecordStatus::Queued,
                Err(TrySendError::Full(_)) => deferred_record_failure("deferred_queue_full"),
                Err(TrySendError::Disconnected(_)) => {
                    deferred_record_failure("deferred_queue_disconnected")
                }
            },
        }
    }

    #[cfg(test)]
    pub(crate) fn drain_deferred_for_test(&self) -> Result<(), String> {
        let writer = self
            .inner
            .writer
            .as_ref()
            .ok_or_else(|| "recorder_disabled".to_string())?;
        let (completion, result) = mpsc::channel();
        writer
            .send(WriterCommand::Drain { completion })
            .map_err(|_| "writer_disconnected".to_string())?;
        result.recv().map_err(|_| "writer_disconnected".to_string())
    }

    #[cfg(test)]
    pub(crate) fn block_writer_for_test(&self) -> Result<mpsc::Sender<()>, String> {
        let writer = self
            .inner
            .writer
            .as_ref()
            .ok_or_else(|| "recorder_disabled".to_string())?;
        let (entered, entered_result) = mpsc::channel();
        let (release, release_result) = mpsc::channel();
        writer
            .send(WriterCommand::Block {
                entered,
                release: release_result,
            })
            .map_err(|_| "writer_disconnected".to_string())?;
        entered_result
            .recv()
            .map_err(|_| "writer_disconnected".to_string())?;
        Ok(release)
    }
}

pub struct DiagnosticSpan {
    recorder: FlightRecorder,
    started: Instant,
    span_id: SpanId,
    start: SpanStart,
    requested_coordination: AppendCoordination,
    requested_status: OnceLock<RecordStatus>,
}

impl DiagnosticSpan {
    pub fn diagnostic_id(&self) -> &DiagnosticId {
        &self.start.diagnostic_id
    }

    pub fn span_id(&self) -> &SpanId {
        &self.span_id
    }

    pub fn requested_status(&self) -> Option<&RecordStatus> {
        self.requested_status.get()
    }

    /// Starts a child resource span on this span's already-selected recorder.
    /// Reusing the recorder is important when the parent holds database
    /// authority: it prevents a failed global recorder initialization from
    /// being retried through filesystem I/O under that authority.
    pub(crate) fn with_deferred_requested_span<T>(
        &self,
        start: SpanStart,
        operation: impl FnOnce(&DiagnosticSpan) -> T,
    ) -> T {
        self.recorder.with_deferred_requested_span(start, operation)
    }

    /// Records a completed child observation on this span's already-selected
    /// recorder without waiting for recorder capacity or file I/O. This is the
    /// late-observation counterpart to `with_deferred_requested_span` for work
    /// performed while the parent still holds database authority.
    pub(crate) fn record_deferred_completed_child(
        &self,
        mut start: SpanStart,
        elapsed: Duration,
        phase: DiagnosticPhase,
        observation: PhaseObservation,
    ) -> RecordStatus {
        start.diagnostic_id = self.start.diagnostic_id.clone();
        start.parent_span_id = Some(self.span_id.clone());
        self.recorder
            .record_completed_observation_with_coordination(
                start,
                elapsed,
                phase,
                observation,
                AppendCoordination::Deferred,
            )
    }

    pub fn record(
        &self,
        phase: DiagnosticPhase,
        mut observation: PhaseObservation,
    ) -> RecordStatus {
        let elapsed_micros = saturating_micros(self.started.elapsed());
        if observation.busy_timeout_millis.is_none() {
            observation.busy_timeout_millis = self.start.busy_timeout_millis;
        }
        if observation.retry_count.is_none() {
            observation.retry_count = Some(self.start.retry_count);
        }
        for cause in &mut observation.causes {
            *cause = redact_text(cause);
        }
        self.recorder.append(
            &DiagnosticEvent {
                schema_version: DIAGNOSTIC_SCHEMA_VERSION,
                event_id: EventId::new(),
                diagnostic_id: self.start.diagnostic_id.clone(),
                span_id: self.span_id.clone(),
                parent_span_id: self.start.parent_span_id.clone(),
                recorded_at: now(),
                elapsed_micros,
                process: self.recorder.inner.process.clone(),
                operation: self.start.operation.clone(),
                resource: self.start.resource.clone(),
                lifecycle_phase: self.start.lifecycle_phase.clone(),
                phase,
                observation,
                correlations: self.start.correlations.clone(),
                sqlite: self.start.sqlite.clone(),
            },
            if phase == DiagnosticPhase::Requested {
                self.requested_coordination
            } else {
                AppendCoordination::Deferred
            },
        )
    }
}

pub fn default_recorder_root() -> Result<PathBuf, String> {
    crate::paths::data_dir().map(|root| root.join(RECORDER_DIRECTORY))
}

pub fn process_recorder() -> FlightRecorder {
    // Initialize the optional reporter before resolving the recorder root so an
    // initialization failure can still receive a best-effort handoff.
    ensure_gap_reporter();
    #[cfg(test)]
    {
        return TEST_PROCESS_RECORDER
            .with(|recorder| recorder.borrow().clone())
            .unwrap_or_else(FlightRecorder::disabled);
    }
    #[cfg(not(test))]
    {
        static PROCESS_RECORDER: OnceLock<Mutex<Option<FlightRecorder>>> = OnceLock::new();
        cached_or_retry_process_recorder(PROCESS_RECORDER.get_or_init(|| Mutex::new(None)), || {
            default_recorder_root()
                .and_then(|root| FlightRecorder::open(root, RecorderConfig::default()))
        })
    }
}

fn cached_or_retry_process_recorder(
    slot: &Mutex<Option<FlightRecorder>>,
    initialize: impl FnOnce() -> Result<FlightRecorder, String>,
) -> FlightRecorder {
    let mut guard = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(recorder) = guard.as_ref() {
        return recorder.clone();
    }
    match initialize() {
        Ok(recorder) => {
            *guard = Some(recorder.clone());
            recorder
        }
        Err(_) => {
            emit_gap_once("process_recorder_init");
            FlightRecorder::disabled()
        }
    }
}

#[cfg(test)]
thread_local! {
    static TEST_PROCESS_RECORDER: std::cell::RefCell<Option<FlightRecorder>> =
        const { std::cell::RefCell::new(None) };
    static TEST_GAP_REPORTER: std::cell::RefCell<Option<SyncSender<&'static str>>> =
        const { std::cell::RefCell::new(None) };
    static TEST_GAP_HANDOFF_ATTEMPTS: std::cell::RefCell<Vec<&'static str>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn gap_handoff_attempts_for_test() -> Vec<&'static str> {
    TEST_GAP_HANDOFF_ATTEMPTS.with(|attempts| attempts.borrow().clone())
}

#[cfg(test)]
struct TestRecorderReset(Option<FlightRecorder>);

#[cfg(test)]
struct TestGapReporterReset(Option<SyncSender<&'static str>>);

#[cfg(test)]
impl Drop for TestRecorderReset {
    fn drop(&mut self) {
        TEST_PROCESS_RECORDER.with(|recorder| {
            *recorder.borrow_mut() = self.0.take();
        });
    }
}

#[cfg(test)]
impl Drop for TestGapReporterReset {
    fn drop(&mut self) {
        TEST_GAP_REPORTER.with(|reporter| {
            *reporter.borrow_mut() = self.0.take();
        });
    }
}

#[cfg(test)]
pub(crate) fn with_test_process_recorder<T>(
    recorder: FlightRecorder,
    operation: impl FnOnce() -> T,
) -> T {
    let previous = TEST_PROCESS_RECORDER.with(|slot| slot.borrow_mut().replace(recorder));
    let _reset = TestRecorderReset(previous);
    operation()
}

#[cfg(test)]
fn with_test_gap_reporter<T>(
    reporter: SyncSender<&'static str>,
    operation: impl FnOnce() -> T,
) -> T {
    let previous = TEST_GAP_REPORTER.with(|slot| slot.borrow_mut().replace(reporter));
    let _reset = TestGapReporterReset(previous);
    operation()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceCoordinate {
    pub file: String,
    pub line: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedEvent {
    pub source: SourceCoordinate,
    pub event: DiagnosticEvent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectionIssue {
    pub source: Option<SourceCoordinate>,
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct InspectionCoverage {
    /// Directory entries whose names were examined for shard discovery.
    pub directory_entries_examined: usize,
    /// True when discovery stopped at its hard directory-work bound. Entries
    /// beyond the bound have unknown names and sizes and are therefore not
    /// included in files_skipped or bytes_skipped.
    pub directory_limit_reached: bool,
    pub files_discovered: usize,
    pub files_seen: usize,
    pub files_skipped: usize,
    pub bytes_read: u64,
    pub bytes_skipped: u64,
    pub records_seen: usize,
    pub duplicate_records: usize,
    pub first_recorded_at: Option<String>,
    pub last_recorded_at: Option<String>,
    pub skipped_records: usize,
    /// Bytes read from retention-status.json, separately from shard bytes.
    pub retention_status_bytes_read: u64,
    /// Known retention-status bytes omitted at its independent hard bound.
    pub retention_status_bytes_skipped: u64,
    /// True when retention-status.json exceeded its hard read bound.
    pub retention_status_limit_reached: bool,
    pub retention: Option<CleanupReport>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectionReport {
    pub events: Vec<RecordedEvent>,
    pub coverage: InspectionCoverage,
    pub issues: Vec<InspectionIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoalescedFailure {
    pub operation: String,
    pub resource: String,
    pub phase: DiagnosticPhase,
    pub primary_code: Option<String>,
    pub extended_code: Option<i32>,
    #[serde(default)]
    pub failure_discriminator: String,
    pub first_recorded_at: String,
    pub last_recorded_at: String,
    pub count: usize,
    pub sources: Vec<SourceCoordinate>,
    pub diagnostic_ids: Vec<DiagnosticId>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoalescedReport {
    pub failures: Vec<CoalescedFailure>,
    pub coverage: InspectionCoverage,
    pub issues: Vec<InspectionIssue>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentFailuresReport {
    pub raw: InspectionReport,
    pub coalesced: CoalescedReport,
}

pub struct FlightRecorderReader {
    root: PathBuf,
}

impl FlightRecorderReader {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    pub fn inspect(&self) -> InspectionReport {
        self.inspect_with_limits(
            MAX_INSPECTION_DIRECTORY_ENTRIES,
            MAX_INSPECTION_SHARDS,
            MAX_INSPECTION_BYTES,
        )
    }

    fn inspect_with_limits(
        &self,
        max_directory_entries: usize,
        max_shards: usize,
        max_bytes: u64,
    ) -> InspectionReport {
        self.inspect_with_limits_after_discovery(
            max_directory_entries,
            max_shards,
            max_bytes,
            |_| {},
        )
    }

    fn inspect_with_limits_after_discovery(
        &self,
        max_directory_entries: usize,
        max_shards: usize,
        max_bytes: u64,
        after_discovery: impl FnOnce(&[ShardCandidate]),
    ) -> InspectionReport {
        let mut report = InspectionReport::default();
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) => {
                report.issues.push(InspectionIssue {
                    source: None,
                    kind: if error.kind() == std::io::ErrorKind::NotFound {
                        "missing_root"
                    } else {
                        "unreadable_root"
                    }
                    .to_string(),
                    message: if error.kind() == std::io::ErrorKind::NotFound {
                        "diagnostic recorder root does not exist"
                    } else {
                        "diagnostic recorder root could not be read"
                    }
                    .to_string(),
                });
                return report;
            }
        };
        read_cleanup_status(&self.root, &mut report);
        let mut candidates = Vec::new();
        for entry in entries.take(max_directory_entries) {
            report.coverage.directory_entries_examined += 1;
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    report.issues.push(InspectionIssue {
                        source: None,
                        kind: "directory_entry_unreadable".to_string(),
                        message: "a diagnostic recorder directory entry could not be read"
                            .to_string(),
                    });
                    continue;
                }
            };
            let path = entry.path();
            if !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("flight-") && name.contains(".jsonl"))
            {
                continue;
            }
            report.coverage.files_discovered += 1;
            match entry.metadata() {
                Ok(metadata) => match shard_file_identity(&metadata) {
                    Some(identity) => candidates.push(ShardCandidate {
                        path,
                        bytes: metadata.len(),
                        modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                        identity,
                    }),
                    None => {
                        report.coverage.files_skipped += 1;
                        report.coverage.bytes_skipped =
                            report.coverage.bytes_skipped.saturating_add(metadata.len());
                        report.issues.push(InspectionIssue {
                            source: Some(source(&path, 1)),
                            kind: "shard_identity_unavailable".to_string(),
                            message: "shard identity could not be established conservatively"
                                .to_string(),
                        });
                    }
                },
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    report.issues.push(InspectionIssue {
                        source: Some(source(&path, 1)),
                        kind: "concurrent_disappearance".to_string(),
                        message: "shard disappeared during inspection".to_string(),
                    });
                }
                Err(_) => report.issues.push(InspectionIssue {
                    source: Some(source(&path, 1)),
                    kind: "unreadable_shard".to_string(),
                    message: "shard metadata could not be read".to_string(),
                }),
            }
        }
        // Do not probe for one more entry: even detecting whether an entry was
        // omitted would exceed the hard directory-work cap. Reaching the cap is
        // therefore reported conservatively as possible omitted coverage.
        if report.coverage.directory_entries_examined == max_directory_entries {
            report.coverage.directory_limit_reached = true;
            report.issues.push(InspectionIssue {
                source: None,
                kind: "inspection_directory_limit".to_string(),
                message: "additional directory entries may have been omitted at the inspection discovery bound"
                    .to_string(),
            });
        }
        candidates.sort_by(|left, right| {
            right
                .modified
                .cmp(&left.modified)
                .then_with(|| right.path.cmp(&left.path))
        });
        if candidates.len() > max_shards {
            let skipped = candidates.split_off(max_shards);
            report.coverage.files_skipped += skipped.len();
            report.coverage.bytes_skipped = report.coverage.bytes_skipped.saturating_add(
                skipped.iter().fold(0_u64, |total, candidate| {
                    total.saturating_add(candidate.bytes)
                }),
            );
            report.issues.push(InspectionIssue {
                source: None,
                kind: "inspection_shard_limit".to_string(),
                message: "older shards were skipped at the inspection shard bound".to_string(),
            });
        }
        let mut selected = Vec::new();
        let mut selected_bytes = 0_u64;
        let mut skipped_for_bytes = false;
        for candidate in candidates {
            if candidate.bytes > max_bytes.saturating_sub(selected_bytes) {
                report.coverage.files_skipped += 1;
                report.coverage.bytes_skipped = report
                    .coverage
                    .bytes_skipped
                    .saturating_add(candidate.bytes);
                skipped_for_bytes = true;
            } else {
                selected_bytes = selected_bytes.saturating_add(candidate.bytes);
                selected.push(candidate);
            }
        }
        if skipped_for_bytes {
            report.issues.push(InspectionIssue {
                source: None,
                kind: "inspection_byte_limit".to_string(),
                message: "shards were skipped at the inspection byte bound".to_string(),
            });
        }
        selected.sort_by(|left, right| left.path.cmp(&right.path));
        after_discovery(&selected);
        for candidate in selected {
            report.coverage.files_seen += 1;
            let remaining = max_bytes.saturating_sub(report.coverage.bytes_read);
            read_shard(&candidate, remaining, &mut report);
        }
        report.events.sort_by(|left, right| {
            left.event
                .recorded_at
                .cmp(&right.event.recorded_at)
                .then_with(|| left.event.event_id.cmp(&right.event.event_id))
        });
        let mut event_ids = HashSet::new();
        let sorted_events = std::mem::take(&mut report.events);
        for record in sorted_events {
            if event_ids.insert(record.event.event_id.clone()) {
                report.events.push(record);
            } else {
                report.coverage.duplicate_records += 1;
                report.coverage.skipped_records += 1;
                report.issues.push(InspectionIssue {
                    source: Some(record.source),
                    kind: "duplicate_event_id".to_string(),
                    message: "duplicate event ID was ignored during inspection".to_string(),
                });
            }
        }
        report.coverage.records_seen = report.events.len();
        report.coverage.first_recorded_at = report
            .events
            .first()
            .map(|record| record.event.recorded_at.clone());
        report.coverage.last_recorded_at = report
            .events
            .last()
            .map(|record| record.event.recorded_at.clone());
        report
    }

    pub fn recent_failures(&self, limit: usize) -> InspectionReport {
        self.recent_and_coalesced_failures(limit).raw
    }

    pub fn trace(&self, diagnostic_id: &DiagnosticId) -> InspectionReport {
        let mut report = self.inspect();
        report
            .events
            .retain(|record| &record.event.diagnostic_id == diagnostic_id);
        report
    }

    pub fn coalesced_failures(&self, limit: usize) -> CoalescedReport {
        self.recent_and_coalesced_failures(limit).coalesced
    }

    pub fn recent_and_coalesced_failures(&self, limit: usize) -> RecentFailuresReport {
        let mut report = self.inspect();
        report.events.retain(|record| {
            record.event.phase.is_failure() || record.event.observation.sqlite_failure.is_some()
        });
        if report.events.len() > limit {
            report.events.drain(..report.events.len() - limit);
        }
        let coalesced = coalesce_failures(&report);
        RecentFailuresReport {
            raw: report,
            coalesced,
        }
    }
}

fn coalesce_failures(report: &InspectionReport) -> CoalescedReport {
    let mut grouped: HashMap<FailureKey, CoalescedFailure> = HashMap::new();
    for record in &report.events {
        let sqlite = record.event.observation.sqlite_failure.as_ref();
        let failure_discriminator = failure_discriminator(&record.event);
        let key = FailureKey {
            operation: record.event.operation.clone(),
            resource: record.event.resource.clone(),
            phase: record.event.phase,
            primary_code: sqlite.and_then(|failure| failure.primary_code.clone()),
            extended_code: sqlite.and_then(|failure| failure.extended_code),
            failure_discriminator: failure_discriminator.clone(),
        };
        let group = grouped.entry(key).or_insert_with(|| CoalescedFailure {
            operation: record.event.operation.clone(),
            resource: record.event.resource.clone(),
            phase: record.event.phase,
            primary_code: sqlite.and_then(|failure| failure.primary_code.clone()),
            extended_code: sqlite.and_then(|failure| failure.extended_code),
            failure_discriminator,
            first_recorded_at: record.event.recorded_at.clone(),
            last_recorded_at: record.event.recorded_at.clone(),
            count: 0,
            sources: Vec::new(),
            diagnostic_ids: Vec::new(),
        });
        group.count += 1;
        group.last_recorded_at = record.event.recorded_at.clone();
        group.sources.push(record.source.clone());
        if !group.diagnostic_ids.contains(&record.event.diagnostic_id) {
            group
                .diagnostic_ids
                .push(record.event.diagnostic_id.clone());
        }
    }
    let mut failures = grouped.into_values().collect::<Vec<_>>();
    failures.sort_by(|left, right| {
        left.first_recorded_at
            .cmp(&right.first_recorded_at)
            .then_with(|| left.operation.cmp(&right.operation))
    });
    CoalescedReport {
        failures,
        coverage: report.coverage.clone(),
        issues: report.issues.clone(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FailureKey {
    operation: String,
    resource: String,
    phase: DiagnosticPhase,
    primary_code: Option<String>,
    extended_code: Option<i32>,
    failure_discriminator: String,
}

struct ShardCandidate {
    path: PathBuf,
    bytes: u64,
    modified: SystemTime,
    identity: ShardFileIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShardFileIdentity {
    #[cfg(unix)]
    Unix { device: u64, inode: u64 },
    #[cfg(windows)]
    Windows { volume: u32, index: u64 },
}

#[cfg(unix)]
fn shard_file_identity(metadata: &fs::Metadata) -> Option<ShardFileIdentity> {
    use std::os::unix::fs::MetadataExt;

    Some(ShardFileIdentity::Unix {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(windows)]
fn shard_file_identity(metadata: &fs::Metadata) -> Option<ShardFileIdentity> {
    use std::os::windows::fs::MetadataExt;

    Some(ShardFileIdentity::Windows {
        volume: metadata.volume_serial_number()?,
        index: metadata.file_index()?,
    })
}

#[cfg(not(any(unix, windows)))]
fn shard_file_identity(_metadata: &fs::Metadata) -> Option<ShardFileIdentity> {
    None
}

impl PhaseObservation {
    fn with_optional_busy_timeout(mut self, timeout_millis: Option<u64>) -> Self {
        self.busy_timeout_millis = timeout_millis;
        self
    }
}

fn read_shard(candidate: &ShardCandidate, byte_budget: u64, report: &mut InspectionReport) {
    let path = &candidate.path;
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            report.issues.push(InspectionIssue {
                source: Some(source(path, 1)),
                kind: "concurrent_disappearance".to_string(),
                message: "shard disappeared during inspection".to_string(),
            });
            return;
        }
        Err(error) => {
            report.issues.push(InspectionIssue {
                source: Some(source(path, 1)),
                kind: "unreadable_shard".to_string(),
                message: redact_text(&error.to_string()),
            });
            return;
        }
    };
    let Some(opened_identity) = file
        .metadata()
        .ok()
        .and_then(|metadata| shard_file_identity(&metadata))
    else {
        report.coverage.files_skipped += 1;
        report.coverage.bytes_skipped = report
            .coverage
            .bytes_skipped
            .saturating_add(candidate.bytes);
        report.issues.push(InspectionIssue {
            source: Some(source(path, 1)),
            kind: "shard_identity_unavailable".to_string(),
            message: "opened shard identity could not be established conservatively".to_string(),
        });
        return;
    };
    if opened_identity != candidate.identity {
        report.coverage.files_skipped += 1;
        report.coverage.bytes_skipped = report
            .coverage
            .bytes_skipped
            .saturating_add(candidate.bytes);
        report.issues.push(InspectionIssue {
            source: Some(source(path, 1)),
            kind: "concurrent_replacement".to_string(),
            message: "shard pathname referred to a different file when opened".to_string(),
        });
        return;
    }
    let mut reader = BufReader::new(file.take(byte_budget));
    let mut line = Vec::new();
    let mut line_number = 0;
    let mut bytes_read = 0_u64;
    let mut reported_limit = false;
    loop {
        line.clear();
        let read = match reader.read_until(b'\n', &mut line) {
            Ok(read) => read,
            Err(error) => {
                report.issues.push(InspectionIssue {
                    source: Some(source(path, line_number + 1)),
                    kind: "read_error".to_string(),
                    message: redact_text(&error.to_string()),
                });
                break;
            }
        };
        if read == 0 {
            break;
        }
        bytes_read = bytes_read.saturating_add(read as u64);
        report.coverage.bytes_read = report.coverage.bytes_read.saturating_add(read as u64);
        line_number += 1;
        let terminated = line.last() == Some(&b'\n');
        if terminated {
            line.pop();
        }
        if line.len() > MAX_READ_RECORD_BYTES {
            report.coverage.skipped_records += 1;
            report.issues.push(InspectionIssue {
                source: Some(source(path, line_number)),
                kind: "oversized_record".to_string(),
                message: "record exceeded the reader bound".to_string(),
            });
            continue;
        }
        let more_bytes_exist = !terminated
            && bytes_read == byte_budget
            && reader
                .get_ref()
                .get_ref()
                .metadata()
                .map(|metadata| metadata.len() > bytes_read)
                .unwrap_or(false);
        if more_bytes_exist {
            report.coverage.skipped_records += 1;
            report.issues.push(InspectionIssue {
                source: Some(source(path, line_number)),
                kind: "inspection_byte_limit".to_string(),
                message: "record was skipped at the inspection byte bound".to_string(),
            });
            reported_limit = true;
            break;
        }
        match serde_json::from_slice::<DiagnosticEvent>(&line) {
            Ok(event) if event.schema_version == DIAGNOSTIC_SCHEMA_VERSION => {
                report.events.push(RecordedEvent {
                    source: source(path, line_number),
                    event,
                });
            }
            Ok(_) => {
                report.coverage.skipped_records += 1;
                report.issues.push(InspectionIssue {
                    source: Some(source(path, line_number)),
                    kind: "unsupported_schema".to_string(),
                    message: "record schema version is not supported".to_string(),
                });
            }
            Err(error) => {
                report.coverage.skipped_records += 1;
                report.issues.push(InspectionIssue {
                    source: Some(source(path, line_number)),
                    kind: if terminated {
                        "invalid_record"
                    } else {
                        "truncated_tail"
                    }
                    .to_string(),
                    message: redact_text(&error.to_string()),
                });
            }
        }
    }
    if let Ok(metadata) = reader.get_ref().get_ref().metadata() {
        if metadata.len() > bytes_read {
            report.coverage.bytes_skipped = report
                .coverage
                .bytes_skipped
                .saturating_add(metadata.len() - bytes_read);
            if !reported_limit {
                report.issues.push(InspectionIssue {
                    source: Some(source(path, line_number.saturating_add(1))),
                    kind: "inspection_byte_limit".to_string(),
                    message: "shard growth was skipped at the inspection byte bound".to_string(),
                });
            }
        }
    }
}

fn source(path: &Path, line: u64) -> SourceCoordinate {
    SourceCoordinate {
        file: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "unknown-shard".to_string()),
        line,
    }
}

fn writer_loop(mut writer: WriterState, receiver: Receiver<WriterCommand>) {
    while let Ok(command) = receiver.recv() {
        match command {
            WriterCommand::Append {
                encoded,
                completion,
            } => {
                let status = append_encoded(&mut writer, &encoded);
                if let Some(completion) = completion {
                    let _ = completion.send(status);
                }
            }
            WriterCommand::Cleanup { completion } => {
                let _ = completion.send(cleanup_stale_shards(&writer));
            }
            #[cfg(test)]
            WriterCommand::Drain { completion } => {
                let _ = completion.send(());
            }
            #[cfg(test)]
            WriterCommand::Block { entered, release } => {
                let _ = entered.send(());
                let _ = release.recv();
            }
        }
        // This thread never owns a protected database transaction. Once it can
        // make progress, it may safely surface producer-path gap notifications.
        emit_pending_gaps();
    }
}

fn append_encoded(writer: &mut WriterState, encoded: &[u8]) -> RecordStatus {
    if encoded.len() as u64 > writer.config.max_shard_bytes {
        return record_failure("record_too_large");
    }
    match should_rotate(writer, encoded.len() as u64) {
        Ok(true) if rotate(writer).is_err() => return record_failure("rotate"),
        Ok(_) => {}
        Err(_) => return record_failure("shard_metadata"),
    }
    if writer.active_file.write_all(encoded).is_err() {
        return record_failure("append");
    }
    if writer.active_file.flush().is_err() {
        return record_failure("flush");
    }
    RecordStatus::Appended
}

fn should_rotate(writer: &WriterState, incoming: u64) -> std::io::Result<bool> {
    writer
        .active_file
        .metadata()
        .map(|metadata| metadata.len().saturating_add(incoming) > writer.config.max_shard_bytes)
}

fn rotate(writer: &mut WriterState) -> std::io::Result<()> {
    if writer.config.max_shards == 1 {
        writer.active_file.set_len(0)?;
        return writer.active_file.flush();
    }
    writer.active_file.flush()?;
    let oldest = rotated_path(writer, writer.config.max_shards - 1);
    if oldest.exists() {
        fs::remove_file(oldest)?;
    }
    for ordinal in (1..writer.config.max_shards - 1).rev() {
        let source = rotated_path(writer, ordinal);
        if source.exists() {
            fs::rename(source, rotated_path(writer, ordinal + 1))?;
        }
    }
    if writer.active_path.exists() {
        fs::rename(&writer.active_path, rotated_path(writer, 1))?;
    }
    match open_leased_private_append_file(&writer.active_path) {
        Ok(new_active_file) => {
            writer.active_file = new_active_file;
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_file(&writer.active_path);
            let _ = fs::rename(rotated_path(writer, 1), &writer.active_path);
            Err(error)
        }
    }
}

fn rotated_path(writer: &WriterState, ordinal: usize) -> PathBuf {
    writer
        .active_path
        .with_file_name(format!("{}.jsonl.{ordinal}", writer.shard_prefix))
}

fn cleanup_stale_shards(writer: &WriterState) -> CleanupReport {
    let mut report = CleanupReport::default();
    let lock_path = writer.root.join(".retention.lock");
    let lock = match private_append_file(&lock_path) {
        Ok(lock) => lock,
        Err(_) => {
            cleanup_issue(&mut report, "retention_lock_open_failed");
            return report;
        }
    };
    if <fs::File as fs4::FileExt>::try_lock(&lock).is_err() {
        cleanup_issue(&mut report, "retention_sweep_busy");
        return report;
    }

    let entries = match fs::read_dir(&writer.root) {
        Ok(entries) => entries,
        Err(_) => {
            cleanup_issue(&mut report, "retention_scan_failed");
            let _ = <fs::File as fs4::FileExt>::unlock(&lock);
            return report;
        }
    };
    let mut paths = Vec::new();
    for entry in entries.take(MAX_CLEANUP_DIRECTORY_ENTRIES) {
        report.directory_entries_examined += 1;
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                report.directory_entries_unreadable += 1;
                cleanup_issue(&mut report, "retention_directory_entry_unreadable");
                continue;
            }
        };
        let path = entry.path();
        if is_flight_shard(&path) {
            paths.push(path);
        }
    }
    // As with offline inspection, do not consume entry N+1 merely to refine
    // the coverage result. Hitting the cap means aggregate satisfaction cannot
    // be established for this cleanup generation.
    if report.directory_entries_examined == MAX_CLEANUP_DIRECTORY_ENTRIES {
        report.directory_limit_reached = true;
        cleanup_issue(&mut report, "retention_directory_limit");
    }
    report.scanned_shards = paths.len();
    let mut inactive = Vec::new();
    let mut disappeared = 0_usize;
    for path in paths {
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(&writer.shard_prefix))
        {
            report.retained_live_shards += 1;
            continue;
        }
        let shard_lease = match try_shard_lease(&path) {
            ShardLease::Acquired(file) => file,
            ShardLease::Busy => {
                report.retained_live_shards += 1;
                continue;
            }
            ShardLease::Disappeared => {
                disappeared += 1;
                cleanup_issue(
                    &mut report,
                    format!(
                        "shard_disappeared:{}",
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("unknown")
                    ),
                );
                continue;
            }
            ShardLease::Uncertain => {
                report.retained_uncertain_shards += 1;
                cleanup_issue(
                    &mut report,
                    format!(
                        "shard_lease_uncertain:{}",
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("unknown")
                    ),
                );
                continue;
            }
        };
        let Some((pid, starttime, boot_fingerprint)) = shard_process_identity(&path) else {
            report.retained_uncertain_shards += 1;
            cleanup_issue(
                &mut report,
                format!(
                    "shard_identity_uncertain:{}",
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("unknown")
                ),
            );
            continue;
        };
        match shard_liveness(pid, starttime, &boot_fingerprint) {
            ShardLiveness::Live => report.retained_live_shards += 1,
            ShardLiveness::Uncertain => report.retained_uncertain_shards += 1,
            ShardLiveness::Inactive => {
                let modified = fs::metadata(&path)
                    .and_then(|metadata| metadata.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                inactive.push((modified, path, shard_lease));
            }
        }
    }
    inactive.sort_by_key(|(modified, _, _)| *modified);
    let now = SystemTime::now();
    let mut retired = Vec::new();
    for (modified, path, _) in &inactive {
        let age = now.duration_since(*modified).unwrap_or_default();
        if age >= writer.config.stale_shard_age && retire_shard(path, &mut report) {
            retired.push(path.clone());
        }
    }
    let mut retained = report
        .scanned_shards
        .saturating_sub(retired.len())
        .saturating_sub(disappeared);
    if retained > writer.config.max_total_shards {
        for (_, path, _) in &inactive {
            if retained <= writer.config.max_total_shards {
                break;
            }
            if !retired.contains(path) && retire_shard(path, &mut report) {
                retired.push(path.clone());
                retained -= 1;
            }
        }
    }
    report.retired_shards = retired.len();
    report.aggregate_limit_satisfied = retained <= writer.config.max_total_shards
        && !report.directory_limit_reached
        && report.directory_entries_unreadable == 0
        && !report.issue_limit_reached;
    if report.directory_limit_reached || report.directory_entries_unreadable > 0 {
        cleanup_issue(
            &mut report,
            "aggregate_limit_unsatisfied_incomplete_directory_coverage",
        );
    }
    if retained > writer.config.max_total_shards {
        cleanup_issue(&mut report, "aggregate_limit_retained_live_or_uncertain");
    }
    if persist_cleanup_status(&writer.root, &report).is_err() {
        cleanup_issue(&mut report, "retention_status_write_failed");
    }
    let _ = <fs::File as fs4::FileExt>::unlock(&lock);
    report
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShardLiveness {
    Live,
    Inactive,
    Uncertain,
}

enum ShardLease {
    Acquired(fs::File),
    Busy,
    Disappeared,
    Uncertain,
}

fn try_shard_lease(path: &Path) -> ShardLease {
    let file = match OpenOptions::new().read(true).write(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ShardLease::Disappeared;
        }
        Err(_) => return ShardLease::Uncertain,
    };
    match <fs::File as fs4::FileExt>::try_lock(&file) {
        Ok(()) => ShardLease::Acquired(file),
        Err(fs4::TryLockError::WouldBlock) => ShardLease::Busy,
        Err(_) => ShardLease::Uncertain,
    }
}

fn shard_liveness(pid: i64, starttime: i64, boot_fingerprint: &str) -> ShardLiveness {
    if pid <= 0 || starttime <= 0 || boot_fingerprint == "unknown" {
        return ShardLiveness::Uncertain;
    }
    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    {
        return match crate::pid_identity::read_live_process_identity(pid) {
            Ok(Some(identity))
                if identity.os_pid_starttime_ticks == starttime
                    && fingerprint(&identity.os_boot_id) == boot_fingerprint =>
            {
                ShardLiveness::Live
            }
            Ok(Some(_)) | Ok(None) => ShardLiveness::Inactive,
            Err(_) => ShardLiveness::Uncertain,
        };
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = (pid, starttime, boot_fingerprint);
        ShardLiveness::Uncertain
    }
}

fn is_flight_shard(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("flight-") && name.contains(".jsonl"))
}

fn shard_process_identity(path: &Path) -> Option<(i64, i64, String)> {
    let name = path.file_name()?.to_str()?.strip_prefix("flight-")?;
    let mut components = name.splitn(4, '-');
    Some((
        components.next()?.parse().ok()?,
        components.next()?.parse().ok()?,
        components.next()?.to_string(),
    ))
}

fn fingerprint(value: &str) -> String {
    let digest = format!("{:x}", Sha256::digest(value.as_bytes()));
    digest[..16].to_string()
}

fn retire_shard(path: &Path, report: &mut CleanupReport) -> bool {
    match fs::remove_file(path) {
        Ok(()) => true,
        Err(_) => {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("unknown");
            cleanup_issue(report, format!("retire_failed:{name}"));
            false
        }
    }
}

fn cleanup_issue(report: &mut CleanupReport, issue: impl AsRef<str>) {
    if report.issues.len() < MAX_CLEANUP_ISSUES {
        report
            .issues
            .push(bounded_text(issue.as_ref(), MAX_CLEANUP_ISSUE_BYTES));
    } else {
        report.issue_limit_reached = true;
        report.issues_omitted = report.issues_omitted.saturating_add(1);
    }
}

fn persist_cleanup_status(root: &Path, report: &CleanupReport) -> std::io::Result<()> {
    let status_path = root.join("retention-status.json");
    let temporary_path = root.join(".retention-status.tmp");
    let encoded = serde_json::to_vec(report).map_err(std::io::Error::other)?;
    if encoded.len().saturating_add(1) as u64 > MAX_RETENTION_STATUS_READ_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "bounded retention status exceeded its reader bound",
        ));
    }
    if temporary_path.exists() {
        fs::remove_file(&temporary_path)?;
    }
    let mut file = private_append_file(&temporary_path)?;
    file.write_all(&encoded)?;
    file.write_all(b"\n")?;
    file.flush()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    drop(file);
    #[cfg(windows)]
    if status_path.exists() {
        fs::remove_file(&status_path)?;
    }
    fs::rename(&temporary_path, status_path)
}

fn read_cleanup_status(root: &Path, report: &mut InspectionReport) {
    let path = root.join("retention-status.json");
    let mut file = match fs::File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(_) => {
            report.issues.push(InspectionIssue {
                source: Some(source(&path, 1)),
                kind: "unreadable_retention_status".to_string(),
                message: "retention status could not be opened".to_string(),
            });
            return;
        }
    };
    let initial_len = match file.metadata() {
        Ok(metadata) => metadata.len(),
        Err(_) => {
            report.issues.push(InspectionIssue {
                source: Some(source(&path, 1)),
                kind: "unreadable_retention_status".to_string(),
                message: "retention status size could not be read".to_string(),
            });
            return;
        }
    };
    if initial_len > MAX_RETENTION_STATUS_READ_BYTES {
        report.coverage.retention_status_limit_reached = true;
        report.coverage.retention_status_bytes_skipped = initial_len;
        report.issues.push(InspectionIssue {
            source: Some(source(&path, 1)),
            kind: "oversized_retention_status".to_string(),
            message: "retention status exceeded its independent reader bound".to_string(),
        });
        return;
    }

    let mut bytes = Vec::with_capacity(initial_len as usize);
    let read_result = (&mut file)
        .take(MAX_RETENTION_STATUS_READ_BYTES)
        .read_to_end(&mut bytes);
    report.coverage.retention_status_bytes_read = bytes.len() as u64;
    if read_result.is_err() {
        report.issues.push(InspectionIssue {
            source: Some(source(&path, 1)),
            kind: "unreadable_retention_status".to_string(),
            message: "retention status could not be read".to_string(),
        });
        return;
    }

    let observed_len = file
        .metadata()
        .map(|metadata| metadata.len())
        .unwrap_or(initial_len);
    if observed_len > MAX_RETENTION_STATUS_READ_BYTES {
        report.coverage.retention_status_limit_reached = true;
        report.coverage.retention_status_bytes_skipped =
            observed_len.saturating_sub(report.coverage.retention_status_bytes_read);
        report.issues.push(InspectionIssue {
            source: Some(source(&path, 1)),
            kind: "oversized_retention_status".to_string(),
            message: "retention status exceeded its independent reader bound".to_string(),
        });
        return;
    }
    if bytes.last() != Some(&b'\n') {
        report.issues.push(InspectionIssue {
            source: Some(source(&path, 1)),
            kind: "truncated_retention_status".to_string(),
            message: "retention status lacked its completed-record terminator".to_string(),
        });
        return;
    }
    match serde_json::from_slice::<CleanupReport>(&bytes) {
        Ok(retention) => report.coverage.retention = Some(retention),
        Err(_) => report.issues.push(InspectionIssue {
            source: Some(source(&path, 1)),
            kind: "invalid_retention_status".to_string(),
            message: "retention status could not be decoded".to_string(),
        }),
    }
}

fn create_private_directory(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path)
        .map_err(|error| format!("Failed to create diagnostic recorder directory: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("Failed to secure diagnostic recorder directory: {error}"))?;
    }
    Ok(())
}

fn open_leased_private_append_file(path: &Path) -> std::io::Result<fs::File> {
    let file = private_append_file(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    <fs::File as fs4::FileExt>::try_lock(&file)?;
    Ok(file)
}

fn private_append_file(path: &Path) -> std::io::Result<fs::File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
}

#[cfg(unix)]
fn parent_pid() -> Option<i64> {
    let pid = unsafe { libc::getppid() };
    (pid > 0).then(|| i64::from(pid))
}

#[cfg(not(unix))]
fn parent_pid() -> Option<i64> {
    None
}

fn record_failure(stage: &'static str) -> RecordStatus {
    emit_gap_once(stage);
    RecordStatus::Failed {
        stage: stage.to_string(),
    }
}

fn deferred_record_failure(stage: &'static str) -> RecordStatus {
    defer_gap(stage);
    RecordStatus::Failed {
        stage: stage.to_string(),
    }
}

fn coordinated_record_failure(
    coordination: AppendCoordination,
    stage: &'static str,
) -> RecordStatus {
    match coordination {
        AppendCoordination::Synchronous => record_failure(stage),
        AppendCoordination::Deferred => deferred_record_failure(stage),
    }
}

fn coordinated_gap(coordination: AppendCoordination, stage: &'static str) {
    match coordination {
        AppendCoordination::Synchronous => emit_gap_once(stage),
        AppendCoordination::Deferred => defer_gap(stage),
    }
}

fn defer_gap(stage: &'static str) {
    let bit = 1_u64 << gap_stage_index(stage);
    PENDING_DIAGNOSTIC_GAP_STAGES.fetch_or(bit, Ordering::Relaxed);
}

fn emit_pending_gaps() {
    let pending = PENDING_DIAGNOSTIC_GAP_STAGES.swap(0, Ordering::AcqRel);
    for (index, stage) in GAP_STAGE_NAMES.iter().enumerate() {
        if pending & (1_u64 << index) != 0 {
            emit_gap_once(stage);
        }
    }
    if pending & (1_u64 << 63) != 0 {
        emit_gap_once("unknown");
    }
}

fn emit_gap_once(stage: &'static str) {
    #[cfg(test)]
    if TEST_GAP_REPORTER.with(|reporter| reporter.borrow().is_some()) {
        submit_gap_report(stage);
        return;
    }
    let bit = 1_u64 << gap_stage_index(stage);
    if DIAGNOSTIC_GAP_STAGES.fetch_or(bit, Ordering::Relaxed) & bit == 0 {
        submit_gap_report(stage);
    }
}

fn start_gap_reporter() -> Option<SyncSender<&'static str>> {
    let (sender, receiver) = mpsc::sync_channel(GAP_REPORTER_QUEUE_CAPACITY);
    std::thread::Builder::new()
        .name("oulipoly-diagnostic-gap-reporter".to_string())
        .spawn(move || gap_reporter_loop(receiver))
        .ok()
        .map(|_| sender)
}

fn ensure_gap_reporter() {
    let _ = GAP_REPORTER.get_or_init(start_gap_reporter);
}

fn submit_gap_report(stage: &'static str) {
    #[cfg(test)]
    {
        TEST_GAP_HANDOFF_ATTEMPTS.with(|attempts| attempts.borrow_mut().push(stage));
        let test_reporter = TEST_GAP_REPORTER.with(|reporter| reporter.borrow().clone());
        if let Some(reporter) = test_reporter {
            let _ = reporter.try_send(stage);
            return;
        }
    }
    if let Some(Some(reporter)) = GAP_REPORTER.get() {
        let _ = reporter.try_send(stage);
    }
}

fn gap_reporter_loop(receiver: Receiver<&'static str>) {
    while let Ok(stage) = receiver.recv() {
        // Do not retain the process stderr lock while waiting for the next
        // notification. Recorder operation/writer paths never wait for this
        // worker, including while its write is blocked.
        let stderr = std::io::stderr();
        write_gap_report(stderr.lock(), stage);
    }
}

#[cfg(test)]
fn write_gap_reports(receiver: Receiver<&'static str>, mut sink: impl Write) {
    while let Ok(stage) = receiver.recv() {
        write_gap_report(&mut sink, stage);
    }
}

fn write_gap_report(mut sink: impl Write, stage: &'static str) {
    // Reporting is diagnostic-only. A closed or failing stderr must not
    // terminate the worker through a panic or feed back into protected work.
    let _ = writeln!(sink, "oulipoly diagnostic gap: stage={stage}");
}

const GAP_STAGE_NAMES: [&str; 11] = [
    "process_recorder_init",
    "recorder_disabled",
    "serialize",
    "writer_disconnected",
    "deferred_queue_full",
    "record_too_large",
    "rotate",
    "append",
    "flush",
    "shard_metadata",
    "deferred_queue_disconnected",
];

fn gap_stage_index(stage: &str) -> u32 {
    GAP_STAGE_NAMES
        .iter()
        .position(|candidate| *candidate == stage)
        .map_or(63, |index| index as u32)
}

fn failure_discriminator(event: &DiagnosticEvent) -> String {
    let (kind, material) = if let Some(sqlite) = event.observation.sqlite_failure.as_ref() {
        (
            "sqlite",
            format!(
                "{}|{}|{}|{}",
                sqlite.primary_code.as_deref().unwrap_or("unknown"),
                sqlite
                    .extended_code
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                sqlite.contention,
                redact_text(&sqlite.message)
            ),
        )
    } else if !event.observation.causes.is_empty() {
        (
            "cause",
            event
                .observation
                .causes
                .iter()
                .map(|cause| redact_text(cause))
                .collect::<Vec<_>>()
                .join("|"),
        )
    } else {
        ("phase", format!("{:?}", event.phase))
    };
    format!("{kind}:{}", fingerprint(&material))
}

fn safe_key(value: &str) -> String {
    let filtered = value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
        .take(64)
        .collect::<String>();
    if filtered.is_empty() {
        "correlation".to_string()
    } else {
        filtered
    }
}

fn stable_sqlite_label(value: &str) -> String {
    let valid = !value.is_empty()
        && value.len() <= 128
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.' | ':')
        });
    if valid {
        value.to_ascii_lowercase()
    } else {
        "invalid_query_family".to_string()
    }
}

fn sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "token",
        "secret",
        "password",
        "credential",
        "cursor",
        "prompt",
        "transcript",
        "environment",
        "payload",
    ]
    .iter()
    .any(|sensitive| key.contains(sensitive))
}

fn redact_text(value: &str) -> String {
    let flattened = value.replace(['\r', '\n'], " ");
    let mut redact_next = false;
    let mut parts = Vec::new();
    for part in flattened.split_whitespace() {
        if redact_next {
            parts.push("[REDACTED]".to_string());
            redact_next = false;
            continue;
        }
        let lower = part.to_ascii_lowercase();
        if !sensitive_key(&lower) {
            parts.push(part.to_string());
            continue;
        }
        if let Some(delimiter) = part.find(['=', ':']) {
            let prefix = &part[..delimiter];
            parts.push(format!("{prefix}=[REDACTED]"));
            if delimiter + 1 == part.len() {
                redact_next = true;
            }
        } else {
            parts.push(part.to_string());
            redact_next = true;
        }
    }
    let redacted = parts.join(" ");
    bounded_text(&redacted, MAX_TEXT_BYTES)
}

fn bounded_text(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

fn saturating_micros(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

fn saturating_millis(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::sync::Arc;

    fn recorder(root: &Path, max_shard_bytes: u64, max_shards: usize) -> FlightRecorder {
        FlightRecorder::open(
            root,
            RecorderConfig {
                max_shard_bytes,
                max_shards,
                ..RecorderConfig::default()
            },
        )
        .unwrap()
    }

    fn failed_span(recorder: &FlightRecorder, operation: &str) -> DiagnosticId {
        let diagnostic_id = DiagnosticId::new();
        recorder.with_requested_span(
            SpanStart::new(operation, "state").with_diagnostic_id(diagnostic_id.clone()),
            |span| {
                span.record(
                    DiagnosticPhase::Failed,
                    PhaseObservation::started_unknown().with_cause("fixture failure"),
                );
            },
        );
        diagnostic_id
    }

    #[test]
    fn diagnostic_id_round_trips_display_and_parse() {
        let id = DiagnosticId::new();
        assert_eq!(id.to_string().parse::<DiagnosticId>().unwrap(), id);
    }

    #[test]
    fn requested_is_durable_before_operation_closure_runs() {
        let directory = tempfile::tempdir().unwrap();
        let recorder = recorder(directory.path(), 64 * 1024, 2);
        recorder.with_requested_span(
            SpanStart::new("begin", "state").with_lifecycle_phase("launch_admission"),
            |span| {
                assert_eq!(span.requested_status(), Some(&RecordStatus::Appended));
                let report = FlightRecorderReader::new(directory.path()).inspect();
                assert_eq!(report.events.len(), 1);
                assert_eq!(report.events[0].event.phase, DiagnosticPhase::Requested);
                assert_eq!(
                    report.events[0].event.lifecycle_phase.as_deref(),
                    Some("launch_admission")
                );
                assert_eq!(
                    report.events[0].event.observation.certainty,
                    OutcomeCertainty::NotStarted
                );
                assert_eq!(
                    span.record(
                        DiagnosticPhase::Acquired,
                        PhaseObservation::effects_possible().with_wait(Duration::from_micros(7))
                    ),
                    RecordStatus::Queued
                );
            },
        );
        recorder.drain_deferred_for_test().unwrap();
        let report = FlightRecorderReader::new(directory.path()).inspect();
        assert_eq!(
            report
                .events
                .iter()
                .find(|record| record.event.phase == DiagnosticPhase::Acquired)
                .unwrap()
                .event
                .observation
                .wait_micros,
            Some(7)
        );
        let mut legacy = serde_json::to_value(&report.events[0].event).unwrap();
        legacy.as_object_mut().unwrap().remove("lifecycle_phase");
        assert_eq!(
            serde_json::from_value::<DiagnosticEvent>(legacy)
                .unwrap()
                .lifecycle_phase,
            None
        );
    }

    #[test]
    fn disabled_recorder_exposes_status_without_changing_closure_result() {
        let recorder = FlightRecorder::disabled();
        let result = recorder.with_requested_span(SpanStart::new("disabled", "state"), |span| {
            assert_eq!(span.requested_status(), Some(&RecordStatus::Disabled));
            assert_eq!(
                span.record(DiagnosticPhase::Failed, PhaseObservation::started_unknown()),
                RecordStatus::Disabled
            );
            42
        });
        assert_eq!(result, 42);
    }

    #[test]
    fn test_process_recorder_is_injected_without_environment_mutation() {
        let directory = tempfile::tempdir().unwrap();
        let injected = recorder(directory.path(), 64 * 1024, 2);
        with_test_process_recorder(injected, || {
            process_recorder().with_requested_span(SpanStart::new("injected", "state"), |_| ());
        });
        assert_eq!(
            FlightRecorderReader::new(directory.path())
                .inspect()
                .events
                .len(),
            1
        );
        process_recorder()
            .with_requested_span(SpanStart::new("disabled-after-scope", "state"), |span| {
                assert_eq!(span.requested_status(), Some(&RecordStatus::Disabled))
            });
    }

    #[test]
    fn correlations_and_causes_cannot_emit_secret_values() {
        let directory = tempfile::tempdir().unwrap();
        let recorder = recorder(directory.path(), 64 * 1024, 2);
        let sentinel = "never-write-this-claim-capability";
        recorder.with_requested_span(
            SpanStart::new("redaction", "sidecar")
                .with_identifier("claim_token", sentinel)
                .with_hashed_correlation("wake_secret", sentinel)
                .with_hashed_correlation("external_reference", sentinel),
            |span| {
                span.record(
                    DiagnosticPhase::Failed,
                    PhaseObservation::terminal().with_cause(format!(
                        "token={sentinel} credential: {sentinel} ordinary=context"
                    )),
                );
            },
        );
        recorder.drain_deferred_for_test().unwrap();
        let bytes = fs::read_dir(directory.path())
            .unwrap()
            .filter_map(Result::ok)
            .flat_map(|entry| fs::read(entry.path()).unwrap_or_default())
            .collect::<Vec<_>>();
        let text = String::from_utf8(bytes).unwrap();
        assert!(!text.contains(sentinel), "{text}");
        assert!(text.contains("[REDACTED]"), "{text}");
        assert!(text.contains("sha256:"), "{text}");
    }

    #[test]
    fn rotation_is_bounded_and_truncated_tail_is_reported() {
        let directory = tempfile::tempdir().unwrap();
        let recorder = recorder(directory.path(), 2_048, 3);
        for ordinal in 0..24 {
            failed_span(&recorder, &format!("rotation-{ordinal}"));
        }
        recorder.drain_deferred_for_test().unwrap();
        let files = fs::read_dir(directory.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| is_flight_shard(&entry.path()))
            .collect::<Vec<_>>();
        assert!(files.len() <= 3, "{files:?}");
        let active_path = recorder.inner.active_path.clone().unwrap();
        OpenOptions::new()
            .append(true)
            .open(active_path)
            .unwrap()
            .write_all(b"{\"truncated\":")
            .unwrap();
        let report = FlightRecorderReader::new(directory.path()).inspect();
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.kind == "truncated_tail"),
            "{:?}",
            report.issues
        );
        assert!(!report.events.is_empty());
    }

    #[test]
    fn threads_share_one_safe_writer_and_process_instances_use_distinct_shards() {
        let directory = tempfile::tempdir().unwrap();
        let first = Arc::new(recorder(directory.path(), 256 * 1024, 2));
        let second = recorder(directory.path(), 256 * 1024, 2);
        let threads = (0..8)
            .map(|ordinal| {
                let recorder = Arc::clone(&first);
                std::thread::spawn(move || {
                    failed_span(&recorder, &format!("thread-{ordinal}"));
                })
            })
            .collect::<Vec<_>>();
        for thread in threads {
            thread.join().unwrap();
        }
        failed_span(&second, "second-instance");
        first.drain_deferred_for_test().unwrap();
        second.drain_deferred_for_test().unwrap();
        let report = FlightRecorderReader::new(directory.path()).inspect();
        assert_eq!(
            report
                .events
                .iter()
                .filter(|record| record.event.phase == DiagnosticPhase::Requested)
                .count(),
            9
        );
        assert_eq!(report.events.len(), 18);
        assert_eq!(report.coverage.files_seen, 2);
        assert!(report.issues.is_empty(), "{:?}", report.issues);
        let first_instance = &report.events[0].event.process.producer_instance;
        assert!(
            report
                .events
                .iter()
                .any(|event| &event.event.process.producer_instance != first_instance)
        );
        #[cfg(unix)]
        assert!(
            report
                .events
                .iter()
                .all(|event| event.event.process.parent_pid.is_some())
        );
        assert!(
            report
                .events
                .iter()
                .all(|event| !event.source.file.contains('/'))
        );
    }

    #[test]
    fn stale_process_sweep_is_bounded_and_reported_to_offline_reader() {
        let directory = tempfile::tempdir().unwrap();
        for ordinal in 0..4 {
            fs::write(
                directory
                    .path()
                    .join(format!("flight-99999999-1-dead-instance.jsonl.{ordinal}")),
                b"",
            )
            .unwrap();
        }
        let _recorder = FlightRecorder::open(
            directory.path(),
            RecorderConfig {
                max_total_shards: 2,
                stale_shard_age: Duration::ZERO,
                ..RecorderConfig::default()
            },
        )
        .unwrap();

        let report = FlightRecorderReader::new(directory.path()).inspect();
        let retention = report.coverage.retention.unwrap();
        assert_eq!(retention.retired_shards, 4);
        assert!(retention.aggregate_limit_satisfied);
        assert_eq!(report.coverage.files_seen, 1);
    }

    #[test]
    fn real_sqlite_contention_retains_typed_codes_and_coalesces() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("contended.db");
        let holder = rusqlite::Connection::open(&database).unwrap();
        holder
            .execute_batch("CREATE TABLE item (id INTEGER); BEGIN IMMEDIATE;")
            .unwrap();
        let contender = rusqlite::Connection::open(&database).unwrap();
        contender.busy_timeout(Duration::ZERO).unwrap();
        let error = contender
            .execute("INSERT INTO item (id) VALUES (1)", [])
            .unwrap_err();
        let recorder_root = directory.path().join("recorder");
        let recorder = recorder(&recorder_root, 64 * 1024, 2);
        recorder.with_requested_span(SpanStart::new("insert", "state"), |span| {
            span.record(
                DiagnosticPhase::Contention,
                PhaseObservation::not_started()
                    .with_busy_timeout(Duration::ZERO)
                    .with_sqlite_failure(&error),
            );
        });
        recorder.drain_deferred_for_test().unwrap();
        holder.execute_batch("ROLLBACK").unwrap();

        let report = FlightRecorderReader::new(&recorder_root).recent_failures(10);
        let failure = report.events[0]
            .event
            .observation
            .sqlite_failure
            .as_ref()
            .unwrap();
        assert!(failure.contention);
        assert!(failure.primary_code.is_some());
        assert!(failure.extended_code.is_some());
        let coalesced = FlightRecorderReader::new(&recorder_root).coalesced_failures(10);
        assert_eq!(coalesced.failures.len(), 1);
        assert_eq!(coalesced.failures[0].count, 1);
        assert_eq!(coalesced.failures[0].sources.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn recorder_artifacts_are_private() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("recorder");
        let recorder = recorder(&root, 64 * 1024, 2);
        failed_span(&recorder, "permissions");
        recorder.drain_deferred_for_test().unwrap();
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for entry in fs::read_dir(root).unwrap().filter_map(Result::ok) {
            assert_eq!(
                entry.metadata().unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn sqlite_deferred_queue_saturation_is_bounded_nonblocking_and_ordered() {
        let directory = tempfile::tempdir().unwrap();
        let recorder = FlightRecorder::open(
            directory.path(),
            RecorderConfig {
                deferred_queue_capacity: 2,
                ..RecorderConfig::default()
            },
        )
        .unwrap();
        let start = SpanStart::new("nonblocking", "state_sqlite").with_sqlite_identity(
            SqliteEventIdentity::new(
                SqliteDatabaseRole::State,
                SqlitePathClass::Memory,
                "fixture.queue_saturation",
            )
            .with_transaction_mode(SqliteTransactionMode::Immediate),
        );
        recorder.with_requested_span(start, |span| {
            let release = recorder.block_writer_for_test().unwrap();
            assert_eq!(
                span.record(
                    DiagnosticPhase::Acquired,
                    PhaseObservation::started_unknown().with_cause("first")
                ),
                RecordStatus::Queued
            );
            assert_eq!(
                span.record(
                    DiagnosticPhase::CommitStarted,
                    PhaseObservation::effects_possible().with_cause("second")
                ),
                RecordStatus::Queued
            );
            let gap_handoffs_before = gap_handoff_attempts_for_test();
            let started = Instant::now();
            assert_eq!(
                span.record(DiagnosticPhase::Committed, PhaseObservation::committed()),
                RecordStatus::Failed {
                    stage: "deferred_queue_full".to_string()
                }
            );
            assert!(started.elapsed() < Duration::from_millis(100));
            assert_eq!(
                gap_handoff_attempts_for_test(),
                gap_handoffs_before,
                "database-held deferred failure must not enter reporter handoff"
            );
            release.send(()).unwrap();
        });
        recorder.drain_deferred_for_test().unwrap();
        let report = FlightRecorderReader::new(directory.path()).inspect();
        assert_eq!(report.events.len(), 3);
        assert!(report.events.iter().all(|record| {
            record
                .event
                .sqlite
                .as_ref()
                .map(|identity| (identity.database_role, identity.query_family.as_str()))
                == Some((SqliteDatabaseRole::State, "fixture.queue_saturation"))
        }));
        let mut phases_by_line = report
            .events
            .iter()
            .map(|record| (record.source.line, record.event.phase))
            .collect::<Vec<_>>();
        phases_by_line.sort_by_key(|(line, _)| *line);
        assert_eq!(
            phases_by_line,
            vec![
                (1, DiagnosticPhase::Requested),
                (2, DiagnosticPhase::Acquired),
                (3, DiagnosticPhase::CommitStarted),
            ]
        );
    }

    #[test]
    fn blocked_or_failed_gap_reporter_does_not_block_or_change_top_level_work() {
        let (full_reporter, full_receiver) = mpsc::sync_channel(1);
        full_reporter.try_send("fixture_already_queued").unwrap();
        let tiny_recorder = FlightRecorder {
            inner: Arc::new(RecorderInner {
                process: RecorderProcessIdentity::current(ProducerInstanceId::new()),
                active_path: None,
                max_shard_bytes: 1,
                writer: None,
            }),
        };
        let handoffs_before = gap_handoff_attempts_for_test().len();
        let started = Instant::now();
        let full_result = with_test_gap_reporter(full_reporter, || {
            tiny_recorder.with_requested_span(SpanStart::new("reporter-full", "state"), |span| {
                assert_eq!(
                    span.requested_status(),
                    Some(&RecordStatus::Failed {
                        stage: "record_too_large".to_string()
                    })
                );
                41
            })
        });
        assert_eq!(full_result, 41);
        assert!(started.elapsed() < Duration::from_millis(100));
        assert!(gap_handoff_attempts_for_test()[handoffs_before..].contains(&"record_too_large"));
        drop(full_receiver);

        let (failed_reporter, failed_receiver) = mpsc::sync_channel(1);
        drop(failed_receiver);
        let disabled = FlightRecorder::disabled();
        let handoffs_before = gap_handoff_attempts_for_test().len();
        let started = Instant::now();
        let failed_result = with_test_gap_reporter(failed_reporter, || {
            disabled.with_requested_span(SpanStart::new("reporter-failed", "state"), |span| {
                assert_eq!(span.requested_status(), Some(&RecordStatus::Disabled));
                42
            })
        });
        assert_eq!(failed_result, 42);
        assert!(started.elapsed() < Duration::from_millis(100));
        assert!(gap_handoff_attempts_for_test()[handoffs_before..].contains(&"recorder_disabled"));
    }

    #[test]
    fn gap_reporter_ignores_sink_write_errors() {
        struct FailingSink;

        impl Write for FailingSink {
            fn write(&mut self, _bytes: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "fixture",
                ))
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "fixture",
                ))
            }
        }

        let (sender, receiver) = mpsc::sync_channel(1);
        sender.try_send("fixture_write_failure").unwrap();
        drop(sender);
        write_gap_reports(receiver, FailingSink);
    }

    #[test]
    fn child_requested_is_queued_while_writer_is_unavailable() {
        let directory = tempfile::tempdir().unwrap();
        let recorder = recorder(directory.path(), 64 * 1024, 2);
        recorder.with_requested_span(SpanStart::new("parent", "state"), |parent| {
            let release = recorder.block_writer_for_test().unwrap();
            parent.with_deferred_requested_span(
                SpanStart::new("child", "sidecar")
                    .with_diagnostic_id(parent.diagnostic_id().clone())
                    .with_parent_span_id(parent.span_id().clone()),
                |child| {
                    assert_eq!(child.requested_status(), Some(&RecordStatus::Queued));
                    assert_eq!(
                        child.record(
                            DiagnosticPhase::Acquired,
                            PhaseObservation::started_unknown()
                        ),
                        RecordStatus::Queued
                    );
                },
            );
            release.send(()).unwrap();
        });
        recorder.drain_deferred_for_test().unwrap();
        let report = FlightRecorderReader::new(directory.path()).inspect();
        assert_eq!(report.events.len(), 3);
        let child_events = report
            .events
            .iter()
            .filter(|record| record.event.operation == "child")
            .collect::<Vec<_>>();
        assert_eq!(child_events.len(), 2);
        let parent_span_id = &report
            .events
            .iter()
            .find(|record| record.event.operation == "parent")
            .unwrap()
            .event
            .span_id;
        assert!(
            child_events
                .iter()
                .all(|record| { record.event.parent_span_id.as_ref() == Some(parent_span_id) })
        );
    }

    #[test]
    fn process_recorder_initialization_failure_is_retryable_and_success_is_cached() {
        let directory = tempfile::tempdir().unwrap();
        let slot = Mutex::new(None);
        ensure_gap_reporter();
        let (reporter, reports) = mpsc::sync_channel(1);
        let disabled = with_test_gap_reporter(reporter, || {
            cached_or_retry_process_recorder(&slot, || Err("fixture".to_string()))
        });
        assert!(disabled.inner.writer.is_none());
        assert_eq!(reports.try_recv().unwrap(), "process_recorder_init");

        let initialized = cached_or_retry_process_recorder(&slot, || {
            FlightRecorder::open(directory.path(), RecorderConfig::default())
        });
        assert!(initialized.inner.writer.is_some());
        let cached = cached_or_retry_process_recorder(&slot, || {
            panic!("cached recorder must not initialize twice")
        });
        assert!(Arc::ptr_eq(&initialized.inner, &cached.inner));
    }

    #[test]
    fn active_shard_holds_an_exclusive_advisory_lease() {
        let directory = tempfile::tempdir().unwrap();
        let recorder = recorder(directory.path(), 64 * 1024, 2);
        let active_path = recorder.inner.active_path.clone().unwrap();
        let foreign = OpenOptions::new()
            .read(true)
            .write(true)
            .open(active_path)
            .unwrap();
        assert!(matches!(
            <fs::File as fs4::FileExt>::try_lock(&foreign),
            Err(fs4::TryLockError::WouldBlock)
        ));
    }

    #[test]
    fn missing_reader_root_is_an_explicit_issue() {
        let directory = tempfile::tempdir().unwrap();
        let report = FlightRecorderReader::new(directory.path().join("missing")).inspect();
        assert!(report.events.is_empty());
        assert_eq!(report.issues.len(), 1);
        assert_eq!(report.issues[0].kind, "missing_root");

        let unreadable_root = directory.path().join("not-a-directory");
        fs::write(&unreadable_root, b"fixture").unwrap();
        let report = FlightRecorderReader::new(unreadable_root).inspect();
        assert!(report.events.is_empty());
        assert_eq!(report.issues.len(), 1);
        assert_eq!(report.issues[0].kind, "unreadable_root");
    }

    #[test]
    fn cleanup_keeps_fallback_identity_shards_as_uncertain() {
        let directory = tempfile::tempdir().unwrap();
        let fallback = directory
            .path()
            .join("flight-99999999-0-unknown-fixture.jsonl");
        fs::write(&fallback, b"").unwrap();
        let recorder = FlightRecorder::open(
            directory.path(),
            RecorderConfig {
                stale_shard_age: Duration::ZERO,
                max_shards: 1,
                max_total_shards: 1,
                ..RecorderConfig::default()
            },
        )
        .unwrap();
        let report = recorder.cleanup_stale_shards();
        assert!(fallback.exists());
        assert!(report.retained_uncertain_shards >= 1, "{report:?}");
        assert!(!report.aggregate_limit_satisfied);
    }

    #[test]
    fn rotation_transfers_the_active_shard_lease() {
        let directory = tempfile::tempdir().unwrap();
        let recorder = recorder(directory.path(), 2_048, 2);
        for ordinal in 0..20 {
            failed_span(&recorder, &format!("lease-rotation-{ordinal}"));
        }
        recorder.drain_deferred_for_test().unwrap();
        let active_path = recorder.inner.active_path.clone().unwrap();
        let rotated_path = active_path.with_file_name(format!(
            "{}.1",
            active_path.file_name().unwrap().to_string_lossy()
        ));
        assert!(rotated_path.exists());
        let active = OpenOptions::new()
            .read(true)
            .write(true)
            .open(active_path)
            .unwrap();
        assert!(matches!(
            <fs::File as fs4::FileExt>::try_lock(&active),
            Err(fs4::TryLockError::WouldBlock)
        ));
        let rotated = OpenOptions::new()
            .read(true)
            .write(true)
            .open(rotated_path)
            .unwrap();
        <fs::File as fs4::FileExt>::try_lock(&rotated).unwrap();
        <fs::File as fs4::FileExt>::unlock(&rotated).unwrap();
    }

    #[test]
    fn inspection_bounds_and_skipped_coverage_are_visible() {
        let directory = tempfile::tempdir().unwrap();
        for ordinal in 0..3 {
            fs::write(
                directory
                    .path()
                    .join(format!("flight-99999-1-dead-{ordinal}.jsonl")),
                b"not-json\n",
            )
            .unwrap();
        }
        let report = FlightRecorderReader::new(directory.path()).inspect_with_limits(32, 2, 9);
        assert_eq!(report.coverage.files_discovered, 3);
        assert_eq!(report.coverage.files_seen, 1);
        assert_eq!(report.coverage.files_skipped, 2);
        assert_eq!(report.coverage.bytes_read, 9);
        assert_eq!(report.coverage.bytes_skipped, 18);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.kind == "inspection_shard_limit")
        );
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.kind == "inspection_byte_limit")
        );
    }

    #[test]
    fn duplicate_event_ids_are_deduplicated_and_reported() {
        let directory = tempfile::tempdir().unwrap();
        let recorder = recorder(directory.path(), 64 * 1024, 2);
        failed_span(&recorder, "duplicate");
        recorder.drain_deferred_for_test().unwrap();
        let active_path = recorder.inner.active_path.clone().unwrap();
        fs::copy(&active_path, active_path.with_extension("jsonl.99")).unwrap();
        let report = FlightRecorderReader::new(directory.path()).inspect();
        assert_eq!(report.events.len(), 2);
        assert_eq!(report.coverage.duplicate_records, 2);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.kind == "duplicate_event_id")
        );
    }

    #[test]
    fn coalescing_keeps_distinct_generic_failure_causes_separate() {
        let directory = tempfile::tempdir().unwrap();
        let recorder = recorder(directory.path(), 64 * 1024, 2);
        for cause in ["authority_open_failed", "authority_fence_failed"] {
            recorder.with_requested_span(SpanStart::new("same-operation", "sidecar"), |span| {
                span.record(
                    DiagnosticPhase::Failed,
                    PhaseObservation::not_started().with_cause(cause),
                );
            });
        }
        recorder.drain_deferred_for_test().unwrap();
        let report = FlightRecorderReader::new(directory.path()).recent_and_coalesced_failures(10);
        assert_eq!(report.raw.events.len(), 2);
        assert_eq!(report.coalesced.failures.len(), 2);
        assert_ne!(
            report.coalesced.failures[0].failure_discriminator,
            report.coalesced.failures[1].failure_discriminator
        );
        assert_eq!(report.raw.coverage, report.coalesced.coverage);
        assert_eq!(report.raw.issues, report.coalesced.issues);
    }

    #[test]
    fn shard_disappearance_is_distinct_from_unreadable_content() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("flight-1-1-dead-fixture.jsonl");
        fs::write(&path, b"").unwrap();
        let metadata = fs::metadata(&path).unwrap();
        let candidate = ShardCandidate {
            path: path.clone(),
            bytes: metadata.len(),
            modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            identity: shard_file_identity(&metadata).unwrap(),
        };
        fs::remove_file(path).unwrap();
        let mut report = InspectionReport::default();
        read_shard(&candidate, 1024, &mut report);
        assert_eq!(report.issues.len(), 1);
        assert_eq!(report.issues[0].kind, "concurrent_disappearance");
    }

    #[test]
    fn concurrent_pathname_replacement_is_reported_and_skipped() {
        let directory = tempfile::tempdir().unwrap();
        let recorder = recorder(directory.path(), 64 * 1024, 2);
        failed_span(&recorder, "original");
        recorder.drain_deferred_for_test().unwrap();
        let active_path = recorder.inner.active_path.clone().unwrap();
        let displaced_path = directory.path().join("displaced-original");
        let report = FlightRecorderReader::new(directory.path())
            .inspect_with_limits_after_discovery(32, 32, 64 * 1024, |_| {
                fs::rename(&active_path, &displaced_path).unwrap();
                fs::write(&active_path, b"replacement must not be parsed\n").unwrap();
            });
        assert!(report.events.is_empty());
        assert!(report.coverage.files_skipped >= 1);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.kind == "concurrent_replacement")
        );
    }

    #[test]
    fn directory_discovery_work_is_capped_and_visible() {
        let directory = tempfile::tempdir().unwrap();
        for ordinal in 0..10 {
            fs::write(directory.path().join(format!("noise-{ordinal}")), b"x").unwrap();
        }
        let report =
            FlightRecorderReader::new(directory.path()).inspect_with_limits(3, 32, 64 * 1024);
        assert_eq!(report.coverage.directory_entries_examined, 3);
        assert!(report.coverage.directory_limit_reached);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.kind == "inspection_directory_limit")
        );
    }

    #[test]
    fn retention_status_read_is_bounded_and_classifies_incomplete_inputs() {
        let directory = tempfile::tempdir().unwrap();
        let status_path = directory.path().join("retention-status.json");
        fs::write(
            &status_path,
            vec![b'x'; MAX_RETENTION_STATUS_READ_BYTES as usize + 1],
        )
        .unwrap();

        let oversized = FlightRecorderReader::new(directory.path()).inspect();
        assert!(oversized.coverage.retention.is_none());
        assert!(oversized.coverage.retention_status_limit_reached);
        assert_eq!(oversized.coverage.retention_status_bytes_read, 0);
        assert_eq!(
            oversized.coverage.retention_status_bytes_skipped,
            MAX_RETENTION_STATUS_READ_BYTES + 1
        );
        assert!(
            oversized
                .issues
                .iter()
                .any(|issue| issue.kind == "oversized_retention_status")
        );

        fs::write(&status_path, b"{\"scanned_shards\":1}").unwrap();
        let truncated = FlightRecorderReader::new(directory.path()).inspect();
        assert!(truncated.coverage.retention.is_none());
        assert!(
            truncated
                .issues
                .iter()
                .any(|issue| issue.kind == "truncated_retention_status")
        );

        fs::write(&status_path, b"not-json\n").unwrap();
        let invalid = FlightRecorderReader::new(directory.path()).inspect();
        assert!(invalid.coverage.retention.is_none());
        assert!(
            invalid
                .issues
                .iter()
                .any(|issue| issue.kind == "invalid_retention_status")
        );
    }

    #[test]
    fn cleanup_discovery_and_persisted_issue_details_are_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let fixture_count = MAX_CLEANUP_DIRECTORY_ENTRIES + 16;
        for ordinal in 0..fixture_count {
            fs::write(
                directory
                    .path()
                    .join(format!("flight-malformed-{ordinal}.jsonl")),
                b"",
            )
            .unwrap();
        }
        let recorder = FlightRecorder::open(
            directory.path(),
            RecorderConfig {
                max_shards: 1,
                max_total_shards: 1,
                stale_shard_age: Duration::ZERO,
                ..RecorderConfig::default()
            },
        )
        .unwrap();

        let cleanup = recorder.cleanup_stale_shards();
        assert_eq!(
            cleanup.directory_entries_examined,
            MAX_CLEANUP_DIRECTORY_ENTRIES
        );
        assert!(cleanup.directory_limit_reached);
        assert!(!cleanup.aggregate_limit_satisfied);
        assert_eq!(cleanup.issues.len(), MAX_CLEANUP_ISSUES);
        assert!(cleanup.issue_limit_reached);
        assert!(cleanup.issues_omitted > 0);
        for ordinal in 0..fixture_count {
            assert!(
                directory
                    .path()
                    .join(format!("flight-malformed-{ordinal}.jsonl"))
                    .exists(),
                "identity-uncertain shard {ordinal} was deleted"
            );
        }

        let status_len = fs::metadata(directory.path().join("retention-status.json"))
            .unwrap()
            .len();
        assert!(
            status_len <= MAX_RETENTION_STATUS_READ_BYTES,
            "{status_len}"
        );
        let inspection = FlightRecorderReader::new(directory.path()).inspect();
        let retained = inspection.coverage.retention.unwrap();
        assert!(retained.directory_limit_reached);
        assert!(retained.issue_limit_reached);
        assert!(!retained.aggregate_limit_satisfied);
    }

    #[test]
    fn worst_case_cleanup_status_fits_the_reader_bound() {
        let directory = tempfile::tempdir().unwrap();
        let report = CleanupReport {
            directory_entries_examined: usize::MAX,
            directory_entries_unreadable: usize::MAX,
            directory_limit_reached: true,
            scanned_shards: usize::MAX,
            retired_shards: usize::MAX,
            retained_live_shards: usize::MAX,
            retained_uncertain_shards: usize::MAX,
            aggregate_limit_satisfied: false,
            issue_limit_reached: true,
            issues_omitted: usize::MAX,
            // NUL has the longest JSON escape emitted for a one-byte input.
            issues: vec!["\0".repeat(MAX_CLEANUP_ISSUE_BYTES); MAX_CLEANUP_ISSUES],
        };
        let encoded = serde_json::to_vec(&report).unwrap();
        assert!(
            encoded.len().saturating_add(1) as u64 <= MAX_RETENTION_STATUS_READ_BYTES,
            "{}",
            encoded.len()
        );
        persist_cleanup_status(directory.path(), &report).unwrap();

        let inspection = FlightRecorderReader::new(directory.path()).inspect();
        assert_eq!(inspection.coverage.retention, Some(report));
        assert!(!inspection.coverage.retention_status_limit_reached);
    }

    #[cfg(unix)]
    #[test]
    fn recorder_child_process_fixture() {
        let Ok(mode) = std::env::var("OULIPOLY_RECORDER_CHILD_MODE") else {
            return;
        };
        let root = PathBuf::from(std::env::var_os("OULIPOLY_RECORDER_CHILD_ROOT").unwrap());
        match mode.as_str() {
            "abrupt" => {
                let recorder = recorder(&root, 64 * 1024, 2);
                recorder.with_requested_span(SpanStart::new("child-abrupt", "state"), |_| {
                    let mut file = OpenOptions::new()
                        .append(true)
                        .open(recorder.inner.active_path.as_ref().unwrap())
                        .unwrap();
                    file.write_all(b"{\"partial\":").unwrap();
                    file.flush().unwrap();
                    std::process::exit(23);
                });
            }
            "hold_foreign_shard" => {
                create_private_directory(&root).unwrap();
                let path = root.join("flight-99999999-1-dead-held.jsonl");
                let file = private_append_file(&path).unwrap();
                <fs::File as fs4::FileExt>::try_lock(&file).unwrap();
                fs::write(root.join(".held-ready"), b"ready").unwrap();
                let _ = std::io::stdin().read(&mut [0_u8; 1]);
            }
            other => panic!("unknown recorder child mode: {other}"),
        }
    }

    #[cfg(unix)]
    fn spawn_recorder_child(mode: &str, root: &Path) -> std::process::Child {
        Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("diagnostic_recorder::tests::recorder_child_process_fixture")
            .arg("--nocapture")
            .env("OULIPOLY_RECORDER_CHILD_MODE", mode)
            .env("OULIPOLY_RECORDER_CHILD_ROOT", root)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn abrupt_child_exit_retains_requested_and_reports_crash_tail() {
        let directory = tempfile::tempdir().unwrap();
        let mut child = spawn_recorder_child("abrupt", directory.path());
        let status = child.wait().unwrap();
        assert_eq!(status.code(), Some(23));

        let report = FlightRecorderReader::new(directory.path()).inspect();
        assert_eq!(report.events.len(), 1);
        assert_eq!(report.events[0].event.phase, DiagnosticPhase::Requested);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.kind == "truncated_tail"),
            "{:?}",
            report.issues
        );
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_preserves_a_lock_held_foreign_dead_shard() {
        let directory = tempfile::tempdir().unwrap();
        let mut child = spawn_recorder_child("hold_foreign_shard", directory.path());
        let ready = directory.path().join(".held-ready");
        for _ in 0..500 {
            if ready.exists() {
                break;
            }
            if let Some(status) = child.try_wait().unwrap() {
                panic!("foreign-shard child exited before ready: {status}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(ready.exists(), "foreign-shard child did not become ready");

        let foreign = directory.path().join("flight-99999999-1-dead-held.jsonl");
        let sweeper = FlightRecorder::open(
            directory.path(),
            RecorderConfig {
                max_shards: 1,
                max_total_shards: 1,
                stale_shard_age: Duration::ZERO,
                ..RecorderConfig::default()
            },
        )
        .unwrap();
        let held_report = sweeper.cleanup_stale_shards();
        assert!(foreign.exists());
        assert!(held_report.retained_live_shards >= 2, "{held_report:?}");
        assert!(!held_report.aggregate_limit_satisfied);

        drop(child.stdin.take());
        assert!(child.wait().unwrap().success());
        let released_report = sweeper.cleanup_stale_shards();
        assert!(!foreign.exists());
        assert!(released_report.retired_shards >= 1, "{released_report:?}");
    }
}
