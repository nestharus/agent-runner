//! Small fail-open helpers for instrumented SQLite transaction boundaries.

use crate::diagnostic_recorder::{
    DiagnosticEvent, DiagnosticPhase, DiagnosticSpan, PhaseObservation, RecorderProcessIdentity,
    SqliteFailure, SqliteMeasurementGap, SqlitePhaseEvidence, SqliteTransactionPhase,
};
use crate::event_store::{
    Digest32, EventCorrelations, EventEnvelopeV1, EventFamily, EventId, EventKind,
    EventWriterConfig, NativeProcessIdentity, NewEventV1, PayloadNormalizationPolicy,
    PendingAppend, ProcessEventWriter, ProcessInstanceId, ProducerIdentity, SpanId, TraceId,
    WriterError, WriterInstanceId,
};
use crate::lifecycle_log::{NormalizedLifecycleObservation, lifecycle_payload_fields};
use chrono::{DateTime, Utc};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use uuid::Uuid;

static EVENT_SINK: OnceLock<Mutex<Option<ConfiguredEventSink>>> = OnceLock::new();
static PRODUCER_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventSinkMode {
    /// Partitioned SQLite is normal; JSONL is used only after sink failure.
    NormalWithJsonlFallback,
    /// Both sinks receive the same immutable envelope during verified cutover.
    ShadowWithJsonl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventSubmitDurability {
    RequiredDurable,
    BestEffort,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, allow(dead_code))]
pub enum EventSubmitStatus {
    Appended,
    Queued,
    Backpressure,
    Disconnected,
    Unavailable,
    Failed(String),
}

#[derive(Debug)]
pub struct EventSinkSubmission {
    status: EventSubmitStatus,
    pending: Option<PendingAppend>,
}

impl EventSinkSubmission {
    pub fn finished(status: EventSubmitStatus) -> Self {
        debug_assert!(status != EventSubmitStatus::Queued);
        Self {
            status,
            pending: None,
        }
    }

    pub fn queued(pending: PendingAppend) -> Self {
        Self {
            status: EventSubmitStatus::Queued,
            pending: Some(pending),
        }
    }
}

pub trait EventEnvelopeSink: Send + Sync {
    fn producer_identity(&self) -> ProducerIdentity;
    fn submit(
        &self,
        envelope: &EventEnvelopeV1,
        durability: EventSubmitDurability,
    ) -> EventSinkSubmission;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventSinkInstallError {
    AlreadyInstalled,
    Unavailable,
}

#[derive(Clone)]
struct ConfiguredEventSink {
    sink: Arc<dyn EventEnvelopeSink>,
    mode: EventSinkMode,
}

#[derive(Debug)]
pub(crate) struct PreparedEventSubmission {
    pub(crate) envelope: EventEnvelopeV1,
    pub(crate) status: EventSubmitStatus,
    pub(crate) pending: Option<PendingAppend>,
    pub(crate) mode: EventSinkMode,
}

/// Installs the process-local evidence sink exactly once. Test isolation clears
/// the slot through the cfg(test)-only helper below; production never replaces
/// a live writer instance.
pub fn install_process_event_sink(
    sink: Arc<dyn EventEnvelopeSink>,
    mode: EventSinkMode,
) -> Result<(), EventSinkInstallError> {
    let mut slot = EVENT_SINK
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map_err(|_| EventSinkInstallError::Unavailable)?;
    if slot.is_some() {
        return Err(EventSinkInstallError::AlreadyInstalled);
    }
    *slot = Some(ConfiguredEventSink { sink, mode });
    Ok(())
}

#[cfg_attr(test, allow(dead_code))]
pub(crate) fn ensure_default_process_event_sink(
    process: &RecorderProcessIdentity,
) -> Result<(), EventSinkInstallError> {
    if configured_sink().is_some() {
        return Ok(());
    }
    let producer = producer_identity_from_recorder(process);
    if producer.native_process.is_none() {
        return Err(EventSinkInstallError::Unavailable);
    }
    let root = crate::paths::data_dir()
        .map_err(|_| EventSinkInstallError::Unavailable)?
        .join("diagnostics/event-store-v1");
    let writer = ProcessEventWriter::start(EventWriterConfig::native(root, producer.clone()))
        .map_err(|_| EventSinkInstallError::Unavailable)?;
    install_process_event_sink(
        Arc::new(ProcessWriterEventSink { producer, writer }),
        EventSinkMode::NormalWithJsonlFallback,
    )
}

#[cfg_attr(test, allow(dead_code))]
struct ProcessWriterEventSink {
    producer: ProducerIdentity,
    writer: ProcessEventWriter,
}

impl EventEnvelopeSink for ProcessWriterEventSink {
    fn producer_identity(&self) -> ProducerIdentity {
        self.producer.clone()
    }

