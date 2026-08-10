//! ## Declared roles
//!
//! `parser`, `validator`, `accessor`, `mapper`, `formatter`

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stage {
    Reserved,
    Running,
    Recorded,
}

impl Stage {
    pub(super) fn parse(value: &str, label: &str) -> Result<Self, ContinuationRepositoryError> {
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

pub(super) struct ContinuationRow {
    pub(super) logical_request_key: String,
    pub(super) continuation_id: String,
    pub(super) fingerprint: String,
    pub(super) resume_invocation_id: String,
    pub(super) resume_parent_invocation_id: String,
    pub(super) resume_stage: String,
    pub(super) resume_outcome_json: Option<String>,
    pub(super) fresh_invocation_id: String,
    pub(super) fresh_parent_invocation_id: String,
    pub(super) fresh_stage: String,
    pub(super) fresh_outcome_json: Option<String>,
    pub(super) handoff_json: Option<String>,
    pub(super) terminal_outcome_json: Option<String>,
}

struct ParsedContinuationRow {
    resume_stage: Stage,
    fresh_stage: Stage,
}

pub(super) fn record_from_row(
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

pub(super) fn validated_row(
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

pub(super) fn row_by_logical_request(
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

#[derive(Debug, Clone, Copy)]
pub(super) enum OutcomeKind {
    Resume,
    Fresh,
}

impl OutcomeKind {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Resume => "resume",
            Self::Fresh => "fresh",
        }
    }

    pub(super) fn reservation(self, continuation: &ContinuationRecord) -> &ContinuationReservation {
        match self {
            Self::Resume => &continuation.resume,
            Self::Fresh => &continuation.fresh,
        }
    }

    pub(super) fn stage(self, row: &ContinuationRow) -> &str {
        match self {
            Self::Resume => &row.resume_stage,
            Self::Fresh => &row.fresh_stage,
        }
    }

    pub(super) fn outcome_json(self, row: &ContinuationRow) -> Option<&String> {
        match self {
            Self::Resume => row.resume_outcome_json.as_ref(),
            Self::Fresh => row.fresh_outcome_json.as_ref(),
        }
    }
}

pub(super) fn require_recorded_outcome(
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

pub(super) fn validate_outcome_identity(
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

pub(super) fn deserialize_outcome(
    json: &str,
    label: &str,
) -> Result<ContinuationInvocationOutcome, ContinuationRepositoryError> {
    serde_json::from_str(json).map_err(|error| {
        ambiguous(format!(
            "durable continuation {label} outcome is invalid: {error}"
        ))
    })
}
