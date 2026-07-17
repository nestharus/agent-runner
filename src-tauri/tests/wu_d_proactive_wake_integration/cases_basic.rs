//! ## Declared roles
//!
//! Roles: orchestration.
//!
//! TEST: proactive wake integration orchestration cases (basic wake/resume flows).

use crate::SESSION;
use crate::fake_cli::{notify_command, provider_script};
use crate::fixtures::Fixture;
use crate::liveness::{
    auto_wake_cap_left_pending, delivered_rows_without_claim, runtime_is_idle, wait_for_file,
    wait_for_mailbox_session, wait_for_runtime_session, wait_until,
};
use crate::parse::{identity, notify_wake};
use crate::test_guard::integration_test_guard;
use crate::validators::{
    assert_exit_code_zero, assert_handles_in_order, assert_no_wake_claim,
    assert_notification_prompt_header, assert_notify_success, assert_pending_mailbox_empty,
    assert_prompt_contains_agent_bash_complete, assert_prompt_contains_handle,
    assert_prompt_contains_rc_zero, assert_prompt_file_missing, assert_session_runtime_idle,
    assert_single_wake_child_launch, assert_single_wake_claim_won, assert_xdg_isolated,
};
use crate::wake_claim_setup::acquire_seed_wake_claim;
use oulipoly_state::mailbox::RuntimeLifecycleState;

pub(crate) fn idle_wake_delivers() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script(
        r#"( sleep 0.3; notify_handle h-idle 0 ) >/dev/null 2>&1 &"#,
        "",
        "resumed-input.txt",
    ));

    let output = fixture.run_agent("dispatch background work");
    assert_exit_code_zero(&output);

    let prompt = wait_for_file(&fixture.prompt_file("resumed-input.txt"));
    assert_notification_prompt_header(&prompt);
    assert_prompt_contains_agent_bash_complete(&prompt);
    assert_prompt_contains_handle(&prompt, "h-idle");
    assert_prompt_contains_rc_zero(&prompt);
    let session_id = wait_for_mailbox_session(&fixture);
    wait_until("mailbox delivered", || {
        delivered_rows_without_claim(&fixture, &session_id, 1)
    });
    fixture.assert_delivery_invocation_is_child_of_owner(&session_id);
    assert_pending_mailbox_empty(&fixture, &session_id);
    assert_session_runtime_idle(&fixture, &session_id);
    assert_no_wake_claim(&fixture, &session_id);
    assert_xdg_isolated(&fixture);
}

pub(crate) fn busy_then_turn_end_delivers() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script(
        r#"( sleep 0.1; notify_handle h-a 0 ) >/dev/null 2>&1 &
( sleep 0.2; notify_handle h-b 0 ) >/dev/null 2>&1 &
( sleep 0.3; notify_handle h-c 0 ) >/dev/null 2>&1 &
sleep 1"#,
        "",
        "busy-resumed-input.txt",
    ));

    let output = fixture.run_agent("dispatch busy work");
    assert_exit_code_zero(&output);

    let prompt = wait_for_file(&fixture.prompt_file("busy-resumed-input.txt"));
    assert_notification_prompt_header(&prompt);
    assert_handles_in_order(&prompt, &["h-a", "h-b", "h-c"]);
    let session_id = wait_for_mailbox_session(&fixture);
    wait_until("busy rows delivered", || {
        delivered_rows_without_claim(&fixture, &session_id, 3)
    });
    assert_no_wake_claim(&fixture, &session_id);
    assert_xdg_isolated(&fixture);
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

pub(crate) fn auto_wake_cap_stops_self_replicating_session() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script(
        r#"( sleep 0.3; notify_handle h-start 0 ) >/dev/null 2>&1 &"#,
        r#"count="${OULIPOLY_AUTO_WAKE_COUNT:-0}"
notify_handle "h-auto-${count}" 0"#,
        "resumed-${OULIPOLY_AUTO_WAKE_COUNT:-0}.txt",
    ));
    let output = fixture.run_agent_with_auto_wake_max("self replicate", "2");
    assert_exit_code_zero(&output);

    let first = wait_for_file(&fixture.prompt_file("resumed-1.txt"));
    let second = wait_for_file(&fixture.prompt_file("resumed-2.txt"));
    let session_id = wait_for_mailbox_session(&fixture);
    assert_prompt_contains_handle(&first, "h-start");
    assert_prompt_contains_handle(&second, "h-auto-1");
    wait_until("cap leaves pending", || {
        auto_wake_cap_left_pending(&fixture, &session_id)
    });
    assert_xdg_isolated(&fixture);
}

pub(crate) fn concurrent_notify_single_flight() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script(
        "",
        r#"printf 'wake\n' >> "$work/concurrent-wake-launches.log"
sleep 0.2"#,
        "concurrent-resume.txt",
    ));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    let identity = identity(9_200, "boot-concurrent", 123);
    fixture.record_identity(&identity);

    let child_a = notify_command(&fixture, "h-concurrent-a", &identity)
        .spawn()
        .unwrap();
    let child_b = notify_command(&fixture, "h-concurrent-b", &identity)
        .spawn()
        .unwrap();
    let output_a = child_a.wait_with_output().unwrap();
    let output_b = child_b.wait_with_output().unwrap();
    assert_notify_success(&output_a);
    assert_notify_success(&output_b);
    assert_single_wake_claim_won(&[notify_wake(&output_a), notify_wake(&output_b)]);

    let prompt = wait_for_file(&fixture.prompt_file("concurrent-resume.txt"));
    assert_prompt_contains_handle(&prompt, "h-concurrent-");
    wait_until("concurrent rows delivered", || {
        delivered_rows_without_claim(&fixture, SESSION, 2)
    });
    assert_single_wake_child_launch(&wait_for_file(
        &fixture.prompt_file("concurrent-wake-launches.log"),
    ));
    assert_no_wake_claim(&fixture, SESSION);
    let generations = fixture
        .mailbox()
        .runtime_generation_history(SESSION)
        .unwrap();
    assert_eq!(
        generations.len(),
        1,
        "concurrent notify spawned duplicate runtimes"
    );
    assert_eq!(
        generations[0].lifecycle_state,
        RuntimeLifecycleState::Exited
    );
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
    assert_exit_code_zero(&output);

    let prompt = wait_for_file(&fixture.prompt_file("manual-race.txt"));
    assert_prompt_contains_handle(&prompt, "h-manual-race");
    wait_until("manual race delivered", || {
        delivered_rows_without_claim(&fixture, SESSION, 1)
    });
    assert_xdg_isolated(&fixture);
}
