//! ## Declared roles
//!
//! Roles: accessor, filter, formatter, orchestration, parser, predicate, validator.
//!
//! TEST: proactive wake integration orchestration cases (basic wake/resume flows).

use crate::SESSION;
use crate::fake_provider::{
    delayed_agent_bash_provider_script, late_received_agent_bash_provider_script,
    mixed_received_agent_bash_provider_script, provider_script,
};
use crate::fixtures::Fixture;
use crate::liveness::{
    delivered_rows_without_claim, runtime_is_idle, wait_for_file, wait_for_runtime_session,
    wait_for_sidecar_session, wait_until,
};
use crate::test_guard::integration_test_guard;
use crate::validators::{
    assert_age270_invocation, assert_exit_code_zero, assert_no_wake_claim,
    assert_pending_mailbox_empty, assert_prompt_contains_handle, assert_prompt_file_missing,
    assert_xdg_isolated,
};
use crate::wake_claim_setup::acquire_seed_wake_claim;
use std::path::PathBuf;
use std::process::Output;
use std::time::{Duration, Instant};

pub(crate) const OUTER_SESSION: &str = "6169694d-de0f-40d1-890c-6e28e55bab28";
const OUTER_EVENT: &str = "h-outer-listener";

pub(crate) fn delayed_agent_bash_completion_wakes_inactive_headless_parent_once() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    if fixture
        .run_under_outer_owner("delayed_agent_bash_completion_wakes_inactive_headless_parent_once")
    {
        return;
    }
    fixture.seed_outer_caller(OUTER_SESSION, OUTER_EVENT);
    let agent_bash = fixture.install_agent_bash(&agent_bash_bin());
    fixture.write_provider(&delayed_agent_bash_provider_script(&agent_bash));

    fixture.assert_missing_owner_rejected();
    let initial = fixture.run_agent("dispatch delayed nested work");
    assert_delayed_dispatch_exit_code_zero(&fixture, &initial);
    // Wake registration may precede the initial caller's return. Count only
    // after the actual receipt and assistant invocation have settled below.
    let handle = dispatch_handle(&fixture, "agent-bash-dispatch.json");
    let prompt = wait_for_file(&fixture.prompt_file("acr329-resumed-input.txt"));
    assert_prompt_contains_handle(&prompt, &handle);
    let session_id = wait_for_sidecar_session(&fixture, "mailbox");
    wait_for_automatic_delivery(&fixture, &session_id, 1);
    assert_delayed_completion_outcome(&fixture, &session_id, &handle);
}

fn assert_delayed_dispatch_exit_code_zero(fixture: &Fixture, output: &Output) {
    let dispatch_error = std::fs::read_to_string(fixture.prompt_file("agent-bash-dispatch.err"))
        .unwrap_or_else(|error| format!("unavailable: {error}"));
    assert_eq!(
        output.status.code(),
        Some(0),
        "{output:?}; agent-bash dispatch stderr: {dispatch_error}"
    );
}

pub(crate) fn local_receipt_after_enqueue_preserves_native_wake() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    if fixture.run_under_outer_owner("local_receipt_after_enqueue_preserves_native_wake") {
        return;
    }
    fixture.seed_outer_caller(OUTER_SESSION, OUTER_EVENT);
    let agent_bash = fixture.install_agent_bash(&agent_bash_bin());
    fixture.write_provider(&late_received_agent_bash_provider_script(&agent_bash));

    fixture.assert_missing_owner_rejected();
    let initial = fixture.run_agent("dispatch and poll fast nested work");
    assert_exit_code_zero(&initial);
    let poll = wait_for_file(&fixture.prompt_file("late-received-poll.txt"));
    assert_terminal_poll(&poll);
    let session_id = wait_for_sidecar_session(&fixture, "mailbox");
    let handle = dispatch_handle(&fixture, "late-received-dispatch.json");
    let prompt = wait_for_file(&fixture.prompt_file("late-received-resumed-input.txt"));
    assert_prompt_contains_handle(&prompt, &handle);
    wait_for_automatic_delivery(&fixture, &session_id, 1);
    assert_locally_received_completion_outcome(&fixture, &session_id, &[&handle]);
}

