//! ## Declared roles
//! orchestration, accessor, mapper, filter, predicate, validator, formatter
//!
//! ## Intrinsic-surface declarations
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/tests/age_61_row_version_migration_pragma.rs
//!     role: intrinsic-surface
//!     Domain: state-db-row-version-pragma-test-domain
//!     Owns:
//!       - TRACKED_TABLES invocations table name
//!       - TRACKED_TABLES providers table name
//!       - TRACKED_TABLES provider_quotas table name
//!       - TRACKED_TABLES provider_quota_windows table name
//!       - TRACKED_TABLES memory_nodes table name
//!       - TRACKED_TABLES memory_edges table name
//!       - TRACKED_TABLES setup_sessions table name
//!       - TRACKED_TABLES setup_turns table name
//!       - TRACKED_TABLES cli_providers table name
//!       - TRACKED_TABLES accounts table name
//!       - TRACKED_TABLES discovered_models table name
//!       - TRACKED_TABLES model_parameters table name
//!       - TRACKED_TABLES session_turns table name
//!       - TRACKED_TABLES session_chains table name
//!       - TRACKED_TABLES session_chain_segments table name
//!       - TRACKED_TABLES invocation_returned_artifacts table name
//!       - oulipoly_state row-version REGISTRY symbol and registry::lookup API
//!       - TableRegistration table, primary_key_columns, payload_columns, and kind fields
//!       - RowKind::Mutable registry row-kind contract
//!       - ordered migration asset identities and schema-version plan membership

mod fixtures;

use fixtures::{table_names, user_version};
use oulipoly_state::CURRENT_SCHEMA_VERSION;
use oulipoly_state::deployment::row_version::registry::{
    self, REGISTRY, RowKind, TableRegistration,
};
use oulipoly_state::migrations;
use rusqlite::Connection;
use std::collections::BTreeSet;
use std::path::Path;

const TRACKED_TABLES: &[&str] = &[
    "invocations",
    "providers",
    "provider_quotas",
    "provider_quota_windows",
    "memory_nodes",
    "memory_edges",
    "setup_sessions",
    "setup_turns",
    "cli_providers",
    "accounts",
    "discovered_models",
    "model_parameters",
    "session_turns",
    "session_chains",
    "session_chain_segments",
    "invocation_returned_artifacts",
];

#[test]
fn ti_03_schema5_to_schema6_adds_row_version_to_every_tracked_table() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    build_schema5_db(&db_path);

    let conn = migrate_schema5_to_schema6(&db_path);

    assert_eq!(user_version(&conn), CURRENT_SCHEMA_VERSION);
    for table in TRACKED_TABLES {
        assert_row_version_column(&conn, table);
    }
}

#[test]
fn registry_enumerates_exactly_the_contract_tracked_tables() {
    let actual = REGISTRY
        .iter()
        .map(|entry| entry.table)
        .collect::<BTreeSet<_>>();
    let expected = TRACKED_TABLES.iter().copied().collect::<BTreeSet<_>>();

    assert_eq!(REGISTRY.len(), TRACKED_TABLES.len());
    assert_eq!(actual, expected);
    for table in TRACKED_TABLES {
        let registration: &TableRegistration = registry::lookup(table).unwrap_or_else(|| {
            panic!("registry::lookup missing tracked table {table}");
        });
        assert_eq!(registration.table, *table);
        assert_eq!(registration.kind, RowKind::Mutable);
    }
    assert!(registry::lookup("not_a_tracked_table").is_none());
}

#[test]
fn invocation_returned_artifacts_is_migration_owned_and_versioned() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    build_schema5_db(&db_path);

    let conn = migrate_schema5_to_schema6(&db_path);

    assert!(
        table_names(&conn).contains(&"invocation_returned_artifacts".to_string()),
        "0006 must create invocation_returned_artifacts when absent from the v5 DB"
    );
    assert_row_version_column(&conn, "invocation_returned_artifacts");
}

