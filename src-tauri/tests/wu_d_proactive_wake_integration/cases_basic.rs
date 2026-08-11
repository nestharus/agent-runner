//! ## Declared roles
//!
//! Roles: accessor, filter, formatter, orchestration, parser, predicate, validator.
//!
//! TEST: proactive wake integration orchestration cases (basic wake/resume flows).

use crate::SESSION;
use crate::fake_cli::{
    delayed_agent_bash_provider_script, late_consumed_agent_bash_provider_script,
    mixed_consumed_agent_bash_provider_script, provider_script,
};
use crate::fixtures::Fixture;
use crate::liveness::{
    delivered_rows_without_claim, runtime_is_idle, wait_for_file, wait_for_runtime_session,
    wait_for_sidecar_session, wait_until,
};
use crate::test_guard::integration_test_guard;
use crate::validators::{
    assert_age270_invocation, assert_exit_code_zero, assert_no_wake_claim,
    assert_pending_mailbox_empty, assert_prompt_contains_handle, assert_prompt_excludes_handle,
    assert_prompt_file_missing, assert_xdg_isolated,
};
use crate::wake_claim_setup::acquire_seed_wake_claim;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const OUTER_SESSION: &str = "6169694d-de0f-40d1-890c-6e28e55bab28";
const OUTER_INVOCATION: &str = "22222222-2222-4222-8222-222222222222";
const OUTER_EVENT: &str = "h-outer-listener";

pub(crate) fn delayed_agent_bash_completion_wakes_inactive_headless_parent_once() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.seed_outer_caller(OUTER_SESSION, OUTER_INVOCATION, OUTER_EVENT);
    fixture.write_provider(&delayed_agent_bash_provider_script(&agent_bash_bin()));

    let initial = fixture.run_agent("dispatch delayed nested work");
    assert_exit_code_zero(&initial);
    assert_eq!(invocation_count(&fixture), 2);

    let handle = dispatch_handle(&fixture, "agent-bash-dispatch.json");
    let prompt = wait_for_file(&fixture.prompt_file("acr329-resumed-input.txt"));
    assert_prompt_contains_handle(&prompt, &handle);
    let session_id = wait_for_sidecar_session(&fixture, "mailbox");
    wait_for_automatic_delivery(&fixture, &session_id, 1);
    assert_delayed_completion_outcome(&fixture, &session_id, &handle);
}

pub(crate) fn polled_completion_after_enqueue_does_not_wake_parent() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.seed_outer_caller(OUTER_SESSION, OUTER_INVOCATION, OUTER_EVENT);
    fixture.write_provider(&late_consumed_agent_bash_provider_script(&agent_bash_bin()));

    let initial = fixture.run_agent("dispatch and poll fast nested work");
    assert_exit_code_zero(&initial);
    let poll = wait_for_file(&fixture.prompt_file("late-consumed-poll.txt"));
    assert_terminal_poll(&poll);
    let session_id = wait_for_sidecar_session(&fixture, "mailbox");
    wait_for_late_consumed_reconciliation(&fixture, &session_id);
    assert_late_consumed_completion_outcome(&fixture, &session_id);
}

pub(crate) fn consumed_completion_preserves_unpolled_completion_wake() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.seed_outer_caller(OUTER_SESSION, OUTER_INVOCATION, OUTER_EVENT);
    fixture.write_provider(&mixed_consumed_agent_bash_provider_script(&agent_bash_bin()));

    let initial = fixture.run_agent("dispatch consumed and unpolled nested work");
    assert_exit_code_zero(&initial);
    let poll = wait_for_file(&fixture.prompt_file("mixed-consumed-poll.txt"));
    assert_terminal_poll(&poll);
    let consumed_handle = dispatch_handle(&fixture, "mixed-consumed-dispatch.json");
    let unpolled_handle = dispatch_handle(&fixture, "mixed-unpolled-dispatch.json");
    let prompt = wait_for_file(&fixture.prompt_file("mixed-resumed-input.txt"));
    assert_prompt_excludes_handle(&prompt, &consumed_handle);
    assert_prompt_contains_handle(&prompt, &unpolled_handle);
    let session_id = wait_for_sidecar_session(&fixture, "mailbox");
    wait_for_automatic_delivery(&fixture, &session_id, 2);
    assert_mixed_completion_outcome(&fixture, &session_id, &consumed_handle, &unpolled_handle);
}

fn dispatch_handle(fixture: &Fixture, file_name: &str) -> String {
    parse_dispatch_handle(&wait_for_file(&fixture.prompt_file(file_name)))
}

fn parse_dispatch_handle(dispatch: &str) -> String {
    let dispatch: serde_json::Value = serde_json::from_str(dispatch).unwrap();
    dispatch["handle"].as_str().unwrap().to_string()
}

