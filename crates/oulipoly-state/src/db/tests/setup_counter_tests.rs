//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }

use super::common::*;
use super::*;
#[test]
fn age132_setup_crud_count_and_call_counter_edge_contracts() {
    let db = test_db();
    db.upsert_cli_provider(&sample_provider()).unwrap();
    let expired = AccountRecord {
        id: "expired".to_string(),
        provider: "provider-a".to_string(),
        profile_name: "expired-profile".to_string(),
        auth_method: AuthMethod::OAuth,
        auth_status: AuthStatus::Expired,
        created_at: "2026-02-19T00:00:00Z".to_string(),
    };
    db.insert_account(&expired).unwrap();
    db.conn
        .execute(
            "UPDATE accounts SET auth_status = 'surprise' WHERE id = 'expired'",
            [],
        )
        .unwrap();
    let accounts = db.list_accounts(Some("provider-a")).unwrap();
    assert_eq!(accounts[0].auth_status, AuthStatus::Unknown);
    assert!(!db.delete_account("missing", "provider-a").unwrap());
    assert_eq!(
        db.delete_stale_models("provider-a", "missing-version")
            .unwrap(),
        0
    );

    let since = ts("2026-04-17T08:00:00Z");
    db.ingest_session_turns_batch(
        "provider-a",
        &[
            SessionTurnIngest {
                session_id: SESSION_A.to_string(),
                turn_id: "at-boundary".to_string(),
                timestamp: since,
                role: "assistant".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
                is_compaction_boundary: false,
                body: None,
            },
            SessionTurnIngest {
                session_id: SESSION_A.to_string(),
                turn_id: "after-boundary".to_string(),
                timestamp: since + chrono::Duration::seconds(1),
                role: "assistant".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
                is_compaction_boundary: false,
                body: None,
            },
            SessionTurnIngest {
                session_id: SESSION_A.to_string(),
                turn_id: "user-after-boundary".to_string(),
                timestamp: since + chrono::Duration::seconds(2),
                role: "user".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
                is_compaction_boundary: false,
                body: None,
            },
        ],
    )
    .unwrap();
    assert_eq!(
        db.count_assistant_turns_since("provider-a", None).unwrap(),
        2
    );
    assert_eq!(
        db.count_assistant_turns_since("provider-a", Some(&since))
            .unwrap(),
        1
    );
    assert_eq!(
        db.count_assistant_turns_since("provider-b", None).unwrap(),
        0
    );
    db.increment_calls_since_refresh("provider-a").unwrap();
    db.increment_calls_since_refresh("provider-a").unwrap();
    assert_eq!(calls_since_refresh(&db, "provider-a"), 2);
}
