//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }

use super::common::*;
use super::*;
#[test]
fn backfill_creates_one_chain_per_provider_session_pair() {
    let dir = pre_chain_db_with_turns(&[
        (
            "claude",
            SESSION_A,
            "turn-a1",
            "2026-04-17T08:00:00Z",
            "assistant",
        ),
        (
            "claude",
            SESSION_A,
            "turn-a2",
            "2026-04-17T08:00:01Z",
            "assistant",
        ),
        (
            "claude2",
            SESSION_B,
            "turn-b1",
            "2026-04-17T09:00:00Z",
            "assistant",
        ),
    ]);

    let db = StateDb::open(&dir.path().join("state.db")).unwrap();

    assert_eq!(chain_count(&db), 2);
    assert_eq!(segment_count(&db), 2);
    let imported: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM session_chain_segments WHERE transition_reason = 'imported' AND ended_at IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
    assert_eq!(imported, 2);
}

#[test]
fn backfill_idempotent_on_second_open() {
    let dir = pre_chain_db_with_turns(&[(
        "claude",
        SESSION_A,
        "turn-a1",
        "2026-04-17T08:00:00Z",
        "assistant",
    )]);
    let path = dir.path().join("state.db");

    let first = StateDb::open(&path).unwrap();
    let first_count = chain_count(&first);
    let first_invocation_checksum = invocation_checksum(&first);
    drop(first);
    let second = StateDb::open(&path).unwrap();

    assert_eq!(chain_count(&second), first_count);
    assert_eq!(segment_count(&second), 1);
    assert_eq!(invocation_checksum(&second), first_invocation_checksum);
}

#[test]
fn migration_backfill_null_null_preserves_running_rows() {
    let invocation_uuid = "11111111-1111-4111-8111-111111111111";
    let dir = legacy_v4_invocation_dual_id_fixture(
        invocation_uuid,
        None,
        None,
        "running",
        Some("still_running"),
        Some("unknown"),
    );
    let db = StateDb::open(&dir.path().join("state.db")).unwrap();

    type MigrationBackfillRow = (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
        Option<String>,
    );

    let row: MigrationBackfillRow = db
        .conn
        .query_row(
            "SELECT session_id, provider_session_id, resume_input_id,
                        provider_session_capture_method, terminal_reason, status, error_category
                 FROM invocations
                 WHERE invocation_uuid = ?1",
            sqlite::params![invocation_uuid],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .unwrap();

    assert_eq!(row.0, None);
    assert_eq!(row.1, None);
    assert_eq!(row.2, None);
    assert_eq!(row.3, None);
    assert_eq!(row.4.as_deref(), Some("still_running"));
    assert_eq!(row.5, "running");
    assert_eq!(row.6.as_deref(), Some("unknown"));
}

#[test]
fn migration_backfill_resumed_chain_id_safe() {
    let invocation_uuid = "22222222-2222-4222-8222-222222222222";
    let dir = legacy_v4_invocation_dual_id_fixture(
        invocation_uuid,
        Some(CHAIN_A),
        Some("resumed"),
        "succeeded",
        None,
        None,
    );
    {
        let conn = sqlite::Connection::open(dir.path().join("state.db")).unwrap();
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
                 VALUES (?1, '2026-04-17T08:00:00Z', '2026-04-17T08:00:00Z', 'claude-opus')",
            sqlite::params![CHAIN_A],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_chain_segments
                    (chain_id, provider_name, session_id, started_at, transition_reason)
                 VALUES (?1, 'claude', ?2, '2026-04-17T08:00:00Z', 'initial')",
            sqlite::params![CHAIN_A, SESSION_A],
        )
        .unwrap();
    }
    let db = StateDb::open(&dir.path().join("state.db")).unwrap();

    let row: (String, Option<String>, Option<String>, Option<String>) = db
        .conn
        .query_row(
            "SELECT session_id, provider_session_id, resume_input_id,
                        provider_session_capture_method
                 FROM invocations
                 WHERE invocation_uuid = ?1",
            sqlite::params![invocation_uuid],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    let models = resolver_model_store();
    let resolved = db.resolve_resume(&models, CHAIN_A, None).unwrap();

    assert_eq!(row.0, CHAIN_A);
    assert_eq!(row.1, None);
    assert_eq!(row.2.as_deref(), Some(CHAIN_A));
    assert_eq!(row.3, None);
    assert_eq!(resolved.active_session_id, SESSION_A);
}

#[test]
fn migration_backfill_non_resumed_with_session_id() {
    let invocation_uuid = "33333333-3333-4333-8333-333333333333";
    let dir = legacy_v4_invocation_dual_id_fixture(
        invocation_uuid,
        Some(SESSION_A),
        Some("forced_flag_verified"),
        "succeeded",
        None,
        None,
    );
    let db = StateDb::open(&dir.path().join("state.db")).unwrap();

    let row: (String, Option<String>, Option<String>, Option<String>) = db
        .conn
        .query_row(
            "SELECT session_id, provider_session_id, resume_input_id,
                        provider_session_capture_method
                 FROM invocations
                 WHERE invocation_uuid = ?1",
            sqlite::params![invocation_uuid],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();

    assert_eq!(row.0, SESSION_A);
    assert_eq!(row.1.as_deref(), Some(SESSION_A));
    assert_eq!(row.2, None);
    assert_eq!(row.3.as_deref(), Some("forced_flag_verified"));
}

#[test]
fn mint_chain_no_op_on_resume_of_existing_chain() {
    let db = test_db();
    seed_chain_row(&db, CHAIN_A, "claude-opus", "2026-04-17T08:00:00Z");

    let first_id = db
        .open_chain_segment(
            CHAIN_A,
            "claude",
            SESSION_A,
            &ts("2026-04-17T08:00:00Z"),
            oulipoly_core::TransitionReason::Initial,
        )
        .unwrap();
    let second_id = db
        .open_chain_segment(
            CHAIN_A,
            "claude",
            SESSION_A,
            &ts("2026-04-17T08:01:00Z"),
            oulipoly_core::TransitionReason::Initial,
        )
        .unwrap();

    assert_eq!(first_id, second_id);
    assert_eq!(segment_count(&db), 1);
    let active: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM session_chain_segments WHERE chain_id = ?1 AND ended_at IS NULL",
            sqlite::params![CHAIN_A],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(active, 1);
}