pub(crate) fn local_receipt_preserves_both_async_completion_wakes() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    if fixture.run_under_outer_owner("local_receipt_preserves_both_async_completion_wakes") {
        return;
    }
    fixture.seed_outer_caller(OUTER_SESSION, OUTER_EVENT);
    let agent_bash = fixture.install_agent_bash(&agent_bash_bin());
    fixture.write_provider(&mixed_received_agent_bash_provider_script(&agent_bash));

    fixture.assert_missing_owner_rejected();
    let initial = fixture.run_agent("dispatch locally received and unpolled nested work");
    assert_exit_code_zero(&initial);
    let poll = wait_for_file(&fixture.prompt_file("mixed-received-poll.txt"));
    assert_terminal_poll(&poll);
    let received_handle = dispatch_handle(&fixture, "mixed-received-dispatch.json");
    let unpolled_handle = dispatch_handle(&fixture, "mixed-unpolled-dispatch.json");
    let prompt = wait_for_file(&fixture.prompt_file("mixed-resumed-input.txt"));
    assert_prompt_contains_handle(&prompt, &received_handle);
    assert_prompt_contains_handle(&prompt, &unpolled_handle);
    let session_id = wait_for_sidecar_session(&fixture, "mailbox");
    wait_for_automatic_delivery(&fixture, &session_id, 2);
    assert_locally_received_completion_outcome(
        &fixture,
        &session_id,
        &[&received_handle, &unpolled_handle],
    );
}

fn dispatch_handle(fixture: &Fixture, file_name: &str) -> String {
    parse_dispatch_handle(&wait_for_file(&fixture.prompt_file(file_name)))
}

fn parse_dispatch_handle(dispatch: &str) -> String {
    let dispatch: serde_json::Value = serde_json::from_str(dispatch).unwrap();
    let handle = dispatch["handle"].as_str().unwrap();
    assert!(
        !handle.trim().is_empty(),
        "dispatch handle must be nonempty"
    );
    handle.to_string()
}

fn assert_delayed_completion_outcome(fixture: &Fixture, session_id: &str, handle: &str) {
    let rows = fixture.mailbox().list_mailbox(session_id, true).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].handle, handle);
    assert_outer_listener_preserved(fixture);
    let delivery_invocation = rows[0].delivered_by_invocation_uuid.as_deref().unwrap();
    wait_until("delayed assistant invocation finalized", || {
        fixture
            .state()
            .get_invocation_by_uuid(delivery_invocation)
            .unwrap()
            .is_some_and(|row| row.finished_at.is_some())
    });
    let invocation = fixture
        .state()
        .get_invocation_by_uuid(delivery_invocation)
        .unwrap()
        .unwrap();
    assert_eq!(
        invocation.status,
        oulipoly_state::InvocationStatus::Succeeded
    );
    assert_eq!(invocation.success, Some(true));
    assert_eq!(invocation.exit_code, Some(0));
    assert!(invocation.error_category.is_none());
    assert_eq!(rows[0].delivery_attempts, 1);
    assert!(rows[0].delivery_error.is_none());
    assert_pending_mailbox_empty(fixture, session_id);
    let listeners = fixture
        .mailbox()
        .completion_event_listeners(handle)
        .unwrap();
    assert_eq!(listeners.len(), 1);
    assert!(listeners[0].acknowledged_at.is_some());
    assert_eq!(
        listeners[0].acknowledgement_reason.as_deref(),
        Some("native_receipt")
    );
    assert_delayed_assistant_turns(fixture, handle);
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
    assert_delayed_assistant_turns(fixture, handle);
    assert_eq!(
        fixture.mailbox().list_mailbox(session_id, true).unwrap(),
        rows
    );
    assert_eq!(
        fixture
            .mailbox()
            .completion_event_listeners(handle)
            .unwrap(),
        listeners
    );
    assert_no_wake_claim(fixture, session_id);
    assert_xdg_isolated(fixture);
}

