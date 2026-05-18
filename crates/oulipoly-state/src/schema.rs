//! ## Declared roles
//! accessor, mapper, predicate, formatter, orchestration, validator

use crate::StateDbError;
use rusqlite::{Connection, Statement};
use std::collections::BTreeSet;

pub const CURRENT_SCHEMA_VERSION: i32 = 6;
pub const MINIMUM_SUPPORTED_SCHEMA_VERSION: i32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaCompatibility {
    Fresh,
    Current { stored: i32 },
    Migratable { stored: i32 },
    Future { stored: i32 },
    LegacyVersionless,
    UnrecognizedVersionless,
    Corrupt { reason: String },
}

pub fn classify(conn: &Connection) -> Result<SchemaCompatibility, StateDbError> {
    let stored = read_user_version(conn)?;

    if is_versionless(stored) {
        return classify_versionless_compatibility(conn);
    }

    Ok(classify_stored_version(stored))
}

fn read_user_version(conn: &Connection) -> Result<i32, StateDbError> {
    conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i32>(0))
        .map_err(read_user_version_error)
}

fn read_user_version_error(error: rusqlite::Error) -> String {
    format!("Failed to read PRAGMA user_version: {error}")
}

fn is_versionless(stored: i32) -> bool {
    stored == 0
}

fn classify_stored_version(stored: i32) -> SchemaCompatibility {
    if stored == CURRENT_SCHEMA_VERSION {
        SchemaCompatibility::Current { stored }
    } else if (MINIMUM_SUPPORTED_SCHEMA_VERSION..CURRENT_SCHEMA_VERSION).contains(&stored) {
        SchemaCompatibility::Migratable { stored }
    } else if stored > CURRENT_SCHEMA_VERSION {
        SchemaCompatibility::Future { stored }
    } else {
        SchemaCompatibility::Corrupt {
            reason: corrupt_stored_version_reason(stored),
        }
    }
}

fn corrupt_stored_version_reason(stored: i32) -> String {
    format!(
        "stored schema version {stored} is below minimum supported {MINIMUM_SUPPORTED_SCHEMA_VERSION}"
    )
}

fn classify_versionless_compatibility(
    conn: &Connection,
) -> Result<SchemaCompatibility, StateDbError> {
    let tables = user_tables(conn)?;
    Ok(classify_versionless_table_compatibility(&tables))
}

fn classify_versionless_table_compatibility(tables: &BTreeSet<String>) -> SchemaCompatibility {
    if tables.is_empty() {
        return SchemaCompatibility::Fresh;
    }
    match classify_versionless_tables(tables) {
        Some(_) => SchemaCompatibility::LegacyVersionless,
        None => SchemaCompatibility::UnrecognizedVersionless,
    }
}

pub(crate) fn classify_versionless(
    conn: &Connection,
) -> Result<Option<&'static str>, StateDbError> {
    let tables = user_tables(conn)?;
    Ok(classify_versionless_tables(&tables))
}

fn classify_versionless_tables(tables: &BTreeSet<String>) -> Option<&'static str> {
    if is_full_legacy_runner_state(tables) {
        return Some("versionless_legacy_runner_state");
    }

    if is_legacy_setup_only_state(tables) {
        return Some("versionless_legacy_setup_only");
    }

    None
}

fn is_full_legacy_runner_state(tables: &BTreeSet<String>) -> bool {
    contains_required_tables(
        tables,
        &[
            "invocations",
            "providers",
            "provider_quotas",
            "provider_quota_windows",
            "session_turns",
            "session_chains",
            "session_chain_segments",
        ],
    )
}

fn is_legacy_setup_only_state(tables: &BTreeSet<String>) -> bool {
    let required = [
        "memory_nodes",
        "memory_edges",
        "setup_sessions",
        "setup_turns",
    ];
    contains_required_tables(tables, &required) && contains_only_allowed_tables(tables, &required)
}

fn contains_required_tables(tables: &BTreeSet<String>, required: &[&str]) -> bool {
    required.iter().all(|table| tables.contains(*table))
}

fn contains_only_allowed_tables(tables: &BTreeSet<String>, allowed: &[&str]) -> bool {
    tables.iter().all(|table| allowed.contains(&table.as_str()))
}

fn user_tables(conn: &Connection) -> Result<BTreeSet<String>, StateDbError> {
    let mut stmt = prepare_user_tables_query(conn)?;
    collect_user_table_names(&mut stmt)
}

fn prepare_user_tables_query(conn: &Connection) -> Result<Statement<'_>, StateDbError> {
    conn.prepare(
        "SELECT name FROM sqlite_schema
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
    )
    .map_err(inspect_sqlite_schema_error)
}

fn inspect_sqlite_schema_error(error: rusqlite::Error) -> String {
    format!("Failed to inspect sqlite_schema: {error}")
}

fn collect_user_table_names(stmt: &mut Statement<'_>) -> Result<BTreeSet<String>, StateDbError> {
    let rows = query_user_table_name_rows(stmt)?;
    collect_table_name_rows(rows)
}

fn query_user_table_name_rows<'stmt>(
    stmt: &'stmt mut Statement<'_>,
) -> Result<
    rusqlite::MappedRows<'stmt, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<String> + 'stmt>,
    StateDbError,
> {
    stmt.query_map([], read_table_name_column)
        .map_err(query_sqlite_schema_error)
}

fn read_table_name_column(row: &rusqlite::Row<'_>) -> rusqlite::Result<String> {
    row.get::<_, String>(0)
}

fn query_sqlite_schema_error(error: rusqlite::Error) -> String {
    format!("Failed to query sqlite_schema: {error}")
}

fn collect_table_name_rows(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<String>>,
) -> Result<BTreeSet<String>, StateDbError> {
    let mut tables = BTreeSet::new();
    for row in rows {
        tables.insert(read_table_name_row(row)?);
    }
    Ok(tables)
}

fn read_table_name_row(row: rusqlite::Result<String>) -> Result<String, StateDbError> {
    row.map_err(read_sqlite_schema_row_error)
}

fn read_sqlite_schema_row_error(error: rusqlite::Error) -> String {
    format!("Failed to read sqlite_schema row: {error}")
}
