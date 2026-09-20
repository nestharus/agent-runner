//! Small fail-open helpers for instrumented SQLite transaction boundaries.

use crate::diagnostic_recorder::{
    DiagnosticPhase, DiagnosticSpan, PhaseObservation, SqliteFailure, SqliteMeasurementGap,
    SqlitePhaseEvidence, SqliteTransactionPhase,
};
use std::time::{Duration, Instant};

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
