//! ## Declared roles
//! orchestration, accessor, mapper, parser, filter, predicate, validator, formatter
//!
//! ## Intrinsic-surface declarations
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/tests/age_32_migration_boundary.rs
//!     role: intrinsic-surface
//!     Domain: state-db-migration-boundary-test-domain
//!     Owns:
//!       - fixtures::schema4_invocations schema-4 invocation fixture
//!       - fixtures::v3_full_state_db representative state fixture and snapshot helpers
//!       - fixtures::v3_setup_only_db versionless setup fixture
//!       - fixtures::versionless_unrecognized unrecognized-state fixture
//!       - fixtures schema_fingerprint, table_names, user_version, count_rows helpers
//!       - oulipoly_state::migrations manifest and plan APIs
//!       - oulipoly_state::schema compatibility constants and classifier APIs
//!       - oulipoly_state::schema_probe inspect_schema API
//!       - oulipoly_state::StateDb open and connection APIs
//!       - crates/oulipoly-state/src/db.rs legacy ensure_schema helper names and schema-mutating SQL source shape
//!       - rusqlite::Connection fixture inspection surface
//!       - std::collections::{BTreeMap, BTreeSet} expected-table and fingerprint comparison surface
//!       - std::ops::Range migration matrix selection surface
//!       - std::sync::{Mutex, OnceLock} migration-failure test synchronization surface
//!       - tempfile::tempdir database fixture directory surface

mod fixtures;

use fixtures::schema4_invocations::build_schema4_invocation_fixture;
use fixtures::v3_full_state_db::{
    assert_representative_state_rows_preserved, build_current_full_state_db,
    build_v3_full_state_db, fixture_schema_version,
};
use fixtures::v3_setup_only_db::build_versionless_setup_only_db;
use fixtures::versionless_unrecognized::build_versionless_unrecognized_db;
use fixtures::{schema_fingerprint, table_names, user_version};
use oulipoly_state::migrations::{self, Migration};
use oulipoly_state::schema::{
    self, CURRENT_SCHEMA_VERSION, MINIMUM_SUPPORTED_SCHEMA_VERSION, SchemaCompatibility,
};
use oulipoly_state::{StateDb, schema_probe};
use rusqlite::Connection;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::sync::{Mutex, OnceLock};

#[test]
fn ti_01_fresh_state_db_open_sets_current_user_version_and_required_tables() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");

    let db = StateDb::open(&db_path).unwrap();
    let conn = Connection::open(db.path()).unwrap();

    assert_eq!(user_version(&conn), CURRENT_SCHEMA_VERSION);
    for table in [
        "invocations",
        "providers",
        "provider_quotas",
        "provider_quota_windows",
        "memory_nodes",
        "memory_edges",
        "setup_sessions",
        "setup_turns",
        "session_turns",
        "session_chains",
        "session_chain_segments",
    ] {
        assert!(
            table_names(&conn).contains(&table.to_string()),
            "fresh DB is missing required table {table}"
        );
    }
}

#[test]
fn ti_02_ti_23_previous_version_db_migrates_forward_and_preserves_representative_rows() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    build_v3_full_state_db(&db_path);

    let before_conn = Connection::open(&db_path).unwrap();
    let before = assert_representative_state_rows_preserved(&before_conn);
    assert_eq!(before.user_version, MINIMUM_SUPPORTED_SCHEMA_VERSION);
    drop(before_conn);

    let db = StateDb::open(&db_path).unwrap();
    let connection = Connection::open(db.path()).unwrap();
    let after = assert_representative_state_rows_preserved(&connection);

    assert_eq!(after.user_version, CURRENT_SCHEMA_VERSION);
    assert_eq!(after.invocations, before.invocations);
    assert_eq!(after.providers, before.providers);
    assert_eq!(after.provider_quotas, before.provider_quotas);
    assert_eq!(after.provider_quota_windows, before.provider_quota_windows);
    assert_eq!(after.session_turns, before.session_turns);
    assert_eq!(after.session_chains, before.session_chains);
    assert_eq!(after.session_chain_segments, before.session_chain_segments);
    assert_eq!(after.assistant_body, before.assistant_body);
    assert_eq!(after.segment_last_turn_id, before.segment_last_turn_id);
}

