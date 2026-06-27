//! ## Declared roles
//!
//! Roles: orchestration.
//!
//! TEST: proactive wake integration orchestration cases (batch delivery and wake-sweep regressions).

use crate::fake_cli::provider_script;
use crate::fixtures::Fixture;
use crate::liveness::{
    assert_dead_owner_debris_reaped, delivered_rows_without_pending_or_claim,
    delivered_single_row_without_error_or_claim, settle_wake_sweep, wait_for_file, wait_until,
};
use crate::test_guard::integration_test_guard;
use crate::validators::{
    assert_additional_notifications_remain_queued, assert_exit_code_zero, assert_live_claim_token,
    assert_no_wake_claim, assert_pending_handle_with_delivery_attempts,
    assert_pending_handle_without_error, assert_pending_mailbox_count,
    assert_prompt_contains_handle, assert_prompt_file_missing, assert_success, assert_xdg_isolated,
};
use crate::wake_claim_setup::{seed_dead_wake_claim, seed_live_wake_claim};
use crate::{MODEL, PROVIDER, SESSION};

pub(crate) fn batch_cap_followup_wake() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script(
        "",
        "",
        "batch-${OULIPOLY_AUTO_WAKE_COUNT:-manual}.txt",
    ));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    for index in 0..25 {
        fixture.seed_mailbox(SESSION, &format!("h-batch-{index:02}"));
    }

    let output = fixture.run_resume();
    assert_exit_code_zero(&output);

    let first = wait_for_file(&fixture.prompt_file("batch-manual.txt"));
    let second = wait_for_file(&fixture.prompt_file("batch-1.txt"));
    assert_additional_notifications_remain_queued(&first);
    assert_prompt_contains_handle(&second, "h-batch-20");
    wait_until("batch rows delivered", || {
        delivered_rows_without_pending_or_claim(&fixture, SESSION, 25)
    });
    assert_xdg_isolated(&fixture);
}

pub(crate) fn wake_sweep_reclaims_dead_claim_and_delivers_pending_mailbox() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "", "sweep-reclaimed.txt"));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    fixture.seed_mailbox(SESSION, "h-sweep-reclaim");
    seed_dead_wake_claim(&fixture, "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa", 601);

    let output = fixture.run_mailbox_list(SESSION);
    assert_success(&output);

    let prompt = wait_for_file(&fixture.prompt_file("sweep-reclaimed.txt"));
    assert_prompt_contains_handle(&prompt, "h-sweep-reclaim");
    wait_until("sweep reclaimed dead claim and delivered mailbox", || {
        delivered_single_row_without_error_or_claim(&fixture, SESSION)
    });
    assert_xdg_isolated(&fixture);
}

pub(crate) fn wake_sweep_does_not_resurrect_abandoned_transient_session() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "", "abandoned-transient-resumed.txt"));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    fixture.seed_mailbox(SESSION, "h-abandoned-transient");

    let output = fixture.run_mailbox_list(SESSION);
    assert_success(&output);
    settle_wake_sweep();

    assert_prompt_file_missing(&fixture, "abandoned-transient-resumed.txt");
    assert_pending_handle_without_error(&fixture, SESSION, "h-abandoned-transient");
    assert_no_wake_claim(&fixture, SESSION);
    assert_xdg_isolated(&fixture);
}

pub(crate) fn wake_sweep_reaps_non_resumable_abandoned_transient_session() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script(
        "",
        "",
        "non-resumable-transient-resumed.txt",
    ));
    // Idle headless runtime with a dead-owner pending row, but NO session turn /
    // chain -> no durable resume evidence. The session is never auto-woken
    // (anti-resurrection) and, being non-resumable, its undeliverable row is reaped.
    fixture.seed_idle_runtime();
    fixture.seed_mailbox(SESSION, "h-non-resumable-transient");

    let output = fixture.run_mailbox_list(SESSION);
    assert_success(&output);
    settle_wake_sweep();

    assert_prompt_file_missing(&fixture, "non-resumable-transient-resumed.txt");
    assert_dead_owner_debris_reaped(&fixture, SESSION);
    assert_no_wake_claim(&fixture, SESSION);
    assert_xdg_isolated(&fixture);
}

