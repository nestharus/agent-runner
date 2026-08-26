#![cfg(unix)]

//! ## Declared roles
//!
//! Roles: orchestration.
//!
//! TEST: proactive wake and wake-reclaim end-to-end fixtures — module entry
//! for orchestration cases and single-concern helper modules.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/tests/wu_d_proactive_wake_integration/main.rs
//!     role: adapter
//!     Translates:
//!       - runtime-cli-dispatch-contract
//!       - wake-claim-sidecar-contract
//!       - pid-identity-sidecar-contract
//!       - mailbox-delivery-contract
//!       - test-fixture-process-contract
//! intrinsic_surface_declarations:
//!   - component: src-tauri/tests/wu_d_proactive_wake_integration/main.rs
//!     role: intrinsic-surface
//!     Domain: proactive wake and wake-reclaim regression suite
//!     Owns:
//!       - integration test binary entrypoint
//!       - orchestration module declarations
//! ```

mod cases_basic;
mod cases_batch_sweep;
mod cases_wake_backlog;
mod fake_cli;
mod fixtures;
mod liveness;
mod model_config;
mod parse;
mod state_mailbox;
mod test_guard;
mod validators;
mod wake_claim_setup;

const MODEL: &str = "wu-d-fixture-model";
const PROVIDER: &str = "wu-d-fixture-provider";
const SESSION: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
const INVOCATION: &str = "11111111-1111-4111-8111-111111111111";

#[test]
fn delayed_agent_bash_completion_wakes_inactive_headless_parent_once() {
    cases_basic::delayed_agent_bash_completion_wakes_inactive_headless_parent_once();
}

#[test]
fn polled_completion_after_enqueue_does_not_wake_parent() {
    cases_basic::polled_completion_after_enqueue_does_not_wake_parent();
}

#[test]
fn consumed_completion_preserves_unpolled_completion_wake() {
    cases_basic::consumed_completion_preserves_unpolled_completion_wake();
}

#[test]
fn no_undelivered_no_wake_and_loop_terminates() {
    cases_basic::no_undelivered_no_wake_and_loop_terminates();
}

#[test]
fn manual_resume_race_is_safe() {
    cases_basic::manual_resume_race_is_safe();
}

#[test]
fn persisted_count_at_five_allows_turn_end_followup_wake() {
    cases_batch_sweep::persisted_count_at_five_allows_turn_end_followup_wake();
}

#[test]
fn wake_sweep_reclaims_dead_claim_and_delivers_pending_mailbox() {
    cases_batch_sweep::wake_sweep_reclaims_dead_claim_and_delivers_pending_mailbox();
}

#[test]
fn wake_sweep_does_not_resurrect_abandoned_transient_session() {
    cases_batch_sweep::wake_sweep_does_not_resurrect_abandoned_transient_session();
}

#[test]
fn wake_sweep_reaps_non_resumable_abandoned_transient_session() {
    cases_batch_sweep::wake_sweep_reaps_non_resumable_abandoned_transient_session();
}

#[test]
fn wake_sweep_reaps_dead_owner_session_with_chain_but_no_turns() {
    cases_batch_sweep::wake_sweep_reaps_dead_owner_session_with_chain_but_no_turns();
}

#[test]
fn wake_sweep_delivers_resumable_session_missing_models_dir() {
    cases_batch_sweep::wake_sweep_delivers_resumable_session_missing_models_dir();
}

#[test]
fn wake_sweep_does_not_disturb_live_identity_matched_claim() {
    cases_batch_sweep::wake_sweep_does_not_disturb_live_identity_matched_claim();
}

#[test]
fn wake_sweep_does_not_rewake_consumed_pending_mailbox() {
    cases_batch_sweep::wake_sweep_does_not_rewake_consumed_pending_mailbox();
}

#[test]
fn wake_sweep_does_not_rewake_twice_unconfirmed_pending_mailbox() {
    cases_batch_sweep::wake_sweep_does_not_rewake_twice_unconfirmed_pending_mailbox();
}

#[test]
fn persisted_count_at_five_allows_startup_sweep_delivery() {
    cases_batch_sweep::persisted_count_at_five_allows_startup_sweep_delivery();
}

#[test]
fn wake_sweep_skips_twice_unconfirmed_rows_and_delivers_newer_pending_mailbox() {
    cases_wake_backlog::wake_sweep_skips_twice_unconfirmed_rows_and_delivers_newer_pending_mailbox(
    );
}

#[test]
fn wake_sweep_backlog_recovers_recent_leak_and_reaps_dead_owner_debris() {
    cases_wake_backlog::wake_sweep_backlog_recovers_recent_leak_and_reaps_dead_owner_debris();
}
