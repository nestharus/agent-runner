//! Declared roles: orchestration, mapper, formatter, validator

use oulipoly_runtime::services::InvocationLifecycleServicePort;

use super::super::formatter;
use super::super::mapper;
use super::super::state_update::{emit_session_capture_failure, update_session_capture};
use super::super::validator;
use super::{BalancedLoopControl, TypedDispositionInput};
use crate::captured_child::supervise_captured_child_invocations;
use crate::terminal_outcome_adapter::{TerminalSignalDisposition, apply_terminal_signal_outcome};

pub(in crate::run::balancing) fn handle_prolonged_silence_fail(
    input: TypedDispositionInput<'_, '_, '_>,
) -> BalancedLoopControl {
    let disposition =
        apply_terminal_signal_outcome(input.terminal_signal, input.terminal_signal_ctx);
    validator::expect_prolonged_silence_fail_disposition(
        disposition,
        TerminalSignalDisposition::ProlongedSilenceFail,
    );
    let terminal_reason = validator::required_typed_terminal_reason(
        super::super::accessor::typed_terminal_reason_option(input.result),
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

pub(in crate::run::balancing) fn handle_interactive_fail(
    input: TypedDispositionInput<'_, '_, '_>,
) -> BalancedLoopControl {
    let disposition =
        apply_terminal_signal_outcome(input.terminal_signal, input.terminal_signal_ctx);
    validator::expect_interactive_fail_disposition(
        disposition,
        TerminalSignalDisposition::InteractiveFail,
    );
    let terminal_reason = validator::required_typed_terminal_reason(
        super::super::accessor::typed_terminal_reason_option(input.result),
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
