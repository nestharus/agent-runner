//! Bounded, value-free SQLite observations over the database-independent
//! flight recorder.
//!
//! This module deliberately does not use rusqlite tracing. SQLite trace SQL can
//! be expanded with bound values, while stable operation/query-family labels at
//! the repository boundary are both safer and more useful to operators.

use crate::diagnostic_recorder::{
    DiagnosticPhase, DiagnosticSpan, FlightRecorder, OutcomeCertainty, PhaseObservation, SpanStart,
    SqliteMeasurementGap, SqlitePhaseEvidence, SqliteQueryPlanEvidence, SqliteQueryPlanOperator,
    SqliteQueryPlanOperatorCount, process_recorder,
};
use rusqlite::{Connection, Params};
use std::collections::BTreeMap;
#[cfg(not(test))]
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub const DEFAULT_SQLITE_SLOW_THRESHOLD: Duration = Duration::from_millis(100);
pub const DEFAULT_QUERY_PLAN_NODE_LIMIT: usize = 32;
pub const MAX_QUERY_PLAN_NODE_LIMIT: usize = 64;

#[cfg(not(test))]
const ENABLE_ENV: &str = "OULIPOLY_SQLITE_OBSERVABILITY";
#[cfg(not(test))]
const SLOW_MILLIS_ENV: &str = "OULIPOLY_SQLITE_SLOW_MILLIS";
#[cfg(not(test))]
const SAMPLE_EVERY_ENV: &str = "OULIPOLY_SQLITE_SAMPLE_EVERY";
#[cfg(not(test))]
const QUERY_PLANS_ENV: &str = "OULIPOLY_SQLITE_QUERY_PLANS";

static SAMPLE_ORDINAL: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SqliteObservationPolicy {
    pub enabled: bool,
    pub slow_threshold: Duration,
    /// Zero drops ordinary fast successes. One retains all of them; N retains
    /// one in every N. Failures, contention, and slow operations bypass this.
    pub normal_sample_every: u64,
    pub query_plans: bool,
    pub query_plan_node_limit: usize,
}

impl Default for SqliteObservationPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            slow_threshold: DEFAULT_SQLITE_SLOW_THRESHOLD,
            normal_sample_every: 0,
            query_plans: false,
            query_plan_node_limit: DEFAULT_QUERY_PLAN_NODE_LIMIT,
        }
    }
}