#[test]
fn ti_03_current_version_open_is_noop_for_rows_and_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    build_current_full_state_db(&db_path);

    let db = StateDb::open(&db_path).unwrap();
    let connection = Connection::open(db.path()).unwrap();
    let first = fixtures::representative_snapshot(&connection);
    drop(connection);
    drop(db);

    let db = StateDb::open(&db_path).unwrap();
    let connection = Connection::open(db.path()).unwrap();
    let second = fixtures::representative_snapshot(&connection);

    assert_eq!(second, first);
    assert_eq!(duplicate_segment_count(&connection), 1);
    assert_eq!(duplicate_provider_count(&connection), 1);
}

#[test]
fn ti_06_probe_and_classifier_report_migratable_without_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    build_v3_full_state_db(&db_path);
    let before = db_bytes(&db_path);

    let conn =
        Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let compatibility = schema::classify(&conn).unwrap();
    assert!(
        matches!(
            compatibility,
            SchemaCompatibility::Migratable {
                stored: MINIMUM_SUPPORTED_SCHEMA_VERSION
            }
        ),
        "expected migratable compatibility"
    );

    let report = schema_probe::inspect_schema(&conn, db_path.clone()).unwrap();
    assert_eq!(report.user_version as i32, MINIMUM_SUPPORTED_SCHEMA_VERSION);
    assert_eq!(report.tables.get("fresh_continuations"), Some(&false));
    assert!(
        report.migratable,
        "old supported DB must be distinguishable from incompatible old state"
    );
    assert!(!report.compatible);
    drop(conn);

    let after = db_bytes(&db_path);
    assert_eq!(after, before, "read-only probe/classifier mutated DB bytes");
}

#[test]
fn ti_07_ti_22_schema_constants_are_single_source_for_probe_and_fixtures() {
    assert_eq!(fixture_schema_version(), MINIMUM_SUPPORTED_SCHEMA_VERSION);

    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    let db = StateDb::open(&db_path).unwrap();
    let connection = Connection::open(db.path()).unwrap();
    let report = schema_probe::inspect_schema(&connection, db_path).unwrap();

    assert_eq!(report.tables.get("fresh_continuations"), Some(&true));
    assert_eq!(
        report.current_schema_version as i32, CURRENT_SCHEMA_VERSION,
        "schema probe must import oulipoly_state::schema::CURRENT_SCHEMA_VERSION"
    );
    assert_eq!(
        report.minimum_supported_schema_version as i32, MINIMUM_SUPPORTED_SCHEMA_VERSION,
        "schema probe must import oulipoly_state::schema::MINIMUM_SUPPORTED_SCHEMA_VERSION"
    );
}

#[test]
fn ti_08_manifest_contains_embedded_ordered_migration_assets() {
    let manifest = migrations::manifest();

    assert!(!manifest.is_empty(), "migration manifest must not be empty");
    for migration in manifest {
        assert!(migration.target_version > 0);
        assert!(!migration.id.trim().is_empty());
        assert!(!migration.sql.trim().is_empty());
        assert!(
            !migration.sql.contains("crates/oulipoly-state/migrations"),
            "manifest SQL must be embedded content, not a runtime path"
        );
    }
}

#[test]
fn ti_09_ti_36_manifest_versions_are_monotonic_and_match_current_schema() {
    let mut previous = None;
    for migration in migrations::manifest() {
        if let Some(previous) = previous {
            assert!(
                migration.target_version > previous,
                "migration versions must be strictly increasing"
            );
        }
        previous = Some(migration.target_version);
    }

    assert_eq!(
        previous,
        Some(CURRENT_SCHEMA_VERSION),
        "highest compiled migration target must equal CURRENT_SCHEMA_VERSION"
    );
}

