//! ## Declared roles
//! orchestration, accessor, mapper, validator, filter, formatter
//!
//! ## Intrinsic-surface declarations
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/tests/age_123_resume_provider_identity.rs
//!     role: intrinsic-surface
//!     Domain: state-db-resume-provider-identity-test-domain
//!     Owns:
//!       - fresh_schema_has_provider_session_resolved_account_on_invocations orchestration and validation body
//!       - migration_0007_adds_identity_column_and_preserves_rows orchestration and validation body
//!       - bind_invocation_provider_session_start_persists_resolved_account_identity orchestration and validation body
//!       - row_version_payload_includes_provider_session_resolved_account orchestration and validation body
//!       - build_schema6_invocation_fixture migration fixture builder
//!       - invocation_columns and historical_identity_values accessors
//!       - invocation_payload_snapshot and payload_values row mappers
//!       - registration row-version registry filter lookup
//!       - oulipoly_state::deployment::row_version checksum and registry APIs
//!       - oulipoly_state migrations, InvocationStart, ProviderSessionBinding, StateDb
//!       - rusqlite::Connection and rusqlite::types::Value helper surface

mod fixtures;

use fixtures::schema5_invocations::build_schema5_invocation_fixture;
use fixtures::user_version;
use oulipoly_state::deployment::row_version::checksum::payload_hash_for_columns;
use oulipoly_state::deployment::row_version::registry::{REGISTRY, TableRegistration};
use oulipoly_state::migrations;
use oulipoly_state::{InvocationStart, ProviderSessionBinding, StateDb};
use rusqlite::Connection;
use rusqlite::types::Value;
use std::path::Path;

#[derive(Debug, PartialEq, Eq)]
struct ColumnInfo {
    name: String,
    column_type: String,
    not_null: bool,
}

#[test]
fn fresh_schema_has_provider_session_resolved_account_on_invocations() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    let db = StateDb::open(&db_path).unwrap();

    assert_eq!(user_version(db.connection()), 7);
    let columns = invocation_columns(db.connection());
    let column = columns
        .iter()
        .find(|column| column.name == "provider_session_resolved_account")
        .expect("fresh invocations schema must include AGE-123 identity column");

    assert_eq!(column.column_type, "TEXT");
    assert!(!column.not_null, "AGE-123 identity column must be nullable");
}

#[test]
fn migration_0007_adds_identity_column_and_preserves_rows() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    build_schema6_invocation_fixture(&db_path);

    let before_conn = Connection::open(&db_path).unwrap();
    assert_eq!(user_version(&before_conn), 6);
    let before_rows = invocation_payload_snapshot(&before_conn);
    drop(before_conn);

    let db = StateDb::open(&db_path).unwrap();
    assert_eq!(user_version(db.connection()), 7);
    let after_rows = invocation_payload_snapshot(db.connection());
    assert_eq!(after_rows, before_rows);

    let historical_identities = historical_identity_values(db.connection());
    assert!(!historical_identities.is_empty());
    assert!(
        historical_identities.iter().all(Option::is_none),
        "migration must leave historical rows with NULL provider_session_resolved_account"
    );
}

#[test]
fn bind_invocation_provider_session_start_persists_resolved_account_identity() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    let db = StateDb::open(&db_path).unwrap();
    let start = InvocationStart {
        invocation_uuid: "12345678-1234-4234-8234-123456789abc".to_string(),
        model_name: "claude-opus".to_string(),
        provider_name: "claude-a".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    };
    let id = db.start_invocation(&start).unwrap();

    db.bind_invocation_provider_session_start(
        id,
        &ProviderSessionBinding {
            provider_session_id: "provider-session-a".to_string(),
            capture_method: "resumed",
            resume_input_id: Some("raw-resume-input".to_string()),
            provider_session_resolved_account: Some("/tmp/age123/claude-a-projects".to_string()),
        },
    )
    .unwrap();

    let row = db
        .get_invocation_by_uuid(&start.invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(
        row.provider_session_id.as_deref(),
        Some("provider-session-a")
    );
    assert_eq!(row.resume_input_id.as_deref(), Some("raw-resume-input"));
    assert_eq!(
        row.provider_session_capture_method.as_deref(),
        Some("resumed")
    );
    assert_eq!(
        row.provider_session_resolved_account.as_deref(),
        Some("/tmp/age123/claude-a-projects")
    );
}

