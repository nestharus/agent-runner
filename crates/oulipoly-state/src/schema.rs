use crate::StateDbError;
use rusqlite::Connection;
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
    let stored = conn
        .query_row("PRAGMA user_version", [], |row| row.get::<_, i32>(0))
        .map_err(|e| format!("Failed to read PRAGMA user_version: {e}"))?;

    if stored == 0 {
        let tables = user_tables(conn)?;
        if tables.is_empty() {
            return Ok(SchemaCompatibility::Fresh);
        }
        return Ok(match classify_versionless_tables(&tables) {
            Some(_) => SchemaCompatibility::LegacyVersionless,
            None => SchemaCompatibility::UnrecognizedVersionless,
        });
    }

    if stored == CURRENT_SCHEMA_VERSION {
        Ok(SchemaCompatibility::Current { stored })
    } else if (MINIMUM_SUPPORTED_SCHEMA_VERSION..CURRENT_SCHEMA_VERSION).contains(&stored) {
        Ok(SchemaCompatibility::Migratable { stored })
    } else if stored > CURRENT_SCHEMA_VERSION {
        Ok(SchemaCompatibility::Future { stored })
    } else {
        Ok(SchemaCompatibility::Corrupt {
            reason: format!(
                "stored schema version {stored} is below minimum supported {MINIMUM_SUPPORTED_SCHEMA_VERSION}"
            ),
        })
    }
}

pub(crate) fn classify_versionless(
    conn: &Connection,
) -> Result<Option<&'static str>, StateDbError> {
    let tables = user_tables(conn)?;
    Ok(classify_versionless_tables(&tables))
}

fn classify_versionless_tables(tables: &BTreeSet<String>) -> Option<&'static str> {
    let full_required = [
        "invocations",
        "providers",
        "provider_quotas",
        "provider_quota_windows",
        "session_turns",
        "session_chains",
        "session_chain_segments",
    ];
    if full_required.iter().all(|table| tables.contains(*table)) {
        return Some("versionless_legacy_runner_state");
    }

    let setup_required = [
        "memory_nodes",
        "memory_edges",
        "setup_sessions",
        "setup_turns",
    ];
    let setup_allowed = setup_required;
    if setup_required.iter().all(|table| tables.contains(*table))
        && tables
            .iter()
            .all(|table| setup_allowed.contains(&table.as_str()))
    {
        return Some("versionless_legacy_setup_only");
    }

    None
}

fn user_tables(conn: &Connection) -> Result<BTreeSet<String>, StateDbError> {
    let mut stmt = conn
        .prepare(
            "SELECT name FROM sqlite_schema
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
        )
        .map_err(|e| format!("Failed to inspect sqlite_schema: {e}"))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| format!("Failed to query sqlite_schema: {e}"))?;
    let mut tables = BTreeSet::new();
    for row in rows {
        tables.insert(row.map_err(|e| format!("Failed to read sqlite_schema row: {e}"))?);
    }
    Ok(tables)
}