    fn submit(
        &self,
        envelope: &EventEnvelopeV1,
        durability: EventSubmitDurability,
    ) -> EventSinkSubmission {
        match durability {
            EventSubmitDurability::RequiredDurable => match self.writer.append(envelope.clone()) {
                Ok(_) => EventSinkSubmission::finished(EventSubmitStatus::Appended),
                Err(WriterError::QueueFull) => {
                    EventSinkSubmission::finished(EventSubmitStatus::Backpressure)
                }
                Err(WriterError::QueueDisconnected | WriterError::ReplyDisconnected { .. }) => {
                    EventSinkSubmission::finished(EventSubmitStatus::Disconnected)
                }
                Err(_) => EventSinkSubmission::finished(EventSubmitStatus::Failed(
                    "append_failed".to_string(),
                )),
            },
            EventSubmitDurability::BestEffort => match self.writer.try_append(envelope.clone()) {
                Ok(pending) => EventSinkSubmission::queued(pending),
                Err(error) => match error.kind {
                    crate::event_store::EnqueueErrorKind::Full => {
                        EventSinkSubmission::finished(EventSubmitStatus::Backpressure)
                    }
                    crate::event_store::EnqueueErrorKind::Disconnected => {
                        EventSinkSubmission::finished(EventSubmitStatus::Disconnected)
                    }
                },
            },
        }
    }
}

#[cfg(test)]
pub(crate) fn clear_process_event_sink_for_test() {
    if let Ok(mut sink) = EVENT_SINK.get_or_init(|| Mutex::new(None)).lock() {
        *sink = None;
    }
    PRODUCER_SEQUENCE.store(0, Ordering::Relaxed);
}

pub(crate) fn submit_diagnostic_event(
    event: &DiagnosticEvent,
    durability: EventSubmitDurability,
) -> Result<PreparedEventSubmission, String> {
    let configured = configured_sink();
    let producer = configured
        .as_ref()
        .map(|configured| configured.sink.producer_identity())
        .unwrap_or_else(|| producer_identity_from_recorder(&event.process));
    let envelope = diagnostic_envelope(event, producer)?;
    let submitted = configured
        .as_ref()
        .map(|configured| configured.sink.submit(&envelope, durability))
        .unwrap_or_else(|| EventSinkSubmission::finished(EventSubmitStatus::Unavailable));
    Ok(PreparedEventSubmission {
        envelope,
        status: submitted.status,
        pending: submitted.pending,
        mode: configured
            .map(|configured| configured.mode)
            .unwrap_or(EventSinkMode::NormalWithJsonlFallback),
    })
}

pub(crate) fn submit_lifecycle_observation(
    observation: &NormalizedLifecycleObservation,
    process: &RecorderProcessIdentity,
) -> Result<PreparedEventSubmission, String> {
    let configured = configured_sink();
    let ingested_at = Utc::now().timestamp_micros().max(0);
    let sequence = next_producer_sequence()?;
    let policy = PayloadNormalizationPolicy::registered(lifecycle_payload_fields())
        .map_err(|error| error.to_string())?;
    let producer = configured
        .as_ref()
        .map(|configured| configured.sink.producer_identity())
        .unwrap_or_else(|| producer_identity_from_recorder(process));
    let envelope = EventEnvelopeV1::normalize(
        NewEventV1 {
            event_id: EventId::random(),
            family: EventFamily::Log,
            kind: observation.kind.clone(),
            recorded_at_unix_micros: observation.recorded_at_unix_micros,
            producer_sequence: sequence,
            producer,
            correlations: observation.correlations.clone(),
            payload: observation.payload.clone(),
            legacy_provenance: None,
            retry_of_generation_id: None,
        },
        &policy,
        ingested_at,
    )
    .map_err(|error| error.to_string())?;
    let submitted = configured
        .as_ref()
        .map(|configured| {
            configured
                .sink
                .submit(&envelope, EventSubmitDurability::BestEffort)
        })
        .unwrap_or_else(|| EventSinkSubmission::finished(EventSubmitStatus::Unavailable));
    Ok(PreparedEventSubmission {
        envelope,
        status: submitted.status,
        pending: submitted.pending,
        mode: configured
            .map(|configured| configured.mode)
            .unwrap_or(EventSinkMode::NormalWithJsonlFallback),
    })
}

fn configured_sink() -> Option<ConfiguredEventSink> {
    EVENT_SINK
        .get_or_init(|| Mutex::new(None))
        .try_lock()
        .ok()
        .and_then(|sink| sink.clone())
}

fn next_producer_sequence() -> Result<i64, String> {
    PRODUCER_SEQUENCE
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            (current < i64::MAX as u64).then_some(current + 1)
        })
        .map(|sequence| sequence as i64)
        .map_err(|_| "producer_sequence_exhausted".to_string())
}

