//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }

use super::common::*;
use super::*;
#[test]
fn ingest_session_turns_batch_persists_parent_and_sidechain_columns() {
    let db = test_db();

    let inserted = db
        .ingest_session_turns_batch(
            "fixture-provider",
            &[SessionTurnIngest {
                session_id: "session-a".to_string(),
                turn_id: "child-turn".to_string(),
                timestamp: ts("2026-04-17T08:00:01Z"),
                role: "assistant".to_string(),
                parent_turn_id: Some("root-turn".to_string()),
                is_sidechain: true,
                is_compaction_boundary: false,
                body: None,
            }],
        )
        .unwrap();

    assert_eq!(inserted, 1);
    let row: (Option<String>, i64) = db
        .conn
        .query_row(
            "SELECT parent_turn_id, is_sidechain
                 FROM session_turns
                 WHERE provider_name = ?1 AND session_id = ?2 AND turn_id = ?3",
            sqlite::params!["fixture-provider", "session-a", "child-turn"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(row.0.as_deref(), Some("root-turn"));
    assert_eq!(row.1, 1);
}

#[test]
fn count_session_turns_reports_total_assistant_and_sidechain_counts() {
    let db = test_db();

    db.ingest_session_turns_batch(
        "fixture-provider",
        &[
            SessionTurnIngest {
                session_id: "session-a".to_string(),
                turn_id: "root".to_string(),
                timestamp: ts("2026-04-17T08:00:00Z"),
                role: "user".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
                is_compaction_boundary: false,
                body: None,
            },
            SessionTurnIngest {
                session_id: "session-a".to_string(),
                turn_id: "assistant-main".to_string(),
                timestamp: ts("2026-04-17T08:00:01Z"),
                role: "assistant".to_string(),
                parent_turn_id: Some("root".to_string()),
                is_sidechain: false,
                is_compaction_boundary: false,
                body: None,
            },
            SessionTurnIngest {
                session_id: "session-a".to_string(),
                turn_id: "assistant-side".to_string(),
                timestamp: ts("2026-04-17T08:00:02Z"),
                role: "assistant".to_string(),
                parent_turn_id: Some("assistant-main".to_string()),
                is_sidechain: true,
                is_compaction_boundary: false,
                body: None,
            },
            SessionTurnIngest {
                session_id: "session-b".to_string(),
                turn_id: "other-session".to_string(),
                timestamp: ts("2026-04-17T08:00:03Z"),
                role: "assistant".to_string(),
                parent_turn_id: None,
                is_sidechain: true,
                is_compaction_boundary: false,
                body: None,
            },
        ],
    )
    .unwrap();
    db.ingest_session_turns_batch(
        "other-provider",
        &[SessionTurnIngest {
            session_id: "session-a".to_string(),
            turn_id: "other-provider-turn".to_string(),
            timestamp: ts("2026-04-17T08:00:04Z"),
            role: "assistant".to_string(),
            parent_turn_id: None,
            is_sidechain: true,
            is_compaction_boundary: false,
            body: None,
        }],
    )
    .unwrap();

    let counts: SessionTurnCounts = db
        .count_session_turns("fixture-provider", "session-a")
        .unwrap();

    assert_eq!(counts.total, 3);
    assert_eq!(counts.assistant, 2);
    assert_eq!(counts.sidechain, 1);
}

#[test]
fn has_session_user_text_turn_requires_exact_user_body_match() {
    let db = test_db();
    let expected = "[OULIPOLY NOTIFICATIONS]\nhandle: h-exact\n";

    db.ingest_session_turns_batch(
        "fixture-provider",
        &[
            SessionTurnIngest {
                session_id: "session-a".to_string(),
                turn_id: "user-exact".to_string(),
                timestamp: ts("2026-04-17T08:00:00Z"),
                role: "user".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
                is_compaction_boundary: false,
                body: Some(serde_json::json!([{ "type": "text", "text": expected }]).to_string()),
            },
            SessionTurnIngest {
                session_id: "session-a".to_string(),
                turn_id: "assistant-same-text".to_string(),
                timestamp: ts("2026-04-17T08:00:01Z"),
                role: "assistant".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
                is_compaction_boundary: false,
                body: Some(
                    serde_json::json!([{ "type": "text", "text": "assistant text" }]).to_string(),
                ),
            },
        ],
    )
    .unwrap();

    let extra_text_body = serde_json::json!([
        { "type": "text", "text": expected },
        { "type": "text", "text": "extra" }
    ])
    .to_string();

    assert!(
        db.has_session_user_text_turn("fixture-provider", "session-a", expected)
            .unwrap()
    );
    assert!(
        !db.has_session_user_text_turn("fixture-provider", "session-a", "handle: h")
            .unwrap(),
        "partial text must not confirm delivery"
    );
    assert!(
        !StateDb::session_turn_body_has_exact_text(&extra_text_body, expected),
        "multi-chunk turns must match the submitted payload exactly"
    );
    assert!(StateDb::session_turn_body_has_exact_text(
        &extra_text_body,
        &format!("{expected}extra")
    ));
    assert!(
        !db.has_session_user_text_turn("other-provider", "session-a", expected)
            .unwrap(),
        "provider identity must match"
    );
}
