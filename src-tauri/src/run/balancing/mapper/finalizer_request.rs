use oulipoly_runtime::executor;
use oulipoly_runtime::services::InvocationLifecycleFinalizeRequest;
use oulipoly_state::StateDb;

pub(in crate::run::balancing) fn spawn_error_finalize_request<'a>(
    state: &'a StateDb,
    invocation_row_id: i64,
    terminal_reason: &'a str,
) -> InvocationLifecycleFinalizeRequest<'a> {
    InvocationLifecycleFinalizeRequest {
        state,
        invocation_row_id,
        success: false,
        exit_code: -1,
        error_category: Some(terminal_reason),
        terminal_reason: Some(terminal_reason),
    }
}

pub(in crate::run::balancing) fn maybe_quota_finalize_request<'a>(
    state: &'a StateDb,
    invocation_row_id: i64,
    exit_code: i32,
    confirmed: bool,
    terminal_reason: &'a str,
) -> InvocationLifecycleFinalizeRequest<'a> {
    InvocationLifecycleFinalizeRequest {
        state,
        invocation_row_id,
        success: false,
        exit_code,
        error_category: maybe_quota_error_category(confirmed),
        terminal_reason: Some(terminal_reason),
    }
}

fn maybe_quota_error_category(confirmed: bool) -> Option<&'static str> {
    confirmed.then_some("quota_exhausted")
}

pub(in crate::run::balancing) fn quota_exhausted_finalize_request<'a>(
    state: &'a StateDb,
    invocation_row_id: i64,
    exit_code: i32,
    terminal_reason: &'a str,
) -> InvocationLifecycleFinalizeRequest<'a> {
    InvocationLifecycleFinalizeRequest {
        state,
        invocation_row_id,
        success: false,
        exit_code,
        error_category: Some("quota_exhausted"),
        terminal_reason: Some(terminal_reason),
    }
}

pub(in crate::run::balancing) fn terminal_failure_finalize_request<'a>(
    state: &'a StateDb,
    invocation_row_id: i64,
    result: &'a executor::ExecutionResult,
    terminal_reason: &'a str,
) -> InvocationLifecycleFinalizeRequest<'a> {
    InvocationLifecycleFinalizeRequest {
        state,
        invocation_row_id,
        success: false,
        exit_code: result.exit_code,
        error_category: terminal_failure_error_category(result, terminal_reason),
        terminal_reason: Some(terminal_reason),
    }
}

pub(in crate::run::balancing) fn returned_artifacts_finalize_request(
    state: &StateDb,
    invocation_row_id: i64,
) -> InvocationLifecycleFinalizeRequest<'_> {
    InvocationLifecycleFinalizeRequest {
        state,
        invocation_row_id,
        success: false,
        exit_code: 1,
        error_category: Some("returned_artifacts"),
        terminal_reason: Some("returned_artifacts_persist_failed"),
    }
}

pub(in crate::run::balancing) fn completed_finalize_request<'a>(
    state: &'a StateDb,
    invocation_row_id: i64,
    success: bool,
    exit_code: i32,
    error_category: Option<&'a str>,
    terminal_reason: Option<&'a str>,
) -> InvocationLifecycleFinalizeRequest<'a> {
    InvocationLifecycleFinalizeRequest {
        state,
        invocation_row_id,
        success,
        exit_code,
        error_category,
        terminal_reason,
    }
}

pub(in crate::run::balancing) fn terminal_failure_error_category<'a>(
    result: &'a executor::ExecutionResult,
    terminal_reason: &'a str,
) -> Option<&'a str> {
    crate::terminal_outcome_adapter::terminal_signal_error_category(
        &result.terminal_signal,
        terminal_reason,
    )
}
