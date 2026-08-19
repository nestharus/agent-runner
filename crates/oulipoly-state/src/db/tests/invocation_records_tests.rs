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
    let page = db
        .list_invocation_children_bounded_page_with_cancel(
            root_id,
            2,
            false,
            &oulipoly_core::CancellationToken::new(),
        )
        .unwrap();

    assert_eq!(StateDb::invocation_row_map_count(), 2);
    assert!(page.has_more_children);
    assert!(!page.live_coverage_incomplete);
    assert_eq!(
        invocation_record_uuids(&page.children),
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
fn bounded_invocation_children_prioritize_terminal_ancestor_of_running_descendant() {
    let db = test_db();
    let unrelated_root_id = insert_invocation_fixture(
        &db,
        "20000000-0000-0000-0000-000000000000",
        None,
        "2026-04-17T07:00:00Z",
    );
    for index in 0..512 {
        insert_invocation_fixture(
            &db,
            &format!("20000000-0000-0000-0001-{index:012}"),
            Some(unrelated_root_id),
            "2026-04-17T07:01:00Z",
        );
    }
    let root_id = insert_invocation_fixture(
        &db,
        "21000000-0000-0000-0000-000000000000",
        None,
        "2026-04-17T08:00:00Z",
    );
    let ancestor_id = insert_invocation_fixture(
        &db,
        "23000000-0000-0000-0000-000000000000",
        Some(root_id),
        "2026-04-17T08:01:00Z",
    );
    db.conn
        .execute(
            "UPDATE invocations SET status = 'succeeded' WHERE id = ?1",
            sqlite::params![ancestor_id],
        )
        .unwrap();
    insert_invocation_fixture(
        &db,
        "24000000-0000-0000-0000-000000000000",
        Some(ancestor_id),
        "2026-04-17T08:02:00Z",
    );
    for index in 0..128 {
        let child_id = insert_invocation_fixture(
            &db,
            &format!("22000000-0000-0000-0000-{index:012}"),
            Some(root_id),
            "2026-04-17T08:03:00Z",
        );
        db.conn
            .execute(
                "UPDATE invocations SET status = 'succeeded' WHERE id = ?1",
                sqlite::params![child_id],
            )
            .unwrap();
    }

    StateDb::reset_invocation_row_map_count();
    let children = db
        .list_invocation_children_with_running_descendants_bounded(root_id, 2)
        .unwrap();

    assert_eq!(children.len(), 2);
    assert_eq!(
        children[0].invocation_uuid,
        "23000000-0000-0000-0000-000000000000"
    );
    assert_eq!(StateDb::invocation_row_map_count(), 2);
}

#[test]
fn bounded_invocation_children_report_live_coverage_when_candidate_window_saturates() {
    let db = test_db();
    let root_id = insert_invocation_fixture(
        &db,
        "28000000-0000-0000-0000-000000000000",
        None,
        "2026-04-17T08:00:00Z",
    );
    for index in 0..8 {
        let child_id = insert_invocation_fixture(
            &db,
            &format!("28000000-0000-0000-0001-{index:012}"),
            Some(root_id),
            "2026-04-17T08:01:00Z",
        );
        db.conn
            .execute(
                "UPDATE invocations SET status = 'succeeded' WHERE id = ?1",
                sqlite::params![child_id],
            )
            .unwrap();
    }
    let hidden_ancestor_id = insert_invocation_fixture(
        &db,
        "29000000-0000-0000-0000-000000000000",
        Some(root_id),
        "2026-04-17T08:02:00Z",
    );
    db.conn
        .execute(
            "UPDATE invocations SET status = 'succeeded' WHERE id = ?1",
            sqlite::params![hidden_ancestor_id],
        )
        .unwrap();
    insert_invocation_fixture(
        &db,
        "2a000000-0000-0000-0000-000000000000",
        Some(hidden_ancestor_id),
        "2026-04-17T08:03:00Z",
    );

    let page = db
        .list_invocation_children_with_running_descendants_bounded_page_with_cancel(
            root_id,
            2,
            &oulipoly_core::CancellationToken::new(),
        )
        .unwrap();

    assert_eq!(page.children.len(), 2);
    assert!(page.has_more_children);
    assert!(page.live_coverage_incomplete);
    assert!(
        page.children
            .iter()
            .all(|child| child.id != hidden_ancestor_id)
    );
}

#[test]
fn bounded_invocation_children_report_live_coverage_when_descendant_window_saturates() {
    let db = test_db();
    let root_id = insert_invocation_fixture(
        &db,
        "2b000000-0000-0000-0000-000000000000",
        None,
        "2026-04-17T08:00:00Z",
    );
    let ancestor_id = insert_invocation_fixture(
        &db,
        "2c000000-0000-0000-0000-000000000000",
        Some(root_id),
        "2026-04-17T08:01:00Z",
    );
    db.conn
        .execute(
            "UPDATE invocations SET status = 'succeeded' WHERE id = ?1",
            sqlite::params![ancestor_id],
        )
        .unwrap();
    let mut parent_id = ancestor_id;
    for index in 0..17 {
        let child_id = insert_invocation_fixture(
            &db,
            &format!("2d000000-0000-0000-0000-{index:012}"),
            Some(parent_id),
            "2026-04-17T08:02:00Z",
        );
        if index < 16 {
            db.conn
                .execute(
                    "UPDATE invocations SET status = 'succeeded' WHERE id = ?1",
                    sqlite::params![child_id],
                )
                .unwrap();
        }
        parent_id = child_id;
    }

    let page = db
        .list_invocation_children_with_running_descendants_bounded_page_with_cancel(
            root_id,
            2,
            &oulipoly_core::CancellationToken::new(),
        )
        .unwrap();

    assert_eq!(page.children.len(), 1);
    assert!(!page.has_more_children);
    assert!(page.live_coverage_incomplete);
}

#[test]
fn bounded_invocation_children_report_live_coverage_when_final_child_limit_saturates() {
    let db = test_db();
    let root_id = insert_invocation_fixture(
        &db,
        "2e000000-0000-0000-0000-000000000000",
        None,
        "2026-04-17T08:00:00Z",
    );
    for index in 0..3 {
        let child_id = insert_invocation_fixture(
            &db,
            &format!("2e000000-0000-0000-0001-{index:012}"),
            Some(root_id),
            "2026-04-17T08:01:00Z",
        );
        db.conn
            .execute(
                "UPDATE invocations SET status = 'succeeded' WHERE id = ?1",
                sqlite::params![child_id],
            )
            .unwrap();
    }

    let page = db
        .list_invocation_children_with_running_descendants_bounded_page_with_cancel(
            root_id,
            2,
            &oulipoly_core::CancellationToken::new(),
        )
        .unwrap();

    assert_eq!(page.children.len(), 2);
    assert!(page.has_more_children);
    assert!(page.live_coverage_incomplete);
}

#[test]
fn bounded_invocation_children_report_live_coverage_at_descendant_scan_boundary() {
    let db = test_db();
    let root_id = insert_invocation_fixture(
        &db,
        "2f000000-0000-0000-0000-000000000000",
        None,
        "2026-04-17T08:00:00Z",
    );
    let ancestor_id = insert_invocation_fixture(
        &db,
        "30000000-0000-0000-0000-000000000000",
        Some(root_id),
        "2026-04-17T08:01:00Z",
    );
    db.conn
        .execute(
            "UPDATE invocations SET status = 'succeeded' WHERE id = ?1",
            sqlite::params![ancestor_id],
        )
        .unwrap();
    for index in 0..16 {
        let child_id = insert_invocation_fixture(
            &db,
            &format!("31000000-0000-0000-0000-{index:012}"),
            Some(ancestor_id),
            "2026-04-17T08:02:00Z",
        );
        if index != 0 {
            db.conn
                .execute(
                    "UPDATE invocations SET status = 'succeeded' WHERE id = ?1",
                    sqlite::params![child_id],
                )
                .unwrap();
        }
    }

    let page = db
        .list_invocation_children_with_running_descendants_bounded_page_with_cancel(
            root_id,
            2,
            &oulipoly_core::CancellationToken::new(),
        )
        .unwrap();

    assert_eq!(
        invocation_record_uuids(&page.children),
        vec!["30000000-0000-0000-0000-000000000000"]
    );
    assert!(!page.has_more_children);
    assert!(page.live_coverage_incomplete);
}

#[test]
fn running_descendant_queries_are_root_scoped_indexed_and_sort_free() {
    let db = test_db();
    let mut candidate_statement = db
        .conn
        .prepare(&format!(
            "EXPLAIN QUERY PLAN {}",
            StateDb::running_descendant_candidates_sql()
        ))
        .unwrap();
    let candidate_details = candidate_statement
        .query_map(sqlite::params![1_i64, 8_i64], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(
        candidate_details
            .iter()
            .any(|detail| detail.contains("idx_invocations_parent_running_created")),
        "{candidate_details:?}"
    );
    assert!(
        candidate_details
            .iter()
            .all(|detail| !detail.contains("TEMP B-TREE")),
        "{candidate_details:?}"
    );

    let mut descendant_statement = db
        .conn
        .prepare(&format!(
            "EXPLAIN QUERY PLAN {}",
            StateDb::running_descendant_exists_sql()
        ))
        .unwrap();
    let descendant_details = descendant_statement
        .query_map(sqlite::params![1_i64, 16_i64], |row| {
            row.get::<_, String>(3)
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(
        descendant_details
            .iter()
            .any(|detail| detail.contains("idx_invocations_parent")),
        "{descendant_details:?}"
    );
    assert!(
        descendant_details
            .iter()
            .all(|detail| !detail.contains("idx_invocations_running_parent")),
        "{descendant_details:?}"
    );
    assert!(
        descendant_details
            .iter()
            .all(|detail| !detail.contains("TEMP B-TREE")),
        "{descendant_details:?}"
    );

    let mut overflow_statement = db
        .conn
        .prepare(&format!(
            "EXPLAIN QUERY PLAN {}",
            StateDb::invocation_children_overflow_sql()
        ))
        .unwrap();
    let overflow_details = overflow_statement
        .query_map(sqlite::params![1_i64, 8_i64], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(
        overflow_details
            .iter()
            .any(|detail| detail.contains("idx_invocations_parent")),
        "{overflow_details:?}"
    );
}

#[test]
fn running_descendant_query_interrupts_after_cancellation() {
    let db = test_db();
    let root_id = insert_invocation_fixture(
        &db,
        "25000000-0000-0000-0000-000000000000",
        None,
        "2026-04-17T08:00:00Z",
    );
    let mut parent_id = insert_invocation_fixture(
        &db,
        "26000000-0000-0000-0000-000000000000",
        Some(root_id),
        "2026-04-17T08:01:00Z",
    );
    for index in 0..512 {
        db.conn
            .execute(
                "UPDATE invocations SET status = 'succeeded' WHERE id = ?1",
                sqlite::params![parent_id],
            )
            .unwrap();
        parent_id = insert_invocation_fixture(
            &db,
            &format!("27000000-0000-0000-0000-{index:012}"),
            Some(parent_id),
            "2026-04-17T08:02:00Z",
        );
    }
    let pause = db.pause_invocation_query_progress_for_test();
    let cancellation = oulipoly_core::CancellationToken::new();
    let query_cancellation = cancellation.clone();
    let query = std::thread::spawn(move || {
        db.list_invocation_children_with_running_descendants_bounded_with_cancel(
            root_id,
            200,
            &query_cancellation,
        )
    });
    assert!(
        pause.wait_until_entered(std::time::Duration::from_secs(5)),
        "production invocation query never reached its SQLite progress callback"
    );
    let cancelled_at = std::time::Instant::now();
    cancellation.cancel();
    let result = query.join().unwrap();

    assert_eq!(result.unwrap_err(), "Invocation child lookup cancelled");
    assert!(cancelled_at.elapsed() < std::time::Duration::from_secs(1));
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