pub(crate) fn wake_sweep_reaps_dead_owner_session_with_chain_but_no_turns() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "", "chain-no-turns-resumed.txt"));
    // A registered chain segment with ZERO produced turns is an empty resume
    // target, not durable work. With a dead owner, it must be reaped (not
    // preserved as if resumable) and never auto-woken.
    fixture.seed_active_chain_for(
        "33333333-3333-4333-8333-333333333333",
        PROVIDER,
        SESSION,
        MODEL,
    );
    fixture.seed_idle_runtime();
    fixture.seed_mailbox(SESSION, "h-chain-no-turns");

    let output = fixture.run_mailbox_list(SESSION);
    assert_success(&output);
    settle_wake_sweep();

    assert_prompt_file_missing(&fixture, "chain-no-turns-resumed.txt");
    assert_dead_owner_debris_reaped(&fixture, SESSION);
    assert_no_wake_claim(&fixture, SESSION);
    assert_xdg_isolated(&fixture);
}

pub(crate) fn wake_sweep_delivers_resumable_session_missing_models_dir() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "", "missing-models-dir-resumed.txt"));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime_without_models_dir(SESSION);
    fixture.seed_mailbox_for(SESSION, "h-missing-models-dir", None);
    seed_dead_wake_claim(&fixture, "dddddddd-dddd-4ddd-8ddd-dddddddddddd", 601);

    let output = fixture.run_mailbox_list(SESSION);
    assert_success(&output);

    let prompt = wait_for_file(&fixture.prompt_file("missing-models-dir-resumed.txt"));
    assert_prompt_contains_handle(&prompt, "h-missing-models-dir");
    wait_until("missing models_dir wake delivered", || {
        delivered_single_row_without_error_or_claim(&fixture, SESSION)
    });
    assert_xdg_isolated(&fixture);
}

pub(crate) fn wake_sweep_does_not_disturb_live_identity_matched_claim() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "", "live-claim-not-disturbed.txt"));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    fixture.seed_mailbox(SESSION, "h-live-claim");
    seed_live_wake_claim(&fixture, "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb");

    let output = fixture.run_mailbox_list(SESSION);
    assert_success(&output);
    settle_wake_sweep();

    assert_prompt_file_missing(&fixture, "live-claim-not-disturbed.txt");
    assert_live_claim_token(&fixture, SESSION, "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb");
    assert_pending_mailbox_count(&fixture, SESSION, 1);
    assert_xdg_isolated(&fixture);
}

pub(crate) fn wake_sweep_does_not_rewake_consumed_pending_mailbox() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "", "consumed-not-rewoken.txt"));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    fixture.seed_mailbox(SESSION, "h-consumed");
    fixture.seed_consumed_notification_turn("h-consumed");
    seed_dead_wake_claim(&fixture, "cccccccc-cccc-4ccc-8ccc-cccccccccccc", 601);

    let output = fixture.run_mailbox_list(SESSION);
    assert_success(&output);
    settle_wake_sweep();

    assert_prompt_file_missing(&fixture, "consumed-not-rewoken.txt");
    assert_pending_mailbox_count(&fixture, SESSION, 1);
    assert_xdg_isolated(&fixture);
}

pub(crate) fn wake_sweep_does_not_rewake_twice_unconfirmed_pending_mailbox() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script(
        "",
        "",
        "twice-unconfirmed-not-rewoken.txt",
    ));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    fixture.seed_mailbox(SESSION, "h-unconfirmed");
    fixture.mark_mailbox_unconfirmed_twice(SESSION, "h-unconfirmed");
    seed_dead_wake_claim(&fixture, "dddddddd-dddd-4ddd-8ddd-dddddddddddd", 601);

    let output = fixture.run_mailbox_list(SESSION);
    assert_success(&output);
    settle_wake_sweep();

    assert_prompt_file_missing(&fixture, "twice-unconfirmed-not-rewoken.txt");
    assert_pending_handle_with_delivery_attempts(&fixture, SESSION, "h-unconfirmed", 2);
    assert_xdg_isolated(&fixture);
}
