//! ## Declared roles
//!
//! `mapper`, `orchestration`, `validator`

use oulipoly_state::mailbox::{MailboxDb, WakeClaimAcquireResult, WakeClaimRequest};
use oulipoly_state::pid_identity::{ProcessIdentity, read_live_process_identity};

use super::auto_wake_env::{
    AutoWakeEnv, auto_wake_marker_present, current_auto_wake, current_auto_wake_child_marker,
};
use super::constants::WAKE_CLAIM_STALE_AFTER_SECONDS;
use super::wake_start::StartWakeInput;

pub(super) fn acquire_wake_claim(
    db: &mut MailboxDb,
    input: StartWakeInput<'_>,
    claim_token: &str,
) -> Result<WakeClaimAcquireResult, String> {
    db.wake_sessions()
        .try_acquire_startable_wake_claim(wake_claim_request(input, claim_token), input.renew_token)
}

fn wake_claim_request<'a>(input: StartWakeInput<'a>, claim_token: &'a str) -> WakeClaimRequest<'a> {
    WakeClaimRequest {
        session_id: input.session_id,
        claim_token,
        reason: input.reason,
        auto_wake_count: input.auto_wake_count,
        wake_invocation_uuid: None,
        stale_after_seconds: WAKE_CLAIM_STALE_AFTER_SECONDS,
    }
}

pub(crate) fn validate_auto_wake_child(session_id: &str) -> Result<Option<i32>, String> {
    if !auto_wake_marker_present() {
        return Ok(None);
    }
    let marker = current_auto_wake_child_marker();
    if !marker.matches_session(session_id) {
        return Ok(Some(0));
    }
    validate_auto_wake_child_claim(session_id, marker.claim_token())
}

fn validate_auto_wake_child_claim(
    session_id: &str,
    claim_token: &str,
) -> Result<Option<i32>, String> {
    let Some(mut db) = MailboxDb::open_default_if_exists()? else {
        return Ok(Some(0));
    };
    let child_identity = current_process_identity()?;
    db.wake_sessions()
        .validate_wake_claim_for_child(session_id, claim_token, &child_identity)
        .map(|valid| if valid { None } else { Some(0) })
}

fn current_process_identity() -> Result<ProcessIdentity, String> {
    let pid = i64::from(std::process::id());
    read_live_process_identity(pid)?
        .ok_or_else(|| format!("Auto-wake child process {pid} is not live during claim admission"))
}

pub(crate) fn reset_manual_resume_wake_claim(session_id: &str) -> Result<(), String> {
    let Some(mut db) = MailboxDb::open_default_if_exists()? else {
        return Ok(());
    };
    let Some(claim) = db.wake_session_reader().wake_claim(session_id)? else {
        return Ok(());
    };
    release_manual_wake_claim(&mut db, session_id, &claim.claim_token)
}

fn release_manual_wake_claim(
    db: &mut MailboxDb,
    session_id: &str,
    claim_token: &str,
) -> Result<(), String> {
    if db
        .wake_sessions()
        .release_wake_claim_for_manual_resume(session_id, claim_token)?
    {
        return Ok(());
    }
    Err(format!(
        "Manual resume lost wake-claim release authority for session {session_id}"
    ))
}

pub(crate) fn release_current_auto_wake_claim_for_session(session_id: &str) {
    let auto_wake = current_auto_wake();
    release_current_auto_wake_claim(session_id, auto_wake.as_ref());
}

pub(super) fn release_current_auto_wake_claim(session_id: &str, auto_wake: Option<&AutoWakeEnv>) {
    let Some(auto_wake) = auto_wake else {
        return;
    };
    match MailboxDb::open_default_if_exists() {
        Ok(Some(mut db)) => release_wake_claim_or_warn(&mut db, session_id, &auto_wake.token),
        Ok(None) => {}
        Err(err) => tracing::warn!(
            session_id,
            "Failed to open sidecar to release wake claim: {err}"
        ),
    }
}

fn release_wake_claim_or_warn(db: &mut MailboxDb, session_id: &str, token: &str) {
    if let Err(err) = db
        .wake_sessions()
        .release_admitted_wake_claim(session_id, token)
    {
        tracing::warn!(session_id, "Failed to release wake claim: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_state::InboxTargetKind;
    use oulipoly_state::mailbox::{InboxTarget, SubmittedInputEnqueue};

    #[test]
    fn manual_resume_stops_when_a_replacement_claim_wins_release() {
        let directory = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&directory.path().join("pid-identity.db")).unwrap();
        db.enqueue_submitted_input(&SubmittedInputEnqueue {
            submission_token: "manual-release-input",
            target: InboxTarget {
                kind: InboxTargetKind::Session,
                id: "session-a",
            },
            input: b"input",
        })
        .unwrap();
        let initial = db
            .wake_sessions()
            .try_acquire_wake_claim(WakeClaimRequest {
                session_id: "session-a",
                claim_token: "token-a",
                reason: "initial",
                auto_wake_count: 1,
                wake_invocation_uuid: Some("wake-a"),
                stale_after_seconds: 600,
            })
            .unwrap();
        assert!(matches!(initial, WakeClaimAcquireResult::Acquired(_)));
        let captured = db
            .wake_session_reader()
            .wake_claim("session-a")
            .unwrap()
            .unwrap();
        let replacement = db
            .wake_sessions()
            .try_acquire_or_renew_wake_claim(
                WakeClaimRequest {
                    session_id: "session-a",
                    claim_token: "token-b",
                    reason: "replacement",
                    auto_wake_count: 2,
                    wake_invocation_uuid: Some("wake-b"),
                    stale_after_seconds: 600,
                },
                Some(&captured.claim_token),
            )
            .unwrap();
        assert!(matches!(replacement, WakeClaimAcquireResult::Acquired(_)));

        let error =
            release_manual_wake_claim(&mut db, "session-a", &captured.claim_token).unwrap_err();

        assert!(error.contains("lost wake-claim release authority"));
        assert_eq!(
            db.wake_session_reader()
                .wake_claim("session-a")
                .unwrap()
                .unwrap()
                .claim_token,
            "token-b"
        );
    }

    #[test]
    fn manual_resume_releases_a_dead_admitted_wake_claim() {
        let directory = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&directory.path().join("pid-identity.db")).unwrap();
        db.enqueue_submitted_input(&SubmittedInputEnqueue {
            submission_token: "manual-dead-release-input",
            target: InboxTarget {
                kind: InboxTargetKind::Session,
                id: "session-a",
            },
            input: b"input",
        })
        .unwrap();
        let initial = db
            .wake_sessions()
            .try_acquire_wake_claim(WakeClaimRequest {
                session_id: "session-a",
                claim_token: "token-a",
                reason: "initial",
                auto_wake_count: 1,
                wake_invocation_uuid: Some("wake-a"),
                stale_after_seconds: 600,
            })
            .unwrap();
        assert!(matches!(initial, WakeClaimAcquireResult::Acquired(_)));
        db.wake_sessions()
            .record_wake_claim_pid("session-a", "token-a", i64::MAX)
            .unwrap();

        release_manual_wake_claim(&mut db, "session-a", "token-a").unwrap();

        assert!(
            db.wake_session_reader()
                .wake_claim("session-a")
                .unwrap()
                .is_none()
        );
    }
}
