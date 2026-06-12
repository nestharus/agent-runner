//! Declared roles: orchestration, mapper, formatter, validator

use oulipoly_runtime::services::InvocationLifecycleServicePort;

use super::super::formatter;
use super::super::mapper;
use super::super::predicate::retry_available;
use super::super::state_update::{
    bump_quota_tick, emit_session_capture_failure, update_session_capture,
};
use super::super::validator;
use super::{BalancedLoopControl, TypedDispositionInput};
use crate::captured_child::supervise_captured_child_invocations;
use crate::terminal_outcome_adapter::{TerminalSignalDisposition, apply_terminal_signal_outcome};

pub(in crate::run::balancing) fn handle_quota_exhausted_retry(
    input: TypedDispositionInput<'_, '_, '_>,
) -> BalancedLoopControl {
    let disposition =
        apply_terminal_signal_outcome(input.terminal_signal, input.terminal_signal_ctx);
    validator::expect_quota_exhausted_retry_disposition(
        disposition,
        TerminalSignalDisposition::QuotaExhaustedRetry,
    );
    let terminal_reason = validator::required_typed_terminal_reason(
        super::super::accessor::typed_terminal_reason_option(input.result),
        "typed quota signal must have terminal reason",
    );
    supervise_captured_child_invocations(
        &input.env.state,
        input.invocation_row_id,
        &input.result.captured_child_invocations,
        Some(terminal_reason),
    );
    emit_session_capture_failure(input.result);
    update_session_capture(input.env, input.invocation_row_id, input.result);
    let finalize_result = input
        .agent_runtime_services
        .invocation_lifecycle_service
        .finalize_invocation(mapper::quota_exhausted_finalize_request(
            &input.env.state,
            input.invocation_row_id,
            input.result.exit_code,
            terminal_reason,
        ));
    match finalize_result {
        Ok(_) => input.guard.mark_finalized(),
        Err(err) => formatter::emit_finalize_invocation_warning(err),
    }
    bump_quota_tick(input.env, input.provider_name);
    // [routing] provider {provider_name} returned quota_exhausted; retrying another provider
    if retry_available(input.attempts, input.max_attempts) {
        formatter::emit_routing_retry(input.provider_name);
    }
    BalancedLoopControl::Continue
}
