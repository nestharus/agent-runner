//! ## Declared roles
//!
//! `accessor`, `formatter`, `mapper`, `orchestration`, `parser`, `predicate`, `validator`

use oulipoly_state::mailbox::MailboxDb;
use oulipoly_state::pid_identity::{ProcessIdentity, read_live_process_identity};
use std::time::Duration;

use super::constants::{
    AUTO_WAKE_COUNT_ENV, AUTO_WAKE_ENV, AUTO_WAKE_RETRY_BASE_MS_ENV, AUTO_WAKE_RETRY_MAX_MS,
    AUTO_WAKE_SESSION_ID_ENV, AUTO_WAKE_TOKEN_ENV, DEFAULT_AUTO_WAKE_RETRY_BASE_MS,
};

pub(super) struct AutoWakeEnv {
    pub(super) token: String,
    pub(super) count: i64,
}

struct AutoWakeChildMarker {
    expected_session: String,
    claim_token: String,
}

pub(crate) fn validate_auto_wake_child(session_id: &str) -> Result<Option<i32>, String> {
    if !auto_wake_marker_present() {
        return Ok(None);
    }
    let marker = auto_wake_child_marker();
    if !auto_wake_child_marker_matches(session_id, &marker) {
        return Ok(Some(0));
    }
    validate_auto_wake_child_claim(session_id, &marker.claim_token)
}

pub(crate) fn is_auto_wake_invocation() -> bool {
    auto_wake_marker_present()
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

pub(super) fn auto_wake_marker_present() -> bool {
    std::env::var_os(AUTO_WAKE_ENV).is_some()
}

fn auto_wake_child_marker() -> AutoWakeChildMarker {
    auto_wake_child_marker_from_parts(auto_wake_expected_session(), auto_wake_child_claim_token())
}

fn auto_wake_expected_session() -> String {
    std::env::var(AUTO_WAKE_SESSION_ID_ENV).unwrap_or_default()
}

fn auto_wake_child_claim_token() -> String {
    std::env::var(AUTO_WAKE_TOKEN_ENV).unwrap_or_default()
}

fn auto_wake_child_marker_from_parts(
    expected_session: String,
    claim_token: String,
) -> AutoWakeChildMarker {
    AutoWakeChildMarker {
        expected_session,
        claim_token,
    }
}

fn auto_wake_child_marker_matches(session_id: &str, marker: &AutoWakeChildMarker) -> bool {
    marker.expected_session == session_id && !marker.claim_token.is_empty()
}

fn validate_auto_wake_child_claim(
    session_id: &str,
    claim_token: &str,
) -> Result<Option<i32>, String> {
    let Some(mut db) = open_optional_wake_mailbox()? else {
        return Ok(Some(0));
    };
    validate_auto_wake_claim_with_db(&mut db, session_id, claim_token)
}

fn open_optional_wake_mailbox() -> Result<Option<MailboxDb>, String> {
    MailboxDb::open_default_if_exists()
}

fn validate_auto_wake_claim_with_db(
    db: &mut MailboxDb,
    session_id: &str,
    claim_token: &str,
) -> Result<Option<i32>, String> {
    let child_identity = current_process_identity()?;
    db.wake_sessions()
        .validate_wake_claim_for_child(session_id, claim_token, &child_identity)
        .map(auto_wake_child_validation_result)
}

fn current_process_identity() -> Result<ProcessIdentity, String> {
    let pid = i64::from(std::process::id());
    read_live_process_identity(pid)?
        .ok_or_else(|| format!("Auto-wake child process {pid} is not live during claim admission"))
}

fn auto_wake_child_validation_result(valid: bool) -> Option<i32> {
    if valid { None } else { Some(0) }
}

pub(super) fn sleep_before_failed_auto_wake_retry(auto_wake_count: i64) {
    std::thread::sleep(auto_wake_retry_delay(auto_wake_count));
}

fn auto_wake_retry_delay(auto_wake_count: i64) -> Duration {
    Duration::from_millis(auto_wake_retry_delay_ms(auto_wake_count))
}

fn auto_wake_retry_delay_ms(auto_wake_count: i64) -> u64 {
    bounded_auto_wake_retry_delay_ms(auto_wake_retry_base_ms(), auto_wake_count)
}

fn bounded_auto_wake_retry_delay_ms(base_ms: u64, auto_wake_count: i64) -> u64 {
    let exponent = auto_wake_count.saturating_sub(1).clamp(0, 10) as u32;
    base_ms
        .saturating_mul(2_u64.saturating_pow(exponent))
        .min(AUTO_WAKE_RETRY_MAX_MS)
}

fn auto_wake_retry_base_ms() -> u64 {
    parsed_auto_wake_retry_base_ms()
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_AUTO_WAKE_RETRY_BASE_MS)
}