fn diagnostic_envelope(
    event: &DiagnosticEvent,
    producer: ProducerIdentity,
) -> Result<EventEnvelopeV1, String> {
    let event_id = uuid_from_display(&event.event_id.to_string())?;
    let trace_id = uuid_from_display(&event.diagnostic_id.to_string())?;
    let span_id = uuid_from_display(&event.span_id.to_string())?;
    let parent_span_id = event
        .parent_span_id
        .as_ref()
        .map(|id| uuid_from_display(&id.to_string()).map(SpanId::from))
        .transpose()?;
    let recorded_at = DateTime::parse_from_rfc3339(&event.recorded_at)
        .map_err(|_| "diagnostic_recorded_at_invalid".to_string())?
        .timestamp_micros()
        .max(0);
    let ingested_at = Utc::now().timestamp_micros().max(0);
    let payload = json!({
        "operation": event.operation,
        "resource": event.resource,
        "lifecycle_phase": event.lifecycle_phase,
        "phase": event.phase,
        "elapsed_micros": event.elapsed_micros,
        "observation": event.observation,
        "diagnostic_correlations": event.correlations,
        "sqlite": event.sqlite,
    });
    let policy = PayloadNormalizationPolicy::registered(&[
        "operation",
        "resource",
        "lifecycle_phase",
        "phase",
        "elapsed_micros",
        "observation",
        "diagnostic_correlations",
        "sqlite",
    ])
    .map_err(|error| error.to_string())?;
    EventEnvelopeV1::normalize(
        NewEventV1 {
            event_id: EventId::from(event_id),
            family: EventFamily::Diagnostic,
            kind: EventKind::registered("diagnostic.observation")
                .map_err(|error| error.to_string())?,
            recorded_at_unix_micros: recorded_at,
            producer_sequence: next_producer_sequence()?,
            producer,
            correlations: EventCorrelations {
                trace_id: Some(TraceId::from(trace_id)),
                span_id: Some(SpanId::from(span_id)),
                parent_span_id,
                invocation_uuid: None,
                session_correlation_sha256: None,
            },
            payload,
            legacy_provenance: None,
            retry_of_generation_id: None,
        },
        &policy,
        ingested_at,
    )
    .map_err(|error| error.to_string())
}

fn uuid_from_display(value: &str) -> Result<Uuid, String> {
    Uuid::parse_str(value).map_err(|_| "diagnostic_identity_invalid".to_string())
}