#[test]
fn row_version_payload_includes_provider_session_resolved_account() {
    let invocations = registration("invocations");
    assert!(
        invocations
            .payload_columns
            .contains(&"provider_session_resolved_account"),
        "AGE-123 identity must be part of durable invocation row-version payload"
    );
    assert_eq!(
        registration("session_chain_segments").payload_columns,
        &[
            "chain_id",
            "provider_name",
            "session_id",
            "started_at",
            "ended_at",
            "last_turn_id",
            "transition_reason",
        ],
        "AGE-123 must not change session_chain_segments payload"
    );

    let source_hash = payload_hash_for_columns(&payload_values(
        invocations,
        "/tmp/age123/claude-a-projects",
    ));
    let target_hash = payload_hash_for_columns(&payload_values(
        invocations,
        "/tmp/age123/claude-b-projects",
    ));
    assert_ne!(
        source_hash, target_hash,
        "invocation payload hash must change when only resolved identity changes"
    );
}

fn build_schema6_invocation_fixture(path: &Path) {
    build_schema5_invocation_fixture(path);
    let mut conn = Connection::open(path).unwrap();
    let plan = migrations::plan(5, 6).unwrap();
    assert_eq!(
        plan.iter()
            .map(|migration| migration.id)
            .collect::<Vec<_>>(),
        vec!["0006_age_58_dual_write_row_versions"]
    );
    migrations::run_with_db_path(&mut conn, &plan, path.to_path_buf()).unwrap();
    assert_eq!(user_version(&conn), 6);
}

fn invocation_columns(conn: &Connection) -> Vec<ColumnInfo> {
    let mut stmt = conn.prepare("PRAGMA table_info(invocations)").unwrap();
    stmt.query_map([], |row| {
        Ok(ColumnInfo {
            name: row.get(1)?,
            column_type: row.get(2)?,
            not_null: row.get::<_, i64>(3)? != 0,
        })
    })
    .unwrap()
    .collect::<Result<Vec<_>, _>>()
    .unwrap()
}

fn invocation_payload_snapshot(conn: &Connection) -> Vec<Vec<Value>> {
    let mut stmt = conn
        .prepare(
            "SELECT invocation_uuid, model_name, provider_name, provider_index,
                    parent_invocation_id, status, success, exit_code, error_category,
                    terminal_reason, session_id, session_capture_method,
                    resume_acceptance_status, resume_acceptance_evidence, created_at,
                    finished_at, provider_session_id, resume_input_id,
                    provider_session_capture_method
             FROM invocations
             ORDER BY id",
        )
        .unwrap();
    stmt.query_map([], |row| {
        (0..row.as_ref().column_count())
            .map(|index| row.get(index))
            .collect::<rusqlite::Result<Vec<Value>>>()
    })
    .unwrap()
    .collect::<Result<Vec<_>, _>>()
    .unwrap()
}

fn historical_identity_values(conn: &Connection) -> Vec<Option<String>> {
    let mut stmt = conn
        .prepare(
            "SELECT provider_session_resolved_account
             FROM invocations
             ORDER BY id",
        )
        .unwrap();
    stmt.query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn registration(table: &str) -> &'static TableRegistration {
    REGISTRY
        .iter()
        .find(|registration| registration.table == table)
        .unwrap_or_else(|| panic!("missing row-version registration for {table}"))
}

fn payload_values(registration: &TableRegistration, resolved_account: &str) -> Vec<Option<Value>> {
    registration
        .payload_columns
        .iter()
        .map(|column| match *column {
            "provider_session_resolved_account" => Some(Value::Text(resolved_account.to_string())),
            "invocation_uuid" => Some(Value::Text(
                "12345678-1234-4234-8234-123456789abc".to_string(),
            )),
            "model_name" => Some(Value::Text("claude-opus".to_string())),
            "provider_name" => Some(Value::Text("claude-a".to_string())),
            "provider_index" => Some(Value::Integer(0)),
            "status" => Some(Value::Text("running".to_string())),
            "created_at" => Some(Value::Text("2026-05-17T00:00:00Z".to_string())),
            _ => Some(Value::Null),
        })
        .collect()
}