fn assert_delayed_assistant_turns(fixture: &Fixture, handle: &str) {
    let mut paths = std::fs::read_dir(fixture.prompt_file("session-turns"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    paths.sort();
    let turns = paths
        .iter()
        .map(|path| {
            serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(path).unwrap())
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        turns.len(),
        2,
        "exactly one receipt and one assistant completion"
    );
    assert_eq!(turns[0]["role"], "user");
    assert_prompt_contains_handle(turns[0]["body"][0]["text"].as_str().unwrap(), handle);
    assert_eq!(turns[1]["role"], "assistant");
    assert_eq!(
        turns[1]["body"][0]["text"],
        "Completed the nested work notification."
    );
    assert_eq!(
        wait_for_file(&fixture.prompt_file("provider-resume-sequence.txt")),
        "1"
    );
}

fn assert_outer_listener_preserved(fixture: &Fixture) {
    let parent: serde_json::Value =
        serde_json::from_str(&std::env::var("OULIPOLY_PARENT_INVOCATION").unwrap()).unwrap();
    let children: i64 = fixture.state().connection().query_row(
        "SELECT COUNT(*) FROM invocations child JOIN invocations parent ON child.parent_invocation_id=parent.id WHERE parent.invocation_uuid=?1",
        [parent["id"].as_str().unwrap()], |row| row.get(0)).unwrap();
    assert_eq!(
        children, 1,
        "initial entry is a child of the actual outer owner"
    );
    println!(
        "outer ancestry: actual invocation={} initial children={children}",
        parent["id"]
    );

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

// An async local byte receipt is neither listener ACK nor event-wide suppression.
// Only the later native user-turn receipt settles these rows. This fixture writes
// no assistant answer, so keep the independent AGE270 failure assertion intact.
fn assert_locally_received_completion_outcome(
    fixture: &Fixture,
    session_id: &str,
    handles: &[&str],
) {
    let rows = fixture.mailbox().list_mailbox(session_id, true).unwrap();
    assert_eq!(rows.len(), handles.len());
    let delivery_invocation = rows[0].delivered_by_invocation_uuid.as_deref().unwrap();
    for handle in handles {
        let row = mailbox_row_with_handle(&rows, handle);
        assert_ne!(row.delivered_by_invocation_uuid, row.owner_invocation_uuid);
        assert_eq!(
            row.delivered_by_invocation_uuid.as_deref(),
            Some(delivery_invocation)
        );
        assert_eq!(row.delivery_attempts, 1);
        assert!(row.delivery_error.is_none());
        let listeners = fixture
            .mailbox()
            .completion_event_listeners(handle)
            .unwrap();
        assert_eq!(listeners.len(), 1);
        assert!(listeners[0].acknowledged_at.is_some());
        assert_eq!(
            listeners[0].acknowledgement_reason.as_deref(),
            Some("native_receipt")
        );
    }
    wait_until("receipt-only delivery invocation finalized", || {
        fixture
            .state()
            .get_invocation_by_uuid(delivery_invocation)
            .unwrap()
            .is_some_and(|row| row.finished_at.is_some())
    });
    println!("actual native receipt rows: {rows:?}");
    for entry in std::fs::read_dir(&fixture.work_dir).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if [
            "-snapshot.json",
            "-receipt.json",
            "-pending-after-receipt.json",
        ]
        .iter()
        .any(|suffix| name.ends_with(suffix))
        {
            println!(
                "actual local receipt evidence {name}: {}",
                std::fs::read_to_string(entry.path()).unwrap()
            );
        }
    }
    assert_age270_invocation(fixture, delivery_invocation);
    assert_pending_mailbox_empty(fixture, session_id);
    assert_outer_listener_preserved(fixture);
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
    let claim = fixture
        .mailbox()
        .wake_session_reader()
        .wake_claim(session_id);
    let runtime = fixture
        .mailbox()
        .wake_session_reader()
        .session_metadata(session_id);
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
        .filter(|path| path.is_absolute() && path.is_file())
        .expect("AGENT_BASH_BIN must name the absolute source-qualified agent-bash binary (see .github/actions/install-agent-bash)")
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
    let runtime = fixture
        .mailbox()
        .wake_session_reader()
        .legacy_runtime_projection(SESSION)
        .unwrap()
        .unwrap();
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
