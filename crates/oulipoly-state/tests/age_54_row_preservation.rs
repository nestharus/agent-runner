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
use oulipoly_state::StateDb;
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
    assert_eq!(after.user_version, 5);
    assert_dual_id_backfill_matrix(&db_path);
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

    assert_eq!(after_first, before);
    assert_eq!(after_second, before);
    assert_eq!(after_schema, before_schema);
    assert_provider_session_index_exists(&db_path);
    assert_dual_id_backfill_matrix(&db_path);
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
            assert_eq!(user_version(db.connection()), 5);
            assert_provider_session_index_exists(&db_path);
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
    let before_schema = schema_fingerprint(&Connection::open(&db_path).unwrap());
    assert!(
        !before_columns.contains(&"resume_input_id".to_string()),
        "fixture must model a populated current table missing a durable schema-5 column"
    );

    let result = StateDb::open(&db_path);

    let after_count = raw_invocation_count(&db_path);
    let after_uuids = invocation_uuids(&db_path);
    let after_columns = table_columns(&db_path, "invocations");
    let after_schema = schema_fingerprint(&Connection::open(&db_path).unwrap());

    assert_eq!(after_count, before_count);
    assert_eq!(after_uuids, before_uuids);
    assert_eq!(
        after_schema, before_schema,
        "current-version dual-id drift must fail closed or no-op; schema repair belongs to ordered migrations"
    );
    assert!(
        !after_columns.contains(&"resume_input_id".to_string()),
        "ensure_invocations_schema must not silently add durable schema-5 columns on populated current tables"
    );
    match result {
        Ok(db) => assert_eq!(user_version(db.connection()), 5),
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
    assert_eq!(user_version(db.connection()), 5);
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
    let after_count = count_rows(db.connection(), "invocations");

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
    let before_fingerprint = schema_fingerprint(&Connection::open(&db_path).unwrap());

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
    assert_eq!(table_columns(&db_path, "invocations"), before_columns);
    assert_eq!(
        schema_fingerprint(&Connection::open(&db_path).unwrap()),
        before_fingerprint
    );
    let conn = Connection::open(&db_path).unwrap();
    let marker: String = conn
        .query_row("SELECT unexpected_hand_edit FROM invocations", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(marker, UNKNOWN_SHAPE_MARKER);
}

// Risk: row-count guard branch structural drift
// Source: contract § Fix Design row-count guard rails; db.rs::migrate_legacy_invocations
// Level: residual source-shape verification for unreachable mismatch branch
#[test]
fn migrate_legacy_invocations_row_count_guards_abort_before_drop_in_source_shape() {
    let source = include_str!("../src/db.rs");
    let body = extract_function_body(source, "migrate_legacy_invocations");

    let old_count = body
        .find("SELECT COUNT(*) FROM invocations")
        .expect("legacy rebuild must count old invocations before copy");
    let old_scan_guard = body
        .find("scanned {} rows but table count was {old_count}")
        .expect("legacy rebuild must abort on old scan/count mismatch");
    let create_new = body
        .find("CREATE TABLE invocations_new")
        .expect("legacy rebuild must create the replacement table after scan guard");
    let new_count = body
        .find("SELECT COUNT(*) FROM invocations_new")
        .expect("legacy rebuild must count migrated rows before replacement");
    let new_count_guard = body
        .find("migrated {new_count} rows from {old_count}")
        .expect("legacy rebuild must abort on migrated row-count mismatch");
    let drop_table = body
        .find("DROP TABLE invocations;")
        .expect("legacy rebuild replacement point must remain explicit");

    assert!(
        old_count < old_scan_guard && old_scan_guard < create_new,
        "old scan/count guard must run before invocations_new is created"
    );
    assert!(
        create_new < new_count && new_count < new_count_guard && new_count_guard < drop_table,
        "migrated row-count guard must abort before DROP TABLE"
    );
}

#[derive(Debug, PartialEq, Eq)]
struct InvocationSnapshot {
    user_version: i32,
    row_count: i64,
    uuids: Vec<String>,
    parent_links: Vec<(String, Option<String>)>,
}

fn invocation_snapshot(path: &Path) -> InvocationSnapshot {
    let conn = Connection::open(path).unwrap();
    InvocationSnapshot {
        user_version: user_version(&conn),
        row_count: count_rows(&conn, "invocations"),
        uuids: invocation_uuids(path),
        parent_links: parent_links(path),
    }
}

fn raw_invocation_count(path: &Path) -> i64 {
    let conn = Connection::open(path).unwrap();
    count_rows(&conn, "invocations")
}

fn invocation_uuids(path: &Path) -> Vec<String> {
    let conn = Connection::open(path).unwrap();
    if !table_columns(path, "invocations").contains(&"invocation_uuid".to_string()) {
        return Vec::new();
    }
    conn.prepare("SELECT invocation_uuid FROM invocations ORDER BY id")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn parent_links(path: &Path) -> Vec<(String, Option<String>)> {
    let conn = Connection::open(path).unwrap();
    conn.prepare(
        "SELECT child.invocation_uuid, parent.invocation_uuid
         FROM invocations child
         LEFT JOIN invocations parent ON parent.id = child.parent_invocation_id
         ORDER BY child.id",
    )
    .unwrap()
    .query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
    })
    .unwrap()
    .collect::<Result<Vec<_>, _>>()
    .unwrap()
}

fn table_columns(path: &Path, table: &str) -> Vec<String> {
    let conn = Connection::open(path).unwrap();
    conn.prepare(&format!("PRAGMA table_info({table})"))
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn assert_provider_session_index_exists(path: &Path) {
    let conn = Connection::open(path).unwrap();
    let indexes: Vec<String> = conn
        .prepare(
            "SELECT name FROM sqlite_schema
             WHERE type = 'index' AND tbl_name = 'invocations'
             ORDER BY name",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(
        indexes.contains(&"idx_invocations_provider_provider_session".to_string()),
        "provider-session index missing: {indexes:?}"
    );
}

fn assert_dual_id_backfill_matrix(path: &Path) {
    type DualIdBackfillRow = (
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    );

    let conn = Connection::open(path).unwrap();
    let actual: Vec<DualIdBackfillRow> = conn
        .prepare(
            "SELECT invocation_uuid, session_id, provider_session_id, resume_input_id,
                    provider_session_capture_method
             FROM invocations ORDER BY id",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let expected = vec![
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
    ];
    assert_eq!(actual, expected);
}

fn extract_function_body<'a>(source: &'a str, name: &str) -> &'a str {
    let signature = format!("fn {name}");
    let start = source.find(&signature).unwrap_or_else(|| {
        panic!("missing function {name}");
    });
    let end_marker = "\n    /// Resolve `(model_name, provider_index) -> provider_name`";
    let end = source[start..]
        .find(end_marker)
        .map(|offset| start + offset)
        .unwrap_or_else(|| panic!("missing end marker after function {name}"));
    &source[start..end]
}

fn build_exact_legacy_pre_uuid_shape(path: &Path) {
    let conn = Connection::open(path).unwrap();
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
    for (model, success, exit_code, created_at) in [
        ("fixture-model", 1, 0, "2026-05-04T00:00:00Z"),
        ("missing-model", 0, 7, "2026-05-04T00:01:00Z"),
    ] {
        conn.execute(
            "INSERT INTO invocations
                (model_name, provider_index, success, exit_code, error_category, created_at)
             VALUES (?1, 0, ?2, ?3, NULL, ?4)",
            params![model, success, exit_code, created_at],
        )
        .unwrap();
    }
}
