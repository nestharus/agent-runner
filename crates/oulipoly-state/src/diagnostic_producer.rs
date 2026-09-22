//! Small fail-open helpers for instrumented SQLite transaction boundaries.

use crate::diagnostic_recorder::{
    DiagnosticEvent, DiagnosticPhase, DiagnosticSpan, PhaseObservation, RecorderProcessIdentity,
    SqliteFailure, SqliteMeasurementGap, SqlitePhaseEvidence, SqliteTransactionPhase,
};
use crate::event_store::{
    Digest32, EventCorrelations, EventEnvelopeV1, EventFamily, EventId, EventKind,
    EventWriterConfig, NativeProcessIdentity, NewEventV1, PayloadNormalizationPolicy,
    PendingAppend, ProcessEventWriter, ProcessEventWriterSubmission, ProcessInstanceId,
    ProducerIdentity, SpanId, TraceId, WriterError, WriterInstanceId,
};
use crate::lifecycle_log::{NormalizedLifecycleObservation, lifecycle_payload_fields};
use chrono::{DateTime, Utc};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use uuid::Uuid;

static EVENT_SINK: OnceLock<Mutex<ProcessEventSinkState>> = OnceLock::new();
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

    /// Finish an ordinary process lifecycle. Evidence sinks without an owned
    /// lifecycle may keep the default no-op; the production process writer
    /// consumes its exact writer and runs the normal rotation/closure path.
    fn shutdown(&self) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub enum EventSinkInstallError {
    AlreadyInstalled,
    ProcessShutDown,
    Unavailable,
}

#[derive(Clone)]
struct ConfiguredEventSink {
    sink: Arc<dyn EventEnvelopeSink>,
    mode: EventSinkMode,
}

enum ProcessEventSinkState {
    NeverInstalled,
    Installed(ConfiguredEventSink),
    Shutdown,
}

#[derive(Debug)]
pub(crate) struct PreparedEventSubmission {
    pub(crate) envelope: EventEnvelopeV1,
    pub(crate) status: EventSubmitStatus,
    pub(crate) pending: Option<PendingAppend>,
    pub(crate) mode: EventSinkMode,
}

/// Installs the process-local evidence sink exactly once before ordinary process
/// shutdown. Test isolation clears the state through the cfg(test)-only helper
/// below; production never replaces a live writer or reopens a terminally shut
/// down lifecycle.
#[cfg_attr(not(test), allow(dead_code))]
pub fn install_process_event_sink(
    sink: Arc<dyn EventEnvelopeSink>,
    mode: EventSinkMode,
) -> Result<(), EventSinkInstallError> {
    let mut state = process_event_sink_state()
        .lock()
        .map_err(|_| EventSinkInstallError::Unavailable)?;
    match &*state {
        ProcessEventSinkState::NeverInstalled => {}
        ProcessEventSinkState::Installed(_) => {
            return Err(EventSinkInstallError::AlreadyInstalled);
        }
        ProcessEventSinkState::Shutdown => {
            return Err(EventSinkInstallError::ProcessShutDown);
        }
    }
    *state = ProcessEventSinkState::Installed(ConfiguredEventSink { sink, mode });
    Ok(())
}

#[cfg_attr(test, allow(dead_code))]
pub(crate) fn ensure_default_process_event_sink(
    process: &RecorderProcessIdentity,
) -> Result<(), EventSinkInstallError> {
    ensure_process_event_sink_with(|| {
        let producer = producer_identity_from_recorder(process);
        if producer.native_process.is_none() {
            return Err(EventSinkInstallError::Unavailable);
        }
        let root = crate::paths::data_dir()
            .map_err(|_| EventSinkInstallError::Unavailable)?
            .join("diagnostics/event-store-v1");
        let writer = ProcessEventWriter::start(EventWriterConfig::native(root, producer.clone()))
            .map_err(|_| EventSinkInstallError::Unavailable)?;
        Ok(ConfiguredEventSink {
            sink: Arc::new(ProcessWriterEventSink::new(producer, writer)),
            mode: EventSinkMode::NormalWithJsonlFallback,
        })
    })
}

