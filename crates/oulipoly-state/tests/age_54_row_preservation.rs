//! ## Declared roles
//! orchestration, accessor, mapper, parser, filter, predicate, validator, formatter
//!
//! ## Intrinsic-surface declarations
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/tests/age_54_row_preservation.rs
//!     role: intrinsic-surface
//!     Domain: state-db-invocation-row-preservation-test-domain
//!     Owns:
//!       - fixtures::schema4_invocations schema-4 invocation constants and fixture builder
//!       - fixtures::schema5_invocations schema-5 invocation fixture builder
//!       - fixtures::invocation_shapes modern, partial-modern, and unknown-shape fixtures
//!       - fixtures::schema5_drift schema-5 drift fixture builders
//!       - fixtures count_rows, schema_fingerprint, user_version helpers
//!       - oulipoly_state::StateDb open and connection APIs
//!       - oulipoly_state::CURRENT_SCHEMA_VERSION
//!       - rusqlite::{Connection, params} row-inspection support surface
//!       - std::path::Path fixture database path support surface
//!       - tempfile::tempdir database fixture directory surface

mod fixtures;

use fixtures::invocation_shapes::{
    MODERN_SHAPE_UUID, PARTIAL_MODERN_SHAPE_UUID, UNKNOWN_SHAPE_MARKER,
    build_modern_invocations_missing_repair_column, build_modern_invocations_shape,
    build_partial_modern_invocations_shape, build_unknown_populated_invocations_shape,
};
use fixtures::schema4_invocations::{
    PROVIDER_SESSION_A, PROVIDER_SESSION_B, RESUME_INPUT_A, SCHEMA4_CHILD_UUID,
    SCHEMA4_FAILED_UUID, SCHEMA4_LEGACY_UUID, SCHEMA4_NULL_SESSION_UUID, SCHEMA4_RESUMED_UUID,
    SCHEMA4_ROOT_UUID, SCHEMA4_RUNNING_UUID, SCHEMA4_SECOND_PROVIDER_UUID,
    build_schema4_invocation_fixture,
};
use fixtures::schema5_drift::{
    build_current_missing_resume_input_id_column, build_failing_0005_duplicate_column_or_index,
    build_schema4_with_dual_id_columns_or_index_drift,
    build_schema4_with_existing_provider_session_index,
};
use fixtures::schema5_invocations::build_schema5_invocation_fixture;
use fixtures::{count_rows, schema_fingerprint, user_version};
use oulipoly_state::{CURRENT_SCHEMA_VERSION, StateDb};
use rusqlite::{Connection, params};
use std::path::Path;

// Risk: incident regression / blast-radius
// Source: proposal TI-StateDb::open schema-4 fixture; contract observable signals 1, 2, 5
// Level: verifies row-preservation on StateDb::open
#[test]
fn state_db_open_preserves_invocation_row_count_for_schema4_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    build_schema4_invocation_fixture(&db_path);

    let before = invocation_snapshot(&db_path);
    assert_eq!(before.row_count, 8);

    let db = StateDb::open(&db_path).unwrap();
    drop(db);

    let after = invocation_snapshot(&db_path);
    assert_eq!(after.row_count, before.row_count);
    assert_eq!(after.uuids, before.uuids);
    assert_eq!(after.parent_links, before.parent_links);
    assert_eq!(after.user_version, CURRENT_SCHEMA_VERSION);
    assert_dual_id_backfill_matrix(
        &actual_dual_id_backfill_rows(&db_path),
        &expected_dual_id_backfill_rows(),
    );
}

// Risk: repeated trace/open drops further
// Source: proposal TI-StateDb::open idempotent schema-5 DB; contract observable signal 3
// Level: reduces repeated-open row-loss risk
#[test]
fn state_db_open_is_idempotent_for_schema5_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    build_schema5_invocation_fixture(&db_path);

    let before = invocation_snapshot(&db_path);
    let before_schema = schema_fingerprint(&Connection::open(&db_path).unwrap());

    drop(StateDb::open(&db_path).unwrap());
    let after_first = invocation_snapshot(&db_path);
    drop(StateDb::open(&db_path).unwrap());
    let after_second = invocation_snapshot(&db_path);
    let after_schema = schema_fingerprint(&Connection::open(&db_path).unwrap());

    assert_eq!(after_first.row_count, before.row_count);
    assert_eq!(after_first.uuids, before.uuids);
    assert_eq!(after_first.parent_links, before.parent_links);
    assert_eq!(after_first.user_version, CURRENT_SCHEMA_VERSION);
    assert_eq!(after_second, after_first);
    assert_ne!(
        after_schema, before_schema,
        "schema-5 fixtures should be upgraded to the current schema on first open"
    );
    assert_provider_session_index_exists(&invocation_index_names(&db_path));
    assert_dual_id_backfill_matrix(
        &actual_dual_id_backfill_rows(&db_path),
        &expected_dual_id_backfill_rows(),
    );
}

