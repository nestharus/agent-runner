use super::*;
use crate::continuation::{
    ContinuationAcceptInput, ContinuationAcceptResult, ContinuationInvocationDisposition,
    ContinuationInvocationOutcome, ContinuationPublishedHandoff, ContinuationRecord,
    ContinuationRepositoryError, ContinuationReservation, ContinuationRunDecision,
    ContinuationTerminalOutcome,
};
use crate::repositories::ContinuationRepository;

const FRESH_INVOCATION_FAILED_REASON: &str = "fresh continuation invocation failed";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Reserved,
    Running,
    Recorded,
}

impl Stage {
    fn parse(value: &str, label: &str) -> Result<Self, ContinuationRepositoryError> {
        match value {
            "reserved" => Ok(Self::Reserved),
            "running" => Ok(Self::Running),
            "recorded" => Ok(Self::Recorded),
            _ => Err(ambiguous(format!(
                "continuation {label} has unknown stage {value:?}"
            ))),
        }
    }
}

struct ContinuationRow {
    logical_request_key: String,
    continuation_id: String,
    fingerprint: String,
    resume_invocation_id: String,
    resume_parent_invocation_id: String,
    resume_stage: String,
    resume_outcome_json: Option<String>,
    fresh_invocation_id: String,
    fresh_parent_invocation_id: String,
    fresh_stage: String,
    fresh_outcome_json: Option<String>,
    handoff_json: Option<String>,
    terminal_outcome_json: Option<String>,
}

impl ContinuationRepository for StateDb {
    fn accept_continuation(
        &mut self,
        input: &ContinuationAcceptInput,
    ) -> Result<ContinuationAcceptResult, ContinuationRepositoryError> {
        with_transaction(self, |tx| {
            if let Some(row) = row_by_logical_request(tx, &input.logical_request_key)? {
                return replay_or_accept_existing(row, input);
            }

            let continuation = new_continuation_record(input);
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

            Ok(ContinuationAcceptResult::Accepted(continuation))
        })
    }

    fn begin_continuation_resume(
        &mut self,
        continuation: &ContinuationRecord,
    ) -> Result<ContinuationRunDecision, ContinuationRepositoryError> {
        with_transaction(self, |tx| {
            let row = validated_row(tx, continuation)?;
            if let Some(terminal) = terminal_from_row(&row)? {
                return Ok(ContinuationRunDecision::Terminal(Box::new(terminal)));
            }

            match Stage::parse(&row.resume_stage, "resume")? {
                Stage::Reserved => {
                    transition_stage(tx, continuation, "resume_stage", "reserved", "running")?;
                    Ok(ContinuationRunDecision::Run(continuation.resume.clone()))
                }
                Stage::Running | Stage::Recorded => Ok(ContinuationRunDecision::Observe(
                    continuation.resume.clone(),
                )),
            }
        })
    }

    fn record_continuation_resume(
        &mut self,
        continuation: &ContinuationRecord,
        outcome: &ContinuationInvocationOutcome,
    ) -> Result<(), ContinuationRepositoryError> {
        record_outcome(self, continuation, outcome, OutcomeKind::Resume)
    }

    fn begin_continuation_fresh(
        &mut self,
        continuation: &ContinuationRecord,
    ) -> Result<ContinuationRunDecision, ContinuationRepositoryError> {
        with_transaction(self, |tx| {
            let row = validated_row(tx, continuation)?;
            if let Some(terminal) = terminal_from_row(&row)? {
                return Ok(ContinuationRunDecision::Terminal(Box::new(terminal)));
            }
            require_recorded_outcome(&row, OutcomeKind::Resume)?;

            match Stage::parse(&row.fresh_stage, "fresh")? {
                Stage::Reserved => {
                    transition_stage(tx, continuation, "fresh_stage", "reserved", "running")?;
                    Ok(ContinuationRunDecision::Run(continuation.fresh.clone()))
                }
                Stage::Running | Stage::Recorded => {
                    Ok(ContinuationRunDecision::Observe(continuation.fresh.clone()))
                }
            }
        })
    }

    fn record_continuation_fresh(
        &mut self,
        continuation: &ContinuationRecord,
        outcome: &ContinuationInvocationOutcome,
    ) -> Result<(), ContinuationRepositoryError> {
        record_outcome(self, continuation, outcome, OutcomeKind::Fresh)
    }