/// Serialize lazy default construction with terminal shutdown. Holding the
/// lifecycle mutex through construction gives the two operations one order:
/// installation either finishes and becomes eligible for shutdown, or shutdown
/// wins before this initializer is invoked and no writer/head side effect occurs.
fn ensure_process_event_sink_with(
    initialize: impl FnOnce() -> Result<ConfiguredEventSink, EventSinkInstallError>,
) -> Result<(), EventSinkInstallError> {
    let mut state = process_event_sink_state()
        .lock()
        .map_err(|_| EventSinkInstallError::Unavailable)?;
    match &*state {
        ProcessEventSinkState::Installed(_) => return Ok(()),
        ProcessEventSinkState::Shutdown => {
            return Err(EventSinkInstallError::ProcessShutDown);
        }
        ProcessEventSinkState::NeverInstalled => {}
    }
    *state = ProcessEventSinkState::Installed(initialize()?);
    Ok(())
}

#[cfg_attr(test, allow(dead_code))]
struct ProcessWriterEventSink {
    producer: ProducerIdentity,
    submissions: ProcessEventWriterSubmission,
    writer: Mutex<Option<ProcessEventWriter>>,
    #[cfg(test)]
    required_admission_observer: Option<std::sync::mpsc::Sender<()>>,
}

impl ProcessWriterEventSink {
    fn new(producer: ProducerIdentity, writer: ProcessEventWriter) -> Self {
        Self {
            producer,
            submissions: writer.submission_handle(),
            writer: Mutex::new(Some(writer)),
            #[cfg(test)]
            required_admission_observer: None,
        }
    }

    #[cfg(test)]
    fn with_required_admission_observer(
        producer: ProducerIdentity,
        writer: ProcessEventWriter,
        observer: std::sync::mpsc::Sender<()>,
    ) -> Self {
        let mut sink = Self::new(producer, writer);
        sink.required_admission_observer = Some(observer);
        sink
    }
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
            EventSubmitDurability::RequiredDurable => {
                match self.submissions.try_append(envelope.clone()) {
                    Ok(pending) => {
                        #[cfg(test)]
                        if let Some(observer) = &self.required_admission_observer {
                            let _ = observer.send(());
                        }
                        match pending.wait() {
                            Ok(_) => EventSinkSubmission::finished(EventSubmitStatus::Appended),
                            Err(WriterError::QueueFull) => {
                                EventSinkSubmission::finished(EventSubmitStatus::Backpressure)
                            }
                            Err(
                                WriterError::QueueDisconnected
                                | WriterError::ReplyDisconnected { .. },
                            ) => EventSinkSubmission::finished(EventSubmitStatus::Disconnected),
                            Err(_) => EventSinkSubmission::finished(EventSubmitStatus::Failed(
                                "append_failed".to_string(),
                            )),
                        }
                    }
                    Err(error) if error.kind == crate::event_store::EnqueueErrorKind::Full => {
                        EventSinkSubmission::finished(EventSubmitStatus::Backpressure)
                    }
                    Err(_) => EventSinkSubmission::finished(EventSubmitStatus::Disconnected),
                }
            }
            EventSubmitDurability::BestEffort => {
                match self.submissions.try_append(envelope.clone()) {
                    Ok(pending) => EventSinkSubmission::queued(pending),
                    Err(error) => match error.kind {
                        crate::event_store::EnqueueErrorKind::Full => {
                            EventSinkSubmission::finished(EventSubmitStatus::Backpressure)
                        }
                        crate::event_store::EnqueueErrorKind::Disconnected => {
                            EventSinkSubmission::finished(EventSubmitStatus::Disconnected)
                        }
                    },
                }
            }
        }
    }

    fn shutdown(&self) -> Result<(), String> {
        let writer = self
            .writer
            .lock()
            .map_err(|_| "event writer lifecycle lock is unavailable".to_string())?
            .take();
        match writer {
            Some(writer) => writer.shutdown().map_err(|error| error.to_string()),
            None => Ok(()),
        }
    }
}

