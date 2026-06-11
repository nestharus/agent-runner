//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }

use super::common::*;
use super::*;
#[test]
fn age132_invocation_projection_maps_full_row_and_rejects_bad_values() {
    let db = test_db();
    let invocation_uuid = "44444444-4444-4444-8444-444444444444";
    let id = db
        .start_invocation(&InvocationStart {
            invocation_uuid: invocation_uuid.to_string(),
            model_name: "provider-a-opus".to_string(),
            provider_name: "provider-a".to_string(),
            provider_index: 7,
            parent_invocation_id: None,
        })
        .unwrap();
    db.update_session_capture(id, Some(SESSION_A), "verified")
        .unwrap();
    db.update_resume_acceptance(id, "accepted", Some("matched"))
        .unwrap();
    db.conn
        .execute(
            "UPDATE invocations
                 SET status = 'succeeded',
                     success = 1,
                     exit_code = 0,
                     terminal_reason = 'exit_zero',
                     created_at = '2026-04-17T08:00:00Z',
                     finished_at = '2026-04-17T08:00:02Z'
                 WHERE id = ?1",
            sqlite::params![id],
        )
        .unwrap();

    let record = db.get_invocation_by_uuid(invocation_uuid).unwrap().unwrap();
    assert_eq!(record.id, id);
    assert_eq!(record.invocation_uuid, invocation_uuid);
    assert_eq!(record.model_name, "provider-a-opus");
    assert_eq!(record.provider_name.as_deref(), Some("provider-a"));
    assert_eq!(record.provider_index, 7);
    assert_eq!(record.parent_invocation_id, None);
    assert_eq!(record.status, InvocationStatus::Succeeded);
    assert_eq!(record.success, Some(true));
    assert_eq!(record.exit_code, Some(0));
    assert_eq!(record.terminal_reason.as_deref(), Some("exit_zero"));
    assert_eq!(record.session_id.as_deref(), Some(SESSION_A));
    assert_eq!(record.provider_session_id.as_deref(), Some(SESSION_A));
    assert_eq!(
        record.provider_session_capture_method.as_deref(),
        Some("verified")
    );
    assert_eq!(record.resume_acceptance_status.as_deref(), Some("accepted"));
    assert_eq!(
        record.resume_acceptance_evidence.as_deref(),
        Some("matched")
    );
    assert_eq!(record.created_at, ts("2026-04-17T08:00:00Z"));
    assert_eq!(record.finished_at, Some(ts("2026-04-17T08:00:02Z")));

    let child_uuid = "55555555-5555-5555-8555-555555555555";
    let child_id = insert_invocation_fixture(&db, child_uuid, Some(id), "2026-04-17T08:00:01Z");
    let children = db.list_invocation_children(id).unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].id, child_id);
    assert_eq!(children[0].invocation_uuid, child_uuid);
    assert_eq!(children[0].parent_invocation_id, Some(id));
    assert_eq!(children[0].created_at, ts("2026-04-17T08:00:01Z"));

    db.conn
        .pragma_update(None, "ignore_check_constraints", true)
        .unwrap();
    db.conn
        .execute(
            "UPDATE invocations SET status = 'paused' WHERE id = ?1",
            sqlite::params![id],
        )
        .unwrap();
    let err = db.get_invocation_by_uuid(invocation_uuid).unwrap_err();
    assert!(err.contains("Unknown invocation status: paused"), "{err}");
    db.conn
            .execute(
                "UPDATE invocations SET status = 'running', created_at = 'not-a-timestamp' WHERE id = ?1",
                sqlite::params![id],
            )
            .unwrap();
    let err = db.get_invocation_by_uuid(invocation_uuid).unwrap_err();
    assert!(err.contains("Conversion error"), "{err}");
}

#[test]
fn age132_backfill_infers_model_from_latest_matching_invocation() {
    let db = test_db();
    db.ingest_session_turns_batch(
        "provider-a",
        &[
            SessionTurnIngest {
                session_id: SESSION_A.to_string(),
                turn_id: "turn-a1".to_string(),
                timestamp: ts("2026-04-17T08:00:00Z"),
                role: "user".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
                is_compaction_boundary: false,
                body: None,
            },
            SessionTurnIngest {
                session_id: SESSION_A.to_string(),
                turn_id: "turn-a2".to_string(),
                timestamp: ts("2026-04-17T08:01:00Z"),
                role: "assistant".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
                is_compaction_boundary: false,
                body: None,
            },
        ],
    )
    .unwrap();
    seed_invocation_for_session(
        &db,
        "provider-a-haiku",
        "provider-a",
        SESSION_A,
        "2026-04-17T08:00:30Z",
    );
    seed_invocation_for_session(
        &db,
        "provider-a-opus",
        "provider-a",
        SESSION_A,
        "2026-04-17T08:01:30Z",
    );

    let report = db.backfill_session_chains().unwrap();
    assert_eq!(
        report,
        BackfillReport {
            skipped_existing: false,
            chains_inserted: 1,
            segments_inserted: 1
        }
    );
    let model_name: String = db
        .conn
        .query_row("SELECT model_name FROM session_chains", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(model_name, "provider-a-opus");
}
