//! ## Declared roles
//!
//! `orchestration`, `mapper`, `predicate`, `formatter`

use oulipoly_runtime::services::InvocationLifecycleServicePort;

use super::{formatter, mapper, predicate};
use crate::invocation::finalize::FinalizerGuard;
use crate::migration_providers::ResumeExecutionEnvironment;
use crate::terminal_outcome_adapter::{TerminalSignalDisposition, terminal_signal_error_category};
use crate::wiring;
use crate::zero_turn_orchestration::ZeroTurnAction;

pub(super) enum ResumeLoopControl {
    Continue,
    Return(i32),
    CompletedAttempt,
}

pub(super) struct ResumeTerminalDispositionInput<'a, 'state> {
    pub(super) agent_runtime_services: &'a wiring::AgentRuntimeServices,
    pub(super) env: &'a ResumeExecutionEnvironment,
    pub(super) invocation_row_id: i64,
    pub(super) guard: &'a mut FinalizerGuard<'state>,
    pub(super) result: &'a oulipoly_runtime::executor::ExecutionResult,
    pub(super) terminal_signal_disposition: TerminalSignalDisposition,
    pub(super) zero_turn_action: ZeroTurnAction,
}

pub(super) fn handle_terminal_signal_disposition(
    input: ResumeTerminalDispositionInput<'_, '_>,
) -> Result<ResumeLoopControl, String> {
    match input.terminal_signal_disposition {
        TerminalSignalDisposition::MaybeQuotaVerify => handle_maybe_quota_verify(
            input.result,
            input.zero_turn_action,
            |request| {
                input
                    .agent_runtime_services
                    .invocation_lifecycle_service
                    .finalize_invocation(request)
                    .map_err(|err| err.to_string())?;
                input.guard.mark_finalized();
                Ok(())
            },
            &input.env.state,
            input.invocation_row_id,
        ),
        TerminalSignalDisposition::QuotaExhaustedRetry => {
            let terminal_reason = crate::terminal_outcome_adapter::terminal_signal_reason(
                &input.result.terminal_signal,
                input.result.terminal_reason.as_deref(),
            )
            .expect("typed quota signal must have terminal reason");
            input
                .agent_runtime_services
                .invocation_lifecycle_service
                .finalize_invocation(mapper::finalize_request(
                    &input.env.state,
                    input.invocation_row_id,
                    false,
                    input.result.exit_code,
                    Some("quota_exhausted"),
                    Some(terminal_reason),
                ))
                .map_err(|err| err.to_string())?;
            input.guard.mark_finalized();
            Ok(ResumeLoopControl::Continue)
        }
        TerminalSignalDisposition::ProlongedSilenceFail
        | TerminalSignalDisposition::InteractiveFail => {
            let terminal_reason = crate::terminal_outcome_adapter::terminal_signal_reason(
                &input.result.terminal_signal,
                input.result.terminal_reason.as_deref(),
            )
            .expect("typed failure signal must have terminal reason");
            input
                .agent_runtime_services
                .invocation_lifecycle_service
                .finalize_invocation(mapper::finalize_request(
                    &input.env.state,
                    input.invocation_row_id,
                    false,
                    input.result.exit_code,
                    terminal_signal_error_category(&input.result.terminal_signal, terminal_reason),
                    Some(terminal_reason),
                ))
                .map_err(|err| err.to_string())?;
            input.guard.mark_finalized();
            formatter::emit_stderr(&input.result.stderr);
            Ok(ResumeLoopControl::Return(input.result.exit_code))
        }
        TerminalSignalDisposition::InteractiveClean | TerminalSignalDisposition::NotApplicable => {
            Ok(ResumeLoopControl::CompletedAttempt)
        }
    }
}

fn handle_maybe_quota_verify(
    result: &oulipoly_runtime::executor::ExecutionResult,
    zero_turn_action: ZeroTurnAction,
    mut finalize: impl FnMut(
        oulipoly_runtime::services::InvocationLifecycleFinalizeRequest<'_>,
    ) -> Result<(), String>,
    state: &oulipoly_state::StateDb,
    invocation_row_id: i64,
) -> Result<ResumeLoopControl, String> {
    match zero_turn_action {
        ZeroTurnAction::ConfirmedExhaustion => {
            finalize(maybe_quota_finalize_request(
                state,
                invocation_row_id,
                result,
                Some("quota_exhausted"),
            ))?;
            Ok(ResumeLoopControl::Continue)
        }
        ZeroTurnAction::VerifySameProvider => {
            finalize(maybe_quota_finalize_request(
                state,
                invocation_row_id,
                result,
                None,
            ))?;
            Ok(ResumeLoopControl::Continue)
        }
        ZeroTurnAction::Continue | ZeroTurnAction::Unclassified => {
            finalize(maybe_quota_finalize_request(
                state,
                invocation_row_id,
                result,
                None,
            ))?;
            formatter::emit_stderr(&result.stderr);
            Ok(ResumeLoopControl::Return(result.exit_code))
        }
    }
}

fn maybe_quota_finalize_request<'a>(
    state: &'a oulipoly_state::StateDb,
    invocation_row_id: i64,
    result: &'a oulipoly_runtime::executor::ExecutionResult,
    error_category: Option<&'a str>,
) -> oulipoly_runtime::services::InvocationLifecycleFinalizeRequest<'a> {
    let terminal_reason = crate::terminal_outcome_adapter::terminal_signal_reason(
        &result.terminal_signal,
        result.terminal_reason.as_deref(),
    )
    .unwrap_or("maybe_quota_exhausted");
    mapper::finalize_request(
        state,
        invocation_row_id,
        false,
        result.exit_code,
        error_category,
        Some(terminal_reason),
    )
}

pub(super) fn confirmed_zero_turn_maybe_quota(
    zero_turn_action: ZeroTurnAction,
    terminal_signal: &Option<oulipoly_runtime::executor::TerminalSignal>,
) -> bool {
    predicate::confirmed_zero_turn_maybe_quota(zero_turn_action, terminal_signal)
}
