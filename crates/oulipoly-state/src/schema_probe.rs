use crate::{ReadOnlyOpenError, StateDb};
use rusqlite::Connection;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

pub const CURRENT_SCHEMA_VERSION: u32 = 3;
pub const MINIMUM_SUPPORTED_SCHEMA_VERSION: u32 = 3;

pub type FeatureMap = BTreeMap<String, bool>;

#[derive(Debug, Clone, Serialize)]
pub struct SchemaProbeReport {
    pub binary: BinaryInfo,
    pub state_db: StateDbReport,
    pub features: FeatureMap,
    pub supported_storage_types: Vec<String>,
    pub safe_for_import_replace: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct BinaryInfo {
    pub name: String,
    pub version: String,
    pub commit: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StateDbReport {
    pub path: PathBuf,
    pub exists: bool,
    pub schema_version: u32,
    pub user_version: u32,
    pub current_schema_version: u32,
    pub minimum_supported_schema_version: u32,
    pub compatible: bool,
    pub tables: BTreeMap<String, bool>,
    pub required_columns: BTreeMap<String, BTreeMap<String, bool>>,
    pub required_indexes: BTreeMap<String, BTreeMap<String, bool>>,
}

#[derive(Debug)]
pub enum ProbeError {
    StatePath { message: String },
    Open { error: ReadOnlyOpenError },
    Inspect { message: String },
}

#[derive(Debug, Clone, Copy)]
struct RequiredIndex {
    name: &'static str,
    columns: &'static [&'static str],
}

#[derive(Debug)]
struct IndexDefinition {
    table: String,
    columns: Vec<String>,
}

pub fn run_schema_probe() -> Result<SchemaProbeReport, ProbeError> {
    let path = StateDb::default_path().map_err(|message| ProbeError::StatePath { message })?;
    if !path.exists() {
        return Ok(missing_report(path));
    }

    let db = match StateDb::open_read_only(&path) {
        Ok(db) => db,
        Err(ReadOnlyOpenError::Missing { .. }) => return Ok(missing_report(path)),
        Err(error) => return Err(ProbeError::Open { error }),
    };

    let state_db =
        inspect_schema(db.connection(), path).map_err(|message| ProbeError::Inspect { message })?;
    Ok(report_from_state_db(state_db))
}

pub fn missing_report(path: PathBuf) -> SchemaProbeReport {
    report_from_state_db(StateDbReport {
        path,
        exists: false,
        schema_version: 0,
        user_version: 0,
        current_schema_version: CURRENT_SCHEMA_VERSION,
        minimum_supported_schema_version: MINIMUM_SUPPORTED_SCHEMA_VERSION,
        compatible: false,
        tables: required_table_map(false),
        required_columns: required_column_map(None)
            .expect("missing report does not inspect columns"),
        required_indexes: required_index_map(None)
            .expect("missing report does not inspect indexes"),
    })
}

pub fn inspect_schema(conn: &Connection, path: PathBuf) -> Result<StateDbReport, String> {
    let user_version = conn
        .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
        .map_err(|e| format!("Failed to read PRAGMA user_version: {e}"))?;

    let mut stmt = conn
        .prepare("SELECT type, name FROM sqlite_schema WHERE type IN ('table', 'index')")
        .map_err(|e| format!("Failed to read sqlite_schema: {e}"))?;
    let mut rows = stmt
        .query([])
        .map_err(|e| format!("Failed to query sqlite_schema: {e}"))?;

    let mut tables_seen = BTreeSet::new();
    while let Some(row) = rows
        .next()
        .map_err(|e| format!("Failed to iterate sqlite_schema: {e}"))?
    {
        let object_type: String = row
            .get(0)
            .map_err(|e| format!("Failed to read sqlite_schema.type: {e}"))?;
        let name: String = row
            .get(1)
            .map_err(|e| format!("Failed to read sqlite_schema.name: {e}"))?;
        match object_type.as_str() {
            "table" => {
                tables_seen.insert(name);
            }
            "index" => {}
            _ => {}
        }
    }

    let tables = required_tables()
        .into_iter()
        .map(|table| (table.to_string(), tables_seen.contains(table)))
        .collect();
    let required_columns = required_column_map(Some(conn))?;
    let required_indexes = required_index_map(Some(conn))?;

    let compatible = (MINIMUM_SUPPORTED_SCHEMA_VERSION..=CURRENT_SCHEMA_VERSION)
        .contains(&user_version)
        && all_values(&tables)
        && all_nested_values(&required_columns)
        && all_nested_values(&required_indexes);

    Ok(StateDbReport {
        path,
        exists: true,
        schema_version: user_version,
        user_version,
        current_schema_version: CURRENT_SCHEMA_VERSION,
        minimum_supported_schema_version: MINIMUM_SUPPORTED_SCHEMA_VERSION,
        compatible,
        tables,
        required_columns,
        required_indexes,
    })
}

pub fn report_from_state_db(state_db: StateDbReport) -> SchemaProbeReport {
    let features = feature_map();
    let supported_storage_types = supported_storage_types();
    let safe_for_import_replace =
        safe_for_import_replace(&state_db, &features, &supported_storage_types);

    SchemaProbeReport {
        binary: BinaryInfo {
            name: "oulipoly-agent-runner".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            commit: option_env!("BUILD_COMMIT").unwrap_or("unknown").to_string(),
        },
        state_db,
        features,
        supported_storage_types,
        safe_for_import_replace,
    }
}

pub fn feature_map() -> FeatureMap {
    BTreeMap::from([
        ("session_locate".to_string(), false),
        ("session_export".to_string(), false),
        ("session_import_replace".to_string(), false),
        ("session_pause_handshake".to_string(), false),
        ("session_schema_probe".to_string(), true),
    ])
}

pub fn supported_storage_types() -> Vec<String> {
    ["claude_code", "codex_session", "other"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn safe_for_import_replace(
    state_db: &StateDbReport,
    features: &FeatureMap,
    supported_storage_types: &[String],
) -> bool {
    state_db.exists
        && state_db.compatible
        && features
            .get("session_import_replace")
            .copied()
            .unwrap_or(false)
        && features
            .get("session_pause_handshake")
            .copied()
            .unwrap_or(false)
        && supported_storage_types == self::supported_storage_types()
}

fn required_tables() -> [&'static str; 4] {
    [
        "invocations",
        "session_turns",
        "session_chains",
        "session_chain_segments",
    ]
}

fn required_columns() -> BTreeMap<&'static str, Vec<&'static str>> {
    BTreeMap::from([
        (
            "invocations",
            vec![
                "session_id",
                "session_capture_method",
                "resume_acceptance_status",
                "resume_acceptance_evidence",
            ],
        ),
        (
            "session_turns",
            vec!["parent_turn_id", "is_sidechain", "is_compaction_boundary"],
        ),
        (
            "session_chains",
            vec!["chain_id", "created_at", "last_used_at", "model_name"],
        ),
        (
            "session_chain_segments",
            vec![
                "chain_id",
                "provider_name",
                "session_id",
                "started_at",
                "ended_at",
                "last_turn_id",
                "transition_reason",
            ],
        ),
    ])
}

fn required_indexes() -> BTreeMap<&'static str, Vec<RequiredIndex>> {
    BTreeMap::from([
        (
            "invocations",
            vec![RequiredIndex {
                name: "idx_invocations_provider_session",
                columns: &["provider_name", "session_id"],
            }],
        ),
        (
            "session_turns",
            vec![RequiredIndex {
                name: "idx_session_turns_session_lookup",
                columns: &["session_id", "timestamp"],
            }],
        ),
        (
            "session_chain_segments",
            vec![
                RequiredIndex {
                    name: "idx_segments_session",
                    columns: &["session_id"],
                },
                RequiredIndex {
                    name: "idx_segments_chain_active",
                    columns: &["chain_id", "ended_at"],
                },
            ],
        ),
    ])
}

fn required_table_map(value: bool) -> BTreeMap<String, bool> {
    required_tables()
        .into_iter()
        .map(|table| (table.to_string(), value))
        .collect()
}

fn required_column_map(
    conn: Option<&Connection>,
) -> Result<BTreeMap<String, BTreeMap<String, bool>>, String> {
    required_columns()
        .into_iter()
        .map(|(table, columns)| {
            let seen = match conn {
                Some(conn) => table_columns(conn, table)?,
                None => BTreeSet::new(),
            };
            let columns = columns
                .into_iter()
                .map(|column| (column.to_string(), seen.contains(column)))
                .collect();
            Ok((table.to_string(), columns))
        })
        .collect()
}

fn required_index_map(
    conn: Option<&Connection>,
) -> Result<BTreeMap<String, BTreeMap<String, bool>>, String> {
    required_indexes()
        .into_iter()
        .map(|(table, indexes)| {
            let indexes = indexes
                .into_iter()
                .map(|index| {
                    let matches = match conn {
                        Some(conn) => required_index_matches(conn, table, index)?,
                        None => false,
                    };
                    Ok((index.name.to_string(), matches))
                })
                .collect::<Result<BTreeMap<_, _>, String>>()?;
            Ok((table.to_string(), indexes))
        })
        .collect()
}

fn table_columns(conn: &Connection, table: &str) -> Result<BTreeSet<String>, String> {
    let sql = format!("PRAGMA table_info({})", quote_identifier(table));
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("Failed to inspect columns for {table}: {e}"))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| format!("Failed to query columns for {table}: {e}"))?;