// Risk: duplicate owner drift
// Source: proposal TI-StateDb::open partial schema-5 drift; contract § Fix Design migration ownership
// Level: verifies non-mutating drift outcome
#[test]
fn schema4_dual_id_column_or_index_drift_preserves_rows_or_rolls_back() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    build_schema4_with_dual_id_columns_or_index_drift(&db_path);

    let before = invocation_snapshot(&db_path);
    let before_schema = schema_fingerprint(&Connection::open(&db_path).unwrap());
    let result = StateDb::open(&db_path);
    let after = invocation_snapshot(&db_path);

    assert_eq!(after.row_count, before.row_count);
    assert_eq!(after.uuids, before.uuids);
    match result {
        Ok(db) => {
            let connection = Connection::open(db.path()).unwrap();
            assert_eq!(user_version(&connection), CURRENT_SCHEMA_VERSION);
            assert_provider_session_index_exists(&invocation_index_names(&db_path));
        }
        Err(_) => {
            assert_eq!(after.user_version, 4);
            assert_eq!(
                schema_fingerprint(&Connection::open(&db_path).unwrap()),
                before_schema
            );
        }
    }
}

// Risk: duplicate-system count / AGE-32 ownership rule
// Source: proposal TI-ensure_invocations_schema durable columns; contract § Fix Design AGE-32 rule
// Level: verifies ensure_invocations_schema does not own schema-5 backfill
#[test]
fn current_version_missing_dual_id_column_does_not_drop_invocations() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    build_current_missing_resume_input_id_column(&db_path);

    let before_count = raw_invocation_count(&db_path);
    let before_uuids = invocation_uuids(&db_path);
    let before_columns = table_columns(&db_path, "invocations");
    assert!(
        !before_columns.contains(&"resume_input_id".to_string()),
        "fixture must model a populated current table missing a durable schema-5 column"
    );

    let result = StateDb::open(&db_path);

    let after_count = raw_invocation_count(&db_path);
    let after_uuids = invocation_uuids(&db_path);
    let after_columns = table_columns(&db_path, "invocations");

    assert_eq!(after_count, before_count);
    assert_eq!(after_uuids, before_uuids);
    assert!(
        !after_columns.contains(&"resume_input_id".to_string()),
        "ensure_invocations_schema must not silently add durable schema-5 columns on populated current tables"
    );
    match result {
        Ok(db) => {
            let connection = Connection::open(db.path()).unwrap();
            assert_eq!(user_version(&connection), CURRENT_SCHEMA_VERSION);
        }
        Err(_) => assert_eq!(user_version(&Connection::open(&db_path).unwrap()), 5),
    }
}

// Risk: partially repaired DB repeated-open hazard
// Source: proposal TI-0005 index idempotence; contract § Fix Design 0005 migration guard rails
// Level: verifies row-preserving index-drift outcome
#[test]
fn schema4_existing_provider_session_index_preserves_rows_or_rolls_back() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    build_schema4_with_existing_provider_session_index(&db_path);

    let before = invocation_snapshot(&db_path);
    let result = StateDb::open(&db_path);
    let after = invocation_snapshot(&db_path);

    assert_eq!(after.row_count, before.row_count);
    assert_eq!(after.uuids, before.uuids);
    if result.is_err() {
        assert_eq!(after.user_version, 4);
    }
}

// Risk: partial destructive state
// Source: proposal TI-migrations::run_step rollback; contract § Observable signals rollback
// Level: verifies migration failure rollback
#[test]
fn failing_0005_duplicate_column_rolls_back_without_row_loss() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    build_failing_0005_duplicate_column_or_index(&db_path);

    let before_count = raw_invocation_count(&db_path);
    let before_version = user_version(&Connection::open(&db_path).unwrap());
    let before_columns = table_columns(&db_path, "invocations");

    let result = StateDb::open(&db_path);

    let after_count = raw_invocation_count(&db_path);
    let after_version = user_version(&Connection::open(&db_path).unwrap());
    let after_columns = table_columns(&db_path, "invocations");
    assert_eq!(after_count, before_count);
    if result.is_err() {
        assert_eq!(after_version, before_version);
        assert_eq!(after_columns, before_columns);
    }
}

