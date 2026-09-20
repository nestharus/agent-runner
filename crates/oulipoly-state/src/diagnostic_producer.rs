//! Small fail-open helpers for instrumented SQLite transaction boundaries.

use crate::diagnostic_recorder::{
    DiagnosticPhase, DiagnosticSpan, PhaseObservation, SqliteFailure,
};
use std::time::Instant;

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
    committed: bool,
    failure_recorded: bool,
    released: bool,
}

impl<'a> TransactionPhaseGuard<'a> {
    pub(crate) fn acquired(span: &'a DiagnosticSpan, attempt: TransactionAttempt) -> Self {
        let _ = span.record(
            DiagnosticPhase::Acquired,
            PhaseObservation::started_unknown().with_wait(attempt.elapsed()),
        );
        Self {
            span,
            committed: false,
            failure_recorded: false,
            released: false,
        }
    }

    pub(crate) fn commit_started(&self) {
        let _ = self.span.record(
            DiagnosticPhase::CommitStarted,
            PhaseObservation::effects_possible(),
        );
    }

    pub(crate) fn committed(&mut self) {
        let _ = self
            .span
            .record(DiagnosticPhase::Committed, PhaseObservation::committed());
        self.committed = true;
    }

    pub(crate) fn sqlite_failure(&mut self, error: &rusqlite::Error) {
        let failure = SqliteFailure::from_error(error);
        let phase = if failure.contention {
            DiagnosticPhase::Contention
        } else {
            DiagnosticPhase::Failed
        };
        let _ = self.span.record(
            phase,
            PhaseObservation::effects_possible().with_sqlite_failure(error),
        );
        self.failure_recorded = true;
    }

    pub(crate) fn failed(&mut self, cause: &'static str) {
        if self.failure_recorded {
            return;
        }
        let _ = self.span.record(
            DiagnosticPhase::Failed,
            PhaseObservation::effects_possible().with_cause(cause),
        );
        self.failure_recorded = true;
    }

    pub(crate) fn release(&mut self) {
        if self.released {
            return;
        }
        let observation = if self.committed {
            PhaseObservation::committed()
        } else {
            PhaseObservation::effects_possible()
        };
        let _ = self.span.record(DiagnosticPhase::Released, observation);
        self.released = true;
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
    let failure = SqliteFailure::from_error(error);
    let phase = if failure.contention {
        DiagnosticPhase::Contention
    } else {
        DiagnosticPhase::Failed
    };
    let _ = span.record(
        phase,
        PhaseObservation::not_started()
            .with_wait(attempt.elapsed())
            .with_sqlite_failure(error),
    );
}

pub(crate) fn record_unacquired_release(span: &DiagnosticSpan) {
    let _ = span.record(DiagnosticPhase::Released, PhaseObservation::not_started());
}

pub(crate) fn record_effects_possible_failure(span: &DiagnosticSpan, cause: &'static str) {
    let _ = span.record(
        DiagnosticPhase::Failed,
        PhaseObservation::effects_possible().with_cause(cause),
    );
}