/// Best-effort ordinary process closure for the installed process-local event
/// sink. The terminal state rejects later first/re-installation. Transitioning
/// before closing the taken sink prevents new admissions from racing behind the
/// closure; an already-admitted append is drained by the exact writer shutdown
/// protocol. Abrupt process death does not call this path and remains fail-closed.
pub fn shutdown_process_event_sink() -> Result<(), String> {
    let configured = {
        let mut state = process_event_sink_state()
            .lock()
            .map_err(|_| "process event sink lifecycle lock is unavailable".to_string())?;
        match std::mem::replace(&mut *state, ProcessEventSinkState::Shutdown) {
            ProcessEventSinkState::Installed(configured) => Some(configured),
            ProcessEventSinkState::NeverInstalled | ProcessEventSinkState::Shutdown => None,
        }
    };
    configured.map_or(Ok(()), |configured| configured.sink.shutdown())
}

#[cfg(test)]
pub(crate) fn clear_process_event_sink_for_test() {
    if let Ok(mut state) = process_event_sink_state().lock() {
        *state = ProcessEventSinkState::NeverInstalled;
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

pub(crate) fn submit_metric_sample(
    sample: &crate::longitudinal_metrics::MetricSample,
    process: &RecorderProcessIdentity,
    recorded_at_unix_micros: i64,
    correlations: EventCorrelations,
) -> Result<PreparedEventSubmission, String> {
    let configured = configured_sink();
    let producer = configured
        .as_ref()
        .map(|configured| configured.sink.producer_identity())
        .unwrap_or_else(|| producer_identity_from_recorder(process));
    let policy = PayloadNormalizationPolicy::registered(&[
        "schema_version",
        "name",
        "unit",
        "outcome",
        "value",
        "labels",
    ])
    .map_err(|error| error.to_string())?;
    let envelope = EventEnvelopeV1::normalize(
        NewEventV1 {
            event_id: EventId::random(),
            family: EventFamily::Metric,
            kind: EventKind::registered("metric.sample").map_err(|error| error.to_string())?,
            recorded_at_unix_micros,
            producer_sequence: next_producer_sequence()?,
            producer,
            correlations,
            payload: crate::longitudinal_metrics::metric_payload(sample)?,
            legacy_provenance: None,
            retry_of_generation_id: None,
        },
        &policy,
        Utc::now().timestamp_micros().max(0),
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
    process_event_sink_state()
        .try_lock()
        .ok()
        .and_then(|state| match &*state {
            ProcessEventSinkState::Installed(configured) => Some(configured.clone()),
            ProcessEventSinkState::NeverInstalled | ProcessEventSinkState::Shutdown => None,
        })
}

fn process_event_sink_state() -> &'static Mutex<ProcessEventSinkState> {
    EVENT_SINK.get_or_init(|| Mutex::new(ProcessEventSinkState::NeverInstalled))
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
    let mut observation =
        serde_json::to_value(&event.observation).map_err(|error| error.to_string())?;
    if let Some(fields) = observation.as_object_mut() {
        if let Some(value) = fields.remove("sqlite_failure") {
            fields.insert("database_failure".to_string(), value);
        }
        if let Some(value) = fields.remove("sqlite") {
            fields.insert("database_evidence".to_string(), value);
        }
    }
    let payload = json!({
        "operation": event.operation,
        "resource": event.resource,
        "lifecycle_phase": event.lifecycle_phase,
        "phase": event.phase,
        "elapsed_micros": event.elapsed_micros,
        "observation": observation,
        "diagnostic_correlations": event.correlations,
        "database_identity": event.sqlite,
    });
    let policy = PayloadNormalizationPolicy::registered(&[
        "operation",
        "resource",
        "lifecycle_phase",
        "phase",
        "elapsed_micros",
        "observation",
        "diagnostic_correlations",
        "database_identity",
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_store::{
        EventFamily, EventKind, GenerationState, NewEventV1, PayloadNormalizationPolicy,
        ProcessInstanceId, WriterLayout, read_generation_metadata,
    };
    use rusqlite::{Connection, OpenFlags};
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::sync::mpsc;
    use std::thread;

    struct CountingSink {
        producer: ProducerIdentity,
        shutdowns: Arc<AtomicUsize>,
    }

    impl EventEnvelopeSink for CountingSink {
        fn producer_identity(&self) -> ProducerIdentity {
            self.producer.clone()
        }

        fn submit(
            &self,
            _envelope: &EventEnvelopeV1,
            _durability: EventSubmitDurability,
        ) -> EventSinkSubmission {
            EventSinkSubmission::finished(EventSubmitStatus::Unavailable)
        }

        fn shutdown(&self) -> Result<(), String> {
            self.shutdowns.fetch_add(1, AtomicOrdering::SeqCst);
            Ok(())
        }
    }

    fn counting_sink(shutdowns: Arc<AtomicUsize>) -> ConfiguredEventSink {
        ConfiguredEventSink {
            sink: Arc::new(CountingSink {
                producer: producer(),
                shutdowns,
            }),
            mode: EventSinkMode::NormalWithJsonlFallback,
        }
    }

    fn run_in_isolated_process(environment: &str, test_name: &str) -> bool {
        if std::env::var_os(environment).is_some() {
            clear_process_event_sink_for_test();
            return true;
        }
        let root = tempfile::tempdir().unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([test_name, "--exact", "--nocapture"])
            .env(environment, "1")
            .env("OULIPOLY_DATA_DIR", root.path())
            .env("OULIPOLY_CONFIG_HOME", root.path().join("config"))
            .status()
            .unwrap();
        assert!(
            status.success(),
            "isolated lifecycle fixture failed: {status}"
        );
        false
    }

    fn producer() -> ProducerIdentity {
        ProducerIdentity {
            writer_instance_id: WriterInstanceId::from_bytes([81; 16]),
            process_instance_id: ProcessInstanceId::from_bytes([82; 16]),
            process_root_id: ProcessInstanceId::from_bytes([82; 16]),
            parent_process_instance_id: None,
            supervisor_authority_id: None,
            native_process: Some(NativeProcessIdentity {
                os_pid: i64::from(std::process::id()),
                os_boot_id_sha256: Digest32::sha256(b"shutdown-fixture-boot"),
                os_pid_starttime_ticks: 1,
            }),
        }
    }

    fn event(producer: &ProducerIdentity, id: u8, sequence: i64) -> EventEnvelopeV1 {
        EventEnvelopeV1::normalize(
            NewEventV1 {
                event_id: EventId::from_bytes([id; 16]),
                family: EventFamily::Diagnostic,
                kind: EventKind::registered("producer.shutdown_fixture").unwrap(),
                recorded_at_unix_micros: 1,
                producer_sequence: sequence,
                producer: producer.clone(),
                correlations: EventCorrelations::default(),
                payload: json!({"status": "accepted"}),
                legacy_provenance: None,
                retry_of_generation_id: None,
            },
            &PayloadNormalizationPolicy::registered(&["status"]).unwrap(),
            2,
        )
        .unwrap()
    }

    #[test]
    fn shutdown_before_first_initialization_is_terminal_and_idempotent() {
        const ENVIRONMENT: &str = "OULIPOLY_AGE374_SHUTDOWN_BEFORE_INIT";
        const TEST_NAME: &str = "diagnostic_producer::tests::shutdown_before_first_initialization_is_terminal_and_idempotent";
        if !run_in_isolated_process(ENVIRONMENT, TEST_NAME) {
            return;
        }

        shutdown_process_event_sink().unwrap();
        shutdown_process_event_sink().unwrap();
        assert!(matches!(
            &*process_event_sink_state().lock().unwrap(),
            ProcessEventSinkState::Shutdown
        ));
    }

    #[test]
    fn installation_racing_shutdown_is_published_then_closed_once() {
        const ENVIRONMENT: &str = "OULIPOLY_AGE374_INSTALL_RACING_SHUTDOWN";
        const TEST_NAME: &str = "diagnostic_producer::tests::installation_racing_shutdown_is_published_then_closed_once";
        if !run_in_isolated_process(ENVIRONMENT, TEST_NAME) {
            return;
        }

        let shutdowns = Arc::new(AtomicUsize::new(0));
        let (initializer_entered, entered) = mpsc::channel();
        let (release_initializer, release) = mpsc::channel();
        let installed_shutdowns = Arc::clone(&shutdowns);
        let installation = thread::spawn(move || {
            ensure_process_event_sink_with(|| {
                initializer_entered.send(()).unwrap();
                release.recv().unwrap();
                Ok(counting_sink(installed_shutdowns))
            })
        });
        entered.recv().unwrap();

        let (shutdown_started, started) = mpsc::channel();
        let shutdown = thread::spawn(move || {
            shutdown_started.send(()).unwrap();
            shutdown_process_event_sink()
        });
        started.recv().unwrap();
        release_initializer.send(()).unwrap();

        installation.join().unwrap().unwrap();
        shutdown.join().unwrap().unwrap();
        assert_eq!(shutdowns.load(AtomicOrdering::SeqCst), 1);
        assert!(matches!(
            &*process_event_sink_state().lock().unwrap(),
            ProcessEventSinkState::Shutdown
        ));
    }

    #[test]
    fn post_shutdown_install_is_rejected_without_running_default_initializer() {
        const ENVIRONMENT: &str = "OULIPOLY_AGE374_POST_SHUTDOWN_INSTALL";
        const TEST_NAME: &str = "diagnostic_producer::tests::post_shutdown_install_is_rejected_without_running_default_initializer";
        if !run_in_isolated_process(ENVIRONMENT, TEST_NAME) {
            return;
        }

        shutdown_process_event_sink().unwrap();
        let shutdowns = Arc::new(AtomicUsize::new(0));
        let process = RecorderProcessIdentity {
            os_pid: i64::from(std::process::id()),
            parent_pid: None,
            os_boot_id: Some("post-shutdown-fixture-boot".to_string()),
            os_pid_starttime_ticks: Some(1),
            producer_instance: crate::diagnostic_recorder::ProducerInstanceId::new(),
        };
        let result = ensure_default_process_event_sink(&process);
        assert_eq!(result, Err(EventSinkInstallError::ProcessShutDown));
        let event_root = crate::paths::data_dir()
            .unwrap()
            .join("diagnostics/event-store-v1");
        assert!(
            !event_root.exists(),
            "terminal shutdown must reject default installation before writer/head creation"
        );

        let direct = counting_sink(Arc::clone(&shutdowns));
        assert_eq!(
            install_process_event_sink(direct.sink, direct.mode),
            Err(EventSinkInstallError::ProcessShutDown)
        );
        assert_eq!(shutdowns.load(AtomicOrdering::SeqCst), 0);
    }

    #[test]
    fn installed_global_sink_closes_exactly_once_across_repeated_shutdown() {
        const ENVIRONMENT: &str = "OULIPOLY_AGE374_INSTALLED_EXACT_CLOSURE";
        const TEST_NAME: &str = "diagnostic_producer::tests::installed_global_sink_closes_exactly_once_across_repeated_shutdown";
        if !run_in_isolated_process(ENVIRONMENT, TEST_NAME) {
            return;
        }

        let shutdowns = Arc::new(AtomicUsize::new(0));
        let configured = counting_sink(Arc::clone(&shutdowns));
        install_process_event_sink(configured.sink, configured.mode).unwrap();
        shutdown_process_event_sink().unwrap();
        shutdown_process_event_sink().unwrap();
        assert_eq!(shutdowns.load(AtomicOrdering::SeqCst), 1);
    }

    #[test]
    fn process_sink_shutdown_drains_accepted_event_and_rotates_exact_head() {
        let root = tempfile::tempdir().unwrap();
        let producer = producer();
        let writer =
            ProcessEventWriter::start(EventWriterConfig::native(root.path(), producer.clone()))
                .unwrap();
        let initial = writer.current_generation_id().unwrap();
        let sink = Arc::new(ProcessWriterEventSink::new(producer.clone(), writer));

        let accepted = sink.submit(&event(&producer, 83, 1), EventSubmitDurability::BestEffort);
        assert_eq!(accepted.status, EventSubmitStatus::Queued);
        sink.shutdown().unwrap();
        assert!(accepted.pending.unwrap().wait().is_ok());
        assert!(sink.writer.lock().unwrap().is_none());

        let layout =
            WriterLayout::open_existing(root.path(), *producer.writer_instance_id.as_bytes())
                .unwrap();
        let selected = layout.read_head(|_, _| Ok(())).unwrap().unwrap();
        assert_ne!(selected.record.generation_id, initial);
        let initial_db = layout.generation_db(initial);
        let connection = Connection::open_with_flags(
            initial_db,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .unwrap();
        assert_eq!(
            read_generation_metadata(&connection).unwrap().state,
            GenerationState::Closed
        );
    }

    #[test]
    fn concurrent_durable_waiters_and_best_effort_reach_real_writer_admission() {
        let root = tempfile::tempdir().unwrap();
        let producer = producer();
        let writer =
            ProcessEventWriter::start(EventWriterConfig::native(root.path(), producer.clone()))
                .unwrap();
        let release_worker = writer.pause_for_test().unwrap();
        let (admitted, observed_admission) = mpsc::channel();
        let sink = Arc::new(ProcessWriterEventSink::with_required_admission_observer(
            producer.clone(),
            writer,
            admitted,
        ));

        thread::scope(|scope| {
            let first_sink = Arc::clone(&sink);
            let first_event = event(&producer, 84, 2);
            let first = scope.spawn(move || {
                first_sink.submit(&first_event, EventSubmitDurability::RequiredDurable)
            });
            observed_admission
                .recv_timeout(Duration::from_secs(2))
                .expect("first durable call did not reach writer admission");

            let second_sink = Arc::clone(&sink);
            let second_event = event(&producer, 85, 3);
            let second = scope.spawn(move || {
                second_sink.submit(&second_event, EventSubmitDurability::RequiredDurable)
            });
            observed_admission
                .recv_timeout(Duration::from_secs(2))
                .expect("second durable call was serialized behind the first durable reply");

            let best_effort =
                sink.submit(&event(&producer, 86, 4), EventSubmitDurability::BestEffort);
            assert_eq!(best_effort.status, EventSubmitStatus::Queued);

            release_worker.send(()).unwrap();
            assert_eq!(first.join().unwrap().status, EventSubmitStatus::Appended);
            assert_eq!(second.join().unwrap().status, EventSubmitStatus::Appended);
            assert!(best_effort.pending.unwrap().wait().is_ok());
        });

        sink.shutdown().unwrap();
    }

    #[test]
    fn concurrent_shutdown_fences_admission_drains_pre_fence_work_and_closes_once() {
        let root = tempfile::tempdir().unwrap();
        let producer = producer();
        let mut config = EventWriterConfig::native(root.path(), producer.clone());
        config.queue_capacity = 2;
        let writer = ProcessEventWriter::start(config).unwrap();
        let initial = writer.current_generation_id().unwrap();
        let release_worker = writer.pause_for_test().unwrap();
        let sink = Arc::new(ProcessWriterEventSink::new(producer.clone(), writer));
        let first_accepted =
            sink.submit(&event(&producer, 87, 5), EventSubmitDurability::BestEffort);
        let second_accepted =
            sink.submit(&event(&producer, 89, 7), EventSubmitDurability::BestEffort);
        assert_eq!(first_accepted.status, EventSubmitStatus::Queued);
        assert_eq!(second_accepted.status, EventSubmitStatus::Queued);

        let closing_sink = Arc::clone(&sink);
        let closing = thread::spawn(move || closing_sink.shutdown());
        let deadline = Instant::now() + Duration::from_secs(2);
        while sink.submissions.accepting_for_test() {
            assert!(
                Instant::now() < deadline,
                "shutdown did not publish its admission fence"
            );
            thread::yield_now();
        }

        let post_fence = sink.submit(&event(&producer, 88, 6), EventSubmitDurability::BestEffort);
        assert_eq!(post_fence.status, EventSubmitStatus::Disconnected);
        assert!(sink.shutdown().is_ok(), "second shutdown must be a no-op");

        release_worker.send(()).unwrap();
        assert!(first_accepted.pending.unwrap().wait().is_ok());
        assert!(second_accepted.pending.unwrap().wait().is_ok());
        closing.join().unwrap().unwrap();

        let layout =
            WriterLayout::open_existing(root.path(), *producer.writer_instance_id.as_bytes())
                .unwrap();
        let selected = layout.read_head(|_, _| Ok(())).unwrap().unwrap();
        assert_ne!(selected.record.generation_id, initial);
        let selected_after_close = selected.record.clone();
        assert!(sink.shutdown().is_ok());
        assert_eq!(
            layout.read_head(|_, _| Ok(())).unwrap().unwrap().record,
            selected_after_close
        );
        assert_eq!(
            std::fs::read_dir(layout.generations_dir()).unwrap().count(),
            2
        );
    }
}
