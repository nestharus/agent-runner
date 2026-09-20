//! Small fail-open helpers for instrumented SQLite transaction boundaries.

use crate::diagnostic_recorder::{
    DiagnosticPhase, DiagnosticSpan, PhaseObservation, SqliteFailure, SqliteMeasurementGap,
    SqlitePhaseEvidence, SqliteTransactionPhase,
};
use std::time::{Duration, Instant};

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
    writer_wait: Duration,
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
        let writer_wait = attempt.elapsed();
        let _ = span.record(
            DiagnosticPhase::Acquired,
            PhaseObservation::started_unknown()
                .with_wait(writer_wait)
                .with_sqlite_evidence(
                    SqlitePhaseEvidence::for_phase(SqliteTransactionPhase::WriterAuthority)
                        .with_writer_wait(writer_wait)
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
            writer_wait,
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

    pub(crate) fn release(&mut self) {
        if self.released {
            return;
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
            evidence = evidence.with_gap(SqliteMeasurementGap::PostCommitNotReached);
        }
        observation = observation.with_sqlite_evidence(evidence);
        let _ = self.span.record(DiagnosticPhase::Released, observation);
        self.released = true;
    }

    fn evidence(&self, phase: SqliteTransactionPhase) -> SqlitePhaseEvidence {
        let mut evidence = SqlitePhaseEvidence::for_phase(phase)
            .with_total_elapsed(self.transaction_started.elapsed())
            .with_writer_wait(self.writer_wait)
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

impl Drop for TransactionPhaseGuard<'_> {
    fn drop(&mut self) {
        self.release();
    }
}

pub(crate) fn record_sqlite_failure(
    span: &DiagnosticSpan,
    error: &rusqlite::Error,
    attempt: TransactionAttempt,
) {
    let writer_wait = attempt.elapsed();
    let failure = SqliteFailure::from_error(error);
    let phase = if failure.contention {
        DiagnosticPhase::Contention
    } else {
        DiagnosticPhase::Failed
    };
    let _ = span.record(
        phase,
        PhaseObservation::not_started()
            .with_wait(writer_wait)
            .with_sqlite_failure(error)
            .with_sqlite_evidence(
                SqlitePhaseEvidence::for_phase(SqliteTransactionPhase::WriterAuthority)
                    .with_total_elapsed(writer_wait)
                    .with_writer_wait(writer_wait)
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