// Risk: destructive branch misrouting
// Source: proposal TI-ensure_invocations_schema modern DB exclusion; contract observable signal modern no-rebuild
// Level: verifies modern repair avoids legacy rebuild
#[test]
fn modern_invocations_missing_repair_column_does_not_enter_legacy_rebuild() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    build_modern_invocations_missing_repair_column(&db_path);

    let before_count = raw_invocation_count(&db_path);
    let db = StateDb::open(&db_path).expect("modern invocations shape must open successfully");
    let after_count = raw_invocation_count(&db_path);

    assert_eq!(after_count, before_count);
    assert!(invocation_uuids(&db_path).contains(&MODERN_SHAPE_UUID.to_string()));
    let connection = Connection::open(db.path()).unwrap();
    assert_eq!(user_version(&connection), CURRENT_SCHEMA_VERSION);
}

// Risk: accidental table replacement
// Source: proposal TI-migrate_legacy_invocations exact pre-UUID shape; contract observable signal legacy reachable
// Level: verifies allowed legacy rebuild preserves rows
#[test]
fn exact_legacy_pre_uuid_shape_can_rebuild_without_row_loss() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    build_exact_legacy_pre_uuid_shape(&db_path);

    let before_count = raw_invocation_count(&db_path);
    let db = StateDb::open(&db_path).unwrap();
    let connection = Connection::open(db.path()).unwrap();
    let after_count = count_rows(&connection, "invocations");

    assert_eq!(before_count, 2);
    assert_eq!(after_count, before_count);
    assert!(table_columns(&db_path, "invocations").contains(&"invocation_uuid".to_string()));
}

// Risk: row-count collapse
// Source: proposal TI-migrate_legacy_invocations modern/partial fail-closed; contract observable signal no DROP
// Level: verifies modern shapes do not rebuild/regenerate rows
#[test]
fn modern_and_partial_modern_shapes_do_not_rebuild_or_regenerate_rows() {
    {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("state.db");
        build_modern_invocations_shape(&db_path);

        let before_count = raw_invocation_count(&db_path);
        let before_columns = table_columns(&db_path, "invocations");
        StateDb::open(&db_path).expect("modern shape must open successfully without rebuild");
        let after_count = raw_invocation_count(&db_path);
        let after_columns = table_columns(&db_path, "invocations");

        assert_eq!(after_count, before_count);
        assert!(invocation_uuids(&db_path).contains(&MODERN_SHAPE_UUID.to_string()));
        assert!(
            after_columns.contains(&"invocation_uuid".to_string()),
            "modern shape must not be replaced by legacy projection"
        );
        assert!(
            after_columns.len() >= before_columns.len(),
            "open must not drop columns on modern shape"
        );
    }

    {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("state.db");
        build_partial_modern_invocations_shape(&db_path);

        let before_count = raw_invocation_count(&db_path);
        let before_columns = table_columns(&db_path, "invocations");
        let result = StateDb::open(&db_path);
        let after_count = raw_invocation_count(&db_path);
        let after_columns = table_columns(&db_path, "invocations");

        assert!(
            result.is_err(),
            "partial-modern shape must not open successfully (contract: returns Err before any DROP TABLE)"
        );
        assert_eq!(after_count, before_count);
        assert!(invocation_uuids(&db_path).contains(&PARTIAL_MODERN_SHAPE_UUID.to_string()));
        assert!(
            after_columns.contains(&"invocation_uuid".to_string()),
            "partial-modern shape must not be replaced by legacy projection"
        );
        assert!(
            after_columns.len() >= before_columns.len(),
            "open must not drop columns on partial-modern shape"
        );
    }
}