#[test]
fn ti_10_migration_plan_applies_only_missing_forward_steps() {
    let plan = migrations::plan(MINIMUM_SUPPORTED_SCHEMA_VERSION, CURRENT_SCHEMA_VERSION).unwrap();

    assert!(
        !plan.is_empty(),
        "v3 to current should require at least one step"
    );
    assert!(
        plan.iter()
            .all(|m| m.target_version > MINIMUM_SUPPORTED_SCHEMA_VERSION)
    );
    assert!(
        plan.iter()
            .all(|m| m.target_version <= CURRENT_SCHEMA_VERSION)
    );
    assert_eq!(
        plan.last().map(|m| m.target_version),
        Some(CURRENT_SCHEMA_VERSION)
    );

    let current_plan = migrations::plan(CURRENT_SCHEMA_VERSION, CURRENT_SCHEMA_VERSION).unwrap();
    assert!(
        current_plan.is_empty(),
        "current-version DB must not replay older migration steps"
    );
}

#[test]
fn ti_10_age_54_schema4_plan_contains_only_schema5_step() {
    let plan = migrations::plan(4, CURRENT_SCHEMA_VERSION).unwrap();

    assert_eq!(
        plan_target_versions(&plan),
        vec![
            5,
            6,
            7,
            8,
            9,
            10,
            11,
            12,
            13,
            14,
            15,
            16,
            CURRENT_SCHEMA_VERSION
        ],
        "schema-4 DBs must take every ordered migration through caller-bound completion authority schema 17"
    );
    assert_eq!(
        plan_ids(&plan),
        vec![
            "0005_invocation_dual_session_ids",
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
        ]
    );
}

#[test]
fn ti_23_age_54_schema4_invocation_fixture_migrates_without_row_loss() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    build_schema4_invocation_fixture(&db_path);
    let before_conn = Connection::open(&db_path).unwrap();
    let before_count = fixtures::count_rows(&before_conn, "invocations");
    drop(before_conn);

    let db = StateDb::open(&db_path).unwrap();
    let connection = Connection::open(db.path()).unwrap();

    assert_eq!(user_version(&connection), CURRENT_SCHEMA_VERSION);
    assert_eq!(
        fixtures::count_rows(&connection, "invocations"),
        before_count
    );
}

#[test]
fn ti_11_fresh_schema_and_migrated_schema_are_structurally_equivalent() {
    let dir = tempfile::tempdir().unwrap();
    let fresh_path = dir.path().join("fresh.db");
    let migrated_path = dir.path().join("migrated.db");

    let fresh = StateDb::open(&fresh_path).unwrap();
    let fresh_connection = Connection::open(fresh.path()).unwrap();
    let fresh_schema = normalized_schema(&fresh_connection);
    drop(fresh_connection);
    drop(fresh);

    build_v3_full_state_db(&migrated_path);
    let migrated = StateDb::open(&migrated_path).unwrap();
    let migrated_connection = Connection::open(migrated.path()).unwrap();
    let migrated_schema = normalized_schema(&migrated_connection);

    assert_eq!(migrated_schema, fresh_schema);
}

#[test]
fn ti_30_versionless_setup_only_db_migrates_to_current_and_preserves_setup_rows() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    build_versionless_setup_only_db(&db_path);

    let db = StateDb::open(&db_path).unwrap();
    let conn = Connection::open(db.path()).unwrap();

    assert_eq!(user_version(&conn), CURRENT_SCHEMA_VERSION);
    assert_eq!(fixtures::count_rows(&conn, "memory_nodes"), 2);
    assert_eq!(fixtures::count_rows(&conn, "memory_edges"), 1);
    assert_eq!(fixtures::count_rows(&conn, "setup_sessions"), 1);
    assert_eq!(fixtures::count_rows(&conn, "setup_turns"), 1);
    let foreign_keys = memory_edge_foreign_keys(&conn);
    assert!(has_required_memory_edge_foreign_keys(&foreign_keys));
}

#[test]
fn ti_35_migrations_are_embedded_and_independent_of_current_working_directory() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    build_v3_full_state_db(&db_path);

    let _guard = cwd_lock().lock().unwrap();
    let cwd = std::env::current_dir().unwrap();
    let other_cwd = tempfile::tempdir().unwrap();
    std::env::set_current_dir(other_cwd.path()).unwrap();
    let result = StateDb::open(&db_path);
    std::env::set_current_dir(cwd).unwrap();

    let db = result.unwrap();
    let connection = Connection::open(db.path()).unwrap();
    assert_eq!(user_version(&connection), CURRENT_SCHEMA_VERSION);
}

