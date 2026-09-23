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
    runtime: Option<&oulipoly_state::mailbox::SessionMetadataRow>,
) -> Result<WakeClaimAcquireResult, String> {
    db.wake_sessions()
        .try_acquire_startable_wake_claim_for_runtime(
            wake_claim_request(input, claim_token),
            input.renew_token,
            runtime,
        )
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

/// Static rejection only: native automatic children must inherit an owner.
/// Presence permits attempting bootstrap, never election or child admission.
/// No storage/path probe belongs here; bootstrap authenticates current ownership
/// and the durable validator rechecks the claim/process/custodian afterwards.
pub(crate) fn reject_auto_wake_entry(session_id: &str, endpoint_hint_present: bool) -> Option<i32> {
    if !auto_wake_marker_present() {
        return None;
    }
    let marker = current_auto_wake_child_marker();
    (!marker.matches_session(session_id) || !endpoint_hint_present).then_some(0)
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
    #[cfg(feature = "age360-fault-fixtures")]
    oulipoly_state::completion_continuation::age360_fault_barrier(
        "wake-child-before-claim-admission",
    );
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

pub(super) fn coordinate_manual_resume_at(
    mailbox_path: &std::path::Path,
    session_id: &str,
    resolved: Option<&oulipoly_state::ResolvedResume>,
) -> Result<oulipoly_state::mailbox::ManualWakeCoordination, String> {
    // The sidecar and State share the data root. Do not hold a sidecar handle
    // while opening State: namespace ordering is State -> sidecar as well.
    let state = oulipoly_state::StateDb::open_existing(&mailbox_path.with_file_name("state.db"))?;
    let observation = match resolved {
        Some(resolved) if resolved.active_session_id == session_id => {
            state.coordinate_resolved_manual_resume(resolved)
        }
        Some(_) => Err("manual_resume_identity_session_mismatch".into()),
        None => state.coordinate_manual_resume(session_id),
    };
    #[cfg(feature = "age360-fault-fixtures")]
    if let Ok(value) = &observation {
        oulipoly_state::completion_continuation::age360_fault_barrier(&format!(
            "manual-after-claim-coordination-{value:?}"
        ));
    }
    observation
}

pub(crate) fn reset_manual_resume_wake_claim(
    resolved: &oulipoly_state::ResolvedResume,
) -> Result<(), String> {
    use oulipoly_state::mailbox::ManualWakeCoordination;
    match coordinate_manual_resume_at(
        &MailboxDb::default_path()?,
        &resolved.active_session_id,
        Some(resolved),
    )? {
        ManualWakeCoordination::Absent | ManualWakeCoordination::Released => Ok(()),
        observation => Err(format!("Manual resume admission changed: {observation:?}")),
    }
}

#[cfg(test)]
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

    fn publish_parent(db: &mut MailboxDb, directory: &std::path::Path) {
        let state = oulipoly_state::StateDb::open(&directory.join("state.db")).unwrap();
        let parent_uuid = uuid::Uuid::new_v4().to_string();
        let parent = state
            .start_invocation(&oulipoly_state::InvocationStart {
                invocation_uuid: parent_uuid.clone(),
                model_name: "model-a".into(),
                provider_name: "provider-a".into(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        state
            .bind_invocation_provider_session_start(
                oulipoly_state::InvocationMutationAuthority::Standalone,
                parent,
                &oulipoly_state::ProviderSessionBinding {
                    provider_session_id: "session-a".into(),
                    capture_method: "fixture",
                    resume_input_id: None,
                    provider_session_resolved_account: None,
                },
            )
            .unwrap();
        db.wake_sessions()
            .upsert_session_metadata(oulipoly_state::mailbox::SessionMetadataUpsert {
                session_id: "session-a",
                mode: "headless",
                invocation_uuid: Some(&parent_uuid),
                provider_name: Some("provider-a"),
                model_name: Some("model-a"),
                models_dir: None,
                effective_cwd: None,
            })
            .unwrap();
    }

    #[test]
    fn manual_resume_stops_when_a_replacement_claim_wins_release() {
        let directory = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&directory.path().join("pid-identity.db")).unwrap();
        crate::completion_owner::test_support::install_owner(&mut db);
        publish_parent(&mut db, directory.path());
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
        crate::completion_owner::test_support::revoke_unspent(&mut db, "session-a", "token-a");
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
        crate::completion_owner::test_support::install_owner(&mut db);
        publish_parent(&mut db, directory.path());
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
