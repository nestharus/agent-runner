//! ## Declared roles
//!
//! - accessor
//! - formatter
//! - mapper
//! - orchestration
//! - validator
//!
//! Role set: { accessor, formatter, mapper, orchestration, validator }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/tests/session_turn_tests_1.rs
//!     role: intrinsic-surface
//!     Domain: session-turn-tests-1-test-fixture
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
fn ingest_session_turns_batch_persists_parent_and_sidechain_columns() {
    let (inserted, parent_turn_id, is_sidechain) = persisted_parent_sidechain_row();
    assert_eq!(inserted, 1);
    assert_eq!(parent_turn_id.as_deref(), Some("root-turn"));
    assert_eq!(is_sidechain, 1);
}

fn persisted_parent_sidechain_row() -> (u64, Option<String>, i64) {
    let db = test_db();
    let inserted = ingest_parent_sidechain_fixture_turn(&db);
    let (parent_turn_id, is_sidechain) = parent_sidechain_persisted_row(&db);
    (inserted, parent_turn_id, is_sidechain)
}

fn ingest_parent_sidechain_fixture_turn(db: &StateDb) -> u64 {
    db.ingest_session_turns_batch("fixture-provider", &[parent_sidechain_fixture_turn()])
        .unwrap()
}

fn parent_sidechain_fixture_turn() -> SessionTurnIngest {
    SessionTurnIngest {
        session_id: "session-a".to_string(),
        turn_id: "child-turn".to_string(),
        timestamp: ts("2026-04-17T08:00:01Z"),
        role: "assistant".to_string(),
        parent_turn_id: Some("root-turn".to_string()),
        is_sidechain: true,
        is_compaction_boundary: false,
        body: None,
    }
}