fn assert_delayed_completion_outcome(fixture: &Fixture, session_id: &str, handle: &str) {
    let rows = fixture.mailbox().list_mailbox(session_id, true).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].handle, handle);
    assert_outer_listener_preserved(fixture);
    let delivery_invocation = rows[0].delivered_by_invocation_uuid.as_deref().unwrap();
    assert_age270_invocation(fixture, delivery_invocation);
    fixture.assert_delivery_invocation_is_child_of_owner(session_id);
    assert_eq!(invocation_count(fixture), 3);

    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        fixture
            .mailbox()
            .list_mailbox(session_id, true)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(invocation_count(fixture), 3);
    assert_no_wake_claim(fixture, session_id);
    assert_xdg_isolated(fixture);
}

fn assert_outer_listener_preserved(fixture: &Fixture) {
    assert!(
        fixture
            .mailbox()
            .list_mailbox(OUTER_SESSION, true)
            .unwrap()
            .is_empty()
    );
    let outer_listeners = fixture
        .mailbox()
        .completion_event_listeners(OUTER_EVENT)
        .unwrap();
    assert_eq!(outer_listeners.len(), 1);
    assert!(outer_listeners[0].active);
    assert!(outer_listeners[0].mailbox_seq.is_none());
    assert!(outer_listeners[0].acknowledged_at.is_none());
}

fn assert_terminal_poll(poll: &str) {
    assert!(poll.starts_with("DONE rc=0"), "{poll}");
}

fn wait_for_late_consumed_reconciliation(fixture: &Fixture, session_id: &str) {
    wait_until("late consumed completion reconciled", || {
        late_consumed_completion_reconciled(fixture, session_id)
    });
}

fn late_consumed_completion_reconciled(fixture: &Fixture, session_id: &str) -> bool {
    pending_mailbox_rows(fixture, session_id).is_ok_and(pending_mailbox_rows_are_empty)
}

fn pending_mailbox_rows(
    fixture: &Fixture,
    session_id: &str,
) -> Result<Vec<oulipoly_state::mailbox::MailboxRow>, String> {
    fixture.mailbox().list_pending(session_id)
}

fn pending_mailbox_rows_are_empty(rows: Vec<oulipoly_state::mailbox::MailboxRow>) -> bool {
    rows.is_empty()
}

fn assert_late_consumed_completion_outcome(fixture: &Fixture, session_id: &str) {
    let rows = fixture.mailbox().list_mailbox(session_id, true).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].delivery_attempts, 1);
    assert_eq!(
        rows[0].delivered_by_invocation_uuid,
        rows[0].owner_invocation_uuid
    );
    let listeners = fixture
        .mailbox()
        .completion_event_listeners(&rows[0].handle)
        .unwrap();
    assert_eq!(listeners.len(), 1);
    assert!(!listeners[0].active);
    assert_eq!(
        listeners[0].acknowledgement_reason.as_deref(),
        Some("consumed_in_call")
    );
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(invocation_count(fixture), 2);
    assert_prompt_file_missing(fixture, "late-consumed-resumed-input.txt");
    assert_no_wake_claim(fixture, session_id);
    assert_xdg_isolated(fixture);
}

fn assert_mixed_completion_outcome(
    fixture: &Fixture,
    session_id: &str,
    consumed_handle: &str,
    unpolled_handle: &str,
) {
    let rows = fixture.mailbox().list_mailbox(session_id, true).unwrap();
    assert_eq!(rows.len(), 2);
    let consumed_row = mailbox_row_with_handle(&rows, consumed_handle);
    assert_eq!(
        consumed_row.delivered_by_invocation_uuid,
        consumed_row.owner_invocation_uuid
    );
    let unpolled_row = mailbox_row_with_handle(&rows, unpolled_handle);
    assert_ne!(
        unpolled_row.delivered_by_invocation_uuid,
        unpolled_row.owner_invocation_uuid
    );
    let delivery_invocation = unpolled_row
        .delivered_by_invocation_uuid
        .as_deref()
        .unwrap();
    assert_age270_invocation(fixture, delivery_invocation);
    fixture.assert_delivery_invocation_is_child_of_owner(session_id);
    assert_eq!(invocation_count(fixture), 3);
    assert_no_wake_claim(fixture, session_id);
    assert_xdg_isolated(fixture);
}

fn mailbox_row_with_handle<'a>(
    rows: &'a [oulipoly_state::mailbox::MailboxRow],
    handle: &str,
) -> &'a oulipoly_state::mailbox::MailboxRow {
    rows.iter().find(|row| row.handle == handle).unwrap()
}

fn wait_for_automatic_delivery(fixture: &Fixture, session_id: &str, expected_len: usize) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if automatic_delivery_settled(fixture, session_id, expected_len) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic_automatic_delivery_timeout(fixture, session_id);
}

fn automatic_delivery_settled(fixture: &Fixture, session_id: &str, expected_len: usize) -> bool {
    delivered_rows_without_claim(fixture, session_id, expected_len)
}