/// Builds the producer identity used by a production writer from the same
/// native recorder evidence. Writer startup may replace the process-root and
/// parent correlations with stronger inherited identities before installing
/// the sink.
pub fn producer_identity_from_recorder(process: &RecorderProcessIdentity) -> ProducerIdentity {
    let process_uuid =
        uuid_from_display(&process.producer_instance.to_string()).unwrap_or_else(|_| Uuid::nil());
    let process_instance_id = ProcessInstanceId::from(process_uuid);
    let mut writer_digest = Sha256::new();
    writer_digest.update(b"oulipoly.event-writer-instance.v1\0");
    writer_digest.update(process_uuid.as_bytes());
    let writer_bytes: [u8; 16] = writer_digest.finalize()[..16].try_into().unwrap_or([0; 16]);
    ProducerIdentity {
        writer_instance_id: WriterInstanceId::from_bytes(writer_bytes),
        process_instance_id,
        process_root_id: process_instance_id,
        parent_process_instance_id: None,
        supervisor_authority_id: None,
        native_process: process.os_boot_id.as_ref().and_then(|boot| {
            process
                .os_pid_starttime_ticks
                .filter(|starttime| *starttime > 0 && process.os_pid > 0)
                .map(|starttime| NativeProcessIdentity {
                    os_pid: process.os_pid,
                    os_boot_id_sha256: Digest32::sha256(boot.as_bytes()),
                    os_pid_starttime_ticks: starttime,
                })
        }),
    }
}

#[cfg(test)]
thread_local! {
    static BEFORE_RELEASE_OBSERVATION: std::cell::RefCell<Option<Box<dyn FnMut()>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn with_before_release_observation_for_test<T>(
    hook: impl FnMut() + 'static,
    operation: impl FnOnce() -> T,
) -> T {
    BEFORE_RELEASE_OBSERVATION.with_borrow_mut(|slot| {
        assert!(
            slot.is_none(),
            "release-observation hook is already installed"
        );
        *slot = Some(Box::new(hook));
    });
    let result = operation();
    BEFORE_RELEASE_OBSERVATION.with_borrow_mut(|slot| {
        *slot = None;
    });
    result
}

#[cfg(test)]
fn before_release_observation_for_test() {
    BEFORE_RELEASE_OBSERVATION.with_borrow_mut(|slot| {
        if let Some(hook) = slot.as_deref_mut() {
            hook();
        }
    });
}

/// Clock started immediately before a real SQLite transaction-begin attempt.
/// It deliberately excludes recorder work performed before the attempt.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TransactionAttempt(Instant);

impl TransactionAttempt {
    pub(crate) fn start() -> Self {
        Self(Instant::now())
    }

    fn elapsed(self) -> std::time::Duration {
        self.0.elapsed()
    }
}

pub(crate) struct TransactionPhaseGuard<'a> {
    span: &'a DiagnosticSpan,
    transaction_started: Instant,
    acquired_at: Instant,
    writer_acquisition: Duration,
    execution: Option<Duration>,
    commit_started_at: Option<Instant>,
    commit: Option<Duration>,
    committed_at: Option<Instant>,
    rows_changed: Option<u64>,
    committed: bool,
    failure_recorded: bool,
    released: bool,
}

impl<'a> TransactionPhaseGuard<'a> {
    pub(crate) fn acquired(span: &'a DiagnosticSpan, attempt: TransactionAttempt) -> Self {
        let writer_acquisition = attempt.elapsed();
        let _ = span.record(
            DiagnosticPhase::Acquired,
            PhaseObservation::started_unknown().with_sqlite_evidence(
                SqlitePhaseEvidence::for_phase(SqliteTransactionPhase::WriterAuthority)
                    .with_writer_acquisition(writer_acquisition)
                    .with_gap(SqliteMeasurementGap::WriterWaitNotExposedByApi)
                    .with_gap(SqliteMeasurementGap::ExecutionNotReached)
                    .with_gap(SqliteMeasurementGap::CommitNotReached)
                    .with_gap(SqliteMeasurementGap::PostCommitNotReached)
                    .with_gap(SqliteMeasurementGap::RowsExaminedNotExposed)
                    .with_gap(SqliteMeasurementGap::RowsChangedNotReported),
            ),
        );
        Self {
            span,
            transaction_started: attempt.0,
            acquired_at: Instant::now(),
            writer_acquisition,
            execution: None,
            commit_started_at: None,
            commit: None,
            committed_at: None,
            rows_changed: None,
            committed: false,
            failure_recorded: false,
            released: false,
        }
    }

