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
mod cases_opencode;
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
const CAPTURED_OPENCODE_SESSION: &str = "ses_capturemidturn";
const INVOCATION: &str = "11111111-1111-4111-8111-111111111111";

#[test]
fn idle_wake_delivers() {
    cases_basic::idle_wake_delivers();
}

#[test]
fn busy_then_turn_end_delivers() {
    cases_basic::busy_then_turn_end_delivers();
}

#[test]
fn no_undelivered_no_wake_and_loop_terminates() {
    cases_basic::no_undelivered_no_wake_and_loop_terminates();
}

#[test]
fn auto_wake_cap_stops_self_replicating_session() {
    cases_basic::auto_wake_cap_stops_self_replicating_session();
}

#[test]
fn concurrent_notify_single_flight() {
    cases_basic::concurrent_notify_single_flight();
}

#[test]
fn manual_resume_race_is_safe() {
    cases_basic::manual_resume_race_is_safe();
}

#[test]
fn opencode_notify_idle_wakes_resume_with_ses_session() {
    cases_opencode::opencode_notify_idle_wakes_resume_with_ses_session();
}

#[test]
fn opencode_mid_turn_notify_resolves_capture_time_sidecar_owner() {
    cases_opencode::opencode_mid_turn_notify_resolves_capture_time_sidecar_owner();
}

#[test]
fn batch_cap_followup_wake() {
    cases_batch_sweep::batch_cap_followup_wake();
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
fn wake_sweep_skips_twice_unconfirmed_rows_and_delivers_newer_pending_mailbox() {
    cases_wake_backlog::wake_sweep_skips_twice_unconfirmed_rows_and_delivers_newer_pending_mailbox(
    );
}

#[test]
fn wake_sweep_backlog_recovers_recent_leak_and_reaps_dead_owner_debris() {
    cases_wake_backlog::wake_sweep_backlog_recovers_recent_leak_and_reaps_dead_owner_debris();
}

#[test]
fn provider_shadow_xdg_notify_uses_pinned_data_dir_and_wakes() {
    cases_opencode::provider_shadow_xdg_notify_uses_pinned_data_dir_and_wakes();
}