// Risk: row-count collapse
// Source: proposal TI-migrate_legacy_invocations unknown populated fail-closed; contract observable signal no DROP
// Level: verifies unrecognized pre-UUID shape is preserved
#[test]
fn unknown_populated_invocations_shape_fails_closed_before_rebuild() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    build_unknown_populated_invocations_shape(&db_path);

    let before_count = raw_invocation_count(&db_path);
    let before_columns = table_columns(&db_path, "invocations");
    let err = match StateDb::open(&db_path) {
        Ok(_) => panic!("unknown populated shape must fail closed"),
        Err(err) => err,
    };
    let message = err.to_string();

    assert!(
        message.contains("rebuild") || message.contains("invocations"),
        "{message}"
    );
    assert_eq!(raw_invocation_count(&db_path), before_count);
    let after_columns = table_columns(&db_path, "invocations");
    assert!(acceptable_unknown_shape_columns(&before_columns).contains(&after_columns));
    assert_eq!(unexpected_hand_edit_marker(&db_path), UNKNOWN_SHAPE_MARKER);
}

// Risk: row-count guard branch structural drift
// Source: contract § Fix Design row-count guard rails; invocation_schema_legacy_migration::migrate_legacy_invocations
// Level: residual source-shape verification for unreachable mismatch branch
#[test]
fn migrate_legacy_invocations_row_count_guards_abort_before_drop_in_source_shape() {
    let body = migrate_legacy_invocations_body();
    assert_legacy_row_count_guard_body(body);
}

// Declared role: mapper
fn acceptable_unknown_shape_columns(before_columns: &[String]) -> Vec<Vec<String>> {
    let with_row_version = columns_with_row_version(before_columns);
    let with_resolved_account = columns_with_resolved_account(&with_row_version);
    vec![
        before_columns.to_vec(),
        with_row_version.clone(),
        with_resolved_account.clone(),
        columns_with_completion_authority(&with_resolved_account),
    ]
}

// Declared role: mapper
fn columns_with_row_version(columns: &[String]) -> Vec<String> {
    let mut updated = columns.to_vec();
    updated.push("row_version".to_string());
    updated
}

// Declared role: mapper
fn columns_with_resolved_account(columns: &[String]) -> Vec<String> {
    let mut updated = columns.to_vec();
    updated.push("provider_session_resolved_account".to_string());
    updated
}

// Declared role: mapper
fn columns_with_completion_authority(columns: &[String]) -> Vec<String> {
    let mut updated = columns.to_vec();
    updated.push("completion_registration_capability_digest".to_string());
    updated
}

// Declared role: accessor
fn unexpected_hand_edit_marker(path: &Path) -> String {
    let conn = Connection::open(path).unwrap();
    conn.query_row("SELECT unexpected_hand_edit FROM invocations", [], |row| {
        row.get(0)
    })
    .unwrap()
}

// Declared role: parser
fn migrate_legacy_invocations_body() -> &'static str {
    extract_function_body(legacy_migration_source(), "migrate_legacy_invocations")
}

// Declared role: accessor
fn legacy_migration_source() -> &'static str {
    include_str!("../src/db/invocation_schema_legacy_migration.rs")
}

// Declared role: validator
fn assert_legacy_row_count_guard_body(body: &str) {
    assert_legacy_row_count_guard_executable_fragments();
    assert_legacy_row_count_guard_order(&legacy_row_count_guard_offsets(body));
}

// Declared role: validator
fn assert_legacy_row_count_guard_executable_fragments() {
    for guard in legacy_row_count_guard_fragments() {
        let body = extract_function_body(legacy_migration_source(), guard.function_name);
        assert!(
            body.contains(guard.fragment),
            "legacy rebuild helper {} must retain executable fragment {:?}",
            guard.function_name,
            guard.fragment
        );
    }
}

// Declared role: validator
fn assert_legacy_row_count_guard_order(offsets: &LegacyRowCountGuardOffsets) {
    assert!(
        offsets.old_count < offsets.old_scan_guard && offsets.old_scan_guard < offsets.create_new,
        "old scan/count guard must run before invocations_new is created"
    );
    assert!(
        offsets.create_new < offsets.new_count
            && offsets.new_count < offsets.new_count_guard
            && offsets.new_count_guard < offsets.drop_table,
        "migrated row-count guard must abort before DROP TABLE"
    );
}

struct LegacyRowCountGuardOffsets {
    old_count: usize,
    old_scan_guard: usize,
    create_new: usize,
    new_count: usize,
    new_count_guard: usize,
    drop_table: usize,
}