    fn finish_continuation(
        &mut self,
        continuation: &ContinuationRecord,
        handoff: &ContinuationPublishedHandoff,
    ) -> Result<ContinuationTerminalOutcome, ContinuationRepositoryError> {
        with_transaction(self, |tx| {
            let row = validated_row(tx, continuation)?;
            if let Some(terminal) = terminal_from_row(&row)? {
                if terminal.handoff() != handoff {
                    return Err(conflict(
                        "continuation was already finished with a different handoff",
                    ));
                }
                return Ok(terminal);
            }

            let resume = require_recorded_outcome(&row, OutcomeKind::Resume)?;
            let fresh = require_recorded_outcome(&row, OutcomeKind::Fresh)?;
            let terminal = terminal_outcome(continuation, resume, fresh, handoff.clone());
            let handoff_json = serialize(handoff, "continuation handoff")?;
            let terminal_json = serialize(&terminal, "continuation terminal outcome")?;
            let updated = tx
                .execute(
                    "UPDATE fresh_continuations
                        SET handoff_json = ?1, terminal_outcome_json = ?2
                      WHERE continuation_id = ?3
                        AND handoff_json IS NULL
                        AND terminal_outcome_json IS NULL",
                    params![handoff_json, terminal_json, continuation.continuation_id],
                )
                .map_err(|error| persistence("finish fresh continuation", error))?;
            require_one_updated(updated, "finish fresh continuation")?;
            Ok(terminal)
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

fn replay_or_accept_existing(
    row: ContinuationRow,
    input: &ContinuationAcceptInput,
) -> Result<ContinuationAcceptResult, ContinuationRepositoryError> {
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

    let continuation = record_from_row(&row)?;
    match terminal_from_row(&row)? {
        Some(terminal) => Ok(ContinuationAcceptResult::Replay(terminal)),
        None => Ok(ContinuationAcceptResult::Accepted(continuation)),
    }
}

fn record_from_row(
    row: &ContinuationRow,
) -> Result<ContinuationRecord, ContinuationRepositoryError> {
    if row.logical_request_key.is_empty()
        || row.continuation_id.is_empty()
        || row.fingerprint.is_empty()
        || row.resume_parent_invocation_id.is_empty()
        || row.fresh_parent_invocation_id.is_empty()
    {
        return Err(ambiguous("continuation record contains an empty identity"));
    }
    Uuid::parse_str(&row.resume_invocation_id).map_err(|error| {
        ambiguous(format!(
            "reserved resume invocation identity is invalid: {error}"
        ))
    })?;
    Uuid::parse_str(&row.fresh_invocation_id).map_err(|error| {
        ambiguous(format!(
            "reserved fresh invocation identity is invalid: {error}"
        ))
    })?;
    if row.fresh_parent_invocation_id != row.resume_invocation_id {
        return Err(ambiguous(
            "fresh continuation parent does not match the reserved resume invocation",
        ));
    }
    let resume_stage = Stage::parse(&row.resume_stage, "resume")?;
    let fresh_stage = Stage::parse(&row.fresh_stage, "fresh")?;
    validate_stage_outcome_pair(resume_stage, row.resume_outcome_json.as_ref(), "resume")?;
    validate_stage_outcome_pair(fresh_stage, row.fresh_outcome_json.as_ref(), "fresh")?;
    validate_recorded_outcome(row, OutcomeKind::Resume, resume_stage)?;
    validate_recorded_outcome(row, OutcomeKind::Fresh, fresh_stage)?;
    if fresh_stage != Stage::Reserved && resume_stage != Stage::Recorded {
        return Err(ambiguous(
            "fresh continuation advanced before the resume outcome became durable",
        ));
    }
    if row.handoff_json.is_some() != row.terminal_outcome_json.is_some() {
        return Err(ambiguous(
            "continuation terminal outcome and handoff are not both durable",
        ));
    }
    if row.terminal_outcome_json.is_some()
        && (resume_stage != Stage::Recorded || fresh_stage != Stage::Recorded)
    {
        return Err(ambiguous(
            "continuation became terminal before both outcomes were durable",
        ));
    }

    Ok(ContinuationRecord {
        logical_request_key: row.logical_request_key.clone(),
        continuation_id: row.continuation_id.clone(),
        fingerprint: row.fingerprint.clone(),
        resume: ContinuationReservation {
            invocation_id: row.resume_invocation_id.clone(),
            parent_invocation_id: row.resume_parent_invocation_id.clone(),
        },
        fresh: ContinuationReservation {
            invocation_id: row.fresh_invocation_id.clone(),
            parent_invocation_id: row.fresh_parent_invocation_id.clone(),
        },
    })
}

fn validate_stage_outcome_pair(
    stage: Stage,
    outcome_json: Option<&String>,
    label: &str,
) -> Result<(), ContinuationRepositoryError> {
    if (stage == Stage::Recorded) != outcome_json.is_some() {
        return Err(ambiguous(format!(
            "continuation {label} stage and durable outcome disagree"
        )));
    }
    Ok(())
}

fn validate_recorded_outcome(
    row: &ContinuationRow,
    kind: OutcomeKind,
    stage: Stage,
) -> Result<(), ContinuationRepositoryError> {
    if stage != Stage::Recorded {
        return Ok(());
    }
    let json = kind.outcome_json(row).ok_or_else(|| {
        ambiguous(format!(
            "{} stage is recorded without an outcome",
            kind.label()
        ))
    })?;
    let outcome = deserialize_outcome(json, kind.label())?;
    validate_outcome_identity(row, kind, &outcome)
}

fn validated_row(
    tx: &Transaction<'_>,
    expected: &ContinuationRecord,
) -> Result<ContinuationRow, ContinuationRepositoryError> {
    let row = row_by_continuation_id(tx, &expected.continuation_id)?.ok_or_else(|| {
        conflict(format!(
            "unknown continuation identity {:?}",
            expected.continuation_id
        ))
    })?;
    let actual = record_from_row(&row)?;
    if actual != *expected {
        return Err(conflict(
            "continuation identity or reservation does not match the durable record",
        ));
    }
    Ok(row)
}

fn row_by_logical_request(
    tx: &Transaction<'_>,
    logical_request_key: &str,
) -> Result<Option<ContinuationRow>, ContinuationRepositoryError> {
    query_row(tx, "logical_request_key = ?1", logical_request_key)
}

fn row_by_continuation_id(
    tx: &Transaction<'_>,
    continuation_id: &str,
) -> Result<Option<ContinuationRow>, ContinuationRepositoryError> {
    query_row(tx, "continuation_id = ?1", continuation_id)
}

fn query_row(
    tx: &Transaction<'_>,
    predicate: &str,
    value: &str,
) -> Result<Option<ContinuationRow>, ContinuationRepositoryError> {
    let sql = format!(
        "SELECT
            logical_request_key,
            continuation_id,
            validated_fingerprint,
            resume_invocation_id,
            resume_parent_invocation_id,
            resume_stage,
            resume_outcome_json,
            fresh_invocation_id,
            fresh_parent_invocation_id,
            fresh_stage,
            fresh_outcome_json,
            handoff_json,
            terminal_outcome_json
         FROM fresh_continuations
         WHERE {predicate}"
    );
    tx.query_row(&sql, [value], |row| {
        Ok(ContinuationRow {
            logical_request_key: row.get(0)?,
            continuation_id: row.get(1)?,
            fingerprint: row.get(2)?,
            resume_invocation_id: row.get(3)?,
            resume_parent_invocation_id: row.get(4)?,
            resume_stage: row.get(5)?,
            resume_outcome_json: row.get(6)?,
            fresh_invocation_id: row.get(7)?,
            fresh_parent_invocation_id: row.get(8)?,
            fresh_stage: row.get(9)?,
            fresh_outcome_json: row.get(10)?,
            handoff_json: row.get(11)?,
            terminal_outcome_json: row.get(12)?,
        })
    })
    .optional()
    .map_err(|error| persistence("read fresh continuation", error))
}

fn transition_stage(
    tx: &Transaction<'_>,
    continuation: &ContinuationRecord,
    column: &str,
    expected: &str,
    next: &str,
) -> Result<(), ContinuationRepositoryError> {
    let sql = format!(
        "UPDATE fresh_continuations SET {column} = ?1
          WHERE continuation_id = ?2 AND {column} = ?3"
    );
    let updated = tx
        .execute(&sql, params![next, continuation.continuation_id, expected])
        .map_err(|error| persistence("transition continuation stage", error))?;
    require_one_updated(updated, "transition continuation stage")
}

#[derive(Debug, Clone, Copy)]
enum OutcomeKind {
    Resume,
    Fresh,
}

impl OutcomeKind {
    fn label(self) -> &'static str {
        match self {
            Self::Resume => "resume",
            Self::Fresh => "fresh",
        }
    }

    fn reservation(self, continuation: &ContinuationRecord) -> &ContinuationReservation {
        match self {
            Self::Resume => &continuation.resume,
            Self::Fresh => &continuation.fresh,
        }
    }

    fn stage(self, row: &ContinuationRow) -> &str {
        match self {
            Self::Resume => &row.resume_stage,
            Self::Fresh => &row.fresh_stage,
        }
    }

    fn outcome_json(self, row: &ContinuationRow) -> Option<&String> {
        match self {
            Self::Resume => row.resume_outcome_json.as_ref(),
            Self::Fresh => row.fresh_outcome_json.as_ref(),
        }
    }
}

fn record_outcome(
    db: &mut StateDb,
    continuation: &ContinuationRecord,
    outcome: &ContinuationInvocationOutcome,
    kind: OutcomeKind,
) -> Result<(), ContinuationRepositoryError> {
    with_transaction(db, |tx| {
        let row = validated_row(tx, continuation)?;
        let reservation = kind.reservation(continuation);
        if outcome.invocation_id != reservation.invocation_id {
            return Err(conflict(format!(
                "{} outcome invocation does not match its exact reservation",
                kind.label()
            )));
        }
        if matches!(kind, OutcomeKind::Fresh) {
            require_recorded_outcome(&row, OutcomeKind::Resume)?;
        }

        if let Some(existing_json) = kind.outcome_json(&row) {
            let existing = deserialize_outcome(existing_json, kind.label())?;
            validate_outcome_identity(&row, kind, &existing)?;
            return if existing == *outcome {
                Ok(())
            } else {
                Err(conflict(format!(
                    "{} outcome was already recorded with a different value",
                    kind.label()
                )))
            };
        }

        if Stage::parse(kind.stage(&row), kind.label())? != Stage::Running {
            return Err(ambiguous(format!(
                "{} outcome cannot be recorded before its reservation begins",
                kind.label()
            )));
        }

        let outcome_json = serialize(outcome, &format!("{} outcome", kind.label()))?;
        let (stage_column, outcome_column) = match kind {
            OutcomeKind::Resume => ("resume_stage", "resume_outcome_json"),
            OutcomeKind::Fresh => ("fresh_stage", "fresh_outcome_json"),
        };
        let sql = format!(
            "UPDATE fresh_continuations
                SET {stage_column} = 'recorded', {outcome_column} = ?1
              WHERE continuation_id = ?2
                AND {stage_column} = 'running'
                AND {outcome_column} IS NULL"
        );
        let updated = tx
            .execute(&sql, params![outcome_json, continuation.continuation_id])
            .map_err(|error| persistence("record continuation outcome", error))?;
        require_one_updated(updated, "record continuation outcome")
    })
}

fn require_recorded_outcome(
    row: &ContinuationRow,
    kind: OutcomeKind,
) -> Result<ContinuationInvocationOutcome, ContinuationRepositoryError> {
    if Stage::parse(kind.stage(row), kind.label())? != Stage::Recorded {
        return Err(ambiguous(format!(
            "exact {} outcome is not durable",
            kind.label()
        )));
    }
    let json = kind.outcome_json(row).ok_or_else(|| {
        ambiguous(format!(
            "{} stage is recorded without an outcome",
            kind.label()
        ))
    })?;
    let outcome = deserialize_outcome(json, kind.label())?;
    validate_outcome_identity(row, kind, &outcome)?;
    Ok(outcome)
}

fn validate_outcome_identity(
    row: &ContinuationRow,
    kind: OutcomeKind,
    outcome: &ContinuationInvocationOutcome,
) -> Result<(), ContinuationRepositoryError> {
    let expected_invocation_id = match kind {
        OutcomeKind::Resume => &row.resume_invocation_id,
        OutcomeKind::Fresh => &row.fresh_invocation_id,
    };
    if outcome.invocation_id != *expected_invocation_id {
        return Err(ambiguous(format!(
            "durable {} outcome does not match its exact reservation",
            kind.label()
        )));
    }
    Ok(())
}

fn deserialize_outcome(
    json: &str,
    label: &str,
) -> Result<ContinuationInvocationOutcome, ContinuationRepositoryError> {
    serde_json::from_str(json).map_err(|error| {
        ambiguous(format!(
            "durable continuation {label} outcome is invalid: {error}"
        ))
    })
}

fn terminal_from_row(
    row: &ContinuationRow,
) -> Result<Option<ContinuationTerminalOutcome>, ContinuationRepositoryError> {
    let Some(terminal_json) = row.terminal_outcome_json.as_deref() else {
        return Ok(None);
    };
    let handoff_json = row.handoff_json.as_deref().ok_or_else(|| {
        ambiguous("continuation terminal outcome exists without its published handoff")
    })?;
    let handoff: ContinuationPublishedHandoff = serde_json::from_str(handoff_json)
        .map_err(|error| ambiguous(format!("durable continuation handoff is invalid: {error}")))?;
    let resume = require_recorded_outcome(row, OutcomeKind::Resume)?;
    let fresh = require_recorded_outcome(row, OutcomeKind::Fresh)?;
    let continuation = record_from_row(row)?;
    let expected = terminal_outcome(&continuation, resume, fresh, handoff);
    let actual: ContinuationTerminalOutcome =
        serde_json::from_str(terminal_json).map_err(|error| {
            ambiguous(format!(
                "durable continuation terminal outcome is invalid: {error}"
            ))
        })?;
    if actual != expected {
        return Err(ambiguous(
            "durable continuation terminal output disagrees with its exact outcomes or handoff",
        ));
    }
    Ok(Some(actual))
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

fn serialize(
    value: &impl serde::Serialize,
    label: &str,
) -> Result<String, ContinuationRepositoryError> {
    serde_json::to_string(value).map_err(|error| {
        ContinuationRepositoryError::Persistence(format!("serialize {label}: {error}"))
    })
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
