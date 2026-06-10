//! ## Declared roles
//!
//! Roles: orchestration.
//!
//! TEST: proactive wake integration orchestration cases (opencode and pinned-data wake flows).

use crate::fake_cli::{notify_command, opencode_capture_provider_script, provider_script};
use crate::fixtures::Fixture;
use crate::liveness::{
    captured_opencode_mailbox_delivered, delivered_rows_without_claim,
    shadow_xdg_mailbox_delivered, wait_for_file, wait_for_mailbox_session, wait_until,
};
use crate::parse::{identity, json_value};
use crate::test_guard::integration_test_guard;
use crate::validators::{
    assert_capture_notify_enqueued, assert_capture_notify_owner,
    assert_capture_notify_session_source, assert_capture_notify_wake_busy, assert_exit_code_zero,
    assert_notify_success, assert_pending_mailbox_empty, assert_pid_identity_session_id,
    assert_prompt_contains_handle, assert_resumed_data_dir_pinned, assert_session_runtime_idle,
    assert_shadow_xdg_state_absent, assert_xdg_isolated,
};
use crate::{CAPTURED_OPENCODE_SESSION, MODEL};

pub(crate) fn opencode_notify_idle_wakes_resume_with_ses_session() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_opencode_provider(&provider_script(
        "",
        r#"if [ "$resume" != "ses_fixture" ]; then
  printf 'expected --session ses_fixture, got %s\n' "$resume" >&2
  exit 66
fi"#,
        "opencode-resumed.txt",
    ));
    fixture.seed_active_chain_for(
        "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        "opencode",
        "ses_fixture",
        MODEL,
    );
    fixture.seed_idle_runtime_for("ses_fixture", "opencode", MODEL);
    let identity = identity(9_400, "boot-opencode", 789);
    fixture.record_identity_for(&identity, "ses_fixture", "opencode", MODEL);

    let output = notify_command(&fixture, "h-opencode", &identity)
        .output()
        .unwrap();

    assert_notify_success(&output);
    let prompt = wait_for_file(&fixture.prompt_file("opencode-resumed.txt"));
    assert_prompt_contains_handle(&prompt, "h-opencode");
    wait_until("opencode mailbox delivered", || {
        delivered_rows_without_claim(&fixture, "ses_fixture", 1)
    });
    assert_pending_mailbox_empty(&fixture, "ses_fixture");
    assert_xdg_isolated(&fixture);
}

pub(crate) fn opencode_mid_turn_notify_resolves_capture_time_sidecar_owner() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_opencode_capture_provider(&opencode_capture_provider_script());
    fixture.seed_active_chain_for(
        "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
        "opencode",
        CAPTURED_OPENCODE_SESSION,
        MODEL,
    );

    let output = fixture.run_agent("dispatch capture race");
    assert_exit_code_zero(&output);

    let notify_json = wait_for_file(&fixture.work_dir.join("h-capture-midturn/notify.json"));
    let notify = json_value(&notify_json);
    assert_capture_notify_enqueued(&notify);
    assert_capture_notify_owner(&notify, CAPTURED_OPENCODE_SESSION);
    assert_capture_notify_session_source(&notify, "sidecar_session_id");
    assert_capture_notify_wake_busy(&notify);
    assert_pid_identity_session_id(&fixture, "opencode", CAPTURED_OPENCODE_SESSION);
    let prompt = wait_for_file(&fixture.prompt_file("opencode-capture-resumed.txt"));
    assert_prompt_contains_handle(&prompt, "h-capture-midturn");
    wait_until("captured opencode mailbox delivered", || {
        captured_opencode_mailbox_delivered(&fixture, CAPTURED_OPENCODE_SESSION)
    });
    assert_session_runtime_idle(&fixture, CAPTURED_OPENCODE_SESSION);
    assert_xdg_isolated(&fixture);
}

pub(crate) fn provider_shadow_xdg_notify_uses_pinned_data_dir_and_wakes() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script(
        r#"if [ -z "${OULIPOLY_DATA_DIR:-}" ]; then
  printf 'missing OULIPOLY_DATA_DIR\n' >&2
  exit 65
fi
export XDG_DATA_HOME="$work/shadow-xdg"
( sleep 0.3; notify_handle h-shadow-xdg 0 ) >/dev/null 2>&1 &"#,
        r#"printf '%s\n' "${OULIPOLY_DATA_DIR:-}" > "$work/shadow-resumed-data-dir.txt""#,
        "shadow-resumed-input.txt",
    ));

    let output = fixture.run_agent("dispatch from shadowed provider");
    assert_exit_code_zero(&output);

    let prompt = wait_for_file(&fixture.prompt_file("shadow-resumed-input.txt"));
    assert_prompt_contains_handle(&prompt, "h-shadow-xdg");
    let resumed_data_dir = wait_for_file(&fixture.prompt_file("shadow-resumed-data-dir.txt"));
    assert_resumed_data_dir_pinned(&fixture, &resumed_data_dir);
    let session_id = wait_for_mailbox_session(&fixture);
    wait_until("shadow-xdg mailbox delivered", || {
        shadow_xdg_mailbox_delivered(&fixture, &session_id)
    });
    assert_shadow_xdg_state_absent(&fixture);
    assert_xdg_isolated(&fixture);
}