// Declared role: parser
fn legacy_row_count_guard_offsets(body: &str) -> LegacyRowCountGuardOffsets {
    LegacyRowCountGuardOffsets {
        old_count: source_offset(
            body,
            "legacy_invocations_count(&tx)",
            "legacy rebuild must call old invocation count before copy",
        ),
        old_scan_guard: source_offset(
            body,
            "validate_legacy_invocation_scan_count",
            "legacy rebuild must validate old scan/count before replacement",
        ),
        create_new: source_offset(
            body,
            "create_migrated_invocations_table(&tx)",
            "legacy rebuild must create the replacement table after scan guard",
        ),
        new_count: source_offset(
            body,
            "migrated_invocations_count(&tx)",
            "legacy rebuild must count migrated rows before replacement",
        ),
        new_count_guard: source_offset(
            body,
            "validate_migrated_invocation_count",
            "legacy rebuild must validate migrated row count before replacement",
        ),
        drop_table: source_offset(
            body,
            "replace_invocations_with_migrated_table(&tx)",
            "legacy rebuild replacement point must remain explicit",
        ),
    }
}

// Declared role: parser
fn source_offset(source: &str, needle: &str, message: &str) -> usize {
    require_source_offset(find_source_offset(source, needle), message)
}

// Declared role: parser
fn find_source_offset(source: &str, needle: &str) -> Option<usize> {
    source.find(needle)
}

// Declared role: validator
fn require_source_offset(offset: Option<usize>, message: &str) -> usize {
    offset.expect(message)
}

// Declared role: accessor
fn legacy_row_count_guard_fragments() -> [LegacyRowCountGuardFragment; 6] {
    [
        LegacyRowCountGuardFragment {
            function_name: "legacy_invocations_count",
            fragment: "SELECT COUNT(*) FROM invocations",
        },
        LegacyRowCountGuardFragment {
            function_name: "format_legacy_invocation_scan_count_error",
            fragment: "scanned {scanned} rows but table count was {old_count}",
        },
        LegacyRowCountGuardFragment {
            function_name: "create_migrated_invocations_table",
            fragment: "CREATE TABLE invocations_new",
        },
        LegacyRowCountGuardFragment {
            function_name: "migrated_invocations_count",
            fragment: "SELECT COUNT(*) FROM invocations_new",
        },
        LegacyRowCountGuardFragment {
            function_name: "format_migrated_invocation_count_mismatch_error",
            fragment: "migrated {new_count} rows from {old_count}",
        },
        LegacyRowCountGuardFragment {
            function_name: "replace_invocations_with_migrated_table",
            fragment: "DROP TABLE invocations;",
        },
    ]
}

struct LegacyRowCountGuardFragment {
    function_name: &'static str,
    fragment: &'static str,
}

#[derive(Debug, PartialEq, Eq)]
struct InvocationSnapshot {
    user_version: i32,
    row_count: i64,
    uuids: Vec<String>,
    parent_links: Vec<(String, Option<String>)>,
}

// Declared role: accessor
fn invocation_snapshot(path: &Path) -> InvocationSnapshot {
    let conn = Connection::open(path).unwrap();
    invocation_snapshot_from_conn(path, &conn)
}

// Declared role: mapper
fn invocation_snapshot_from_conn(path: &Path, conn: &Connection) -> InvocationSnapshot {
    InvocationSnapshot {
        user_version: user_version(conn),
        row_count: count_rows(conn, "invocations"),
        uuids: invocation_uuids(path),
        parent_links: parent_links(path),
    }
}

// Declared role: accessor
fn raw_invocation_count(path: &Path) -> i64 {
    let conn = Connection::open(path).unwrap();
    count_rows(&conn, "invocations")
}

// Declared role: orchestration
fn invocation_uuids(path: &Path) -> Vec<String> {
    let columns = table_columns(path, "invocations");
    invocation_uuids_for_columns(path, &columns)
}

// Declared role: orchestration
fn invocation_uuids_for_columns(path: &Path, columns: &[String]) -> Vec<String> {
    if !has_invocation_uuid_column(columns) {
        return Vec::new();
    }
    select_invocation_uuids(path)
}

// Declared role: predicate
fn has_invocation_uuid_column(columns: &[String]) -> bool {
    columns.iter().any(|column| column == "invocation_uuid")
}

// Declared role: accessor
fn select_invocation_uuids(path: &Path) -> Vec<String> {
    require_invocation_uuid_rows(read_invocation_uuid_rows(path))
}

