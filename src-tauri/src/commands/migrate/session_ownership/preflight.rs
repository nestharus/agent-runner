//! Declared roles: validator, predicate, accessor

use super::DryRunError;
use oulipoly_state::CURRENT_SCHEMA_VERSION;
use rusqlite::Connection;
use std::collections::BTreeSet;

#[derive(Debug, Clone)]
pub(crate) struct IntegrityReport {
    pub(crate) quick_check: String,
    pub(crate) user_version: i64,
}

pub(crate) fn preflight(conn: &Connection) -> Result<IntegrityReport, DryRunError> {
    let report = inspect_integrity(conn)?;
    if report.quick_check != "ok" {
        return Err(DryRunError::new(format!(
            "quick_check failed: {}",
            report.quick_check
        )));
    }
    if report.user_version != i64::from(CURRENT_SCHEMA_VERSION) {
        return Err(DryRunError::new(format!(
            "user_version mismatch: expected {}, got {}",
            CURRENT_SCHEMA_VERSION, report.user_version
        )));
    }
    require_columns(
        conn,
        "session_chains",
        &["chain_id", "created_at", "last_used_at", "model_name"],
    )?;
    require_columns(
        conn,
        "session_chain_segments",
        &[
            "id",
            "chain_id",
            "provider_name",
            "session_id",
            "started_at",
            "ended_at",
            "last_turn_id",
            "transition_reason",
        ],
    )?;
    require_columns(
        conn,
        "session_turns",
        &[
            "id",
            "provider_name",
            "session_id",
            "turn_id",
            "timestamp",
            "role",
            "source_file",
            "ingested_at",
        ],
    )?;
    require_unique(
        conn,
        "session_chain_segments",
        &["chain_id", "provider_name", "session_id"],
    )?;
    require_unique(
        conn,
        "session_turns",
        &["provider_name", "session_id", "turn_id"],
    )?;
    Ok(report)
}

pub(crate) fn inspect_integrity(conn: &Connection) -> Result<IntegrityReport, DryRunError> {
    let quick_check = conn.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    let user_version = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    Ok(IntegrityReport {
        quick_check,
        user_version,
    })
}

fn require_columns(conn: &Connection, table: &str, required: &[&str]) -> Result<(), DryRunError> {
    let columns = table_columns(conn, table)?;
    for column in required {
        if !columns.contains(*column) {
            return Err(DryRunError::new(format!(
                "required column missing: {table}.{column}"
            )));
        }
    }
    Ok(())
}

fn table_columns(conn: &Connection, table: &str) -> Result<BTreeSet<String>, DryRunError> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    rows.collect::<Result<_, _>>().map_err(Into::into)
}

fn require_unique(conn: &Connection, table: &str, columns: &[&str]) -> Result<(), DryRunError> {
    if has_unique_index(conn, table, columns)? {
        return Ok(());
    }
    Err(DryRunError::new(format!(
        "missing UNIQUE({})",
        columns.join(", ")
    )))
}

fn has_unique_index(conn: &Connection, table: &str, columns: &[&str]) -> Result<bool, DryRunError> {
    let mut stmt = conn.prepare(&format!("PRAGMA index_list({table})"))?;
    let indexes = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(2)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (name, unique) in indexes {
        if unique == 0 {
            continue;
        }
        if index_columns(conn, &name)? == columns {
            return Ok(true);
        }
    }
    Ok(false)
}

fn index_columns(conn: &Connection, index: &str) -> Result<Vec<String>, DryRunError> {
    let mut stmt = conn.prepare(&format!("PRAGMA index_info({index})"))?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(2)?))
    })?;
    let mut columns = rows.collect::<Result<Vec<_>, _>>()?;
    columns.sort_by_key(|(seq, _)| *seq);
    Ok(columns.into_iter().map(|(_, column)| column).collect())
}
