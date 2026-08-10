//! ## Declared roles
//!
//! `parser`, `orchestration`, `validator`, `mapper`, `formatter`

use super::*;
use crate::continuation::ContinuationInvocationDisposition;
use row::{ContinuationRow, OutcomeKind, record_from_row, require_recorded_outcome, validated_row};

const FRESH_INVOCATION_FAILED_REASON: &str = "fresh continuation invocation failed";

pub(super) fn finish_continuation_transaction(
    tx: &Transaction<'_>,
    continuation: &ContinuationRecord,
    handoff: &ContinuationPublishedHandoff,
) -> Result<ContinuationTerminalOutcome, ContinuationRepositoryError> {
    let row = validated_row(tx, continuation)?;
    if let Some(terminal) = terminal_from_row(&row)? {
        return validate_existing_terminal(terminal, handoff);
    }

    let resume = require_recorded_outcome(&row, OutcomeKind::Resume)?;
    let fresh = require_recorded_outcome(&row, OutcomeKind::Fresh)?;
    let terminal = terminal_outcome(continuation, resume, fresh, handoff.clone());
    let handoff_json = serialize(handoff, "continuation handoff")?;
    let terminal_json = serialize(&terminal, "continuation terminal outcome")?;
    persist_terminal_outcome(
        tx,
        &continuation.continuation_id,
        &handoff_json,
        &terminal_json,
    )?;
    Ok(terminal)
}

fn validate_existing_terminal(
    terminal: ContinuationTerminalOutcome,
    handoff: &ContinuationPublishedHandoff,
) -> Result<ContinuationTerminalOutcome, ContinuationRepositoryError> {
    if terminal.handoff() != handoff {
        return Err(conflict(
            "continuation was already finished with a different handoff",
        ));
    }
    Ok(terminal)
}

fn persist_terminal_outcome(
    tx: &Transaction<'_>,
    continuation_id: &str,
    handoff_json: &str,
    terminal_json: &str,
) -> Result<(), ContinuationRepositoryError> {
    let updated = tx
        .execute(
            "UPDATE fresh_continuations
                SET handoff_json = ?1, terminal_outcome_json = ?2
              WHERE continuation_id = ?3
                AND handoff_json IS NULL
                AND terminal_outcome_json IS NULL",
            params![handoff_json, terminal_json, continuation_id],
        )
        .map_err(|error| persistence("finish fresh continuation", error))?;
    require_one_updated(updated, "finish fresh continuation")
}

pub(super) fn terminal_from_row(
    row: &ContinuationRow,
) -> Result<Option<ContinuationTerminalOutcome>, ContinuationRepositoryError> {
    let Some(terminal_json) = row.terminal_outcome_json.as_deref() else {
        return Ok(None);
    };
    let handoff_json = require_terminal_handoff_json(row.handoff_json.as_deref())?;
    let parsed = parse_terminal_artifacts(handoff_json, terminal_json)?;
    let expected = expected_terminal_from_row(row, parsed.handoff)?;
    let terminal = validate_terminal_artifact(parsed.terminal, &expected)?;
    Ok(Some(terminal))
}

fn require_terminal_handoff_json(
    handoff_json: Option<&str>,
) -> Result<&str, ContinuationRepositoryError> {
    handoff_json.ok_or_else(|| {
        ambiguous("continuation terminal outcome exists without its published handoff")
    })
}

struct ParsedTerminalArtifacts {
    handoff: ContinuationPublishedHandoff,
    terminal: ContinuationTerminalOutcome,
}

fn parse_terminal_artifacts(
    handoff_json: &str,
    terminal_json: &str,
) -> Result<ParsedTerminalArtifacts, ContinuationRepositoryError> {
    let handoff = serde_json::from_str(handoff_json)
        .map_err(|error| ambiguous(format!("durable continuation handoff is invalid: {error}")))?;
    let terminal = serde_json::from_str(terminal_json).map_err(|error| {
        ambiguous(format!(
            "durable continuation terminal outcome is invalid: {error}"
        ))
    })?;
    Ok(ParsedTerminalArtifacts { handoff, terminal })
}

fn expected_terminal_from_row(
    row: &ContinuationRow,
    handoff: ContinuationPublishedHandoff,
) -> Result<ContinuationTerminalOutcome, ContinuationRepositoryError> {
    let resume = require_recorded_outcome(row, OutcomeKind::Resume)?;
    let fresh = require_recorded_outcome(row, OutcomeKind::Fresh)?;
    let continuation = record_from_row(row)?;
    Ok(terminal_outcome(&continuation, resume, fresh, handoff))
}

fn validate_terminal_artifact(
    terminal: ContinuationTerminalOutcome,
    expected: &ContinuationTerminalOutcome,
) -> Result<ContinuationTerminalOutcome, ContinuationRepositoryError> {
    if terminal != *expected {
        return Err(ambiguous(
            "durable continuation terminal output disagrees with its exact outcomes or handoff",
        ));
    }
    Ok(terminal)
}

fn terminal_outcome(
    continuation: &ContinuationRecord,
    resume: ContinuationInvocationOutcome,
    fresh: ContinuationInvocationOutcome,
    handoff: ContinuationPublishedHandoff,
) -> ContinuationTerminalOutcome {
    match &fresh.disposition {
        ContinuationInvocationDisposition::Succeeded => ContinuationTerminalOutcome::Continued {
            continuation_id: continuation.continuation_id.clone(),
            resume,
            fresh,
            handoff,
        },
        ContinuationInvocationDisposition::Failed { .. } => ContinuationTerminalOutcome::Failed {
            continuation_id: continuation.continuation_id.clone(),
            resume,
            fresh,
            handoff,
            reason: FRESH_INVOCATION_FAILED_REASON.to_string(),
        },
    }
}

pub(super) fn serialize(
    value: &impl serde::Serialize,
    label: &str,
) -> Result<String, ContinuationRepositoryError> {
    serde_json::to_string(value).map_err(|error| {
        ContinuationRepositoryError::Persistence(format!("serialize {label}: {error}"))
    })
}
