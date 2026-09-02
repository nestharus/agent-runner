use chrono::{DateTime, Utc};
use oulipoly_state::{
    FinalizedProviderSessionAuthority, ImportedSessionDisplayMetadataUpsert, InvocationStart,
    SessionTurnIngestStreamKey, SessionTurnStreamProjection, StateDb,
    StoredProviderSessionAuthority,
};
use rusqlite::Connection;

fn ts(value: &str) -> DateTime<Utc> {
    value.parse().unwrap()
}

fn chain_id(db: &StateDb, provider_name: &str, session_id: &str) -> String {
    Connection::open(db.path())
        .unwrap()
        .query_row(
            "SELECT chain_id
             FROM session_chain_segments
             WHERE provider_name = ?1 AND session_id = ?2",
            [provider_name, session_id],
            |row| row.get(0),
        )
        .unwrap()
}

fn authority(instance: &str, settings: &str) -> StoredProviderSessionAuthority {
    StoredProviderSessionAuthority {
        provider_instance_id: instance.to_string(),
        settings_id: settings.to_string(),
    }
}

#[test]
fn finalized_capture_commits_invocation_segment_and_endpoint_authority_together() {
    let dir = tempfile::tempdir().unwrap();
    let db = StateDb::open(&dir.path().join("state.db")).unwrap();
    let invocation_id = db
        .start_invocation(&InvocationStart {
            invocation_uuid: "11111111-1111-4111-8111-111111111111".to_string(),
            model_name: "model-a".to_string(),
            provider_name: "account-a".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    db.finalize_invocation(invocation_id, true, 0, None, None)
        .unwrap();

    db.commit_finalized_provider_session_authority(
        invocation_id,
        &FinalizedProviderSessionAuthority {
            provider_session_id: "session-a",
            capture_method: "provider_session_capture",
            provider_instance_id: "provider-a-instance",
            settings_id: "account-a-settings",
        },
    )
    .unwrap();

    let row = db
        .get_invocation_by_uuid("11111111-1111-4111-8111-111111111111")
        .unwrap()
        .unwrap();
    assert_eq!(row.provider_session_id.as_deref(), Some("session-a"));
    assert_eq!(
        row.provider_session_capture_method.as_deref(),
        Some("provider_session_capture")
    );
    let chain_id = chain_id(&db, "account-a", "session-a");
    assert_eq!(
        db.active_provider_session_authority(&chain_id).unwrap(),
        Some(authority("provider-a-instance", "account-a-settings"))
    );

    let error = db
        .commit_finalized_provider_session_authority(
            invocation_id,
            &FinalizedProviderSessionAuthority {
                provider_session_id: "session-a",
                capture_method: "provider_session_capture",
                provider_instance_id: "provider-a-instance",
                settings_id: "different-account-settings",
            },
        )
        .unwrap_err();
    assert!(
        error.contains("provider_session_authority_mismatch"),
        "{error}"
    );
    assert_eq!(
        db.active_provider_session_authority(&chain_id).unwrap(),
        Some(authority("provider-a-instance", "account-a-settings"))
    );
}

#[test]
fn import_binds_metadata_and_stream_to_the_selected_account_endpoint() {
    let dir = tempfile::tempdir().unwrap();
    let db = StateDb::open(&dir.path().join("state.db")).unwrap();
    let metadata = ImportedSessionDisplayMetadataUpsert {
        provider_name: "account-a".to_string(),
        provider_session_id: "session-a".to_string(),
        title: Some("Imported session".to_string()),
        cwd: Some("/workspace/account-a".to_string()),
        turn_count: Some(2),
        provider_updated_at: Some(ts("2026-09-01T23:59:00Z")),
        seen_at: ts("2026-09-02T00:00:00Z"),
    };
    let key = SessionTurnIngestStreamKey {
        provider_name: "account-a".to_string(),
        provider_instance_id: "provider-a-instance".to_string(),
        settings_id: "account-a-settings".to_string(),
        session_id: "session-a".to_string(),
        projection: SessionTurnStreamProjection::CanonicalIngest,
    };

    assert!(
        db.import_session_and_enqueue_turn_ingest(
            &metadata,
            &key,
            &ts("2026-09-01T23:00:00Z"),
            "model-a",
        )
        .unwrap()
    );

    let chain_id = chain_id(&db, "account-a", "session-a");
    let stored = authority("provider-a-instance", "account-a-settings");
    assert_eq!(
        db.active_provider_session_authority(&chain_id).unwrap(),
        Some(stored.clone())
    );
    assert_eq!(
        db.imported_session_cwd_for_authority("account-a", "session-a", &stored)
            .unwrap()
            .as_deref(),
        Some("/workspace/account-a")
    );
    assert_eq!(
        db.imported_session_cwd_for_authority(
            "account-a",
            "session-a",
            &authority("provider-a-instance", "different-account-settings"),
        )
        .unwrap(),
        None
    );

    let mut different_account_key = key.clone();
    different_account_key.settings_id = "different-account-settings".to_string();
    let error = db
        .import_session_and_enqueue_turn_ingest(
            &metadata,
            &different_account_key,
            &ts("2026-09-01T23:00:00Z"),
            "model-a",
        )
        .unwrap_err();
    assert!(
        error.contains("provider_session_authority_mismatch"),
        "{error}"
    );
    assert!(
        db.session_turn_ingest_stream(&different_account_key)
            .unwrap()
            .is_none()
    );
}