impl SqliteObservationPolicy {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::default()
        }
    }

    pub fn all() -> Self {
        Self {
            normal_sample_every: 1,
            ..Self::default()
        }
    }

    fn from_values(
        enable: Option<&str>,
        slow_millis: Option<&str>,
        sample_every: Option<&str>,
        query_plans: Option<&str>,
    ) -> Self {
        let mut policy = Self::default();
        match enable
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("0" | "false" | "off" | "disabled") => policy.enabled = false,
            Some("all") => policy.normal_sample_every = 1,
            _ => {}
        }
        if let Some(milliseconds) = slow_millis
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value <= 60_000)
        {
            policy.slow_threshold = Duration::from_millis(milliseconds);
        }
        if let Some(sample_every) = sample_every
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value <= 1_000_000)
        {
            policy.normal_sample_every = sample_every;
        }
        policy.query_plans = matches!(
            query_plans
                .map(str::trim)
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("1" | "true" | "on" | "enabled")
        );
        policy
    }

    #[cfg(not(test))]
    fn from_environment() -> Self {
        Self::from_values(
            std::env::var(ENABLE_ENV).ok().as_deref(),
            std::env::var(SLOW_MILLIS_ENV).ok().as_deref(),
            std::env::var(SAMPLE_EVERY_ENV).ok().as_deref(),
            std::env::var(QUERY_PLANS_ENV).ok().as_deref(),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqliteRecordDecision {
    Disabled,
    FilteredFastSuccess,
    RecordedFailure,
    RecordedContention,
    RecordedSlow,
    RecordedSample,
}

/// A late-emitting observation. Fast successful operations do not initialize
/// or touch the flight recorder under the default policy. Top-level retained
/// events are synchronously handed off only after database authority has been
/// released. Nested callers that still hold an outer authority must use the
/// deferred parent methods so they reuse its recorder and never wait.
pub struct SqliteOperationObserver {
    policy: SqliteObservationPolicy,
    started: Option<Instant>,
}

pub(crate) enum DeferredObservationTarget<'a> {
    Parent(&'a DiagnosticSpan),
    SelectedRecorder(&'a FlightRecorder),
}

enum ObservationTarget<'a> {
    Process,
    Deferred(DeferredObservationTarget<'a>),
}

impl SqliteOperationObserver {
    pub fn process() -> Self {
        Self::with_policy(process_policy())
    }

    pub fn with_policy(policy: SqliteObservationPolicy) -> Self {
        Self {
            policy,
            started: policy.enabled.then(Instant::now),
        }
    }

    pub fn elapsed(&self) -> Option<Duration> {
        self.started.map(|started| started.elapsed())
    }

    pub fn record_success(
        self,
        span: impl FnOnce() -> SpanStart,
        phase: DiagnosticPhase,
        certainty: OutcomeCertainty,
        evidence: impl FnOnce(Duration) -> SqlitePhaseEvidence,
    ) -> SqliteRecordDecision {
        let Some(elapsed) = self.elapsed() else {
            return SqliteRecordDecision::Disabled;
        };
        let decision = if elapsed >= self.policy.slow_threshold {
            SqliteRecordDecision::RecordedSlow
        } else if sampled(self.policy.normal_sample_every) {
            SqliteRecordDecision::RecordedSample
        } else {
            return SqliteRecordDecision::FilteredFastSuccess;
        };
        Self::emit(
            ObservationTarget::Process,
            span,
            elapsed,
            phase,
            certainty,
            evidence(elapsed).with_total_elapsed(elapsed),
            None,
        );
        decision
    }

    pub(crate) fn record_success_deferred(
        self,
        target: DeferredObservationTarget<'_>,
        span: impl FnOnce() -> SpanStart,
        phase: DiagnosticPhase,
        certainty: OutcomeCertainty,
        evidence: impl FnOnce(Duration) -> SqlitePhaseEvidence,
    ) -> SqliteRecordDecision {
        let Some(elapsed) = self.elapsed() else {
            return SqliteRecordDecision::Disabled;
        };
        let decision = if elapsed >= self.policy.slow_threshold {
            SqliteRecordDecision::RecordedSlow
        } else if sampled(self.policy.normal_sample_every) {
            SqliteRecordDecision::RecordedSample
        } else {
            return SqliteRecordDecision::FilteredFastSuccess;
        };
        Self::emit(
            ObservationTarget::Deferred(target),
            span,
            elapsed,
            phase,
            certainty,
            evidence(elapsed).with_total_elapsed(elapsed),
            None,
        );
        decision
    }

    pub fn record_failure(
        self,
        span: impl FnOnce() -> SpanStart,
        error: &rusqlite::Error,
        certainty: OutcomeCertainty,
        evidence: impl FnOnce(Duration) -> SqlitePhaseEvidence,
    ) -> SqliteRecordDecision {
        let Some(elapsed) = self.elapsed() else {
            return SqliteRecordDecision::Disabled;
        };
        let contention = matches!(
            error.sqlite_error_code(),
            Some(rusqlite::ffi::ErrorCode::DatabaseBusy)
                | Some(rusqlite::ffi::ErrorCode::DatabaseLocked)
        );
        Self::emit(
            ObservationTarget::Process,
            span,
            elapsed,
            if contention {
                DiagnosticPhase::Contention
            } else {
                DiagnosticPhase::Failed
            },
            certainty,
            evidence(elapsed).with_total_elapsed(elapsed),
            Some(error),
        );
        if contention {
            SqliteRecordDecision::RecordedContention
        } else {
            SqliteRecordDecision::RecordedFailure
        }
    }

    pub(crate) fn record_failure_deferred(
        self,
        target: DeferredObservationTarget<'_>,
        span: impl FnOnce() -> SpanStart,
        error: &rusqlite::Error,
        certainty: OutcomeCertainty,
        evidence: impl FnOnce(Duration) -> SqlitePhaseEvidence,
    ) -> SqliteRecordDecision {
        let Some(elapsed) = self.elapsed() else {
            return SqliteRecordDecision::Disabled;
        };
        let contention = matches!(
            error.sqlite_error_code(),
            Some(rusqlite::ffi::ErrorCode::DatabaseBusy)
                | Some(rusqlite::ffi::ErrorCode::DatabaseLocked)
        );
        Self::emit(
            ObservationTarget::Deferred(target),
            span,
            elapsed,
            if contention {
                DiagnosticPhase::Contention
            } else {
                DiagnosticPhase::Failed
            },
            certainty,
            evidence(elapsed).with_total_elapsed(elapsed),
            Some(error),
        );
        if contention {
            SqliteRecordDecision::RecordedContention
        } else {
            SqliteRecordDecision::RecordedFailure
        }
    }

    fn emit(
        target: ObservationTarget<'_>,
        span: impl FnOnce() -> SpanStart,
        elapsed: Duration,
        phase: DiagnosticPhase,
        certainty: OutcomeCertainty,
        evidence: SqlitePhaseEvidence,
        error: Option<&rusqlite::Error>,
    ) {
        let mut observation = PhaseObservation {
            certainty,
            ..PhaseObservation::default()
        }
        .with_sqlite_evidence(evidence);
        if let Some(error) = error {
            observation = observation.with_sqlite_failure(error);
        }
        let start = span();
        let _ = match target {
            ObservationTarget::Process => {
                process_recorder().record_completed_observation(start, elapsed, phase, observation)
            }
            ObservationTarget::Deferred(target) => match target {
                DeferredObservationTarget::Parent(parent) => {
                    parent.record_deferred_completed_child(start, elapsed, phase, observation)
                }
                DeferredObservationTarget::SelectedRecorder(recorder) => recorder
                    .record_deferred_completed_observation(start, elapsed, phase, observation),
            },
        };
    }
}

pub(crate) fn process_query_plans_enabled() -> bool {
    process_policy().query_plans
}

pub(crate) fn process_query_plan_node_limit() -> usize {
    process_policy().query_plan_node_limit
}

fn process_policy() -> SqliteObservationPolicy {
    #[cfg(test)]
    {
        TEST_PROCESS_POLICY
            .with(|policy| *policy.borrow())
            .unwrap_or_default()
    }
    #[cfg(not(test))]
    {
        static POLICY: OnceLock<SqliteObservationPolicy> = OnceLock::new();
        *POLICY.get_or_init(SqliteObservationPolicy::from_environment)
    }
}

#[cfg(test)]
thread_local! {
    static TEST_PROCESS_POLICY: std::cell::RefCell<Option<SqliteObservationPolicy>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
struct TestProcessPolicyReset(Option<SqliteObservationPolicy>);

#[cfg(test)]
impl Drop for TestProcessPolicyReset {
    fn drop(&mut self) {
        TEST_PROCESS_POLICY.with(|slot| {
            *slot.borrow_mut() = self.0.take();
        });
    }
}

#[cfg(test)]
pub(crate) fn with_test_process_policy<T>(
    policy: SqliteObservationPolicy,
    operation: impl FnOnce() -> T,
) -> T {
    let previous = TEST_PROCESS_POLICY.with(|slot| slot.borrow_mut().replace(policy));
    let _reset = TestProcessPolicyReset(previous);
    operation()
}

fn sampled(every: u64) -> bool {
    every != 0
        && SAMPLE_ORDINAL
            .fetch_add(1, Ordering::Relaxed)
            .is_multiple_of(every)
}

/// Collects only a bounded structural plan shape. The SQL template, SQLite
/// detail text, table/index names, filesystem paths, and bound parameters are
/// never copied into the evidence object.
pub fn capture_query_plan<P: Params>(
    connection: &Connection,
    sql_template: &'static str,
    params: P,
    enabled: bool,
    node_limit: usize,
) -> SqliteQueryPlanEvidence {
    if !enabled {
        return SqliteQueryPlanEvidence::Disabled;
    }
    let node_limit = node_limit.clamp(1, MAX_QUERY_PLAN_NODE_LIMIT);
    let explain = format!("EXPLAIN QUERY PLAN {sql_template}");
    let result = (|| -> rusqlite::Result<(u32, bool, BTreeMap<SqliteQueryPlanOperator, u32>)> {
        let mut statement = connection.prepare(&explain)?;
        let rows = statement.query_map(params, |row| row.get::<_, String>(3))?;
        let mut nodes_seen = 0_u32;
        let mut truncated = false;
        let mut operators = BTreeMap::new();
        for detail in rows {
            let detail = detail?;
            if nodes_seen as usize >= node_limit {
                truncated = true;
                break;
            }
            nodes_seen = nodes_seen.saturating_add(1);
            let operator = classify_plan_operator(&detail);
            let count = operators.entry(operator).or_insert(0_u32);
            *count = count.saturating_add(1);
        }
        Ok((nodes_seen, truncated, operators))
    })();
    match result {
        Ok((nodes_seen, truncated, counts)) => SqliteQueryPlanEvidence::Captured {
            nodes_seen,
            truncated,
            operators: counts
                .into_iter()
                .map(|(operator, count)| SqliteQueryPlanOperatorCount { operator, count })
                .collect(),
        },
        Err(error) => {
            let sqlite = error.sqlite_error();
            SqliteQueryPlanEvidence::Unavailable {
                primary_code: sqlite.map(|failure| format!("{:?}", failure.code)),
                extended_code: sqlite.map(|failure| failure.extended_code),
            }
        }
    }
}

fn classify_plan_operator(detail: &str) -> SqliteQueryPlanOperator {
    let detail = detail.trim_start().to_ascii_uppercase();
    if detail.starts_with("SCAN") {
        SqliteQueryPlanOperator::Scan
    } else if detail.starts_with("SEARCH") {
        SqliteQueryPlanOperator::Search
    } else if detail.contains("TEMP B-TREE") {
        SqliteQueryPlanOperator::TemporaryBTree
    } else if detail.contains("COMPOUND") {
        SqliteQueryPlanOperator::Compound
    } else if detail.contains("CO-ROUTINE") {
        SqliteQueryPlanOperator::Coroutine
    } else if detail.contains("MATERIALIZE") {
        SqliteQueryPlanOperator::Materialize
    } else if detail.contains("MULTI-INDEX") {
        SqliteQueryPlanOperator::MultiIndex
    } else if detail.contains("BLOOM FILTER") {
        SqliteQueryPlanOperator::BloomFilter
    } else if detail.contains("SUBQUERY") {
        SqliteQueryPlanOperator::Subquery
    } else {
        SqliteQueryPlanOperator::Other
    }
}

pub fn statement_evidence(elapsed: Duration) -> SqlitePhaseEvidence {
    SqlitePhaseEvidence::for_phase(
        crate::diagnostic_recorder::SqliteTransactionPhase::StatementExecution,
    )
    .with_statement_total(elapsed)
    .with_gap(SqliteMeasurementGap::WriterWaitAndExecutionNotSeparable)
    .with_gap(SqliteMeasurementGap::CommitNotExposedByApi)
    .with_gap(SqliteMeasurementGap::RowsExaminedNotExposed)
    .with_gap(SqliteMeasurementGap::RowsChangedNotReported)
    .with_gap(SqliteMeasurementGap::PostCommitOutsideBoundary)
}

pub fn connection_open_evidence(elapsed: Duration) -> SqlitePhaseEvidence {
    SqlitePhaseEvidence::for_phase(
        crate::diagnostic_recorder::SqliteTransactionPhase::ConnectionOpen,
    )
    .with_execution(elapsed)
    .with_gap(SqliteMeasurementGap::WriterAuthorityNotApplicable)
    .with_gap(SqliteMeasurementGap::CommitNotApplicable)
    .with_gap(SqliteMeasurementGap::PostCommitNotApplicable)
    .with_gap(SqliteMeasurementGap::RowsExaminedNotApplicable)
    .with_gap(SqliteMeasurementGap::RowsChangedNotApplicable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic_producer::{TransactionAttempt, TransactionPhaseGuard};
    use crate::diagnostic_recorder::{
        FlightRecorder, FlightRecorderReader, RecorderConfig, SqliteDatabaseRole,
        SqliteEventIdentity, SqliteFailure, SqlitePathClass, SqliteTransactionMode,
        SqliteTransactionPhase, process_recorder, with_test_process_recorder,
    };
    use std::fs;

    fn recorder(root: &std::path::Path) -> FlightRecorder {
        FlightRecorder::open(root, RecorderConfig::default()).unwrap()
    }

    fn span(query_family: &'static str) -> SpanStart {
        SpanStart::new("sqlite_fixture", "pid_mailbox_sqlite").with_sqlite_identity(
            SqliteEventIdentity::new(
                SqliteDatabaseRole::PidMailbox,
                SqlitePathClass::ManagedFile,
                query_family,
            )
            .with_transaction_mode(SqliteTransactionMode::Autocommit),
        )
    }

    #[test]
    fn disabled_and_default_fast_success_do_not_touch_the_recorder() {
        let directory = tempfile::tempdir().unwrap();
        let recorder = recorder(directory.path());
        with_test_process_recorder(recorder.clone(), || {
            let disabled =
                SqliteOperationObserver::with_policy(SqliteObservationPolicy::disabled());
            assert_eq!(disabled.elapsed(), None);
            assert_eq!(
                disabled.record_success(
                    || panic!("disabled observation must not construct a span"),
                    DiagnosticPhase::Released,
                    OutcomeCertainty::Terminal,
                    |_| panic!("disabled observation must not construct evidence"),
                ),
                SqliteRecordDecision::Disabled
            );
            let fast = SqliteOperationObserver::with_policy(SqliteObservationPolicy::default());
            assert_eq!(
                fast.record_success(
                    || panic!("filtered fast success must not construct a span"),
                    DiagnosticPhase::Released,
                    OutcomeCertainty::Terminal,
                    |_| panic!("filtered fast success must not construct evidence"),
                ),
                SqliteRecordDecision::FilteredFastSuccess
            );
        });
        assert!(
            FlightRecorderReader::new(directory.path())
                .inspect()
                .events
                .is_empty()
        );
    }

    #[test]
    fn locked_database_failure_survives_with_typed_identity_phase_timing_and_trace() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("locked.db");
        let holder = Connection::open(&database).unwrap();
        holder
            .execute_batch("CREATE TABLE item (id INTEGER); BEGIN IMMEDIATE;")
            .unwrap();
        let contender = Connection::open(&database).unwrap();
        contender.busy_timeout(Duration::ZERO).unwrap();
        let recorder_root = directory.path().join("recorder");
        let recorder = recorder(&recorder_root);
        let trace = "3e5cf350-2304-4509-b912-0a0c34d4df89";
        with_test_process_recorder(recorder.clone(), || {
            let observer = SqliteOperationObserver::with_policy(SqliteObservationPolicy::all());
            let started = Instant::now();
            let error = contender
                .execute("INSERT INTO item (id) VALUES (?1)", [71])
                .unwrap_err();
            assert_eq!(
                observer.record_failure(
                    || {
                        span("mailbox.fixture.insert")
                            .with_identifier("invocation_uuid", trace)
                            .with_hashed_correlation("claim_token", "credential-sentinel")
                    },
                    &error,
                    OutcomeCertainty::StartedUnknown,
                    |_| statement_evidence(started.elapsed()),
                ),
                SqliteRecordDecision::RecordedContention
            );
        });
        recorder.drain_deferred_for_test().unwrap();
        holder.execute_batch("ROLLBACK").unwrap();

        let report = FlightRecorderReader::new(&recorder_root).inspect();
        assert_eq!(
            report
                .events
                .iter()
                .filter(|record| record.event.operation == "sqlite_fixture")
                .map(|record| record.event.phase)
                .collect::<Vec<_>>(),
            vec![DiagnosticPhase::Contention],
            "late observations must not invent a post-hoc requested phase"
        );
        let event = report
            .events
            .iter()
            .find(|record| record.event.phase == DiagnosticPhase::Contention)
            .unwrap();
        let identity = event.event.sqlite.as_ref().unwrap();
        assert_eq!(identity.database_role, SqliteDatabaseRole::PidMailbox);
        assert_eq!(identity.path_class, SqlitePathClass::ManagedFile);
        assert_eq!(identity.query_family, "mailbox.fixture.insert");
        assert_eq!(
            event
                .event
                .correlations
                .get("invocation_uuid")
                .map(String::as_str),
            Some(trace)
        );
        let evidence = event.event.observation.sqlite.as_ref().unwrap();
        assert_eq!(
            evidence.transaction_phase,
            Some(SqliteTransactionPhase::StatementExecution)
        );
        assert!(evidence.statement_total_micros.is_some());
        assert!(evidence.total_elapsed_micros.is_some());
        assert_eq!(
            event.event.elapsed_micros,
            evidence.total_elapsed_micros.unwrap()
        );
        assert!(evidence.execution_micros.is_none());
        assert!(
            evidence
                .measurement_gaps
                .contains(&SqliteMeasurementGap::WriterWaitAndExecutionNotSeparable)
        );
        let failure = event.event.observation.sqlite_failure.as_ref().unwrap();
        assert!(failure.contention);
        assert!(failure.primary_code.is_some());
        assert!(failure.extended_code.is_some());
        let encoded = serde_json::to_string(&event.event).unwrap();
        assert!(!encoded.contains("locked.db"), "{encoded}");
        assert!(!encoded.contains("credential-sentinel"), "{encoded}");
    }

    #[test]
    fn transaction_guard_separates_writer_execution_commit_and_post_commit_measurements() {
        let directory = tempfile::tempdir().unwrap();
        let recorder = recorder(directory.path());
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE item (id INTEGER PRIMARY KEY)")
            .unwrap();
        with_test_process_recorder(recorder.clone(), || {
            process_recorder().with_requested_span(
                SpanStart::new("transaction_fixture", "state_sqlite")
                    .with_sqlite_identity(
                        SqliteEventIdentity::new(
                            SqliteDatabaseRole::State,
                            SqlitePathClass::Memory,
                            "fixture.transaction",
                        )
                        .with_transaction_mode(SqliteTransactionMode::Immediate),
                    )
                    .with_identifier("invocation_uuid", "3758143f-547f-4296-834b-dadea4531ab8"),
                |span| {
                    let attempt = TransactionAttempt::start();
                    let transaction = connection
                        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                        .unwrap();
                    let mut phases = TransactionPhaseGuard::acquired(span, attempt);
                    let changed = transaction
                        .execute("INSERT INTO item (id) VALUES (?1)", [1])
                        .unwrap();
                    phases.add_rows_changed(changed);
                    phases.commit_started();
                    transaction.commit().unwrap();
                    phases.committed();
                    std::thread::sleep(Duration::from_millis(1));
                    phases.release_after_owner();
                },
            );
        });
        recorder.drain_deferred_for_test().unwrap();

        let report = FlightRecorderReader::new(directory.path()).inspect();
        let acquired = report
            .events
            .iter()
            .find(|record| record.event.phase == DiagnosticPhase::Acquired)
            .unwrap();
        assert_eq!(acquired.event.observation.wait_micros, None);
        let acquired_evidence = acquired.event.observation.sqlite.as_ref().unwrap();
        assert!(
            acquired_evidence
                .writer_authority_acquisition_micros
                .is_some()
        );
        assert_eq!(acquired_evidence.writer_authority_wait_micros, None);
        assert!(
            acquired_evidence
                .measurement_gaps
                .contains(&SqliteMeasurementGap::WriterWaitNotExposedByApi)
        );
        let released = report
            .events
            .iter()
            .find(|record| record.event.phase == DiagnosticPhase::Released)
            .unwrap();
        let evidence = released.event.observation.sqlite.as_ref().unwrap();
        assert_eq!(
            evidence.transaction_phase,
            Some(SqliteTransactionPhase::Released)
        );
        assert!(evidence.writer_authority_acquisition_micros.is_some());
        assert_eq!(evidence.writer_authority_wait_micros, None);
        assert!(
            evidence
                .measurement_gaps
                .contains(&SqliteMeasurementGap::WriterWaitNotExposedByApi)
        );
        assert!(evidence.total_elapsed_micros.is_some());
        assert!(evidence.execution_micros.is_some());
        assert!(evidence.commit_micros.is_some());
        assert!(evidence.post_commit_micros.is_some());
        assert_eq!(evidence.rows_changed, Some(1));
        assert_eq!(evidence.rows_examined, None);
        assert!(
            evidence
                .measurement_gaps
                .contains(&SqliteMeasurementGap::RowsExaminedNotExposed)
        );
        assert_eq!(
            released
                .event
                .correlations
                .get("invocation_uuid")
                .map(String::as_str),
            Some("3758143f-547f-4296-834b-dadea4531ab8")
        );
        assert!(released.event.elapsed_micros >= evidence.post_commit_micros.unwrap());
    }

    #[test]
    fn rollback_release_follows_owner_drop_and_reports_noncommit_terminal_gaps() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("rollback-release.db");
        let recorder_root = directory.path().join("recorder");
        let recorder = recorder(&recorder_root);
        let mut owner = Connection::open(&database).unwrap();
        owner
            .execute_batch("CREATE TABLE item (id INTEGER PRIMARY KEY)")
            .unwrap();
        let contender = Connection::open(&database).unwrap();
        contender.busy_timeout(Duration::ZERO).unwrap();

        with_test_process_recorder(recorder.clone(), || {
            process_recorder().with_requested_span(
                SpanStart::new("rollback_fixture", "pid_mailbox_sqlite").with_sqlite_identity(
                    SqliteEventIdentity::new(
                        SqliteDatabaseRole::PidMailbox,
                        SqlitePathClass::ManagedFile,
                        "fixture.rollback_release",
                    )
                    .with_transaction_mode(SqliteTransactionMode::Immediate),
                ),
                |span| {
                    let attempt = TransactionAttempt::start();
                    let transaction = owner
                        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                        .unwrap();
                    let mut phases = TransactionPhaseGuard::acquired(span, attempt);
                    transaction
                        .execute("INSERT INTO item (id) VALUES (?1)", [1])
                        .unwrap();
                    drop(transaction);
                    contender
                        .execute_batch("BEGIN IMMEDIATE; ROLLBACK;")
                        .expect("rollback must release the owner before terminal evidence");
                    phases.release_after_rollback();
                },
            );
        });
        recorder.drain_deferred_for_test().unwrap();

        let report = FlightRecorderReader::new(&recorder_root).inspect();
        let released = report
            .events
            .iter()
            .find(|record| record.event.phase == DiagnosticPhase::Released)
            .unwrap();
        let evidence = released.event.observation.sqlite.as_ref().unwrap();
        assert!(evidence.execution_micros.is_some());
        assert_eq!(evidence.commit_micros, None);
        assert!(
            evidence
                .measurement_gaps
                .contains(&SqliteMeasurementGap::CommitNotApplicable)
        );
        assert!(
            evidence
                .measurement_gaps
                .contains(&SqliteMeasurementGap::PostCommitNotApplicable)
        );
    }

    #[test]
    fn sqlite_primary_and_extended_codes_are_retained_without_error_text() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE item (value TEXT UNIQUE); INSERT INTO item VALUES ('x');")
            .unwrap();
        let error = connection
            .execute("INSERT INTO item VALUES (?1)", ["x"])
            .unwrap_err();
        let failure = SqliteFailure::from_error(&error);
        assert_eq!(failure.primary_code.as_deref(), Some("ConstraintViolation"));
        assert_eq!(
            failure.extended_code,
            Some(rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE)
        );
        assert_eq!(failure.message, "sqlite operation failed");
    }

    #[test]
    fn opt_in_query_plan_is_bounded_and_never_retains_sql_or_bound_values() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE credential (id INTEGER PRIMARY KEY, secret TEXT);")
            .unwrap();
        let sentinel = "bound-password-must-not-appear";
        let evidence = capture_query_plan(
            &connection,
            "SELECT id FROM credential WHERE secret=?1 UNION ALL SELECT id FROM credential WHERE secret=?1",
            [sentinel],
            true,
            1,
        );
        let encoded = serde_json::to_string(&evidence).unwrap();
        assert!(!encoded.contains(sentinel), "{encoded}");
        assert!(!encoded.contains("credential"), "{encoded}");
        assert!(!encoded.contains("secret"), "{encoded}");
        assert!(matches!(
            evidence,
            SqliteQueryPlanEvidence::Captured {
                nodes_seen: 1,
                truncated: true,
                ..
            }
        ));

        assert_eq!(
            capture_query_plan(&connection, "SELECT ?1", [sentinel], false, usize::MAX,),
            SqliteQueryPlanEvidence::Disabled
        );
    }

    #[test]
    fn policy_inputs_are_bounded_and_explicit() {
        let disabled = SqliteObservationPolicy::from_values(
            Some("off"),
            Some("999999"),
            Some("999999999"),
            Some("true"),
        );
        assert!(!disabled.enabled);
        assert_eq!(disabled.slow_threshold, DEFAULT_SQLITE_SLOW_THRESHOLD);
        assert_eq!(disabled.normal_sample_every, 0);
        assert!(disabled.query_plans);

        let all =
            SqliteObservationPolicy::from_values(Some("all"), Some("7"), Some("5"), Some("false"));
        assert_eq!(all.slow_threshold, Duration::from_millis(7));
        assert_eq!(all.normal_sample_every, 5);
        assert!(!all.query_plans);
    }

    #[test]
    fn invalid_query_family_cannot_be_used_as_a_sql_or_value_channel() {
        let identity = SqliteEventIdentity::new(
            SqliteDatabaseRole::State,
            SqlitePathClass::ExternalFile,
            "SELECT * FROM secret WHERE token='credential'",
        );
        assert_eq!(identity.query_family, "invalid_query_family");
        let encoded = serde_json::to_string(&identity).unwrap();
        assert!(!encoded.contains("credential"));
        assert!(!encoded.contains("SELECT"));

        let directory = tempfile::tempdir().unwrap();
        let recorder = recorder(directory.path());
        with_test_process_recorder(recorder.clone(), || {
            let error = rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(
                "password=credential-sentinel",
            )));
            SqliteOperationObserver::with_policy(SqliteObservationPolicy::all()).record_failure(
                || span("fixture.failure"),
                &error,
                OutcomeCertainty::NotStarted,
                |_| SqlitePhaseEvidence::for_phase(SqliteTransactionPhase::StatementExecution),
            );
        });
        recorder.drain_deferred_for_test().unwrap();
        let text = fs::read_dir(directory.path())
            .unwrap()
            .filter_map(Result::ok)
            .flat_map(|entry| fs::read(entry.path()).unwrap_or_default())
            .collect::<Vec<_>>();
        assert!(
            !String::from_utf8(text)
                .unwrap()
                .contains("credential-sentinel")
        );
    }
}
