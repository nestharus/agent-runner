//! ## Declared roles
//!
//! Roles: validator.
//!
//! TEST: durable-mailbox, wake-claim, filesystem, and command-output
//! validators for proactive wake integration cases.

use crate::fixtures::Fixture;
use oulipoly_state::mailbox::{MailboxRow, WakeClaimRow};
use std::path::PathBuf;
use std::process::Output;

pub(crate) fn assert_exit_code_zero(output: &Output) {
    assert_eq!(output.status.code(), Some(0), "{output:?}");
}

pub(crate) fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

pub(crate) fn assert_age270_invocation(fixture: &Fixture, invocation_id: &str) {
    let row = fixture
        .state()
        .get_invocation_by_uuid(invocation_id)
        .unwrap()
        .unwrap();
    assert_eq!(row.status, oulipoly_state::InvocationStatus::Failed);
    assert_eq!(row.success, Some(false));
    assert_eq!(row.exit_code, Some(0));
    assert_eq!(
        row.error_category.as_deref(),
        Some("resume_completion_unconfirmed")
    );
    assert_eq!(
        row.terminal_reason.as_deref(),
        Some("resume_completion_unconfirmed")
    );
    assert!(row.finished_at.is_some());
}

pub(crate) fn assert_prompt_contains_handle(prompt: &str, handle: &str) {
    assert!(prompt.contains(&format!("handle: {handle}")), "{prompt}");
}

pub(crate) fn assert_prompt_excludes_handle(prompt: &str, handle: &str) {
    assert!(!prompt.contains(&format!("handle: {handle}")), "{prompt}");
}

pub(crate) fn assert_additional_notifications_remain_queued(prompt: &str) {
    assert!(
        prompt.contains("5 additional notification(s) remain queued"),
        "{prompt}"
    );
}

pub(crate) fn assert_pending_mailbox_empty(fixture: &Fixture, session_id: &str) {
    let rows = pending_mailbox_rows(fixture, session_id);
    assert!(rows.is_empty(), "unexpected pending rows: {rows:?}");
}

pub(crate) fn assert_pending_mailbox_count(fixture: &Fixture, session_id: &str, expected: usize) {
    let rows = pending_mailbox_rows(fixture, session_id);
    assert_eq!(rows.len(), expected, "pending rows were {rows:?}");
}

pub(crate) fn assert_no_wake_claim(fixture: &Fixture, session_id: &str) {
    assert!(wake_claim(fixture, session_id).is_none());
}

pub(crate) fn assert_prompt_file_missing(fixture: &Fixture, name: &str) {
    assert!(!fixture.prompt_file(name).exists());
}

pub(crate) fn assert_pending_handle_without_error(
    fixture: &Fixture,
    session_id: &str,
    handle: &str,
) {
    let rows = pending_mailbox_rows(fixture, session_id);
    assert_eq!(rows.len(), 1, "pending rows were {rows:?}");
    assert_eq!(rows[0].handle, handle, "pending rows were {rows:?}");
    assert!(
        rows[0].delivery_error.is_none(),
        "pending rows were {rows:?}"
    );
}

pub(crate) fn assert_pending_handle_with_delivery_attempts(
    fixture: &Fixture,
    session_id: &str,
    handle: &str,
    attempts: i64,
) {
    let rows = pending_mailbox_rows(fixture, session_id);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].handle, handle);
    assert_eq!(rows[0].delivery_attempts, attempts);
}

pub(crate) fn assert_live_claim_token(fixture: &Fixture, session_id: &str, claim_token: &str) {
    assert_eq!(
        required_wake_claim(fixture, session_id).claim_token,
        claim_token
    );
}

pub(crate) fn assert_dead_owner_prompts_missing(fixture: &Fixture, session_ids: &[String]) {
    for session_id in session_ids {
        assert_path_missing_with_message(
            fixture.prompt_file(&format!("backlog-{session_id}.txt")),
            &format!("dead-owner debris must not be re-woken: {session_id}"),
        );
    }
}

pub(crate) fn assert_xdg_isolated(fixture: &Fixture) {
    assert_path_missing_with_message(
        fixture.home_dir.join(".local/share/oulipoly-agent-runner"),
        "state must stay in isolated XDG_DATA_HOME",
    );
    assert_path_missing_with_message(
        fixture.home_dir.join(".config/oulipoly-agent-runner"),
        "config must stay in isolated XDG_CONFIG_HOME",
    );
}

fn pending_mailbox_rows(fixture: &Fixture, session_id: &str) -> Vec<MailboxRow> {
    fixture.mailbox().list_pending(session_id).unwrap()
}

fn wake_claim(fixture: &Fixture, session_id: &str) -> Option<WakeClaimRow> {
    fixture.mailbox().wake_claim(session_id).unwrap()
}

fn required_wake_claim(fixture: &Fixture, session_id: &str) -> WakeClaimRow {
    wake_claim(fixture, session_id).unwrap()
}

fn assert_path_missing_with_message(path: PathBuf, message: &str) {
    assert!(!path.exists(), "{message}");
}
