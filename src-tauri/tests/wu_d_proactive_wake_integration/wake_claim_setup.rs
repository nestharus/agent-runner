//! ## Declared roles
//!
//! Roles: mutator.
//!
//! TEST: wake-claim setup mutators for proactive wake integration cases.

use crate::fixtures::Fixture;
use crate::{MODEL, PROVIDER, SESSION};
use chrono::Utc;
use oulipoly_state::pid_identity::{
    PidIdentityDb, PidIdentityRecord, ProcessIdentity, read_live_process_identity,
};

pub(crate) fn seed_dead_wake_claim(fixture: &Fixture, claim_token: &str, seconds_old: i64) {
    seed_dead_wake_claim_for(fixture, SESSION, claim_token, seconds_old);
}

pub(crate) fn seed_dead_wake_claim_for(
    fixture: &Fixture,
    session_id: &str,
    claim_token: &str,
    seconds_old: i64,
) {
    acquire_seed_wake_claim_for(fixture, session_id, claim_token);
    fixture
        .mailbox()
        .wake_sessions()
        .record_wake_claim_pid(session_id, claim_token, 999_999_999)
        .unwrap();
    age_wake_claim_for(fixture, session_id, seconds_old);
}

pub(crate) fn seed_live_wake_claim(fixture: &Fixture, claim_token: &str) {
    acquire_seed_wake_claim(fixture, claim_token);
    let identity = current_process_identity();
    PidIdentityDb::open(&fixture.sidecar_path())
        .unwrap()
        .record_identity(PidIdentityRecord {
            identity: &identity,
            os_pgid: None,
            invocation_uuid: claim_token,
            session_id: Some(SESSION),
            provider_name: Some(PROVIDER),
            model_name: Some(MODEL),
            recorded_at: "2026-06-04T12:02:00Z",
        })
        .unwrap();
    fixture
        .mailbox()
        .wake_sessions()
        .record_wake_claim_pid(SESSION, claim_token, identity.os_pid)
        .unwrap();
}

pub(crate) fn acquire_seed_wake_claim(fixture: &Fixture, claim_token: &str) {
    acquire_seed_wake_claim_for(fixture, SESSION, claim_token);
}

pub(crate) fn acquire_seed_wake_claim_for(fixture: &Fixture, session_id: &str, claim_token: &str) {
    let conn = fixture.sidecar_conn();
    let (min_pending, max_pending): (Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT MIN(seq), MAX(seq)
             FROM mailbox
             WHERE session_id = ?1 AND delivered_at IS NULL",
            rusqlite::params![session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    conn.execute(
        "INSERT INTO session_wake_claim (
            session_id, claim_token, claimed_at, wake_invocation_uuid,
            reason, auto_wake_count, min_pending_seq_at_claim, max_pending_seq_at_claim
         ) VALUES (?1, ?2, ?3, NULL, 'test_fixture', 1, ?4, ?5)",
        rusqlite::params![
            session_id,
            claim_token,
            Utc::now().to_rfc3339(),
            min_pending,
            max_pending
        ],
    )
    .unwrap();
}

pub(crate) fn age_wake_claim_for(fixture: &Fixture, session_id: &str, seconds_old: i64) {
    let claimed_at = (Utc::now() - chrono::Duration::seconds(seconds_old)).to_rfc3339();
    fixture
        .sidecar_conn()
        .execute(
            "UPDATE session_wake_claim SET claimed_at = ?2 WHERE session_id = ?1",
            rusqlite::params![session_id, claimed_at],
        )
        .unwrap();
}

pub(crate) fn current_process_identity() -> ProcessIdentity {
    read_live_process_identity(i64::from(std::process::id()))
        .unwrap()
        .expect("test process should have a live identity")
}
