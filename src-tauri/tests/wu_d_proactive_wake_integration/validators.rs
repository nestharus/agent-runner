//! ## Declared roles
//!
//! Roles: validator.
//!
//! TEST: wake-claim, durable-mailbox, process-liveness, and command-output
//! validators for proactive wake integration cases.

use crate::fixtures::Fixture;
use oulipoly_state::mailbox::{MailboxRow, SessionRuntimeRow, WakeClaimRow};
use serde_json::Value;
use std::path::PathBuf;
use std::process::Output;

pub(crate) fn assert_exit_code_zero(output: &Output) {
    assert_eq!(output.status.code(), Some(0), "{output:?}");
}

pub(crate) fn assert_notify_success(output: &Output) {
    assert_success(output);
}

pub(crate) fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

pub(crate) fn assert_single_wake_claim_won(wakes: &[Value]) {
    let statuses = wake_statuses(wakes);
    assert_spawned_count(&statuses, 1);
    assert_unique_claim_token_count(&unique_claim_tokens(wakes), 1, wakes);
}

pub(crate) fn assert_single_wake_child_launch(log: &str) {
    assert_wake_launch_count(wake_launch_count(log), 1, log);
}

pub(crate) fn assert_notification_prompt_header(prompt: &str) {
    assert!(prompt.starts_with("[OULIPOLY NOTIFICATIONS]"), "{prompt}");
}

pub(crate) fn assert_prompt_contains_agent_bash_complete(prompt: &str) {
    assert!(prompt.contains("kind: agent_bash_complete"), "{prompt}");
}

pub(crate) fn assert_prompt_contains_handle(prompt: &str, handle: &str) {
    assert!(prompt.contains(&format!("handle: {handle}")), "{prompt}");
}

pub(crate) fn assert_prompt_excludes_handle(prompt: &str, handle: &str) {
    assert!(!prompt.contains(&format!("handle: {handle}")), "{prompt}");
}

pub(crate) fn assert_prompt_contains_rc_zero(prompt: &str) {
    assert!(prompt.contains("rc: 0"), "{prompt}");
}

pub(crate) fn assert_handles_in_order(prompt: &str, handles: &[&str]) {
    assert_positions_in_order(&handle_positions(prompt, handles), prompt);
}

pub(crate) fn assert_additional_notifications_remain_queued(prompt: &str) {
    assert!(
        prompt.contains("5 additional notification(s) remain queued"),
        "{prompt}"
    );
}

pub(crate) fn assert_pending_mailbox_empty(fixture: &Fixture, session_id: &str) {
    assert_rows_empty(&pending_mailbox_rows(fixture, session_id));
}

pub(crate) fn assert_pending_mailbox_count(fixture: &Fixture, session_id: &str, expected: usize) {
    assert_row_count(pending_mailbox_count(fixture, session_id), expected);
}

pub(crate) fn assert_no_wake_claim(fixture: &Fixture, session_id: &str) {
    assert_wake_claim_absent(&wake_claim(fixture, session_id));
}

pub(crate) fn assert_session_runtime_idle(fixture: &Fixture, session_id: &str) {
    assert_runtime_idle(&session_runtime_row(fixture, session_id));
}

pub(crate) fn assert_prompt_file_missing(fixture: &Fixture, name: &str) {
    assert_path_missing(prompt_file_path(fixture, name));
}

pub(crate) fn assert_pending_handle_without_error(
    fixture: &Fixture,
    session_id: &str,
    handle: &str,
) {
    assert_single_pending_handle_without_error(&pending_mailbox_rows(fixture, session_id), handle);
}

pub(crate) fn assert_pending_handle_with_delivery_attempts(
    fixture: &Fixture,
    session_id: &str,
    handle: &str,
    attempts: i64,
) {
    assert_single_pending_handle_with_attempts(
        &pending_mailbox_rows(fixture, session_id),
        handle,
        attempts,
    );
}

