//! orchestration

use oulipoly_runtime::executor;
use oulipoly_runtime::services::InvocationLifecycleServicePort;
use oulipoly_state::CompositeInvocationId;

use super::accessor::BalancedExecutionEnvironment;
use super::formatter;
use super::mapper;
use super::predicate::{confirmed_zero_turn_action, retry_available};
use super::state_update::{bump_quota_tick, emit_session_capture_failure, update_session_capture};
use super::validator;
use crate::captured_child::supervise_captured_child_invocations;
use crate::invocation::finalize::FinalizerGuard;
use crate::terminal_outcome_adapter::{
    TerminalSignalContext, TerminalSignalDisposition, apply_terminal_signal_outcome,
};
use crate::wiring;
use crate::zero_turn_orchestration::ZeroTurnAction;

pub(super) enum BalancedLoopControl {
    Continue,
    Return(Result<i32, String>),
}

pub(super) struct TypedDispositionInput<'a, 'state, 'ctx> {
    pub(super) agent_runtime_services: &'a wiring::AgentRuntimeServices,
    pub(super) env: &'a BalancedExecutionEnvironment,
    pub(super) invocation: &'a CompositeInvocationId,
    pub(super) invocation_row_id: i64,
    pub(super) guard: &'a mut FinalizerGuard<'state>,
    pub(super) provider_name: &'a str,
    pub(super) provider_index: usize,
    pub(super) result: &'a executor::ExecutionResult,
    pub(super) terminal_signal: &'a Option<executor::TerminalSignal>,
    pub(super) terminal_signal_ctx: &'a mut TerminalSignalContext<'ctx, std::io::Stderr>,
    pub(super) zero_turn_provider_session_id: Option<&'a str>,
    pub(super) attempts: usize,
    pub(super) max_attempts: usize,
}

pub(super) struct MaybeQuotaVerifyInput<'a, 'state, 'ctx> {
    pub(super) typed: TypedDispositionInput<'a, 'state, 'ctx>,
    pub(super) zero_turn_action: ZeroTurnAction,
    pub(super) pending_same_provider_verification: &'a mut Option<(usize, Option<String>)>,
    pub(super) signal_already_applied: bool,
}

pub(super) fn handle_maybe_quota_verify(
    input: MaybeQuotaVerifyInput<'_, '_, '_>,
) -> BalancedLoopControl {
    if !input.signal_already_applied {
        let disposition = apply_terminal_signal_outcome(
            input.typed.terminal_signal,
            input.typed.terminal_signal_ctx,
        );
        validator::expect_maybe_quota_verify_disposition(disposition);
    }
    let terminal_reason =
        super::accessor::typed_terminal_reason(input.typed.result, "maybe_quota_exhausted");
    supervise_captured_child_invocations(
        &input.typed.env.state,
        input.typed.invocation_row_id,
        &input.typed.result.captured_child_invocations,
        Some(terminal_reason),
    );
    emit_session_capture_failure(input.typed.result);
    update_session_capture(
        input.typed.env,
        input.typed.invocation_row_id,
        input.typed.result,
    );
    let confirmed = confirmed_zero_turn_action(input.zero_turn_action);
    let finalize_result = input
        .typed
        .agent_runtime_services
        .invocation_lifecycle_service
        .finalize_invocation(mapper::maybe_quota_finalize_request(
            &input.typed.env.state,
            input.typed.invocation_row_id,
            input.typed.result.exit_code,
            confirmed,
            terminal_reason,
        ));
    match finalize_result {
        Ok(_) => input.typed.guard.mark_finalized(),
        Err(err) => formatter::emit_finalize_invocation_warning(err),
    }
    bump_quota_tick(input.typed.env, input.typed.provider_name);
    match input.zero_turn_action {
        ZeroTurnAction::VerifySameProvider => {
            *input.pending_same_provider_verification =
                Some(mapper::pending_same_provider_verification(
                    input.typed.provider_index,
                    input.typed.zero_turn_provider_session_id,
                ));
            BalancedLoopControl::Continue
        }
        ZeroTurnAction::ConfirmedExhaustion => {
            if retry_available(input.typed.attempts, input.typed.max_attempts) {
                formatter::emit_routing_retry(input.typed.provider_name);
            }
            BalancedLoopControl::Continue
        }
        ZeroTurnAction::Continue | ZeroTurnAction::Unclassified => {
            formatter::emit_failure_output(
                mapper::failure_result_envelope_input(
                    &input.typed.env.state,
                    &input.typed.invocation.id,
                    input.typed.provider_name,
                    input.typed.zero_turn_provider_session_id,
                    input.typed.result.exit_code,
                    None,
                    Some(terminal_reason),
                ),
                &input.typed.result.stderr,
            );
            BalancedLoopControl::Return(Ok(input.typed.result.exit_code))
        }
    }
}