#[test]
fn ti_37_state_migrations_stay_rusqlite_embedded_without_runtime_migration_dependencies() {
    let cargo_toml = include_str!("../Cargo.toml");
    for forbidden in ["sqlx", "refinery", "rusqlite_migration"] {
        assert!(
            !cargo_toml.contains(forbidden),
            "AGE-32 contract chose embedded rusqlite migrations; document and review any {forbidden} dependency"
        );
    }
}

#[test]
fn ti_38_versionless_unrecognized_shape_fails_closed_without_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    build_versionless_unrecognized_db(&db_path);
    let before_conn = Connection::open(&db_path).unwrap();
    let before_fingerprint = schema_fingerprint(&before_conn);
    drop(before_conn);

    let err = match StateDb::open(&db_path) {
        Ok(_) => panic!("unrecognized versionless DB unexpectedly opened"),
        Err(err) => err,
    };
    let message = err.to_string();

    assert!(message.contains("agents migrate --rebuild"), "{message}");
    assert!(
        message.contains(db_path.to_string_lossy().as_ref()),
        "{message}"
    );
    let after_conn = Connection::open(&db_path).unwrap();
    assert_eq!(schema_fingerprint(&after_conn), before_fingerprint);
    assert_eq!(user_version(&after_conn), 0);

    let compatibility = schema::classify(&after_conn).unwrap();
    assert!(
        matches!(compatibility, SchemaCompatibility::UnrecognizedVersionless),
        "expected unrecognized versionless compatibility"
    );
}

#[test]
fn ti_40_legacy_repair_helpers_are_allow_listed_and_migration_represented() {
    let db_source = legacy_repair_source();
    assert_legacy_repair_source_uses_runtime_helper_bodies(&db_source);
    let helper_names = find_ensure_schema_helpers(&db_source);
    assert_schema_helpers_are_allow_listed(&helper_names, &allowed_schema_helpers());

    let migration_sql = normalized_migration_sql_corpus();
    assert_schema_mutations_are_migration_represented(
        &db_source,
        &migration_sql,
        &schema_mutation_helpers(),
    );
}

// Declared role: accessor
fn duplicate_segment_count(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM session_chain_segments WHERE chain_id = ?1",
        [fixtures::CHAIN_ID],
        |row| row.get::<_, i64>(0),
    )
    .unwrap()
}

// Declared role: accessor
fn duplicate_provider_count(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM providers WHERE provider_name = ?1",
        [fixtures::PROVIDER_NAME],
        |row| row.get::<_, i64>(0),
    )
    .unwrap()
}

// Declared role: accessor
fn db_bytes(path: &std::path::Path) -> Vec<u8> {
    std::fs::read(path).unwrap()
}

// Declared role: mapper
fn plan_target_versions(plan: &[&Migration]) -> Vec<i32> {
    plan.iter()
        .map(|migration| migration.target_version)
        .collect()
}

// Declared role: mapper
fn plan_ids(plan: &[&Migration]) -> Vec<&'static str> {
    plan.iter().map(|migration| migration.id).collect()
}

// Declared role: mapper
fn allowed_schema_helpers() -> BTreeSet<&'static str> {
    [
        "ensure_invocations_schema",
        "ensure_providers_schema",
        "ensure_session_turns_schema",
        "ensure_provider_quotas_schema",
        "ensure_provider_quotas_topology_schema",
        "ensure_provider_quota_windows_schema",
    ]
    .into_iter()
    .collect()
}

// Declared role: validator
fn assert_schema_helpers_are_allow_listed(helper_names: &[String], allowed: &BTreeSet<&str>) {
    for helper in helper_names {
        assert!(
            allowed.contains(helper.as_str()),
            "new ad hoc schema repair helper {helper} must be represented as an ordered migration"
        );
    }
}

// Declared role: formatter
fn normalized_migration_sql_corpus() -> String {
    let mut migration_sql = String::new();
    for migration in migrations::manifest() {
        migration_sql.push_str(&normalize_sql(migration.sql));
        migration_sql.push('\n');
    }
    migration_sql
}