fn panic_automatic_delivery_timeout(fixture: &Fixture, session_id: &str) -> ! {
    let rows = fixture.mailbox().list_mailbox(session_id, true);
    let claim = fixture.mailbox().wake_claim(session_id);
    let runtime = fixture.mailbox().session_runtime(session_id);
    panic!(
        "{}",
        format_automatic_delivery_timeout(&rows, &claim, &runtime)
    );
}

fn format_automatic_delivery_timeout(
    rows: &impl std::fmt::Debug,
    claim: &impl std::fmt::Debug,
    runtime: &impl std::fmt::Debug,
) -> String {
    format!(
        "automatic delivery did not settle: rows={:?} claim={:?} runtime={:?}",
        rows, claim, runtime,
    )
}

fn agent_bash_bin() -> PathBuf {
    std::env::var_os("AGENT_BASH_BIN")
        .map(PathBuf::from)
        .or_else(find_agent_bash_in_path)
        .filter(|path| path.is_file())
        .expect("AGENT_BASH_BIN must name an agent-bash binary or agent-bash must be on PATH")
}

fn find_agent_bash_in_path() -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join("agent-bash"))
            .find(|path| path.is_file())
    })
}

fn invocation_count(fixture: &Fixture) -> i64 {
    fixture
        .state()
        .connection()
        .query_row("SELECT COUNT(*) FROM invocations", [], |row| row.get(0))
        .unwrap()
}

fn assert_direct_unconfirmed(output: &std::process::Output) -> String {
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines = stdout
        .lines()
        .filter_map(|line| line.strip_prefix("OULIPOLY_RESULT="))
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 1, "{stdout}");
    let result: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    let mut keys = result
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    keys.sort();
    assert_eq!(
        keys,
        [
            "agent_runner_chain_id",
            "agent_runner_invocation_id",
            "error_category",
            "exit_code",
            "finished_at",
            "id",
            "provider_name",
            "provider_session_id",
            "status",
            "success",
            "terminal_reason"
        ]
    );
    assert_eq!(result["status"], "failed");
    assert_eq!(result["success"], false);
    assert_eq!(result["exit_code"], 0);
    assert_eq!(result["error_category"], "resume_completion_unconfirmed");
    assert_eq!(result["terminal_reason"], "resume_completion_unconfirmed");
    assert_eq!(result["provider_name"], crate::PROVIDER);
    assert_eq!(result["provider_session_id"], SESSION);
    assert_eq!(result["agent_runner_invocation_id"], result["id"]);
    result["id"].as_str().unwrap().to_string()
}

fn assert_failed_delivery(fixture: &Fixture, invocation_id: &str) {
    let invocation = fixture
        .state()
        .get_invocation_by_uuid(invocation_id)
        .unwrap()
        .unwrap();
    assert_eq!(invocation.status, oulipoly_state::InvocationStatus::Failed);
    assert_eq!(invocation.success, Some(false));
    assert_eq!(invocation.exit_code, Some(0));
    assert_eq!(
        invocation.error_category.as_deref(),
        Some("resume_completion_unconfirmed")
    );
    assert_eq!(
        invocation.terminal_reason.as_deref(),
        Some("resume_completion_unconfirmed")
    );
    let rows = fixture.mailbox().list_mailbox(SESSION, true).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].delivered_by_invocation_uuid.as_deref(),
        Some(invocation_id)
    );
    assert_eq!(rows[0].delivery_attempts, 1);
    assert!(rows[0].delivery_error.is_none());
    let runtime = fixture.mailbox().session_runtime(SESSION).unwrap().unwrap();
    assert_eq!(runtime.run_state, "idle");
    assert_eq!(runtime.last_exit_code, Some(0));
    assert_no_wake_claim(fixture, SESSION);
}

pub(crate) fn no_undelivered_no_wake_and_loop_terminates() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "", "no-pending-resume.txt"));

    let output = fixture.run_agent("no pending");
    assert_exit_code_zero(&output);

    let session_id = wait_for_runtime_session(&fixture);
    wait_until("runtime idle", || runtime_is_idle(&fixture, &session_id));
    assert_pending_mailbox_empty(&fixture, &session_id);
    assert_no_wake_claim(&fixture, &session_id);
    assert_prompt_file_missing(&fixture, "no-pending-resume.txt");
    assert_xdg_isolated(&fixture);
}

pub(crate) fn manual_resume_race_is_safe() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "", "manual-race.txt"));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    fixture.seed_mailbox(SESSION, "h-manual-race");
    acquire_seed_wake_claim(&fixture, "manual-race-token");

    let output = fixture.run_resume();
    let invocation_id = assert_direct_unconfirmed(&output);

    let prompt = wait_for_file(&fixture.prompt_file("manual-race.txt"));
    assert_prompt_contains_handle(&prompt, "h-manual-race");
    wait_until("manual race delivered", || {
        delivered_rows_without_claim(&fixture, SESSION, 1)
    });
    assert_failed_delivery(&fixture, &invocation_id);
    assert_xdg_isolated(&fixture);
}