pub(super) fn handle_quota_exhausted_retry(
    input: TypedDispositionInput<'_, '_, '_>,
) -> BalancedLoopControl {
    let disposition =
        apply_terminal_signal_outcome(input.terminal_signal, input.terminal_signal_ctx);
    validator::expect_quota_exhausted_retry_disposition(
        disposition,
        TerminalSignalDisposition::QuotaExhaustedRetry,
    );
    let terminal_reason = validator::required_typed_terminal_reason(
        super::accessor::typed_terminal_reason_option(input.result),
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

pub(super) fn handle_prolonged_silence_fail(
    input: TypedDispositionInput<'_, '_, '_>,
) -> BalancedLoopControl {
    let disposition =
        apply_terminal_signal_outcome(input.terminal_signal, input.terminal_signal_ctx);
    validator::expect_prolonged_silence_fail_disposition(
        disposition,
        TerminalSignalDisposition::ProlongedSilenceFail,
    );
    let terminal_reason = validator::required_typed_terminal_reason(
        super::accessor::typed_terminal_reason_option(input.result),
        "typed failure signal must have terminal reason",
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
        .finalize_invocation(mapper::terminal_failure_finalize_request(
            &input.env.state,
            input.invocation_row_id,
            input.result,
            terminal_reason,
        ));
    match finalize_result {
        Ok(_) => input.guard.mark_finalized(),
        Err(err) => formatter::emit_finalize_invocation_warning(err),
    }
    formatter::emit_failure_output(
        mapper::failure_result_envelope_input(
            &input.env.state,
            &input.invocation.id,
            input.provider_name,
            input.zero_turn_provider_session_id,
            input.result.exit_code,
            Some(terminal_reason),
            Some(terminal_reason),
        ),
        &input.result.stderr,
    );
    BalancedLoopControl::Return(Ok(input.result.exit_code))
}

pub(super) fn handle_interactive_fail(
    input: TypedDispositionInput<'_, '_, '_>,
) -> BalancedLoopControl {
    let disposition =
        apply_terminal_signal_outcome(input.terminal_signal, input.terminal_signal_ctx);
    validator::expect_interactive_fail_disposition(
        disposition,
        TerminalSignalDisposition::InteractiveFail,
    );
    let terminal_reason = validator::required_typed_terminal_reason(
        super::accessor::typed_terminal_reason_option(input.result),
        "typed failure signal must have terminal reason",
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
        .finalize_invocation(mapper::terminal_failure_finalize_request(
            &input.env.state,
            input.invocation_row_id,
            input.result,
            terminal_reason,
        ));
    match finalize_result {
        Ok(_) => input.guard.mark_finalized(),
        Err(err) => formatter::emit_finalize_invocation_warning(err),
    }
    formatter::emit_failure_output(
        mapper::failure_result_envelope_input(
            &input.env.state,
            &input.invocation.id,
            input.provider_name,
            input.zero_turn_provider_session_id,
            input.result.exit_code,
            Some(terminal_reason),
            Some(terminal_reason),
        ),
        &input.result.stderr,
    );
    BalancedLoopControl::Return(Ok(input.result.exit_code))
}