// Declared role: accessor
fn schema_mutation_helpers() -> [&'static str; 8] {
    [
        "ensure_invocations_schema",
        "ensure_providers_schema",
        "validate_providers_schema",
        "ensure_session_turns_schema",
        "ensure_provider_quotas_schema",
        "ensure_provider_quotas_topology_schema",
        "ensure_provider_quota_windows_schema",
        "backfill_session_chains",
    ]
}

// Declared role: validator
fn assert_schema_mutations_are_migration_represented(
    db_source: &str,
    migration_sql: &str,
    helpers: &[&str],
) {
    for helper in helpers {
        assert_helper_mutations_are_migration_represented(db_source, migration_sql, helper);
    }
}

// Declared role: validator
fn assert_helper_mutations_are_migration_represented(
    db_source: &str,
    migration_sql: &str,
    helper: &str,
) {
    let statements = helper_mutating_sql_statements(db_source, helper);
    assert_helper_statements_are_migration_represented(migration_sql, helper, statements);
}

// Declared role: parser
fn helper_mutating_sql_statements(db_source: &str, helper: &str) -> Vec<String> {
    let body = extract_function_body(db_source, helper);
    mutating_sql_statements(&body)
}

// Declared role: validator
fn assert_helper_statements_are_migration_represented(
    migration_sql: &str,
    helper: &str,
    statements: Vec<String>,
) {
    for statement in statements {
        assert!(
            helper_statement_is_migration_represented(migration_sql, &statement),
            "{helper} contains schema-mutating SQL not represented in compiled migrations: {statement}"
        );
    }
}

// Declared role: formatter
fn helper_statement_is_migration_represented(migration_sql: &str, statement: &str) -> bool {
    migration_sql_contains_normalized_statement(
        migration_sql,
        &normalized_helper_statement(statement),
    )
}

// Declared role: formatter
fn normalized_helper_statement(statement: &str) -> String {
    normalize_sql(statement)
}

// Declared role: predicate
fn migration_sql_contains_normalized_statement(migration_sql: &str, statement: &str) -> bool {
    migration_sql.contains(statement)
}

// Declared role: accessor
fn legacy_repair_source() -> String {
    [
        include_str!("../src/db.rs"),
        include_str!("../src/db/chain_backfill.rs"),
        include_str!("../src/db/invocation_schema_table.rs"),
        include_str!("../src/db/invocation_schema_repair.rs"),
        include_str!("../src/db/invocation_schema_projection.rs"),
        include_str!("../src/db/invocation_schema_session_turns.rs"),
        include_str!("../src/db/invocation_schema_legacy_migration.rs"),
        include_str!("../src/db/opening_read_only.rs"),
        include_str!("../src/db/opening_write.rs"),
        include_str!("../src/db/opening_migrations.rs"),
        include_str!("../src/db/provider_quotas.rs"),
        include_str!("../src/db/provider_schema_migration.rs"),
        include_str!("../src/db/provider_schema_validation.rs"),
    ]
    .join("\n")
}

// Declared role: validator
fn assert_legacy_repair_source_uses_runtime_helper_bodies(source: &str) {
    assert!(
        source.contains("fn apply_current_schema_repairs"),
        "legacy repair validation must inspect the StateDb::open helper bodies"
    );
    assert!(
        source.contains("pub fn backfill_session_chains"),
        "legacy repair validation must inspect the backfill helper body"
    );
}

// Declared role: mapper
fn normalized_schema(conn: &Connection) -> BTreeMap<String, String> {
    schema_rows(conn)
        .into_iter()
        .map(normalized_schema_entry)
        .collect()
}

struct SchemaRow {
    object_type: String,
    name: String,
    table_name: String,
    sql: String,
}

// Declared role: accessor
fn schema_rows(conn: &Connection) -> Vec<SchemaRow> {
    require_schema_rows(read_schema_rows(conn))
}

// Declared role: accessor
fn read_schema_rows(conn: &Connection) -> rusqlite::Result<Vec<SchemaRow>> {
    let mut stmt = conn.prepare(
        "SELECT type, name, tbl_name, COALESCE(sql, '') FROM sqlite_schema
         WHERE name NOT LIKE 'sqlite_%'
         ORDER BY type, name",
    )?;
    stmt.query_map([], schema_row)
        .and_then(|rows| rows.collect::<rusqlite::Result<Vec<_>>>())
}