    let mut columns = BTreeSet::new();
    for row in rows {
        columns.insert(row.map_err(|e| format!("Failed to read column for {table}: {e}"))?);
    }
    Ok(columns)
}

fn required_index_matches(
    conn: &Connection,
    table: &str,
    required: RequiredIndex,
) -> Result<bool, String> {
    let Some(actual) = index_definition(conn, required.name)? else {
        return Ok(false);
    };
    let expected_columns: Vec<String> = required
        .columns
        .iter()
        .map(|column| column.to_string())
        .collect();
    Ok(actual.table == table && actual.columns == expected_columns)
}

fn index_definition(conn: &Connection, index: &str) -> Result<Option<IndexDefinition>, String> {
    let mut stmt = conn
        .prepare("SELECT tbl_name FROM sqlite_schema WHERE type = 'index' AND name = ?1")
        .map_err(|e| format!("Failed to inspect index {index}: {e}"))?;
    let mut rows = stmt
        .query([index])
        .map_err(|e| format!("Failed to query index {index}: {e}"))?;
    let Some(row) = rows
        .next()
        .map_err(|e| format!("Failed to iterate index {index}: {e}"))?
    else {
        return Ok(None);
    };
    let table = row
        .get::<_, String>(0)
        .map_err(|e| format!("Failed to read table for index {index}: {e}"))?;
    drop(rows);
    drop(stmt);

    let sql = format!("PRAGMA index_info({})", quote_identifier(index));
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("Failed to inspect columns for index {index}: {e}"))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(2))
        .map_err(|e| format!("Failed to query columns for index {index}: {e}"))?;

    let mut columns = Vec::new();
    for row in rows {
        columns.push(row.map_err(|e| format!("Failed to read column for index {index}: {e}"))?);
    }

    Ok(Some(IndexDefinition { table, columns }))
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn all_values(values: &BTreeMap<String, bool>) -> bool {
    values.values().all(|value| *value)
}

fn all_nested_values(values: &BTreeMap<String, BTreeMap<String, bool>>) -> bool {
    values
        .values()
        .all(|nested| nested.values().all(|value| *value))
}
