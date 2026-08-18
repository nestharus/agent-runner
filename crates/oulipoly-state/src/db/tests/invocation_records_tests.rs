//! ## Declared roles
//!
//! - validator
//! - mapper
//! - accessor
//!
//! Role set: { validator, mapper, accessor }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/tests/invocation_records_tests.rs
//!     role: intrinsic-surface
//!     Domain: invocation-records-tests-test-fixture
//!     Owns:
//!       - the db test fixture surface this module owns: StateDb-owned temp databases,
//!       -   schema/rows, and concern DTOs it seeds and inspects via `use super::*`
//!       - all StateDb/rusqlite carriers referenced via `use super::*`, subordinate to
//!       -   this fixture domain: StateDb, sqlite, params, Connection, Transaction, Row,
//!       -   Statement, Uuid, and the concern-owned DTOs each test exercises
//! ```

use super::common::*;
use super::*;
#[test]
fn composite_invocation_id_formats_to_marker_line() {
    let composite = CompositeInvocationId {
        source: "fixture-provider".to_string(),
        id: "7ad2916c-38dd-49e6-a1f7-3ef22766ff70".to_string(),
    };
    assert_eq!(
        composite.stderr_line(),
        r#"OULIPOLY_INVOCATION={"source":"fixture-provider","id":"7ad2916c-38dd-49e6-a1f7-3ef22766ff70"}"#
    );
}

#[test]
fn composite_invocation_id_parses_marker_payload() {
    let composite = CompositeInvocationId {
        source: "fixture-provider".to_string(),
        id: "7ad2916c-38dd-49e6-a1f7-3ef22766ff70".to_string(),
    };
    let payload = r#"{"source":"fixture-provider","id":"7ad2916c-38dd-49e6-a1f7-3ef22766ff70"}"#;
    assert_eq!(
        CompositeInvocationId::parse_env_value(payload).unwrap(),
        composite
    );
}

#[test]
fn composite_invocation_id_parses_shell_mangled_env_values() {
    let parsed = CompositeInvocationId::parse_env_value(
        "{source:fixture-provider,id:7ad2916c-38dd-49e6-a1f7-3ef22766ff70}",
    )
    .unwrap();

    assert_eq!(
        parsed,
        CompositeInvocationId {
            source: "fixture-provider".to_string(),
            id: "7ad2916c-38dd-49e6-a1f7-3ef22766ff70".to_string(),
        }
    );
}

#[test]
fn composite_invocation_id_parses_quoted_shell_mangled_env_values() {
    let parsed = CompositeInvocationId::parse_env_value(
        r#"{source:"fixture-provider",id:"7ad2916c-38dd-49e6-a1f7-3ef22766ff70"}"#,
    )
    .unwrap();

    assert_eq!(
        parsed,
        CompositeInvocationId {
            source: "fixture-provider".to_string(),
            id: "7ad2916c-38dd-49e6-a1f7-3ef22766ff70".to_string(),
        }
    );
}

#[test]
fn composite_invocation_id_rejects_malformed_env_values() {
    for raw in [
        "not-json",
        r#"{"source":"fixture-provider"}"#,
        r#"{"source":"fixture-provider","id":"not-a-uuid"}"#,
        r#"{"source":"fixture-provider","id":"7ad2916c-38dd-49e6-a1f7-3ef22766ff70","extra":true}"#,
    ] {
        assert!(
            CompositeInvocationId::parse_env_value(raw).is_err(),
            "{raw}"
        );
    }
}

#[test]
fn invocation_status_formats_each_variant_to_str() {
    for (status, text) in [
        (InvocationStatus::Running, "running"),
        (InvocationStatus::Succeeded, "succeeded"),
        (InvocationStatus::Failed, "failed"),
        (InvocationStatus::Legacy, "legacy"),
    ] {
        assert_eq!(status.as_str(), text);
    }
}

#[test]
fn invocation_status_parses_each_string_to_variant() {
    for (text, status) in [
        ("running", InvocationStatus::Running),
        ("succeeded", InvocationStatus::Succeeded),
        ("failed", InvocationStatus::Failed),
        ("legacy", InvocationStatus::Legacy),
    ] {
        // Inherent contracted API: Option<Self>; FromStr trait surface: Result<Self, _>.
        assert_eq!(InvocationStatus::from_str(text), Some(status));
        assert_eq!(text.parse::<InvocationStatus>().ok(), Some(status));
    }
    assert_eq!(InvocationStatus::from_str("unknown"), None);
    assert!("unknown".parse::<InvocationStatus>().is_err());
}