fn parent_sidechain_persisted_row(db: &StateDb) -> (Option<String>, i64) {
    db.conn
        .query_row(
            "SELECT parent_turn_id, is_sidechain
                 FROM session_turns
                 WHERE provider_name = ?1 AND session_id = ?2 AND turn_id = ?3",
            sqlite::params!["fixture-provider", "session-a", "child-turn"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
}

#[test]
fn count_session_turns_reports_total_assistant_and_sidechain_counts() {
    let counts = fixture_session_turn_counts();
    assert_eq!(counts.total, 3);
    assert_eq!(counts.assistant, 2);
    assert_eq!(counts.sidechain, 1);
}

fn fixture_session_turn_counts() -> SessionTurnCounts {
    let db = test_db();
    ingest_fixture_provider_count_turns(&db);
    ingest_other_provider_count_turn(&db);
    fixture_provider_session_turn_counts(&db)
}

fn ingest_fixture_provider_count_turns(db: &StateDb) {
    db.ingest_session_turns_batch("fixture-provider", &fixture_provider_count_turns())
        .unwrap();
}

fn fixture_provider_count_turns() -> Vec<SessionTurnIngest> {
    vec![
        session_turn_fixture(
            "session-a",
            "root",
            "2026-04-17T08:00:00Z",
            "user",
            None,
            false,
        ),
        session_turn_fixture(
            "session-a",
            "assistant-main",
            "2026-04-17T08:00:01Z",
            "assistant",
            Some("root"),
            false,
        ),
        session_turn_fixture(
            "session-a",
            "assistant-side",
            "2026-04-17T08:00:02Z",
            "assistant",
            Some("assistant-main"),
            true,
        ),
        session_turn_fixture(
            "session-b",
            "other-session",
            "2026-04-17T08:00:03Z",
            "assistant",
            None,
            true,
        ),
    ]
}

fn ingest_other_provider_count_turn(db: &StateDb) {
    db.ingest_session_turns_batch("other-provider", &[other_provider_count_turn()])
        .unwrap();
}

fn other_provider_count_turn() -> SessionTurnIngest {
    session_turn_fixture(
        "session-a",
        "other-provider-turn",
        "2026-04-17T08:00:04Z",
        "assistant",
        None,
        true,
    )
}

fn session_turn_fixture(
    session_id: &str,
    turn_id: &str,
    timestamp: &str,
    role: &str,
    parent_turn_id: Option<&str>,
    is_sidechain: bool,
) -> SessionTurnIngest {
    SessionTurnIngest {
        session_id: session_id.to_string(),
        turn_id: turn_id.to_string(),
        timestamp: ts(timestamp),
        role: role.to_string(),
        parent_turn_id: parent_turn_id.map(str::to_string),
        is_sidechain,
        is_compaction_boundary: false,
        body: None,
    }
}

fn fixture_provider_session_turn_counts(db: &StateDb) -> SessionTurnCounts {
    db.count_session_turns("fixture-provider", "session-a")
        .unwrap()
}

#[test]
fn has_session_user_text_turn_requires_exact_user_body_match() {
    let expected = "[OULIPOLY NOTIFICATIONS]\nhandle: h-exact\n";
    let probe = exact_text_probe(expected);

    assert!(probe.exact_exists);
    assert!(
        !probe.partial_exists,
        "partial text must not confirm delivery"
    );
    assert!(
        !probe.extra_matches_expected,
        "multi-chunk turns must match the submitted payload exactly"
    );
    assert!(probe.extra_matches_combined);
    assert!(!probe.other_provider_exists, "provider identity must match");
}

struct ExactTextProbe {
    exact_exists: bool,
    partial_exists: bool,
    extra_matches_expected: bool,
    extra_matches_combined: bool,
    other_provider_exists: bool,
}

fn exact_text_probe(expected: &str) -> ExactTextProbe {
    exact_text_probe_fields(&exact_text_probe_db(expected), expected)
}

fn exact_text_probe_db(expected: &str) -> StateDb {
    let db = test_db();
    db.ingest_session_turns_batch("fixture-provider", &exact_user_text_turns(expected))
        .unwrap();
    db
}

fn exact_text_probe_fields(db: &StateDb, expected: &str) -> ExactTextProbe {
    let extra_text_body = extra_text_body(expected);

    ExactTextProbe {
        exact_exists: db
            .has_session_user_text_turn("fixture-provider", "session-a", expected)
            .unwrap(),
        partial_exists: db
            .has_session_user_text_turn("fixture-provider", "session-a", "handle: h")
            .unwrap(),
        extra_matches_expected: StateDb::session_turn_body_has_exact_text(
            &extra_text_body,
            expected,
        ),
        extra_matches_combined: StateDb::session_turn_body_has_exact_text(
            &extra_text_body,
            &expected_plus_extra(expected),
        ),
        other_provider_exists: db
            .has_session_user_text_turn("other-provider", "session-a", expected)
            .unwrap(),
    }
}

fn exact_user_text_turns(expected: &str) -> Vec<SessionTurnIngest> {
    vec![
        user_exact_text_turn(expected),
        assistant_same_text_turn("assistant text"),
    ]
}

fn user_exact_text_turn(expected: &str) -> SessionTurnIngest {
    SessionTurnIngest {
        session_id: "session-a".to_string(),
        turn_id: "user-exact".to_string(),
        timestamp: ts("2026-04-17T08:00:00Z"),
        role: "user".to_string(),
        parent_turn_id: None,
        is_sidechain: false,
        is_compaction_boundary: false,
        body: Some(text_body(expected)),
    }
}

fn assistant_same_text_turn(text: &str) -> SessionTurnIngest {
    SessionTurnIngest {
        session_id: "session-a".to_string(),
        turn_id: "assistant-same-text".to_string(),
        timestamp: ts("2026-04-17T08:00:01Z"),
        role: "assistant".to_string(),
        parent_turn_id: None,
        is_sidechain: false,
        is_compaction_boundary: false,
        body: Some(text_body(text)),
    }
}

fn text_body(text: &str) -> String {
    serde_json::json!([{ "type": "text", "text": text }]).to_string()
}

fn extra_text_body(expected: &str) -> String {
    serde_json::json!([
        { "type": "text", "text": expected },
        { "type": "text", "text": "extra" }
    ])
    .to_string()
}

fn expected_plus_extra(expected: &str) -> String {
    format!("{expected}extra")
}