// Declared role: accessor
fn read_invocation_uuid_rows(path: &Path) -> rusqlite::Result<Vec<String>> {
    let conn = Connection::open(path)?;
    let mut stmt = conn.prepare("SELECT invocation_uuid FROM invocations ORDER BY id")?;
    stmt.query_map([], invocation_uuid_row)
        .and_then(|rows| rows.collect::<rusqlite::Result<Vec<_>>>())
}

// Declared role: validator
fn require_invocation_uuid_rows(result: rusqlite::Result<Vec<String>>) -> Vec<String> {
    result.unwrap()
}

// Declared role: mapper
fn invocation_uuid_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<String> {
    row.get(0)
}

// Declared role: accessor
fn parent_links(path: &Path) -> Vec<(String, Option<String>)> {
    require_parent_link_rows(read_parent_link_rows(path))
}

// Declared role: accessor
fn read_parent_link_rows(path: &Path) -> rusqlite::Result<Vec<(String, Option<String>)>> {
    let conn = Connection::open(path)?;
    let mut stmt = conn.prepare(parent_links_sql())?;
    stmt.query_map([], parent_link_row)
        .and_then(|rows| rows.collect::<rusqlite::Result<Vec<_>>>())
}

// Declared role: validator
fn require_parent_link_rows(
    result: rusqlite::Result<Vec<(String, Option<String>)>>,
) -> Vec<(String, Option<String>)> {
    result.unwrap()
}

// Declared role: accessor
fn parent_links_sql() -> &'static str {
    "SELECT child.invocation_uuid, parent.invocation_uuid
         FROM invocations child
         LEFT JOIN invocations parent ON parent.id = child.parent_invocation_id
         ORDER BY child.id"
}

// Declared role: mapper
fn parent_link_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(String, Option<String>)> {
    Ok((row.get(0)?, row.get(1)?))
}

// Declared role: accessor
fn table_columns(path: &Path, table: &str) -> Vec<String> {
    let conn = Connection::open(path).unwrap();
    query_table_columns(&conn, &table_info_sql(table))
}

// Declared role: accessor
fn query_table_columns(conn: &Connection, sql: &str) -> Vec<String> {
    require_table_column_rows(read_table_column_rows(conn, sql))
}

// Declared role: accessor
fn read_table_column_rows(conn: &Connection, sql: &str) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(sql)?;
    stmt.query_map([], table_column_name_row)
        .and_then(|rows| rows.collect::<rusqlite::Result<Vec<_>>>())
}

// Declared role: validator
fn require_table_column_rows(result: rusqlite::Result<Vec<String>>) -> Vec<String> {
    result.unwrap()
}

// Declared role: mapper
fn table_column_name_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<String> {
    row.get::<_, String>(1)
}

// Declared role: formatter
fn table_info_sql(table: &str) -> String {
    format!("PRAGMA table_info({table})")
}

// Declared role: validator
fn assert_provider_session_index_exists(indexes: &[String]) {
    assert_contains_provider_session_index(indexes);
}

// Declared role: accessor
fn invocation_index_names(path: &Path) -> Vec<String> {
    require_invocation_index_rows(read_invocation_index_rows(path))
}

// Declared role: accessor
fn read_invocation_index_rows(path: &Path) -> rusqlite::Result<Vec<String>> {
    let conn = Connection::open(path)?;
    let mut stmt = conn.prepare(invocation_index_names_sql())?;
    stmt.query_map([], invocation_index_name_row)
        .and_then(|rows| rows.collect::<rusqlite::Result<Vec<_>>>())
}

// Declared role: validator
fn require_invocation_index_rows(result: rusqlite::Result<Vec<String>>) -> Vec<String> {
    result.unwrap()
}

// Declared role: mapper
fn invocation_index_name_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<String> {
    row.get::<_, String>(0)
}

// Declared role: accessor
fn invocation_index_names_sql() -> &'static str {
    "SELECT name FROM sqlite_schema
             WHERE type = 'index' AND tbl_name = 'invocations'
             ORDER BY name"
}

// Declared role: validator
fn assert_contains_provider_session_index(indexes: &[String]) {
    assert!(
        indexes.contains(&"idx_invocations_provider_provider_session".to_string()),
        "provider-session index missing: {indexes:?}"
    );
}

// Declared role: validator
fn assert_dual_id_backfill_matrix(actual: &[DualIdBackfillRow], expected: &[DualIdBackfillRow]) {
    assert_eq!(actual, expected);
}

