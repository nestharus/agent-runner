//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }

use super::common::*;
use super::*;
#[test]
fn has_session_user_turn_containing_matches_user_body_substring() {
    let db = test_db();
    let nonce = "11111111-2222-4333-8444-555555555555";

    db.ingest_session_turns_batch("fixture-provider", &delivery_fixture_turns(nonce))
        .unwrap();

    assert!(
        db.has_session_user_turn_containing("fixture-provider", "session-a", nonce)
            .unwrap(),
        "the delivery nonce should match inside a non-exact quote-wrapped user body"
    );
    assert!(
        !db.has_session_user_turn_containing("fixture-provider", "session-a", "missing-nonce")
            .unwrap(),
        "missing nonce must not confirm delivery"
    );
    assert!(
        !db.has_session_user_turn_containing("fixture-provider", "session-a", "")
            .unwrap(),
        "empty needles must not match every body"
    );
    assert!(
        !db.has_session_user_turn_containing("other-provider", "session-a", nonce)
            .unwrap(),
        "provider identity must match"
    );
    assert!(
        !db.has_session_user_turn_containing("fixture-provider", "other-session", nonce)
            .unwrap(),
        "session identity must match"
    );
}

fn delivery_fixture_turns(nonce: &str) -> [SessionTurnIngest; 2] {
    [user_delivery_turn(nonce), assistant_delivery_turn(nonce)]
}

fn user_delivery_turn(nonce: &str) -> SessionTurnIngest {
    SessionTurnIngest {
        session_id: "session-a".to_string(),
        turn_id: "user-quoted-delivery".to_string(),
        timestamp: ts("2026-04-17T08:00:00Z"),
        role: "user".to_string(),
        parent_turn_id: None,
        is_sidechain: false,
        is_compaction_boundary: false,
        body: Some(quoted_delivery_body(nonce)),
    }
}

fn assistant_delivery_turn(nonce: &str) -> SessionTurnIngest {
    SessionTurnIngest {
        session_id: "session-a".to_string(),
        turn_id: "assistant-same-nonce".to_string(),
        timestamp: ts("2026-04-17T08:00:01Z"),
        role: "assistant".to_string(),
        parent_turn_id: None,
        is_sidechain: false,
        is_compaction_boundary: false,
        body: Some(assistant_delivery_body(nonce)),
    }
}

fn quoted_delivery_body(nonce: &str) -> String {
    delivery_body_json(&quoted_delivery_text(nonce))
}

fn quoted_delivery_text(nonce: &str) -> String {
    format!("\"[OULIPOLY NOTIFICATIONS]\n[OULIPOLY-DELIVERY {nonce}]\nbody\"")
}

fn assistant_delivery_body(nonce: &str) -> String {
    delivery_body_json(&assistant_delivery_text(nonce))
}

fn assistant_delivery_text(nonce: &str) -> String {
    format!("assistant [OULIPOLY-DELIVERY {nonce}]")
}

fn delivery_body_json(text: &str) -> String {
    serde_json::json!([{ "type": "text", "text": text }]).to_string()
}
