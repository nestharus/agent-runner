//! ## Declared roles
//!
//! Roles: orchestration.
//!
//! TEST: proactive wake integration orchestration cases (basic wake/resume flows).

use crate::SESSION;
use crate::fake_cli::provider_script;
use crate::fixtures::Fixture;
use crate::liveness::{
    delivered_rows_without_claim, runtime_is_idle, wait_for_file, wait_for_runtime_session,
    wait_until,
};
use crate::test_guard::integration_test_guard;
use crate::validators::{
    assert_exit_code_zero, assert_no_wake_claim, assert_pending_mailbox_empty,
    assert_prompt_contains_handle, assert_prompt_file_missing, assert_xdg_isolated,
};
use crate::wake_claim_setup::acquire_seed_wake_claim;

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
