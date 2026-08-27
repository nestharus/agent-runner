//! ## Declared roles
//!
//! `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `parser`,
//! `predicate`, `validator`

mod admission;
mod auto_wake_env;
mod constants;
mod consumed_completion;
mod diagnostics;
mod idle;
mod spawn;
mod sweep;
mod turn_recheck;
mod wake_start;

pub(crate) type WakeDiagnostic = diagnostics::WakeDiagnostic;
pub(crate) use sweep::{LivePtyRetryDriverGuard, StartupWakeReclaimGuard};

pub(crate) use admission::SessionAdmissionGuard;

pub(crate) fn admit_session_launch(
    registration_identity: &str,
    session_id: Option<&str>,
) -> Result<SessionAdmissionGuard, String> {
    admission::enqueue_and_wait(registration_identity, session_id)
}

pub(crate) fn mark_session_idle_after_turn(
    session_id: &str,
    invocation_uuid: &str,
    exit_code: Option<i32>,
) -> Result<(), String> {
    idle::mark_session_idle_after_turn(session_id, invocation_uuid, exit_code)
}

pub(crate) fn trigger_notify_wake(session_id: &str) -> WakeDiagnostic {
    wake_start::trigger_notify_wake(session_id)
}

pub(crate) fn run_startup_wake_reclaim_sweep() {
    sweep::run_startup_wake_reclaim_sweep();
}

pub(crate) fn start_startup_wake_reclaim_sweep() -> Option<StartupWakeReclaimGuard> {
    sweep::start_startup_wake_reclaim_sweep()
}

pub(crate) fn is_wake_reclaim_handoff_invocation() -> bool {
    sweep::is_wake_reclaim_handoff_invocation()
}

pub(crate) fn run_wake_reclaim_handoff_invocation() -> Result<(), String> {
    sweep::run_wake_reclaim_handoff_invocation()
}

pub(crate) fn start_wake_reclaim_maintenance_driver() {
    sweep::start_wake_reclaim_maintenance_driver();
}

pub(crate) fn start_live_pty_retry_driver_for_owner() -> Option<LivePtyRetryDriverGuard> {
    sweep::start_live_pty_retry_driver_for_owner()
}

pub(crate) fn mark_terminal_attempt_idle_and_recheck(
    session_id: &str,
    invocation_uuid: &str,
    exit_code: i32,
) -> Result<WakeDiagnostic, String> {
    turn_recheck::mark_terminal_attempt_idle_and_recheck(session_id, invocation_uuid, exit_code)
}

pub(crate) fn validate_auto_wake_child(session_id: &str) -> Result<Option<i32>, String> {
    auto_wake_env::validate_auto_wake_child(session_id)
}

pub(crate) fn is_auto_wake_invocation() -> bool {
    auto_wake_env::is_auto_wake_invocation()
}

pub(crate) fn reset_manual_resume_wake_claim(session_id: &str) -> Result<(), String> {
    auto_wake_env::reset_manual_resume_wake_claim(session_id)
}

pub(crate) fn release_current_auto_wake_claim_for_session(session_id: &str) {
    auto_wake_env::release_current_auto_wake_claim_for_session(session_id);
}

pub(crate) fn recheck_after_failed_auto_wake(session_id: &str) -> WakeDiagnostic {
    turn_recheck::recheck_after_failed_auto_wake(session_id)
}