// Declared role: validator
fn require_schema_rows(result: rusqlite::Result<Vec<SchemaRow>>) -> Vec<SchemaRow> {
    result.unwrap()
}

// Declared role: mapper
fn schema_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SchemaRow> {
    Ok(SchemaRow {
        object_type: row.get(0)?,
        name: row.get(1)?,
        table_name: row.get(2)?,
        sql: row.get(3)?,
    })
}

// Declared role: mapper
fn normalized_schema_entry(row: SchemaRow) -> (String, String) {
    (schema_key(&row), normalize_sql(&row.sql))
}

// Declared role: formatter
fn schema_key(row: &SchemaRow) -> String {
    format!("{}:{}:{}", row.object_type, row.name, row.table_name)
}

type ForeignKeyEdge = (String, String);

// Declared role: accessor
fn memory_edge_foreign_keys(conn: &Connection) -> Vec<ForeignKeyEdge> {
    require_memory_edge_foreign_keys(read_memory_edge_foreign_keys(conn))
}

// Declared role: accessor
fn read_memory_edge_foreign_keys(conn: &Connection) -> rusqlite::Result<Vec<ForeignKeyEdge>> {
    let mut stmt = conn.prepare("PRAGMA foreign_key_list(memory_edges)")?;
    stmt.query_map([], foreign_key_edge)
        .and_then(|rows| rows.collect::<rusqlite::Result<Vec<_>>>())
}

// Declared role: validator
fn require_memory_edge_foreign_keys(
    result: rusqlite::Result<Vec<ForeignKeyEdge>>,
) -> Vec<ForeignKeyEdge> {
    result.unwrap()
}

// Declared role: mapper
fn foreign_key_edge(row: &rusqlite::Row<'_>) -> rusqlite::Result<ForeignKeyEdge> {
    Ok((row.get(2)?, row.get(3)?))
}

// Declared role: predicate
fn has_required_memory_edge_foreign_keys(rows: &[ForeignKeyEdge]) -> bool {
    rows.contains(&memory_node_source_edge()) && rows.contains(&memory_node_target_edge())
}

// Declared role: accessor
fn memory_node_source_edge() -> ForeignKeyEdge {
    ("memory_nodes".to_string(), "source_id".to_string())
}

// Declared role: accessor
fn memory_node_target_edge() -> ForeignKeyEdge {
    ("memory_nodes".to_string(), "target_id".to_string())
}

// Declared role: parser
fn find_ensure_schema_helpers(source: &str) -> Vec<String> {
    source
        .lines()
        .filter_map(parse_ensure_schema_helper_name)
        .collect()
}

// Declared role: parser
fn parse_ensure_schema_helper_name(line: &str) -> Option<String> {
    let rest = ensure_schema_function_tail(line)?;
    let name_tail = function_name_tail(rest)?;
    let name = ensure_schema_name(name_tail);
    is_schema_helper_name(&name).then_some(name)
}

// Declared role: parser
fn ensure_schema_function_tail(line: &str) -> Option<&str> {
    line.trim_start()
        .split_once("fn ensure_")
        .map(|(_, rest)| rest)
}

// Declared role: parser
fn function_name_tail(rest: &str) -> Option<&str> {
    let (name_tail, _) = rest.split_once('(')?;
    Some(name_tail)
}

// Declared role: formatter
fn ensure_schema_name(name_tail: &str) -> String {
    format!("ensure_{name_tail}")
}

// Declared role: predicate
fn is_schema_helper_name(name: &str) -> bool {
    name.ends_with("_schema")
}

// Declared role: parser
fn extract_function_body(source: &str, function_name: &str) -> String {
    let body_range = function_body_range(source, function_name);
    source[body_range].to_string()
}

// Declared role: parser
fn function_body_range(source: &str, function_name: &str) -> Range<usize> {
    let start = function_start(source, function_name);
    let brace_start = opening_brace(source, start);
    let end = closing_brace(source, brace_start);
    brace_start..end
}

// Declared role: parser
fn function_start(source: &str, function_name: &str) -> usize {
    let needle = function_start_needle(function_name);
    require_function_start(function_name, source.find(&needle))
}