type DualIdBackfillRow = (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

// Declared role: accessor
fn actual_dual_id_backfill_rows(path: &Path) -> Vec<DualIdBackfillRow> {
    require_dual_id_backfill_rows(read_dual_id_backfill_rows(path))
}

// Declared role: accessor
fn read_dual_id_backfill_rows(path: &Path) -> rusqlite::Result<Vec<DualIdBackfillRow>> {
    let conn = Connection::open(path)?;
    let mut stmt = conn.prepare(
        "SELECT invocation_uuid, session_id, provider_session_id, resume_input_id,
                    provider_session_capture_method
             FROM invocations ORDER BY id",
    )?;
    stmt.query_map([], dual_id_backfill_row)
        .and_then(|rows| rows.collect::<rusqlite::Result<Vec<_>>>())
}

// Declared role: validator
fn require_dual_id_backfill_rows(
    result: rusqlite::Result<Vec<DualIdBackfillRow>>,
) -> Vec<DualIdBackfillRow> {
    result.unwrap()
}

// Declared role: mapper
fn dual_id_backfill_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DualIdBackfillRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
    ))
}

// Declared role: accessor
fn expected_dual_id_backfill_rows() -> Vec<DualIdBackfillRow> {
    vec![
        (
            SCHEMA4_ROOT_UUID.to_string(),
            Some(PROVIDER_SESSION_A.to_string()),
            Some(PROVIDER_SESSION_A.to_string()),
            None,
            Some("stdout".to_string()),
        ),
        (
            SCHEMA4_CHILD_UUID.to_string(),
            Some(PROVIDER_SESSION_A.to_string()),
            Some(PROVIDER_SESSION_A.to_string()),
            None,
            Some("stdout_json_event".to_string()),
        ),
        (
            SCHEMA4_RESUMED_UUID.to_string(),
            Some(RESUME_INPUT_A.to_string()),
            None,
            Some(RESUME_INPUT_A.to_string()),
            None,
        ),
        (
            SCHEMA4_NULL_SESSION_UUID.to_string(),
            None,
            None,
            None,
            None,
        ),
        (
            SCHEMA4_RUNNING_UUID.to_string(),
            Some(PROVIDER_SESSION_B.to_string()),
            Some(PROVIDER_SESSION_B.to_string()),
            None,
            Some("forced_flag_verified".to_string()),
        ),
        (
            SCHEMA4_FAILED_UUID.to_string(),
            Some("failed-session".to_string()),
            Some("failed-session".to_string()),
            None,
            Some("stdout".to_string()),
        ),
        (
            SCHEMA4_LEGACY_UUID.to_string(),
            Some("legacy-session".to_string()),
            Some("legacy-session".to_string()),
            None,
            None,
        ),
        (
            SCHEMA4_SECOND_PROVIDER_UUID.to_string(),
            Some("other-session".to_string()),
            Some("other-session".to_string()),
            None,
            Some("stdout".to_string()),
        ),
    ]
}

// Declared role: parser
fn extract_function_body<'a>(source: &'a str, name: &str) -> &'a str {
    let span = function_source_span(source, name);
    &source[span]
}

// Declared role: parser
fn function_source_span(source: &str, name: &str) -> std::ops::Range<usize> {
    let start = function_start(source, name);
    let end = function_end(source, start, name);
    start..end
}

// Declared role: parser
fn function_start(source: &str, name: &str) -> usize {
    let signature = function_signature_needle(name);
    require_function_start(name, source.find(&signature))
}

// Declared role: formatter
fn function_signature_needle(name: &str) -> String {
    format!("fn {name}")
}

// Declared role: validator
fn require_function_start(name: &str, start: Option<usize>) -> usize {
    start.unwrap_or_else(|| panic!("missing function {name}"))
}

// Declared role: parser
fn function_end(source: &str, start: usize, name: &str) -> usize {
    let offset = require_function_end_offset(name, function_end_offset(source, start));
    map_function_end(start, offset)
}

// Declared role: parser
fn function_end_offset(source: &str, start: usize) -> Option<usize> {
    source[start..].find(function_end_marker())
}

// Declared role: accessor
fn function_end_marker() -> &'static str {
    "\npub type LegacyProviderNames"
}

// Declared role: validator
fn require_function_end_offset(name: &str, offset: Option<usize>) -> usize {
    offset.unwrap_or_else(|| panic!("missing end marker after function {name}"))
}

