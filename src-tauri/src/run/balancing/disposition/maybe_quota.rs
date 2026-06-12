//! Declared roles: orchestration, mapper, formatter, validator

use oulipoly_runtime::services::InvocationLifecycleServicePort;

use super::super::formatter;
use super::super::mapper;
use super::super::predicate::{confirmed_zero_turn_action, retry_available};
use super::super::state_update::{
    bump_quota_tick, emit_session_capture_failure, update_session_capture,
};
use super::super::validator;
use super::{BalancedLoopControl, MaybeQuotaVerifyInput};
use crate::captured_child::supervise_captured_child_invocations;
use crate::terminal_outcome_adapter::apply_terminal_signal_outcome;
use crate::zero_turn_orchestration::ZeroTurnAction;

pub(in crate::run::balancing) fn handle_maybe_quota_verify(
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
        super::super::accessor::typed_terminal_reason(input.typed.result, "maybe_quota_exhausted");
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