// Declared role: formatter
fn function_start_needle(function_name: &str) -> String {
    format!("{function_name}(")
}

// Declared role: validator
fn require_function_start(function_name: &str, start: Option<usize>) -> usize {
    start.unwrap_or_else(|| panic!("missing function {function_name}"))
}

// Declared role: parser
fn opening_brace(source: &str, start: usize) -> usize {
    let offset = require_opening_brace_offset(source[start..].find('{'));
    map_opening_brace_offset(start, offset)
}

// Declared role: validator
fn require_opening_brace_offset(offset: Option<usize>) -> usize {
    offset.unwrap()
}

// Declared role: mapper
fn map_opening_brace_offset(start: usize, offset: usize) -> usize {
    offset + start
}

// Declared role: parser
fn closing_brace(source: &str, brace_start: usize) -> usize {
    let mut depth = 0usize;
    let mut end = brace_start;
    for (offset, ch) in source[brace_start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = brace_start + offset + 1;
                    break;
                }
            }
            _ => {}
        }
    }
    end
}

// Declared role: filter
fn mutating_sql_statements(rust_body: &str) -> Vec<String> {
    filter_schema_mutations(sql_statements_from_body(rust_body))
}

// Declared role: parser
fn sql_statements_from_body(rust_body: &str) -> Vec<String> {
    extract_rust_string_literals(rust_body)
        .into_iter()
        .flat_map(sql_statements_from_literal)
        .collect()
}

// Declared role: filter
fn filter_schema_mutations(statements: Vec<String>) -> Vec<String> {
    statements
        .into_iter()
        .filter(|statement| is_schema_mutation(statement))
        .collect()
}

// Declared role: parser
fn sql_statements_from_literal(literal: String) -> Vec<String> {
    let uncommented = strip_sql_comments(&literal);
    let statements = split_sql_statements(&uncommented);
    let statements = trim_sql_statements(statements);
    owned_sql_statements(statements)
}

// Declared role: parser
fn split_sql_statements(literal: &str) -> Vec<&str> {
    literal.split(';').collect()
}

// Declared role: formatter
fn trim_sql_statements(statements: Vec<&str>) -> Vec<&str> {
    statements.into_iter().map(str::trim).collect()
}

// Declared role: mapper
fn owned_sql_statements(statements: Vec<&str>) -> Vec<String> {
    statements.into_iter().map(str::to_string).collect()
}

// Declared role: parser
fn extract_rust_string_literals(source: &str) -> Vec<String> {
    let bytes = source.as_bytes();
    let mut literals = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'"' {
            let mut literal = String::new();
            index += 1;
            let mut escaped = false;
            while index < bytes.len() {
                let ch = bytes[index] as char;
                if escaped {
                    literal.push(ch);
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    break;
                } else {
                    literal.push(ch);
                }
                index += 1;
            }
            literals.push(literal);
        }
        index += 1;
    }
    literals
}

// Declared role: parser
fn strip_sql_comments(sql: &str) -> String {
    join_sql_lines(sql_comment_prefixes(sql))
}

// Declared role: parser
fn sql_comment_prefixes(sql: &str) -> Vec<&str> {
    sql.lines().map(sql_comment_prefix).collect()
}

// Declared role: parser
fn sql_comment_prefix(line: &str) -> &str {
    line.split_once("--").map_or(line, |(prefix, _)| prefix)
}

// Declared role: formatter
fn join_sql_lines(lines: Vec<&str>) -> String {
    lines.join("\n")
}

// Declared role: predicate
fn is_schema_mutation(statement: &str) -> bool {
    is_normalized_schema_mutation(&normalize_sql(statement))
}

// Declared role: predicate
fn is_normalized_schema_mutation(normalized: &str) -> bool {
    [
        "alter table",
        "create table",
        "create index",
        "drop table",
        "drop column",
    ]
    .iter()
    .any(|prefix| normalized.starts_with(prefix))
        || (normalized.contains("alter table") && normalized.contains(" rename "))
}

// Declared role: formatter
fn normalize_sql(sql: &str) -> String {
    sql.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

// Declared role: accessor
fn cwd_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}
