mod fixtures;

use fixtures::{table_names, user_version};
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

    assert_eq!(user_version(&conn), 6);
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

    let conn = migrate_schema5_to_schema6(&db_path);

    for registration in REGISTRY {
        let columns = table_info(&conn, registration.table);
        let column_names = columns
            .iter()
            .map(|column| column.name.as_str())
            .collect::<BTreeSet<_>>();
        let primary_keys = columns
            .iter()
            .filter(|column| column.pk > 0)
            .map(|column| column.name.as_str())
            .collect::<BTreeSet<_>>();
        let payload_columns = columns
            .iter()
            .filter(|column| {
                column.name != "row_version"
                    && !registration
                        .primary_key_columns
                        .contains(&column.name.as_str())
            })
            .map(|column| column.name.as_str())
            .collect::<BTreeSet<_>>();
        let registered_pk = registration
            .primary_key_columns
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        let registered_payload = registration
            .payload_columns
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();

        assert_eq!(
            primary_keys, registered_pk,
            "registry primary keys disagree with PRAGMA table_info for {}",
            registration.table
        );
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
}

fn build_schema5_db(path: &Path) {
    let mut conn = Connection::open(path).unwrap();
    conn.pragma_update(None, "user_version", 0).unwrap();
    let plan = migrations::plan(0, 5).unwrap();
    migrations::run_with_db_path(&mut conn, &plan, path.to_path_buf()).unwrap();
    assert_eq!(user_version(&conn), 5);
}

fn migrate_schema5_to_schema6(path: &Path) -> Connection {
    let mut conn = Connection::open(path).unwrap();
    let plan = migrations::plan(5, 6).unwrap();
    assert_eq!(
        plan.iter()
            .map(|migration| migration.id)
            .collect::<Vec<_>>(),
        vec!["0006_age_58_dual_write_row_versions"]
    );
    migrations::run_with_db_path(&mut conn, &plan, path.to_path_buf()).unwrap();
    conn
}

fn assert_row_version_column(conn: &Connection, table: &str) {
    let row_version = table_info(conn, table)
        .into_iter()
        .find(|column| column.name == "row_version")
        .unwrap_or_else(|| panic!("{table} is missing row_version"));

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

fn table_info(conn: &Connection, table: &str) -> Vec<ColumnInfo> {
    conn.prepare(&format!("PRAGMA table_info({table})"))
        .unwrap()
        .query_map([], |row| {
            Ok(ColumnInfo {
                name: row.get(1)?,
                ty: row.get(2)?,
                notnull: row.get(3)?,
                dflt_value: row.get(4)?,
                pk: row.get(5)?,
            })
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn index_names(conn: &Connection) -> BTreeSet<String> {
    conn.prepare("SELECT name FROM sqlite_master WHERE type = 'index'")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<BTreeSet<_>, _>>()
        .unwrap()
}