    pub(crate) fn commit_started(&mut self) {
        let execution = self.acquired_at.elapsed();
        self.execution = Some(execution);
        let _ = self.span.record(
            DiagnosticPhase::CommitStarted,
            PhaseObservation::effects_possible().with_sqlite_evidence(
                self.evidence(SqliteTransactionPhase::Commit)
                    .with_execution(execution)
                    .with_gap(SqliteMeasurementGap::CommitNotReached)
                    .with_gap(SqliteMeasurementGap::PostCommitNotReached),
            ),
        );
        // Exclude recorder serialization/queue work from the measured commit
        // call that begins immediately after this method returns.
        self.commit_started_at = Some(Instant::now());
    }

    pub(crate) fn committed(&mut self) {
        let commit = self
            .commit_started_at
            .map(|started| started.elapsed())
            .unwrap_or_default();
        self.commit = Some(commit);
        self.committed = true;
        let _ = self.span.record(
            DiagnosticPhase::Committed,
            PhaseObservation::committed().with_sqlite_evidence(
                self.evidence(SqliteTransactionPhase::Commit)
                    .with_commit(commit)
                    .with_gap(SqliteMeasurementGap::PostCommitOutsideBoundary),
            ),
        );
        // Post-commit work starts after the committed observation itself so
        // recorder overhead is not attributed to application cleanup.
        self.committed_at = Some(Instant::now());
    }

    /// Adds only a count returned directly by a rusqlite execute API. Callers
    /// must not estimate examined or changed rows from query shape.
    #[allow(dead_code)]
    pub(crate) fn add_rows_changed(&mut self, rows: usize) {
        self.rows_changed = Some(
            self.rows_changed
                .unwrap_or(0)
                .saturating_add(u64::try_from(rows).unwrap_or(u64::MAX)),
        );
    }

    pub(crate) fn sqlite_failure(&mut self, error: &rusqlite::Error) {
        let failure = SqliteFailure::from_error(error);
        let phase = if failure.contention {
            DiagnosticPhase::Contention
        } else {
            DiagnosticPhase::Failed
        };
        let evidence = if let Some(started) = self.commit_started_at {
            let commit = started.elapsed();
            self.commit = Some(commit);
            self.evidence(SqliteTransactionPhase::Commit)
                .with_commit(commit)
                .with_gap(SqliteMeasurementGap::PostCommitNotReached)
        } else {
            let execution = self.acquired_at.elapsed();
            self.execution = Some(execution);
            self.evidence(SqliteTransactionPhase::StatementExecution)
                .with_execution(execution)
                .with_gap(SqliteMeasurementGap::CommitNotReached)
                .with_gap(SqliteMeasurementGap::PostCommitNotReached)
        };
        let _ = self.span.record(
            phase,
            PhaseObservation::effects_possible()
                .with_sqlite_failure(error)
                .with_sqlite_evidence(evidence),
        );
        self.failure_recorded = true;
    }

    pub(crate) fn failed(&mut self, cause: &'static str) {
        if self.failure_recorded {
            return;
        }
        let execution = self.execution.unwrap_or_else(|| self.acquired_at.elapsed());
        self.execution = Some(execution);
        let _ = self.span.record(
            DiagnosticPhase::Failed,
            PhaseObservation::effects_possible()
                .with_cause(cause)
                .with_sqlite_evidence(
                    self.evidence(SqliteTransactionPhase::StatementExecution)
                        .with_execution(execution)
                        .with_gap(SqliteMeasurementGap::CommitNotReached)
                        .with_gap(SqliteMeasurementGap::PostCommitNotReached),
                ),
        );
        self.failure_recorded = true;
    }

    /// Emits terminal evidence only after the caller has explicitly ended the
    /// owning SQLite transaction (commit, rollback, or drop).
    pub(crate) fn release_after_owner(&mut self) {
        if self.released {
            return;
        }
        if self.execution.is_none() {
            self.execution = Some(self.acquired_at.elapsed());
        }
        let mut observation = if self.committed {
            PhaseObservation::committed()
        } else {
            PhaseObservation::effects_possible()
        };
        let mut evidence = self.evidence(SqliteTransactionPhase::Released);
        if let Some(committed_at) = self.committed_at {
            evidence = evidence.with_post_commit(committed_at.elapsed());
        } else {
            if self.commit.is_none() {
                evidence = evidence.with_gap(SqliteMeasurementGap::CommitNotReached);
            }
            evidence = evidence.with_gap(SqliteMeasurementGap::PostCommitNotReached);
        }
        observation = observation.with_sqlite_evidence(evidence);
        #[cfg(test)]
        before_release_observation_for_test();
        let _ = self.span.record(DiagnosticPhase::Released, observation);
        self.released = true;
    }

