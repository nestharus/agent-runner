//! Declared roles: accessor, mapper, validator, predicate

use super::DryRunError;
use rusqlite::Connection;
use std::collections::BTreeMap;
use std::path::Path;

const FORWARD_SQL: &str = include_str!("forward.sql");
const ROLLBACK_SQL: &str = include_str!("s11_wu4_restore_session_ownership_preimage.sql");

#[derive(Debug, Clone)]
pub(crate) struct ForwardCounts {
    pub(crate) counts: BTreeMap<String, i64>,
}

#[derive(Debug, Clone)]
pub(crate) struct RollbackCounts {
    pub(crate) counts: BTreeMap<String, i64>,
    pub(crate) mismatches: BTreeMap<String, i64>,
    pub(crate) restored: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct CwdCompleteness {
    pub(crate) missing: i64,
    pub(crate) null: i64,
    pub(crate) non_absolute: i64,
}

#[derive(Debug, Clone)]
enum RuntimeCwd {
    Missing,
    Null,
    Value(String),
}

#[derive(Debug, Clone)]
struct DriftProbeResult {
    kind: &'static str,
    exists: bool,
}

pub(crate) fn apply_forward(conn: &Connection) -> Result<ForwardCounts, DryRunError> {
    conn.execute_batch(FORWARD_SQL)?;
    Ok(ForwardCounts {
        counts: read_key_counts(conn, "s11_wu4_last_run_counts")?,
    })
}

pub(crate) fn apply_rollback(conn: &Connection) -> Result<RollbackCounts, DryRunError> {
    assert_no_rollback_drift(conn)?;
    conn.execute_batch(ROLLBACK_SQL)?;
    let mismatches = preimage_mismatch_counts(conn)?;
    Ok(rollback_counts(
        read_key_counts(conn, "s11_wu4_last_rollback_counts")?,
        mismatches,
    ))
}

pub(crate) fn cwd_completeness(
    state: &Connection,
    mailbox_copy: Option<&Path>,
) -> Result<CwdCompleteness, DryRunError> {
    let active_sessions = migrated_active_sessions(state)?;
    let runtime_cwds = runtime_cwds(mailbox_copy, &active_sessions)?;
    Ok(cwd_completeness_counts(&runtime_cwds))
}

fn assert_no_rollback_drift(conn: &Connection) -> Result<(), DryRunError> {
    validate_no_rollback_drift(read_rollback_drift(conn)?)
}

fn read_rollback_drift(conn: &Connection) -> Result<Vec<DriftProbeResult>, DryRunError> {
    rollback_drift_probes()
        .iter()
        .map(|(kind, sql)| {
            Ok(DriftProbeResult {
                kind,
                exists: conn.query_row(sql, [], |row| row.get(0))?,
            })
        })
        .collect()
}

fn rollback_drift_probes() -> [(&'static str, &'static str); 3] {
    [
        (
            "chain drift before rollback",
            "SELECT EXISTS(
                SELECT 1 FROM s11_wu4_restore_session_ownership_preimage p
                JOIN session_chains c ON c.chain_id = p.chain_id
                WHERE p.entity_kind = 'chain' AND c.model_name <> p.new_model_name
             )",
        ),
        (
            "segment drift before rollback",
            "SELECT EXISTS(
                SELECT 1 FROM s11_wu4_restore_session_ownership_preimage p
                JOIN session_chain_segments s ON s.id = p.segment_id
                WHERE p.entity_kind = 'segment' AND s.provider_name <> p.new_provider_name
             )",
        ),
        (
            "turn drift before rollback",
            "SELECT EXISTS(
                SELECT 1 FROM s11_wu4_restore_session_ownership_preimage p
                JOIN session_turns t ON t.id = p.turn_row_id
                WHERE p.entity_kind = 'turn' AND t.provider_name <> p.new_provider_name
             )",
        ),
    ]
}

fn validate_no_rollback_drift(results: Vec<DriftProbeResult>) -> Result<(), DryRunError> {
    for result in results {
        validate_drift_probe(result)?;
    }
    Ok(())
}

fn validate_drift_probe(result: DriftProbeResult) -> Result<(), DryRunError> {
    if result.exists {
        return Err(DryRunError::new(result.kind));
    }
    Ok(())
}

fn preimage_mismatch_counts(conn: &Connection) -> Result<BTreeMap<String, i64>, DryRunError> {
    Ok(mismatch_count_map(mismatch_count_rows(conn)?))
}

fn mismatch_count_rows(conn: &Connection) -> Result<Vec<(&'static str, i64)>, DryRunError> {
    Ok(vec![
        ("chain_mismatch", read_chain_mismatch_count(conn)?),
        ("segment_mismatch", read_segment_mismatch_count(conn)?),
        ("turn_mismatch", read_turn_mismatch_count(conn)?),
    ])
}