fn parsed_auto_wake_retry_base_ms() -> Option<u64> {
    parse_auto_wake_retry_base_ms(auto_wake_retry_base_ms_text())
}

fn auto_wake_retry_base_ms_text() -> Option<String> {
    std::env::var(AUTO_WAKE_RETRY_BASE_MS_ENV).ok()
}

fn parse_auto_wake_retry_base_ms(value: Option<String>) -> Option<u64> {
    value.and_then(|value| value.parse().ok())
}

pub(super) fn current_auto_wake_count(auto_wake: Option<&AutoWakeEnv>) -> i64 {
    auto_wake.map(|wake| wake.count).unwrap_or(0)
}

pub(super) fn current_auto_wake() -> Option<AutoWakeEnv> {
    auto_wake_marker_present()
        .then(current_auto_wake_env)
        .flatten()
}

fn current_auto_wake_env() -> Option<AutoWakeEnv> {
    Some(auto_wake_env(auto_wake_token()?, auto_wake_count()))
}

fn auto_wake_count() -> i64 {
    parse_auto_wake_count(auto_wake_count_value())
}

fn auto_wake_token() -> Option<String> {
    std::env::var(AUTO_WAKE_TOKEN_ENV).ok()
}

fn auto_wake_count_value() -> Option<String> {
    std::env::var(AUTO_WAKE_COUNT_ENV).ok()
}

fn parse_auto_wake_count(value: Option<String>) -> i64 {
    value.and_then(|value| value.parse().ok()).unwrap_or(1)
}

fn auto_wake_env(token: String, count: i64) -> AutoWakeEnv {
    AutoWakeEnv { token, count }
}

pub(super) fn release_current_auto_wake_claim(session_id: &str, auto_wake: Option<&AutoWakeEnv>) {
    let Some(auto_wake) = auto_wake else {
        return;
    };
    match MailboxDb::open_default_if_exists() {
        Ok(Some(mut db)) => release_wake_claim_or_warn(&mut db, session_id, &auto_wake.token),
        Ok(None) => {}
        Err(err) => warn_open_sidecar_for_release_failed(session_id, err),
    }
}

fn release_wake_claim_or_warn(db: &mut MailboxDb, session_id: &str, token: &str) {
    if let Err(err) = db
        .wake_sessions()
        .release_admitted_wake_claim(session_id, token)
    {
        warn_release_wake_claim_failed(session_id, err);
    }
}

fn warn_release_wake_claim_failed(session_id: &str, err: String) {
    tracing::warn!(session_id, "Failed to release wake claim: {err}");
}

fn warn_open_sidecar_for_release_failed(session_id: &str, err: String) {
    tracing::warn!(
        session_id,
        "Failed to open sidecar to release wake claim: {err}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_state::InboxTargetKind;
    use oulipoly_state::mailbox::{
        InboxTarget, SubmittedInputEnqueue, WakeClaimAcquireResult, WakeClaimRequest,
    };

    #[test]
    fn long_failed_wake_sequence_keeps_bounded_exponential_retry_cadence() {
        let delays = (1..=20)
            .map(|count| bounded_auto_wake_retry_delay_ms(1_000, count))
            .collect::<Vec<_>>();

        assert_eq!(&delays[..6], &[1_000, 2_000, 4_000, 8_000, 16_000, 30_000]);
        assert!(delays[6..].iter().all(|delay| *delay == 30_000));
    }

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