// Declared role: mapper
fn map_function_end(start: usize, offset: usize) -> usize {
    start + offset
}

// Declared role: orchestration
fn build_exact_legacy_pre_uuid_shape(path: &Path) {
    let conn = Connection::open(path).unwrap();
    create_exact_legacy_pre_uuid_schema(&conn);
    seed_exact_legacy_pre_uuid_rows(&conn);
}

// Declared role: accessor
fn create_exact_legacy_pre_uuid_schema(conn: &Connection) {
    conn.execute_batch(
        "
        PRAGMA user_version = 5;
        CREATE TABLE invocations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            model_name TEXT NOT NULL,
            provider_index INTEGER NOT NULL,
            success INTEGER NOT NULL,
            exit_code INTEGER NOT NULL,
            error_category TEXT,
            created_at TEXT NOT NULL
        );
        CREATE TABLE providers (
            model_name TEXT NOT NULL,
            provider_name TEXT NOT NULL,
            invocation_count INTEGER NOT NULL DEFAULT 0,
            error_count INTEGER NOT NULL DEFAULT 0,
            last_error TEXT,
            last_error_at TEXT,
            last_invoked_at TEXT,
            PRIMARY KEY (model_name, provider_name)
        );
        CREATE TABLE provider_quotas (
            provider_name TEXT PRIMARY KEY,
            used_percent REAL NOT NULL DEFAULT 0,
            resets_at TEXT,
            calls_since_refresh INTEGER NOT NULL DEFAULT 0,
            refreshed_at TEXT,
            last_empty_refresh_at TEXT,
            exhausted_at TEXT NULL,
            topology_peak_live_window_count INTEGER NOT NULL DEFAULT 0,
            last_topology_probe_at TEXT
        );
        CREATE TABLE provider_quota_windows (
            provider_name TEXT NOT NULL,
            window_id INTEGER NOT NULL,
            used_percent REAL NOT NULL DEFAULT 0,
            resets_at TEXT NOT NULL,
            last_delta_percent REAL,
            last_delta_calls INTEGER,
            PRIMARY KEY (provider_name, window_id)
        );
        CREATE TABLE session_turns (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            provider_name TEXT NOT NULL,
            session_id TEXT NOT NULL,
            turn_id TEXT NOT NULL,
            timestamp TEXT NOT NULL,
            role TEXT NOT NULL,
            parent_turn_id TEXT,
            is_sidechain INTEGER NOT NULL DEFAULT 0,
            is_compaction_boundary INTEGER NOT NULL DEFAULT 0,
            source_file TEXT NOT NULL,
            ingested_at TEXT NOT NULL,
            body TEXT,
            UNIQUE (provider_name, session_id, turn_id)
        );
        CREATE TABLE session_chains (
            chain_id TEXT PRIMARY KEY,
            created_at TEXT NOT NULL,
            last_used_at TEXT NOT NULL,
            model_name TEXT NOT NULL
        );
        CREATE TABLE session_chain_segments (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            chain_id TEXT NOT NULL REFERENCES session_chains(chain_id),
            provider_name TEXT NOT NULL,
            session_id TEXT NOT NULL,
            started_at TEXT NOT NULL,
            ended_at TEXT,
            last_turn_id TEXT,
            transition_reason TEXT NOT NULL
        );
        ",
    )
    .unwrap();
}

// Declared role: accessor
fn seed_exact_legacy_pre_uuid_rows(conn: &Connection) {
    for (model, success, exit_code, created_at) in legacy_pre_uuid_rows() {
        insert_exact_legacy_pre_uuid_row(conn, model, success, exit_code, created_at);
    }
}

// Declared role: accessor
fn legacy_pre_uuid_rows() -> [(&'static str, i64, i64, &'static str); 2] {
    [
        ("fixture-model", 1, 0, "2026-05-04T00:00:00Z"),
        ("missing-model", 0, 7, "2026-05-04T00:01:00Z"),
    ]
}

// Declared role: accessor
fn insert_exact_legacy_pre_uuid_row(
    conn: &Connection,
    model: &str,
    success: i64,
    exit_code: i64,
    created_at: &str,
) {
    conn.execute(
        "INSERT INTO invocations
                (model_name, provider_index, success, exit_code, error_category, created_at)
             VALUES (?1, 0, ?2, ?3, NULL, ?4)",
        params![model, success, exit_code, created_at],
    )
    .unwrap();
}
