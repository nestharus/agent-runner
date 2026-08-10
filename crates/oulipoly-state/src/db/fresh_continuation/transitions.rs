//! ## Declared roles
//!
//! `orchestration`, `validator`, `formatter`

use super::*;
use crate::continuation::{ContinuationInvocationDisposition, ContinuationResumeAcceptance};
use row::{
    ContinuationRow, OutcomeKind, Stage, deserialize_outcome, require_recorded_outcome,
    validate_outcome_identity, validated_row,
};
use terminal::serialize;

pub(super) fn begin_continuation_resume_transaction(
    tx: &Transaction<'_>,
    continuation: &ContinuationRecord,
) -> Result<ContinuationRunDecision, ContinuationRepositoryError> {
    let row = validated_row(tx, continuation)?;
    if let Some(terminal) = terminal_from_row(&row)? {
        return Ok(terminal_run_decision(terminal));
    }

    let stage = Stage::parse(&row.resume_stage, "resume")?;
    transition_resume_if_reserved(tx, continuation, stage)?;
    Ok(stage_run_decision(stage, &continuation.resume))
}

fn transition_resume_if_reserved(
    tx: &Transaction<'_>,
    continuation: &ContinuationRecord,
    stage: Stage,
) -> Result<(), ContinuationRepositoryError> {
    if stage == Stage::Reserved {
        return transition_stage(tx, continuation, "resume_stage", "reserved", "running");
    }
    Ok(())
}

pub(super) fn begin_continuation_fresh_transaction(
    tx: &Transaction<'_>,
    continuation: &ContinuationRecord,
) -> Result<ContinuationRunDecision, ContinuationRepositoryError> {
    let row = validated_row(tx, continuation)?;
    if let Some(terminal) = terminal_from_row(&row)? {
        return Ok(terminal_run_decision(terminal));
    }

    let resume = require_recorded_outcome(&row, OutcomeKind::Resume)?;
    require_fresh_trigger(&resume)?;
    let stage = Stage::parse(&row.fresh_stage, "fresh")?;
    transition_fresh_if_reserved(tx, continuation, stage)?;
    Ok(stage_run_decision(stage, &continuation.fresh))
}

fn require_fresh_trigger(
    resume: &ContinuationInvocationOutcome,
) -> Result<(), ContinuationRepositoryError> {
    let unconfirmed_completion = matches!(
        &resume.disposition,
        ContinuationInvocationDisposition::Failed {
            error_category,
            terminal_reason,
        } if error_category == "resume_completion_unconfirmed"
            && terminal_reason == "resume_completion_unconfirmed"
    );
    if resume.physical_exit_code == 0
        && resume.acceptance == ContinuationResumeAcceptance::Accepted
        && unconfirmed_completion
    {
        return Ok(());
    }
    Err(conflict(
        "fresh continuation requires a zero-exit accepted resume with unconfirmed completion",
    ))
}

fn transition_fresh_if_reserved(
    tx: &Transaction<'_>,
    continuation: &ContinuationRecord,
    stage: Stage,
) -> Result<(), ContinuationRepositoryError> {
    if stage == Stage::Reserved {
        return transition_stage(tx, continuation, "fresh_stage", "reserved", "running");
    }
    Ok(())
}

fn terminal_run_decision(terminal: ContinuationTerminalOutcome) -> ContinuationRunDecision {
    ContinuationRunDecision::Terminal(Box::new(terminal))
}

fn stage_run_decision(
    stage: Stage,
    reservation: &ContinuationReservation,
) -> ContinuationRunDecision {
    match stage {
        Stage::Reserved => ContinuationRunDecision::Run(reservation.clone()),
        Stage::Running | Stage::Recorded => ContinuationRunDecision::Observe(reservation.clone()),
    }
}

fn transition_stage(
    tx: &Transaction<'_>,
    continuation: &ContinuationRecord,
    column: &str,
    expected: &str,
    next: &str,
) -> Result<(), ContinuationRepositoryError> {
    let sql = format_transition_sql(column);
    execute_stage_transition(tx, continuation, expected, next, &sql)
}

fn execute_stage_transition(
    tx: &Transaction<'_>,
    continuation: &ContinuationRecord,
    expected: &str,
    next: &str,
    sql: &str,
) -> Result<(), ContinuationRepositoryError> {
    let updated = tx
        .execute(sql, params![next, continuation.continuation_id, expected])
        .map_err(|error| persistence("transition continuation stage", error))?;
    require_one_updated(updated, "transition continuation stage")
}