#[test]
fn get_invocation_by_uuid_returns_matching_and_missing_rows() {
    let db = test_db();
    let start = InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "legacy-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    };
    db.start_invocation(&start).unwrap();
    let running = db
        .get_invocation_by_uuid(&start.invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(running.invocation_uuid, start.invocation_uuid);

    // A legacy row for a model absent from the pushed provider lookup migrates
    // to status='legacy' (empty lookup here — no config discovery).
    let dir = legacy_invocations_db(&[("missing-model", 0, 0, 7, None, "2026-04-17T08:05:00Z")]);
    let migrated = StateDb::open(&dir.path().join("state.db")).unwrap();
    let legacy_uuid = legacy_invocation_uuid(&migrated);
    let legacy = migrated
        .get_invocation_by_uuid(&legacy_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(legacy.status, InvocationStatus::Legacy);
    assert!(
        migrated
            .get_invocation_by_uuid("00000000-0000-0000-0000-000000000000")
            .unwrap()
            .is_none()
    );
}

#[test]
fn list_invocation_children_returns_empty_for_unknown_parent() {
    let db = test_db();

    let children = db.list_invocation_children(999).unwrap();

    assert!(children.is_empty());
}

#[test]
fn list_invocation_children_orders_by_created_at_then_row_id() {
    let db = test_db();
    let root_id = insert_invocation_fixture(
        &db,
        "10000000-0000-0000-0000-000000000000",
        None,
        "2026-04-17T08:00:00Z",
    );
    insert_invocation_fixture(
        &db,
        "30000000-0000-0000-0000-000000000000",
        Some(root_id),
        "2026-04-17T08:02:00Z",
    );
    insert_invocation_fixture(
        &db,
        "20000000-0000-0000-0000-000000000000",
        Some(root_id),
        "2026-04-17T08:01:00Z",
    );
    insert_invocation_fixture(
        &db,
        "40000000-0000-0000-0000-000000000000",
        Some(root_id),
        "2026-04-17T08:01:00Z",
    );

    let children = db.list_invocation_children(root_id).unwrap();
    let ordered = invocation_record_uuids(&children);

    assert_eq!(
        ordered,
        vec![
            "20000000-0000-0000-0000-000000000000",
            "40000000-0000-0000-0000-000000000000",
            "30000000-0000-0000-0000-000000000000",
        ]
    );
}

#[test]
fn bounded_invocation_children_stop_at_the_requested_row_limit() {
    let db = test_db();
    let root_id = insert_invocation_fixture(
        &db,
        "11000000-0000-0000-0000-000000000000",
        None,
        "2026-04-17T08:00:00Z",
    );
    for index in 1..=128 {
        insert_invocation_fixture(
            &db,
            &format!("{index:08x}-0000-0000-0000-000000000000"),
            Some(root_id),
            &format!("2026-04-17T08:{:02}:00Z", index % 60),
        );
    }

    StateDb::reset_invocation_row_map_count();
    let children = db
        .list_invocation_children_bounded(root_id, 2, false)
        .unwrap();

    assert_eq!(StateDb::invocation_row_map_count(), 2);
    assert_eq!(
        invocation_record_uuids(&children),
        vec![
            "0000003c-0000-0000-0000-000000000000",
            "00000078-0000-0000-0000-000000000000",
        ]
    );
}

#[test]
fn bounded_invocation_children_prioritize_running_over_terminal_history() {
    let db = test_db();
    let root_id = insert_invocation_fixture(
        &db,
        "16000000-0000-0000-0000-000000000000",
        None,
        "2026-04-17T08:00:00Z",
    );
    for (uuid, created_at) in [
        (
            "17000000-0000-0000-0000-000000000000",
            "2026-04-17T08:01:00Z",
        ),
        (
            "18000000-0000-0000-0000-000000000000",
            "2026-04-17T08:02:00Z",
        ),
        (
            "19000000-0000-0000-0000-000000000000",
            "2026-04-17T08:03:00Z",
        ),
    ] {
        insert_invocation_fixture(&db, uuid, Some(root_id), created_at);
    }
    db.conn
        .execute(
            "UPDATE invocations SET status = 'succeeded' WHERE invocation_uuid != ?1",
            sqlite::params!["19000000-0000-0000-0000-000000000000"],
        )
        .unwrap();

    let children = db
        .list_invocation_children_bounded(root_id, 1, true)
        .unwrap();

    assert_eq!(
        invocation_record_uuids(&children),
        vec!["19000000-0000-0000-0000-000000000000"]
    );
}

#[test]
fn running_first_bounded_query_uses_the_projection_index_without_a_temp_sort() {
    let db = test_db();
    let sql = StateDb::invocation_record_select_sql(
        &db.conn,
        "WHERE parent_invocation_id = ?1
         ORDER BY (status = 'running') DESC, created_at, id
         LIMIT ?2",
    )
    .unwrap();
    let mut statement = db
        .conn
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .unwrap();
    let details = statement
        .query_map(sqlite::params![1_i64, 2_i64], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert!(
        details
            .iter()
            .any(|detail| detail.contains("idx_invocations_parent_running_created")),
        "{details:?}"
    );
    assert!(
        details.iter().all(|detail| !detail.contains("TEMP B-TREE")),
        "{details:?}"
    );
}

#[test]
fn list_invocation_children_returns_only_direct_children() {
    let db = test_db();
    let root_id = insert_invocation_fixture(
        &db,
        "50000000-0000-0000-0000-000000000000",
        None,
        "2026-04-17T08:00:00Z",
    );
    let child_id = insert_invocation_fixture(
        &db,
        "60000000-0000-0000-0000-000000000000",
        Some(root_id),
        "2026-04-17T08:01:00Z",
    );
    insert_invocation_fixture(
        &db,
        "70000000-0000-0000-0000-000000000000",
        Some(child_id),
        "2026-04-17T08:02:00Z",
    );
    insert_invocation_fixture(
        &db,
        "80000000-0000-0000-0000-000000000000",
        Some(root_id),
        "2026-04-17T08:03:00Z",
    );

    let children = db.list_invocation_children(root_id).unwrap();
    let uuids = invocation_record_uuids(&children);

    assert_eq!(
        uuids,
        vec![
            "60000000-0000-0000-0000-000000000000",
            "80000000-0000-0000-0000-000000000000",
        ]
    );
}

fn invocation_record_uuids(records: &[InvocationRecord]) -> Vec<&str> {
    records
        .iter()
        .map(|record| record.invocation_uuid.as_str())
        .collect()
}
