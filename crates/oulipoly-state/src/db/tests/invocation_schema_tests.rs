//! ## Declared roles
//!
//! - validator
//! - predicate
//! - mapper
//! - accessor
//!
//! Role set: { validator, predicate, mapper, accessor }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/tests/invocation_schema_tests.rs
//!     role: intrinsic-surface
//!     Domain: invocation-schema-tests-persistence
//!     Owns:
//!       - StateDb invocation-schema-tests persistence surface: the StateDb methods, owned
//!         tables/rows, and SQL this concern extends, split out of the StateDb
//!         facade by the WU #65 decomposition with the public API preserved
//!       - Intrinsic StateDb/rusqlite carriers and concern-owned DTOs referenced
//!         via `use super::*`, subordinate to this domain: FROM, invocation_column_notnull, invocation_columns, invocation_table_sql, invocation_terminal_reason_for_model, legacy_invocations_db, mark_current_schema_version, test_db
//! ```

use super::common::*;
use super::*;
#[test]
fn schema_creation() {
    let db = test_db();
    let sql = invocation_table_sql(&db);
    assert!(sql.contains("invocation_uuid TEXT NOT NULL UNIQUE"));
    assert!(sql.contains("provider_name TEXT"));
    assert!(sql.contains("parent_invocation_id INTEGER REFERENCES invocations(id)"));
    assert!(sql.contains(
        "status TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed', 'legacy'))"
    ));
    assert!(sql.contains("success INTEGER"));
    assert!(sql.contains("finished_at TEXT"));
    assert!(sql.contains("session_id TEXT"));
    assert!(sql.contains("session_capture_method TEXT"));
    assert!(sql.contains("resume_acceptance_status TEXT"));
    assert!(sql.contains("resume_acceptance_evidence TEXT"));

    let indexes = invocation_index_names(&db);
    assert_eq!(
        indexes,
        vec![
            "idx_invocations_parent".to_string(),
            "idx_invocations_provider_created".to_string(),
            "idx_invocations_provider_provider_session".to_string(),
            "idx_invocations_provider_session".to_string(),
            "idx_invocations_uuid".to_string(),
            "sqlite_autoindex_invocations_1".to_string(),
        ]
    );
}

#[test]
fn t_schema_fresh_invocations_schema_includes_nullable_terminal_reason() {
    let db = test_db();
    let columns = invocation_columns(&db);

    assert!(
        has_column(&columns, "terminal_reason"),
        "fresh invocations schema must expose terminal_reason: {columns:?}"
    );

    let nullable = invocation_column_notnull(&db, "terminal_reason");
    assert_eq!(nullable, 0, "terminal_reason must be nullable");
}

#[test]
fn t_schema_incremental_adds_terminal_reason_without_losing_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let conn = sqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE invocations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                invocation_uuid TEXT NOT NULL UNIQUE,
                model_name TEXT NOT NULL,
                provider_name TEXT,
                provider_index INTEGER NOT NULL,
                parent_invocation_id INTEGER REFERENCES invocations(id),
                status TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed', 'legacy')),
                success INTEGER,
                exit_code INTEGER,
                error_category TEXT,
                session_id TEXT,
                session_capture_method TEXT,
                resume_acceptance_status TEXT,
                resume_acceptance_evidence TEXT,
                created_at TEXT NOT NULL,
                finished_at TEXT
            );
            INSERT INTO invocations (
                invocation_uuid, model_name, provider_name, provider_index,
                status, success, exit_code, error_category, created_at, finished_at
            ) VALUES (
                '11111111-1111-1111-1111-111111111111',
                'fixture-model', 'fixture-provider', 0,
                'failed', 0, 7, 'fixture_error',
                '2026-04-17T08:00:00Z', '2026-04-17T08:00:01Z'
            );",
    )
    .unwrap();
    mark_current_schema_version(&conn);
    drop(conn);

    let db = StateDb::open(&path).unwrap();
    let columns = invocation_columns(&db);
    assert!(
        has_column(&columns, "terminal_reason"),
        "incremental migration must add terminal_reason: {columns:?}"
    );

    let row = db
        .get_invocation_by_uuid("11111111-1111-1111-1111-111111111111")
        .unwrap()
        .unwrap();
    assert_eq!(row.status, InvocationStatus::Failed);
    assert_eq!(row.exit_code, Some(7));
    assert_eq!(row.error_category.as_deref(), Some("fixture_error"));
    assert_eq!(row.terminal_reason, None);
}

#[test]
fn t_schema_legacy_rebuild_adds_terminal_reason_and_migrates_null() {
    let dir = legacy_invocations_db(&[(
        "mapped-model",
        0,
        0,
        7,
        Some("rate_limit"),
        "2026-04-17T08:00:00Z",
    )]);

    let db = StateDb::open(&dir.path().join("state.db")).unwrap();
    let columns = invocation_columns(&db);
    assert!(
        has_column(&columns, "terminal_reason"),
        "legacy rebuild must add terminal_reason: {columns:?}"
    );

    let terminal_reason = invocation_terminal_reason_for_model(&db, "mapped-model");
    assert_eq!(terminal_reason, None);
}

#[test]
fn update_resume_acceptance_persists_status_and_evidence() {
    let db = test_db();
    let start = InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "test-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    };
    let id = db.start_invocation(&start).unwrap();

    db.update_resume_acceptance(id, "accepted", Some("matched session id"))
        .unwrap();

    let row = db
        .get_invocation_by_uuid(&start.invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(row.resume_acceptance_status.as_deref(), Some("accepted"));
    assert_eq!(
        row.resume_acceptance_evidence.as_deref(),
        Some("matched session id")
    );
}

fn invocation_index_names(db: &StateDb) -> Vec<String> {
    db.conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'invocations' ORDER BY name")
        .unwrap()
        .query_map([], invocation_index_name_row)
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn invocation_index_name_row(row: &sqlite::Row<'_>) -> sqlite::Result<String> {
    row.get(0)
}

fn has_column(columns: &[String], expected: &str) -> bool {
    columns.iter().any(|column| column == expected)
}
