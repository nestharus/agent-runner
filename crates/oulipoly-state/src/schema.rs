//! ## Declared roles
//! accessor, formatter, mapper, orchestration, predicate, validator
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-state/src/schema.rs
//!     role: adapter
//!     Translates:
//!       - rusqlite SQLite schema-introspection contract
//!       - oulipoly-state typed schema-compatibility and error contract
//!       - std ordered table-name comparison support contract
//! ```

use crate::StateDbError;
use rusqlite::{Connection, Statement};
use std::collections::BTreeSet;

pub const CURRENT_SCHEMA_VERSION: i32 = 18;
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

const FULL_LEGACY_RUNNER_TABLES: &[&str] = &[
    "invocations",
    "providers",
    "provider_quotas",
    "provider_quota_windows",
    "session_turns",
    "session_chains",
    "session_chain_segments",
];

const LEGACY_SETUP_ONLY_TABLES: &[&str] = &[
    "memory_nodes",
    "memory_edges",
    "setup_sessions",
    "setup_turns",
];

pub fn classify(conn: &Connection) -> Result<SchemaCompatibility, StateDbError> {
    let stored = read_user_version(conn)?;

    if has_versionless_user_version(stored) {
        return classify_versionless_compatibility(conn);
    }

    Ok(classify_numbered_schema_version(stored))
}

fn read_user_version(conn: &Connection) -> Result<i32, StateDbError> {
    conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i32>(0))
        .map_err(format_read_user_version_error)
}

fn format_read_user_version_error(error: rusqlite::Error) -> String {
    format!("Failed to read PRAGMA user_version: {error}")
}

fn has_versionless_user_version(stored: i32) -> bool {
    stored == 0
}

fn classify_numbered_schema_version(stored: i32) -> SchemaCompatibility {
    match validate_numbered_schema_version(stored) {
        Ok(validated) => map_numbered_schema_version(validated),
        Err(invalid) => map_corrupt_schema_version(invalid),
    }
}

fn validate_numbered_schema_version(stored: i32) -> Result<i32, i32> {
    if is_supported_or_future_schema_version(stored) {
        return Ok(stored);
    }

    Err(stored)
}

fn is_supported_or_future_schema_version(stored: i32) -> bool {
    stored >= MINIMUM_SUPPORTED_SCHEMA_VERSION
}

fn map_numbered_schema_version(stored: i32) -> SchemaCompatibility {
    if stored == CURRENT_SCHEMA_VERSION {
        return SchemaCompatibility::Current { stored };
    }

    if is_migratable_schema_version(stored) {
        return SchemaCompatibility::Migratable { stored };
    }

    SchemaCompatibility::Future { stored }
}

fn is_migratable_schema_version(stored: i32) -> bool {
    (MINIMUM_SUPPORTED_SCHEMA_VERSION..CURRENT_SCHEMA_VERSION).contains(&stored)
}

fn map_corrupt_schema_version(stored: i32) -> SchemaCompatibility {
    SchemaCompatibility::Corrupt {
        reason: format_corrupt_schema_version_reason(stored),
    }
}

fn format_corrupt_schema_version_reason(stored: i32) -> String {
    format!(
        "stored schema version {stored} is below minimum supported {MINIMUM_SUPPORTED_SCHEMA_VERSION}"
    )
}

fn classify_versionless_compatibility(
    conn: &Connection,
) -> Result<SchemaCompatibility, StateDbError> {
    let tables = user_tables(conn)?;
    Ok(map_versionless_tables_to_compatibility(&tables))
}

fn map_versionless_tables_to_compatibility(tables: &BTreeSet<String>) -> SchemaCompatibility {
    if has_no_user_tables(tables) {
        return SchemaCompatibility::Fresh;
    }

    map_versionless_legacy_label_to_compatibility(classify_versionless_tables(tables))
}

fn has_no_user_tables(tables: &BTreeSet<String>) -> bool {
    tables.is_empty()
}

fn map_versionless_legacy_label_to_compatibility(
    legacy_label: Option<&'static str>,
) -> SchemaCompatibility {
    match legacy_label {
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
    if let Some(label) = map_full_legacy_runner_state_label(tables) {
        return Some(label);
    }

    map_legacy_setup_only_state_label(tables)
}

fn map_full_legacy_runner_state_label(tables: &BTreeSet<String>) -> Option<&'static str> {
    if is_full_legacy_runner_state(tables) {
        return Some("versionless_legacy_runner_state");
    }

    None
}

fn map_legacy_setup_only_state_label(tables: &BTreeSet<String>) -> Option<&'static str> {
    if is_legacy_setup_only_state(tables) {
        return Some("versionless_legacy_setup_only");
    }

    None
}

fn is_full_legacy_runner_state(tables: &BTreeSet<String>) -> bool {
    contains_required_tables(tables, FULL_LEGACY_RUNNER_TABLES)
}

fn is_legacy_setup_only_state(tables: &BTreeSet<String>) -> bool {
    contains_required_tables(tables, LEGACY_SETUP_ONLY_TABLES)
        && contains_only_allowed_tables(tables, LEGACY_SETUP_ONLY_TABLES)
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
    .map_err(format_inspect_sqlite_schema_error)
}

fn format_inspect_sqlite_schema_error(error: rusqlite::Error) -> String {
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
        .map_err(format_query_sqlite_schema_error)
}

fn read_table_name_column(row: &rusqlite::Row<'_>) -> rusqlite::Result<String> {
    row.get::<_, String>(0)
}

fn format_query_sqlite_schema_error(error: rusqlite::Error) -> String {
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
    row.map_err(format_read_sqlite_schema_row_error)
}

fn format_read_sqlite_schema_row_error(error: rusqlite::Error) -> String {
    format!("Failed to read sqlite_schema row: {error}")
}