pub(crate) fn assert_live_claim_token(fixture: &Fixture, session_id: &str, claim_token: &str) {
    assert_claim_token(&required_wake_claim(fixture, session_id), claim_token);
}

pub(crate) fn assert_capture_notify_enqueued(notify: &Value) {
    assert_eq!(
        notify.get("status").and_then(Value::as_str),
        Some("enqueued")
    );
    assert_eq!(notify.get("enqueued").and_then(Value::as_bool), Some(true));
}

pub(crate) fn assert_capture_notify_owner(notify: &Value, session_id: &str) {
    assert_eq!(
        notify.get("owner_session_id").and_then(Value::as_str),
        Some(session_id)
    );
}

pub(crate) fn assert_capture_notify_session_source(notify: &Value, source: &str) {
    assert_eq!(
        notify.get("session_source").and_then(Value::as_str),
        Some(source)
    );
}

pub(crate) fn assert_capture_notify_wake_busy(notify: &Value) {
    assert_eq!(
        notify
            .get("wake")
            .and_then(|wake| wake.get("status"))
            .and_then(Value::as_str),
        Some("busy")
    );
}

pub(crate) fn assert_pid_identity_session_id(
    fixture: &Fixture,
    provider_name: &str,
    session_id: &str,
) {
    assert_provider_session_id(
        &fixture.pid_identity_session_id_for_provider(provider_name),
        session_id,
    );
}

pub(crate) fn assert_resumed_data_dir_pinned(fixture: &Fixture, resumed_data_dir: &str) {
    assert_data_dir_matches(
        resumed_data_dir.trim_end(),
        &pinned_data_dir_string(fixture),
    );
}

pub(crate) fn assert_shadow_xdg_state_absent(fixture: &Fixture) {
    assert_path_missing_with_message(
        shadow_xdg_state_path(fixture),
        "shadow XDG_DATA_HOME must not receive agent-runner state",
    );
}

pub(crate) fn assert_dead_owner_prompts_missing(fixture: &Fixture, session_ids: &[String]) {
    for session_id in session_ids {
        assert_path_missing_with_message(
            dead_owner_prompt_path(fixture, session_id),
            &format!("dead-owner debris must not be re-woken: {session_id}"),
        );
    }
}

pub(crate) fn assert_xdg_isolated(fixture: &Fixture) {
    assert_path_missing_with_message(
        xdg_data_state_path(fixture),
        "state must stay in isolated XDG_DATA_HOME",
    );
    assert_path_missing_with_message(
        xdg_config_state_path(fixture),
        "config must stay in isolated XDG_CONFIG_HOME",
    );
}

fn wake_statuses(wakes: &[Value]) -> Vec<&str> {
    wakes
        .iter()
        .filter_map(|wake| wake.get("status").and_then(Value::as_str))
        .collect()
}

fn assert_spawned_count(statuses: &[&str], expected: usize) {
    assert_eq!(
        spawned_count(statuses),
        expected,
        "wake statuses: {statuses:?}"
    );
}

fn spawned_count(statuses: &[&str]) -> usize {
    statuses
        .iter()
        .filter(|status| **status == "spawned")
        .count()
}

fn unique_claim_tokens(wakes: &[Value]) -> Vec<&str> {
    let mut claim_tokens = claim_tokens(wakes);
    claim_tokens.sort_unstable();
    claim_tokens.dedup();
    claim_tokens
}

fn claim_tokens(wakes: &[Value]) -> Vec<&str> {
    wakes
        .iter()
        .filter_map(|wake| wake.get("claim_token").and_then(Value::as_str))
        .collect()
}

fn assert_unique_claim_token_count(claim_tokens: &[&str], expected: usize, wakes: &[Value]) {
    assert_eq!(claim_tokens.len(), expected, "wake diagnostics: {wakes:?}");
}

fn wake_launch_count(log: &str) -> usize {
    log.lines().filter(|line| *line == "wake").count()
}