fn mismatch_count_map(rows: Vec<(&'static str, i64)>) -> BTreeMap<String, i64> {
    let mut counts = BTreeMap::new();
    for (key, value) in rows {
        counts.insert(key.to_string(), value);
    }
    counts
}

fn read_chain_mismatch_count(conn: &Connection) -> Result<i64, DryRunError> {
    conn.query_row(
        "SELECT COUNT(*) FROM s11_wu4_restore_session_ownership_preimage p
         JOIN session_chains c ON c.chain_id = p.chain_id
         WHERE p.entity_kind = 'chain' AND c.model_name <> p.old_model_name",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn read_segment_mismatch_count(conn: &Connection) -> Result<i64, DryRunError> {
    conn.query_row(
        "SELECT COUNT(*) FROM s11_wu4_restore_session_ownership_preimage p
         JOIN session_chain_segments s ON s.id = p.segment_id
         WHERE p.entity_kind = 'segment' AND s.provider_name <> p.old_provider_name",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn read_turn_mismatch_count(conn: &Connection) -> Result<i64, DryRunError> {
    conn.query_row(
        "SELECT COUNT(*) FROM s11_wu4_restore_session_ownership_preimage p
         JOIN session_turns t ON t.id = p.turn_row_id
         WHERE p.entity_kind = 'turn' AND t.provider_name <> p.old_provider_name",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn rollback_counts(
    counts: BTreeMap<String, i64>,
    mismatches: BTreeMap<String, i64>,
) -> RollbackCounts {
    RollbackCounts {
        restored: all_zero(&mismatches),
        counts,
        mismatches,
    }
}

fn all_zero(counts: &BTreeMap<String, i64>) -> bool {
    counts.values().all(|value| *value == 0)
}

fn read_key_counts(conn: &Connection, table: &str) -> Result<BTreeMap<String, i64>, DryRunError> {
    let mut stmt = conn.prepare(&format!("SELECT key, value FROM {table}"))?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect::<Result<_, _>>().map_err(Into::into)
}

fn migrated_active_sessions(state: &Connection) -> Result<Vec<String>, DryRunError> {
    let mut stmt = state.prepare(
        "SELECT DISTINCT segment.session_id
         FROM session_chain_segments segment
         JOIN s11_wu4_restore_session_ownership_preimage preimage
           ON preimage.entity_kind = 'chain'
          AND preimage.chain_id = segment.chain_id
         WHERE segment.ended_at IS NULL
         ORDER BY segment.session_id",
    )?;
    let rows = stmt.query_map([], |row| row.get(0))?;
    rows.collect::<Result<_, _>>().map_err(Into::into)
}

fn runtime_cwds(
    mailbox_copy: Option<&Path>,
    session_ids: &[String],
) -> Result<Vec<RuntimeCwd>, DryRunError> {
    let Some(mailbox_copy) = mailbox_copy else {
        return Ok(missing_runtime_cwds(session_ids));
    };
    read_runtime_cwds(mailbox_copy, session_ids)
}

fn missing_runtime_cwds(session_ids: &[String]) -> Vec<RuntimeCwd> {
    session_ids.iter().map(|_| RuntimeCwd::Missing).collect()
}

fn read_runtime_cwds(
    mailbox_copy: &Path,
    session_ids: &[String],
) -> Result<Vec<RuntimeCwd>, DryRunError> {
    let mailbox = Connection::open(mailbox_copy)?;
    session_ids
        .iter()
        .map(|session_id| read_runtime_cwd(&mailbox, session_id))
        .collect()
}

fn read_runtime_cwd(mailbox: &Connection, session_id: &str) -> Result<RuntimeCwd, DryRunError> {
    let cwd: Option<Option<String>> = mailbox
        .query_row(
            "SELECT effective_cwd FROM session_runtime WHERE session_id = ?1",
            [session_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(runtime_cwd(cwd))
}

fn runtime_cwd(cwd: Option<Option<String>>) -> RuntimeCwd {
    match cwd {
        None => RuntimeCwd::Missing,
        Some(None) => RuntimeCwd::Null,
        Some(Some(path)) => RuntimeCwd::Value(path),
    }
}

fn cwd_completeness_counts(cwds: &[RuntimeCwd]) -> CwdCompleteness {
    let mut missing = 0;
    let mut null = 0;
    let mut non_absolute = 0;
    for cwd in cwds {
        match cwd {
            RuntimeCwd::Missing => missing += 1,
            RuntimeCwd::Null => null += 1,
            RuntimeCwd::Value(path) if !is_absolute_path(path) => non_absolute += 1,
            RuntimeCwd::Value(_) => {}
        }
    }
    CwdCompleteness {
        missing,
        null,
        non_absolute,
    }
}

fn is_absolute_path(path: &str) -> bool {
    Path::new(path).is_absolute()
}

trait OptionalRow<T> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error>;
}

impl<T> OptionalRow<T> for Result<T, rusqlite::Error> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(err),
        }
    }
}