#[test]
fn partial_schema5_session_tables_are_recreated_with_lookup_indexes() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    build_schema5_db(&db_path);
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "DROP TABLE session_turns;
             DROP TABLE session_chain_segments;",
        )
        .unwrap();
        assert_eq!(user_version(&conn), 5);
    }

    let conn = migrate_schema5_to_schema6(&db_path);

    for table in ["session_turns", "session_chain_segments"] {
        assert_row_version_column(&conn, table);
    }
    for index in [
        "idx_session_turns_provider_ts",
        "idx_session_turns_session_ts",
        "idx_session_turns_session_lookup",
        "idx_session_turns_parent",
        "idx_segments_session",
        "idx_segments_chain_active",
    ] {
        assert!(
            index_names(&conn).contains(index),
            "missing recreated index {index}"
        );
    }
}

#[test]
fn registry_payload_columns_match_migrated_table_info_for_every_entry() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    build_schema5_db(&db_path);

    let conn = migrate_schema5_to_current(&db_path);

    for registration in REGISTRY {
        assert_registration_matches_table_info(&conn, registration);
    }
}

// Declared role: validator
fn assert_registration_matches_table_info(conn: &Connection, registration: &TableRegistration) {
    let columns = table_info(conn, registration.table);
    assert_registration_keys_match_table_info(&columns, registration);
    assert_registered_payload_matches_table_info(&columns, registration);
    assert_registered_columns_exist(registration, &column_name_set(&columns));
}

// Declared role: validator
fn assert_registration_keys_match_table_info(
    columns: &[ColumnInfo],
    registration: &TableRegistration,
) {
    let primary_keys = column_ref_name_set(&primary_key_columns(columns));
    let registered_pk = registered_primary_key_set(registration);
    assert_eq!(
        primary_keys, registered_pk,
        "registry primary keys disagree with PRAGMA table_info for {}",
        registration.table
    );
}

// Declared role: validator
fn assert_registered_payload_matches_table_info(
    columns: &[ColumnInfo],
    registration: &TableRegistration,
) {
    let payload_columns = column_ref_name_set(&payload_columns(columns, registration));
    let registered_payload = registered_payload_set(registration);
    assert_eq!(
        payload_columns, registered_payload,
        "registry payload columns disagree with PRAGMA table_info for {}",
        registration.table
    );
    assert!(
        !registered_payload.contains("row_version"),
        "registry payload_columns must exclude row_version for {}",
        registration.table
    );
}

// Declared role: mapper
fn column_name_set(columns: &[ColumnInfo]) -> BTreeSet<&str> {
    columns.iter().map(|column| column.name.as_str()).collect()
}

// Declared role: mapper
fn column_ref_name_set<'a>(columns: &[&'a ColumnInfo]) -> BTreeSet<&'a str> {
    columns.iter().map(|column| column.name.as_str()).collect()
}

// Declared role: filter
fn primary_key_columns(columns: &[ColumnInfo]) -> Vec<&ColumnInfo> {
    columns.iter().filter(|column| column.pk > 0).collect()
}

// Declared role: filter
fn payload_columns<'a>(
    columns: &'a [ColumnInfo],
    registration: &TableRegistration,
) -> Vec<&'a ColumnInfo> {
    columns
        .iter()
        .filter(|column| is_payload_column(column, registration))
        .collect()
}

// Declared role: predicate
fn is_payload_column(column: &ColumnInfo, registration: &TableRegistration) -> bool {
    column.name != "row_version"
        && !registration
            .primary_key_columns
            .contains(&column.name.as_str())
}

// Declared role: mapper
fn registered_primary_key_set(registration: &TableRegistration) -> BTreeSet<&str> {
    registration.primary_key_columns.iter().copied().collect()
}

// Declared role: mapper
fn registered_payload_set(registration: &TableRegistration) -> BTreeSet<&str> {
    registration.payload_columns.iter().copied().collect()
}

// Declared role: validator
fn assert_registered_columns_exist(
    registration: &TableRegistration,
    column_names: &BTreeSet<&str>,
) {
    for column in registration
        .primary_key_columns
        .iter()
        .chain(registration.payload_columns.iter())
    {
        assert!(
            column_names.contains(column),
            "registry column {column} missing from migrated table {}",
            registration.table
        );
    }
}

