//! ## Declared roles
//!
//! Roles: validator.
//!
//! TEST: wake-claim, durable-mailbox, process-liveness, and command-output
//! validators for proactive wake integration cases.

use crate::fixtures::Fixture;
use serde_json::Value;
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
    let statuses = wakes
        .iter()
        .filter_map(|wake| wake.get("status").and_then(Value::as_str))
        .collect::<Vec<_>>();
    let spawned_count = statuses
        .iter()
        .filter(|status| **status == "spawned")
        .count();
    assert_eq!(spawned_count, 1, "wake statuses: {statuses:?}");

    let mut claim_tokens = wakes
        .iter()
        .filter_map(|wake| wake.get("claim_token").and_then(Value::as_str))
        .collect::<Vec<_>>();
    claim_tokens.sort_unstable();
    claim_tokens.dedup();
    assert_eq!(claim_tokens.len(), 1, "wake diagnostics: {wakes:?}");
}

pub(crate) fn assert_single_wake_child_launch(log: &str) {
    let launches = log.lines().filter(|line| *line == "wake").count();
    assert_eq!(launches, 1, "wake launch log: {log:?}");
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
    let positions = handles
        .iter()
        .map(|handle| prompt.find(&format!("handle: {handle}")).unwrap())
        .collect::<Vec<_>>();
    assert!(
        positions.windows(2).all(|window| window[0] < window[1]),
        "{prompt}"
    );
}

pub(crate) fn assert_additional_notifications_remain_queued(prompt: &str) {
    assert!(
        prompt.contains("5 additional notification(s) remain queued"),
        "{prompt}"
    );
}

pub(crate) fn assert_pending_mailbox_empty(fixture: &Fixture, session_id: &str) {
    assert!(
        fixture
            .mailbox()
            .list_pending(session_id)
            .unwrap()
            .is_empty()
    );
}

pub(crate) fn assert_pending_mailbox_count(fixture: &Fixture, session_id: &str, expected: usize) {
    assert_eq!(
        fixture.mailbox().list_pending(session_id).unwrap().len(),
        expected
    );
}

pub(crate) fn assert_no_wake_claim(fixture: &Fixture, session_id: &str) {
    assert!(fixture.mailbox().wake_claim(session_id).unwrap().is_none());
}

pub(crate) fn assert_session_runtime_idle(fixture: &Fixture, session_id: &str) {
    assert_eq!(
        fixture
            .mailbox()
            .session_runtime(session_id)
            .unwrap()
            .unwrap()
            .run_state,
        "idle"
    );
}

pub(crate) fn assert_prompt_file_missing(fixture: &Fixture, name: &str) {
    assert!(!fixture.prompt_file(name).exists());
}

pub(crate) fn assert_pending_handle_without_error(
    fixture: &Fixture,
    session_id: &str,
    handle: &str,
) {
    let rows = fixture.mailbox().list_pending(session_id).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].handle, handle);
    assert!(rows[0].delivery_error.is_none());
}

pub(crate) fn assert_pending_handle_with_delivery_attempts(
    fixture: &Fixture,
    session_id: &str,
    handle: &str,
    attempts: i64,
) {
    let rows = fixture.mailbox().list_pending(session_id).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].handle, handle);
    assert_eq!(rows[0].delivery_attempts, attempts);
}

pub(crate) fn assert_live_claim_token(fixture: &Fixture, session_id: &str, claim_token: &str) {
    let claim = fixture.mailbox().wake_claim(session_id).unwrap().unwrap();
    assert_eq!(claim.claim_token, claim_token);
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
    assert_eq!(
        fixture.pid_identity_session_id_for_provider(provider_name),
        session_id
    );
}

pub(crate) fn assert_resumed_data_dir_pinned(fixture: &Fixture, resumed_data_dir: &str) {
    let expected_data_dir = fixture.pinned_data_dir();
    let expected_data_dir = expected_data_dir.to_string_lossy();
    assert_eq!(resumed_data_dir.trim_end(), expected_data_dir.as_ref());
}

pub(crate) fn assert_shadow_xdg_state_absent(fixture: &Fixture) {
    assert!(
        !fixture
            .work_dir
            .join("shadow-xdg")
            .join("oulipoly-agent-runner")
            .exists(),
        "shadow XDG_DATA_HOME must not receive agent-runner state"
    );
}

pub(crate) fn assert_dead_owner_prompts_missing(fixture: &Fixture, session_ids: &[String]) {
    for session_id in session_ids {
        assert!(
            !fixture
                .prompt_file(&format!("backlog-{session_id}.txt"))
                .exists(),
            "dead-owner debris must not be re-woken: {session_id}"
        );
    }
}

pub(crate) fn assert_xdg_isolated(fixture: &Fixture) {
    assert!(
        !fixture
            .home_dir
            .join(".local/share/oulipoly-agent-runner")
            .exists(),
        "state must stay in isolated XDG_DATA_HOME"
    );
    assert!(
        !fixture
            .home_dir
            .join(".config/oulipoly-agent-runner")
            .exists(),
        "config must stay in isolated XDG_CONFIG_HOME"
    );
}