fn assert_wake_launch_count(actual: usize, expected: usize, log: &str) {
    assert_eq!(actual, expected, "wake launch log: {log:?}");
}

fn handle_positions(prompt: &str, handles: &[&str]) -> Vec<usize> {
    handles
        .iter()
        .map(|handle| prompt.find(&format!("handle: {handle}")).unwrap())
        .collect()
}

fn assert_positions_in_order(positions: &[usize], prompt: &str) {
    assert!(positions.windows(2).all(positions_are_in_order), "{prompt}");
}

fn positions_are_in_order(window: &[usize]) -> bool {
    window[0] < window[1]
}

fn pending_mailbox_rows(fixture: &Fixture, session_id: &str) -> Vec<MailboxRow> {
    fixture.mailbox().list_pending(session_id).unwrap()
}

fn pending_mailbox_count(fixture: &Fixture, session_id: &str) -> usize {
    pending_mailbox_rows(fixture, session_id).len()
}

fn wake_claim(fixture: &Fixture, session_id: &str) -> Option<WakeClaimRow> {
    fixture.mailbox().wake_claim(session_id).unwrap()
}

fn required_wake_claim(fixture: &Fixture, session_id: &str) -> WakeClaimRow {
    wake_claim(fixture, session_id).unwrap()
}

fn session_runtime_row(fixture: &Fixture, session_id: &str) -> SessionRuntimeRow {
    fixture
        .mailbox()
        .session_runtime(session_id)
        .unwrap()
        .unwrap()
}

fn prompt_file_path(fixture: &Fixture, name: &str) -> PathBuf {
    fixture.prompt_file(name)
}

fn pinned_data_dir_string(fixture: &Fixture) -> String {
    fixture.pinned_data_dir().to_string_lossy().into_owned()
}

fn shadow_xdg_state_path(fixture: &Fixture) -> PathBuf {
    fixture
        .work_dir
        .join("shadow-xdg")
        .join("oulipoly-agent-runner")
}

fn dead_owner_prompt_path(fixture: &Fixture, session_id: &str) -> PathBuf {
    fixture.prompt_file(&format!("backlog-{session_id}.txt"))
}

fn xdg_data_state_path(fixture: &Fixture) -> PathBuf {
    fixture.home_dir.join(".local/share/oulipoly-agent-runner")
}

fn xdg_config_state_path(fixture: &Fixture) -> PathBuf {
    fixture.home_dir.join(".config/oulipoly-agent-runner")
}

fn assert_rows_empty(rows: &[MailboxRow]) {
    assert!(rows.is_empty());
}

fn assert_row_count(actual: usize, expected: usize) {
    assert_eq!(actual, expected);
}

fn assert_wake_claim_absent(claim: &Option<WakeClaimRow>) {
    assert!(claim.is_none());
}

fn assert_runtime_idle(row: &SessionRuntimeRow) {
    assert_eq!(row.run_state, "idle");
}

fn assert_path_missing(path: PathBuf) {
    assert!(!path.exists());
}

fn assert_path_missing_with_message(path: PathBuf, message: &str) {
    assert!(!path.exists(), "{message}");
}

fn assert_single_pending_handle_without_error(rows: &[MailboxRow], handle: &str) {
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].handle, handle);
    assert!(rows[0].delivery_error.is_none());
}

fn assert_single_pending_handle_with_attempts(rows: &[MailboxRow], handle: &str, attempts: i64) {
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].handle, handle);
    assert_eq!(rows[0].delivery_attempts, attempts);
}

fn assert_claim_token(claim: &WakeClaimRow, claim_token: &str) {
    assert_eq!(claim.claim_token, claim_token);
}

fn assert_provider_session_id(actual: &str, expected: &str) {
    assert_eq!(actual, expected);
}

fn assert_data_dir_matches(actual: &str, expected: &str) {
    assert_eq!(actual, expected);
}
