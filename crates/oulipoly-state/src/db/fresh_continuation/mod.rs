//! ## Declared roles
//!
//! `orchestration`, `mapper`, `formatter`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-state/src/db/fresh_continuation/mod.rs
//!     role: adapter
//!     Translates:
//!       - typed-continuation-repository-contract
//!       - fresh-continuations-SQLite-schema-contract
//! ```

mod row;
mod terminal;
mod transitions;

use super::*;
use crate::continuation::{
    ContinuationAcceptInput, ContinuationAcceptResult, ContinuationInvocationOutcome,
    ContinuationPublishedHandoff, ContinuationRecord, ContinuationRepositoryError,
    ContinuationReservation, ContinuationRunDecision, ContinuationTerminalOutcome,
};
use crate::repositories::ContinuationRepository;
use row::{OutcomeKind, record_from_row, row_by_logical_request};
use terminal::{finish_continuation_transaction, terminal_from_row};
use transitions::{
    begin_continuation_fresh_transaction, begin_continuation_resume_transaction,
    record_outcome_transaction,
};

impl ContinuationRepository for StateDb {
    fn accept_continuation(
        &mut self,
        input: &ContinuationAcceptInput,
    ) -> Result<ContinuationAcceptResult, ContinuationRepositoryError> {
        with_transaction(self, |tx| accept_continuation_transaction(tx, input))
    }

    fn begin_continuation_resume(
        &mut self,
        continuation: &ContinuationRecord,
    ) -> Result<ContinuationRunDecision, ContinuationRepositoryError> {
        with_transaction(self, |tx| {
            begin_continuation_resume_transaction(tx, continuation)
        })
    }

    fn record_continuation_resume(
        &mut self,
        continuation: &ContinuationRecord,
        outcome: &ContinuationInvocationOutcome,
    ) -> Result<(), ContinuationRepositoryError> {
        with_transaction(self, |tx| {
            record_outcome_transaction(tx, continuation, outcome, OutcomeKind::Resume)
        })
    }

    fn begin_continuation_fresh(
        &mut self,
        continuation: &ContinuationRecord,
    ) -> Result<ContinuationRunDecision, ContinuationRepositoryError> {
        with_transaction(self, |tx| {
            begin_continuation_fresh_transaction(tx, continuation)
        })
    }

    fn record_continuation_fresh(
        &mut self,
        continuation: &ContinuationRecord,
        outcome: &ContinuationInvocationOutcome,
    ) -> Result<(), ContinuationRepositoryError> {
        with_transaction(self, |tx| {
            record_outcome_transaction(tx, continuation, outcome, OutcomeKind::Fresh)
        })
    }

    fn finish_continuation(
        &mut self,
        continuation: &ContinuationRecord,
        handoff: &ContinuationPublishedHandoff,
    ) -> Result<ContinuationTerminalOutcome, ContinuationRepositoryError> {
        with_transaction(self, |tx| {
            finish_continuation_transaction(tx, continuation, handoff)
        })
    }
}

fn with_transaction<R>(
    db: &mut StateDb,
    operation: impl FnOnce(&Transaction<'_>) -> Result<R, ContinuationRepositoryError>,
) -> Result<R, ContinuationRepositoryError> {
    let tx = db
        .conn
        .transaction_with_behavior(sqlite::TransactionBehavior::Immediate)
        .map_err(|error| persistence("begin continuation transaction", error))?;
    let value = operation(&tx)?;
    tx.commit()
        .map_err(|error| persistence("commit continuation transaction", error))?;
    Ok(value)
}

fn accept_continuation_transaction(
    tx: &Transaction<'_>,
    input: &ContinuationAcceptInput,
) -> Result<ContinuationAcceptResult, ContinuationRepositoryError> {
    if let Some(row) = row_by_logical_request(tx, &input.logical_request_key)? {
        return replay_or_accept_existing(row, input);
    }

    let continuation = new_continuation_record(input);
    insert_continuation(tx, &continuation)?;
    Ok(accepted_continuation(continuation))
}

fn new_continuation_record(input: &ContinuationAcceptInput) -> ContinuationRecord {
    let continuation_id = format!("continuation-{}", Uuid::new_v4());
    let resume_invocation_id = Uuid::new_v4().to_string();
    let fresh_invocation_id = Uuid::new_v4().to_string();
    ContinuationRecord {
        logical_request_key: input.logical_request_key.clone(),
        continuation_id,
        fingerprint: input.fingerprint.clone(),
        resume: ContinuationReservation {
            invocation_id: resume_invocation_id.clone(),
            parent_invocation_id: input.origin_invocation_id.clone(),
        },
        fresh: ContinuationReservation {
            invocation_id: fresh_invocation_id,
            parent_invocation_id: resume_invocation_id,
        },
    }
}

fn insert_continuation(
    tx: &Transaction<'_>,
    continuation: &ContinuationRecord,
) -> Result<(), ContinuationRepositoryError> {
    tx.execute(
        "INSERT INTO fresh_continuations (
            logical_request_key,
            continuation_id,
            validated_fingerprint,
            resume_invocation_id,
            resume_parent_invocation_id,
            fresh_invocation_id,
            fresh_parent_invocation_id
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            continuation.logical_request_key,
            continuation.continuation_id,
            continuation.fingerprint,
            continuation.resume.invocation_id,
            continuation.resume.parent_invocation_id,
            continuation.fresh.invocation_id,
            continuation.fresh.parent_invocation_id,
        ],
    )
    .map_err(|error| persistence("insert fresh continuation", error))?;
    Ok(())
}

fn accepted_continuation(continuation: ContinuationRecord) -> ContinuationAcceptResult {
    ContinuationAcceptResult::Accepted(continuation)
}

fn replay_or_accept_existing(
    row: row::ContinuationRow,
    input: &ContinuationAcceptInput,
) -> Result<ContinuationAcceptResult, ContinuationRepositoryError> {
    validate_logical_request_identity(&row, input)?;
    let continuation = record_from_row(&row)?;
    let terminal = terminal_from_row(&row)?;
    Ok(existing_acceptance(continuation, terminal))
}

fn validate_logical_request_identity(
    row: &row::ContinuationRow,
    input: &ContinuationAcceptInput,
) -> Result<(), ContinuationRepositoryError> {
    if row.fingerprint != input.fingerprint {
        return Err(conflict(
            "logical continuation request already exists with a different validated fingerprint",
        ));
    }
    if row.resume_parent_invocation_id != input.origin_invocation_id {
        return Err(conflict(
            "logical continuation request has a different origin invocation",
        ));
    }
    Ok(())
}

fn existing_acceptance(
    continuation: ContinuationRecord,
    terminal: Option<ContinuationTerminalOutcome>,
) -> ContinuationAcceptResult {
    match terminal {
        Some(terminal) => ContinuationAcceptResult::Replay(terminal),
        None => ContinuationAcceptResult::Accepted(continuation),
    }
}

fn require_one_updated(updated: usize, operation: &str) -> Result<(), ContinuationRepositoryError> {
    if updated == 1 {
        Ok(())
    } else {
        Err(ambiguous(format!(
            "{operation} did not update exactly one continuation record"
        )))
    }
}

fn persistence(operation: &str, error: sqlite::Error) -> ContinuationRepositoryError {
    ContinuationRepositoryError::Persistence(format!("{operation}: {error}"))
}

fn conflict(message: impl Into<String>) -> ContinuationRepositoryError {
    ContinuationRepositoryError::Conflict(message.into())
}

fn ambiguous(message: impl Into<String>) -> ContinuationRepositoryError {
    ContinuationRepositoryError::AmbiguousState(message.into())
}