// Declared role: orchestration
fn build_schema5_db(path: &Path) {
    let mut conn = Connection::open(path).unwrap();
    conn.pragma_update(None, "user_version", 0).unwrap();
    let plan = migrations::plan(0, 5).unwrap();
    migrations::run_with_db_path(&mut conn, &plan, path.to_path_buf()).unwrap();
    assert_schema5_user_version(conn);
}

// Declared role: validator
fn assert_schema5_user_version(conn: Connection) {
    assert_eq!(user_version(&conn), 5);
}

// Declared role: orchestration
fn migrate_schema5_to_schema6(path: &Path) -> Connection {
    let mut conn = Connection::open(path).unwrap();
    let plan = migrations::current_plan_from(5).unwrap();
    assert_schema5_current_plan(&plan);
    migrations::run_with_db_path(&mut conn, &plan, path.to_path_buf()).unwrap();
    conn
}

fn migrate_schema5_to_current(path: &Path) -> Connection {
    let mut conn = Connection::open(path).unwrap();
    let plan = migrations::current_plan_from(5).unwrap();
    migrations::run_with_db_path(&mut conn, &plan, path.to_path_buf()).unwrap();
    conn
}

// Declared role: validator
fn assert_schema5_current_plan(plan: &[&migrations::Migration]) {
    assert_eq!(
        plan.iter()
            .map(|migration| migration.id)
            .collect::<Vec<_>>(),
        vec![
            "0006_age_58_dual_write_row_versions",
            "0007_age_123_resume_provider_identity",
            "0008_owned_turn_events",
            "0009_age163_working_set_and_round_robin",
            "0010_imported_session_display_metadata",
            "0011_durable_session_lifecycle",
            "0012_session_ingress_evidence",
            "0013_fresh_continuations",
            "0014_invocation_completion_obligations",
            "0015_invocation_completion_continuity",
            "0016_invocation_completion_authority_summary",
            "0017_completion_registration_authority",
            "0018_invocation_completion_materialization_summary",
            "0019_invocation_running_projection_index",
            "0020_session_turn_pages",
            "0021_invocation_output_delivery",
            "0022_provider_session_authority",
        ]
    );
}

// Declared role: validator
fn assert_row_version_column(conn: &Connection, table: &str) {
    let row_version = row_version_column(conn, table);
    assert_row_version_column_info(table, &row_version);
}

// Declared role: accessor
fn row_version_column(conn: &Connection, table: &str) -> ColumnInfo {
    table_info(conn, table)
        .into_iter()
        .find(|column| column.name == "row_version")
        .unwrap_or_else(|| panic!("{table} is missing row_version"))
}

// Declared role: validator
fn assert_row_version_column_info(table: &str, row_version: &ColumnInfo) {
    assert_eq!(
        row_version.ty.to_uppercase(),
        "INTEGER",
        "{table}.row_version must be INTEGER"
    );
    assert_eq!(
        row_version.notnull, 1,
        "{table}.row_version must be NOT NULL"
    );
    assert_eq!(
        row_version.dflt_value.as_deref(),
        Some("0"),
        "{table}.row_version must default to 0"
    );
}

#[derive(Debug)]
struct ColumnInfo {
    name: String,
    ty: String,
    notnull: i64,
    dflt_value: Option<String>,
    pk: i64,
}

// Declared role: accessor
fn table_info(conn: &Connection, table: &str) -> Vec<ColumnInfo> {
    query_table_info(conn, &table_info_sql(table))
}

// Declared role: formatter
fn table_info_sql(table: &str) -> String {
    format!("PRAGMA table_info({table})")
}

// Declared role: accessor
fn query_table_info(conn: &Connection, sql: &str) -> Vec<ColumnInfo> {
    conn.prepare(sql)
        .unwrap()
        .query_map([], column_info)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

// Declared role: mapper
fn column_info(row: &rusqlite::Row<'_>) -> rusqlite::Result<ColumnInfo> {
    Ok(ColumnInfo {
        name: row.get(1)?,
        ty: row.get(2)?,
        notnull: row.get(3)?,
        dflt_value: row.get(4)?,
        pk: row.get(5)?,
    })
}

// Declared role: accessor
fn index_names(conn: &Connection) -> BTreeSet<String> {
    conn.prepare("SELECT name FROM sqlite_master WHERE type = 'index'")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<BTreeSet<_>, _>>()
        .unwrap()
}
