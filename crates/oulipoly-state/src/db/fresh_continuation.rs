//! ## Declared roles
//!
//! `parser`, `orchestration`, `mapper`, `validator`, `accessor`, `formatter`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-state/src/db/fresh_continuation.rs
//!     role: adapter
//!     Translates:
//!       - typed-continuation-repository-contract
//!       - fresh-continuations-SQLite-schema-contract
//! ```

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
        record_outcome(self, continuation, outcome, OutcomeKind::Resume)
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
        record_outcome(self, continuation, outcome, OutcomeKind::Fresh)
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

fn begin_continuation_resume_transaction(
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

fn begin_continuation_fresh_transaction(
    tx: &Transaction<'_>,
    continuation: &ContinuationRecord,
) -> Result<ContinuationRunDecision, ContinuationRepositoryError> {
    let row = validated_row(tx, continuation)?;
    if let Some(terminal) = terminal_from_row(&row)? {
        return Ok(terminal_run_decision(terminal));
    }

    require_recorded_outcome(&row, OutcomeKind::Resume)?;
    let stage = Stage::parse(&row.fresh_stage, "fresh")?;
    transition_fresh_if_reserved(tx, continuation, stage)?;
    Ok(stage_run_decision(stage, &continuation.fresh))
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

fn finish_continuation_transaction(
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

fn replay_or_accept_existing(
    row: ContinuationRow,
    input: &ContinuationAcceptInput,
) -> Result<ContinuationAcceptResult, ContinuationRepositoryError> {
    validate_logical_request_identity(&row, input)?;
    let continuation = record_from_row(&row)?;
    let terminal = terminal_from_row(&row)?;
    Ok(existing_acceptance(continuation, terminal))
}

fn validate_logical_request_identity(
    row: &ContinuationRow,
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

struct ParsedContinuationRow {
    resume_stage: Stage,
    fresh_stage: Stage,
}

fn record_from_row(
    row: &ContinuationRow,
) -> Result<ContinuationRecord, ContinuationRepositoryError> {
    let parsed = parse_continuation_row(row)?;
    validate_continuation_row(row, &parsed)?;
    Ok(map_continuation_record(row))
}

fn parse_continuation_row(
    row: &ContinuationRow,
) -> Result<ParsedContinuationRow, ContinuationRepositoryError> {
    parse_invocation_identity(&row.resume_invocation_id, "resume")?;
    parse_invocation_identity(&row.fresh_invocation_id, "fresh")?;
    Ok(ParsedContinuationRow {
        resume_stage: Stage::parse(&row.resume_stage, "resume")?,
        fresh_stage: Stage::parse(&row.fresh_stage, "fresh")?,
    })
}

fn parse_invocation_identity(
    value: &str,
    label: &str,
) -> Result<Uuid, ContinuationRepositoryError> {
    Uuid::parse_str(value).map_err(|error| {
        ambiguous(format!(
            "reserved {label} invocation identity is invalid: {error}"
        ))
    })
}

fn validate_continuation_row(
    row: &ContinuationRow,
    parsed: &ParsedContinuationRow,
) -> Result<(), ContinuationRepositoryError> {
    if row.logical_request_key.is_empty()
        || row.continuation_id.is_empty()
        || row.fingerprint.is_empty()
        || row.resume_parent_invocation_id.is_empty()
        || row.fresh_parent_invocation_id.is_empty()
    {
        return Err(ambiguous("continuation record contains an empty identity"));
    }
    if row.fresh_parent_invocation_id != row.resume_invocation_id {
        return Err(ambiguous(
            "fresh continuation parent does not match the reserved resume invocation",
        ));
    }
    validate_stage_outcome_pair(
        parsed.resume_stage,
        row.resume_outcome_json.as_ref(),
        "resume",
    )?;
    validate_stage_outcome_pair(parsed.fresh_stage, row.fresh_outcome_json.as_ref(), "fresh")?;
    validate_recorded_outcome(row, OutcomeKind::Resume, parsed.resume_stage)?;
    validate_recorded_outcome(row, OutcomeKind::Fresh, parsed.fresh_stage)?;
    if parsed.fresh_stage != Stage::Reserved && parsed.resume_stage != Stage::Recorded {
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
        && (parsed.resume_stage != Stage::Recorded || parsed.fresh_stage != Stage::Recorded)
    {
        return Err(ambiguous(
            "continuation became terminal before both outcomes were durable",
        ));
    }
    Ok(())
}

fn map_continuation_record(row: &ContinuationRow) -> ContinuationRecord {
    ContinuationRecord {
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
    }
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
    let row = row_by_continuation_id(tx, &expected.continuation_id)?;
    let row = require_continuation_row(row, &expected.continuation_id)?;
    validate_row_identity(row, expected)
}

fn require_continuation_row(
    row: Option<ContinuationRow>,
    continuation_id: &str,
) -> Result<ContinuationRow, ContinuationRepositoryError> {
    row.ok_or_else(|| conflict(format!("unknown continuation identity {continuation_id:?}")))
}

fn validate_row_identity(
    row: ContinuationRow,
    expected: &ContinuationRecord,
) -> Result<ContinuationRow, ContinuationRepositoryError> {
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
    let sql = format_row_query("logical_request_key = ?1");
    query_row(tx, &sql, logical_request_key)
}

fn row_by_continuation_id(
    tx: &Transaction<'_>,
    continuation_id: &str,
) -> Result<Option<ContinuationRow>, ContinuationRepositoryError> {
    let sql = format_row_query("continuation_id = ?1");
    query_row(tx, &sql, continuation_id)
}

fn query_row(
    tx: &Transaction<'_>,
    sql: &str,
    value: &str,
) -> Result<Option<ContinuationRow>, ContinuationRepositoryError> {
    tx.query_row(sql, [value], map_continuation_row_from_sql)
        .optional()
        .map_err(|error| persistence("read fresh continuation", error))
}

fn format_row_query(predicate: &str) -> String {
    format!(
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
    )
}

fn map_continuation_row_from_sql(row: &sqlite::Row<'_>) -> Result<ContinuationRow, sqlite::Error> {
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
        record_outcome_transaction(tx, continuation, outcome, kind)
    })
}

fn record_outcome_transaction(
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
