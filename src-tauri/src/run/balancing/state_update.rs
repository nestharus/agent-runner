//! Declared roles: orchestration, formatter

use oulipoly_runtime::executor;
use oulipoly_state::StateDb;

use super::accessor::{BalancedExecutionEnvironment, session_capture_failure_reason};
use super::formatter;
use super::mapper::provider_session_binding;
use super::predicate::has_provider_session_id;

pub(super) fn emit_session_capture_failure(result: &executor::ExecutionResult) {
    if let Some(reason) = session_capture_failure_reason(result) {
        formatter::emit_session_capture_failure(reason);
    }
}

pub(super) fn update_session_capture(
    env: &BalancedExecutionEnvironment,
    invocation_row_id: i64,
    result: &executor::ExecutionResult,
) {
    env.state
        .update_session_capture(
            invocation_row_id,
            result.session_capture.session_id.as_deref(),
            result.session_capture.method.db_value(),
        )
        .unwrap_or_else(formatter::emit_session_capture_update_warning);
}

pub(super) fn bump_quota_tick(env: &BalancedExecutionEnvironment, provider_name: &str) {
    env.state
        .increment_calls_since_refresh(provider_name)
        .unwrap_or_else(formatter::emit_quota_tick_warning);
}

pub(super) fn bind_start_known_provider_session_if_present(
    state: &StateDb,
    invocation_row_id: i64,
    provider_session_id: Option<&str>,
) {
    if has_provider_session_id(provider_session_id) {
        bind_start_known_provider_session(state, invocation_row_id, provider_session_id);
    }
}

fn bind_start_known_provider_session(
    state: &StateDb,
    invocation_row_id: i64,
    provider_session_id: Option<&str>,
) {
    let Some(provider_session_id) = provider_session_id else {
        return;
    };
    state
        .bind_invocation_provider_session_start(
            invocation_row_id,
            &provider_session_binding(provider_session_id),
        )
        .unwrap_or_else(formatter::emit_provider_session_binding_warning);
}