    /// Records a deliberately non-committing transaction after its owner has
    /// been rolled back/dropped. Validation fences use this to distinguish an
    /// inapplicable commit from a commit that failed or was abandoned.
    pub(crate) fn release_after_rollback(&mut self) {
        if self.execution.is_none() {
            self.execution = Some(self.acquired_at.elapsed());
        }
        if self.released {
            return;
        }
        let evidence = self
            .evidence(SqliteTransactionPhase::Released)
            .with_gap(SqliteMeasurementGap::CommitNotApplicable)
            .with_gap(SqliteMeasurementGap::PostCommitNotApplicable);
        #[cfg(test)]
        before_release_observation_for_test();
        let _ = self.span.record(
            DiagnosticPhase::Released,
            PhaseObservation::terminal().with_sqlite_evidence(evidence),
        );
        self.released = true;
    }

    fn evidence(&self, phase: SqliteTransactionPhase) -> SqlitePhaseEvidence {
        let mut evidence = SqlitePhaseEvidence::for_phase(phase)
            .with_total_elapsed(self.transaction_started.elapsed())
            .with_writer_acquisition(self.writer_acquisition)
            .with_gap(SqliteMeasurementGap::WriterWaitNotExposedByApi)
            .with_gap(SqliteMeasurementGap::RowsExaminedNotExposed);
        if let Some(execution) = self.execution {
            evidence = evidence.with_execution(execution);
        }
        if let Some(commit) = self.commit {
            evidence = evidence.with_commit(commit);
        }
        if let Some(rows) = self.rows_changed {
            evidence = evidence.with_rows_changed(rows);
        } else {
            evidence = evidence.with_gap(SqliteMeasurementGap::RowsChangedNotReported);
        }
        evidence
    }
}

pub(crate) fn record_sqlite_failure(
    span: &DiagnosticSpan,
    error: &rusqlite::Error,
    attempt: TransactionAttempt,
) {
    let writer_acquisition = attempt.elapsed();
    let failure = SqliteFailure::from_error(error);
    let phase = if failure.contention {
        DiagnosticPhase::Contention
    } else {
        DiagnosticPhase::Failed
    };
    let _ = span.record(
        phase,
        PhaseObservation::not_started()
            .with_sqlite_failure(error)
            .with_sqlite_evidence(
                SqlitePhaseEvidence::for_phase(SqliteTransactionPhase::WriterAuthority)
                    .with_total_elapsed(writer_acquisition)
                    .with_writer_acquisition(writer_acquisition)
                    .with_gap(SqliteMeasurementGap::WriterWaitNotExposedByApi)
                    .with_gap(SqliteMeasurementGap::ExecutionNotReached)
                    .with_gap(SqliteMeasurementGap::CommitNotReached)
                    .with_gap(SqliteMeasurementGap::PostCommitNotReached)
                    .with_gap(SqliteMeasurementGap::RowsExaminedNotExposed)
                    .with_gap(SqliteMeasurementGap::RowsChangedNotReported),
            ),
    );
}

pub(crate) fn record_unacquired_release(span: &DiagnosticSpan) {
    let _ = span.record(
        DiagnosticPhase::Released,
        PhaseObservation::not_started().with_sqlite_evidence(
            SqlitePhaseEvidence::for_phase(SqliteTransactionPhase::Released)
                .with_gap(SqliteMeasurementGap::WriterAuthorityNotReached)
                .with_gap(SqliteMeasurementGap::ExecutionNotReached)
                .with_gap(SqliteMeasurementGap::CommitNotReached)
                .with_gap(SqliteMeasurementGap::PostCommitNotReached)
                .with_gap(SqliteMeasurementGap::RowsExaminedNotExposed)
                .with_gap(SqliteMeasurementGap::RowsChangedNotReported),
        ),
    );
}
