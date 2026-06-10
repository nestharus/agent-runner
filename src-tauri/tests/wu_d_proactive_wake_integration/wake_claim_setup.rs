//! ## Declared roles
//!
//! Roles: mutator.
//!
//! TEST: wake-claim setup mutators for proactive wake integration cases.

use crate::fixtures::Fixture;
use crate::{MODEL, PROVIDER, SESSION};
use chrono::Utc;
use oulipoly_state::mailbox::{WakeClaimAcquireResult, WakeClaimRequest};
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
        .record_wake_claim_pid(SESSION, claim_token, identity.os_pid)
        .unwrap();
}

pub(crate) fn acquire_seed_wake_claim(fixture: &Fixture, claim_token: &str) {
    acquire_seed_wake_claim_for(fixture, SESSION, claim_token);
}

pub(crate) fn acquire_seed_wake_claim_for(fixture: &Fixture, session_id: &str, claim_token: &str) {
    let mut db = fixture.mailbox();
    assert_wake_claim_acquired(
        db.try_acquire_wake_claim(seed_wake_claim_request(session_id, claim_token))
            .unwrap(),
    );
}

fn seed_wake_claim_request<'a>(session_id: &'a str, claim_token: &'a str) -> WakeClaimRequest<'a> {
    WakeClaimRequest {
        session_id,
        claim_token,
        reason: "notify_idle",
        auto_wake_count: 1,
        wake_invocation_uuid: None,
        stale_after_seconds: 600,
    }
}

fn assert_wake_claim_acquired(result: WakeClaimAcquireResult) {
    assert!(matches!(result, WakeClaimAcquireResult::Acquired(_)));
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