fn format_transition_sql(column: &str) -> String {
    format!(
        "UPDATE fresh_continuations SET {column} = ?1
          WHERE continuation_id = ?2 AND {column} = ?3"
    )
}

pub(super) fn record_outcome_transaction(
    tx: &Transaction<'_>,
    continuation: &ContinuationRecord,
    outcome: &ContinuationInvocationOutcome,
    kind: OutcomeKind,
) -> Result<(), ContinuationRepositoryError> {
    let row = validated_row(tx, continuation)?;
    validate_outcome_prerequisites(&row, continuation, outcome, kind)?;
    if let Some(existing_json) = kind.outcome_json(&row) {
        return validate_existing_outcome(&row, existing_json, outcome, kind);
    }

    validate_outcome_stage(&row, kind)?;
    let outcome_json = serialize_outcome(outcome, kind)?;
    let columns = outcome_columns(kind);
    let sql = format_record_outcome_sql(columns);
    persist_recorded_outcome(tx, &continuation.continuation_id, &outcome_json, &sql)
}

fn validate_outcome_prerequisites(
    row: &ContinuationRow,
    continuation: &ContinuationRecord,
    outcome: &ContinuationInvocationOutcome,
    kind: OutcomeKind,
) -> Result<(), ContinuationRepositoryError> {
    let reservation = kind.reservation(continuation);
    if outcome.invocation_id != reservation.invocation_id {
        return Err(conflict(format!(
            "{} outcome invocation does not match its exact reservation",
            kind.label()
        )));
    }
    if matches!(kind, OutcomeKind::Fresh) {
        require_recorded_outcome(row, OutcomeKind::Resume)?;
    }
    Ok(())
}

fn validate_existing_outcome(
    row: &ContinuationRow,
    existing_json: &str,
    outcome: &ContinuationInvocationOutcome,
    kind: OutcomeKind,
) -> Result<(), ContinuationRepositoryError> {
    let existing = deserialize_outcome(existing_json, kind.label())?;
    validate_outcome_identity(row, kind, &existing)?;
    if existing != *outcome {
        return Err(conflict(format!(
            "{} outcome was already recorded with a different value",
            kind.label()
        )));
    }
    Ok(())
}

fn validate_outcome_stage(
    row: &ContinuationRow,
    kind: OutcomeKind,
) -> Result<(), ContinuationRepositoryError> {
    if Stage::parse(kind.stage(row), kind.label())? != Stage::Running {
        return Err(ambiguous(format!(
            "{} outcome cannot be recorded before its reservation begins",
            kind.label()
        )));
    }
    Ok(())
}

fn serialize_outcome(
    outcome: &ContinuationInvocationOutcome,
    kind: OutcomeKind,
) -> Result<String, ContinuationRepositoryError> {
    serialize(outcome, &format!("{} outcome", kind.label()))
}

#[derive(Debug, Clone, Copy)]
struct OutcomeColumns {
    stage: &'static str,
    outcome: &'static str,
}

fn outcome_columns(kind: OutcomeKind) -> OutcomeColumns {
    match kind {
        OutcomeKind::Resume => OutcomeColumns {
            stage: "resume_stage",
            outcome: "resume_outcome_json",
        },
        OutcomeKind::Fresh => OutcomeColumns {
            stage: "fresh_stage",
            outcome: "fresh_outcome_json",
        },
    }
}

fn format_record_outcome_sql(columns: OutcomeColumns) -> String {
    let stage_column = columns.stage;
    let outcome_column = columns.outcome;
    format!(
        "UPDATE fresh_continuations
            SET {stage_column} = 'recorded', {outcome_column} = ?1
          WHERE continuation_id = ?2
            AND {stage_column} = 'running'
            AND {outcome_column} IS NULL"
    )
}

fn persist_recorded_outcome(
    tx: &Transaction<'_>,
    continuation_id: &str,
    outcome_json: &str,
    sql: &str,
) -> Result<(), ContinuationRepositoryError> {
    let updated = tx
        .execute(sql, params![outcome_json, continuation_id])
        .map_err(|error| persistence("record continuation outcome", error))?;
    require_one_updated(updated, "record continuation outcome")
}
