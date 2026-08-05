//! ## Declared roles
//!
//! `accessor`, `formatter`, `mapper`, `orchestration`, `predicate`

use oulipoly_runtime::services::InvocationLifecycleServicePort;

use super::{formatter, mapper};
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
    pub(super) recovered_generic_nonzero: bool,
}

pub(super) fn handle_terminal_signal_disposition(
    input: ResumeTerminalDispositionInput<'_, '_>,
) -> Result<ResumeLoopControl, String> {
    if recovered_generic_nonzero_completed_attempt(&input) {
        return Ok(ResumeLoopControl::CompletedAttempt);
    }
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
            let terminal_reason = quota_retry_terminal_reason(input.result);
            input
                .agent_runtime_services
                .invocation_lifecycle_service
                .finalize_invocation(terminal_disposition_finalize_request(
                    &input,
                    Some("quota_exhausted"),
                    terminal_reason,
                ))
                .map_err(|err| err.to_string())?;
            input.guard.mark_finalized();
            Ok(ResumeLoopControl::Continue)
        }
        TerminalSignalDisposition::ProlongedSilenceFail
        | TerminalSignalDisposition::InteractiveFail => {
            let terminal_reason = typed_failure_terminal_reason(input.result);
            input
                .agent_runtime_services
                .invocation_lifecycle_service
                .finalize_invocation(terminal_disposition_finalize_request(
                    &input,
                    terminal_signal_error_category(&input.result.terminal_signal, terminal_reason),
                    terminal_reason,
                ))
                .map_err(|err| err.to_string())?;
            input.guard.mark_finalized();
            emit_resume_terminal_failure_output(input.result);
            Ok(ResumeLoopControl::Return(failure_exit_code(
                input.result.exit_code,
            )))
        }
        TerminalSignalDisposition::InteractiveClean | TerminalSignalDisposition::NotApplicable => {
            Ok(ResumeLoopControl::CompletedAttempt)
        }
    }
}

fn recovered_generic_nonzero_completed_attempt(
    input: &ResumeTerminalDispositionInput<'_, '_>,
) -> bool {
    input.recovered_generic_nonzero
        && input.result.terminal_signal.as_ref().is_some_and(|signal| {
            signal.kind
                == oulipoly_runtime::executor::terminal_signal::TerminalSignalKind::NonzeroExit
        })
        && matches!(
            input.terminal_signal_disposition,
            TerminalSignalDisposition::InteractiveFail
        )
}

fn quota_retry_terminal_reason(result: &oulipoly_runtime::executor::ExecutionResult) -> &str {
    crate::terminal_outcome_adapter::terminal_signal_reason(
        &result.terminal_signal,
        result.terminal_reason.as_deref(),
    )
    .expect("typed quota signal must have terminal reason")
}

fn typed_failure_terminal_reason(result: &oulipoly_runtime::executor::ExecutionResult) -> &str {
    crate::terminal_outcome_adapter::terminal_signal_reason(
        &result.terminal_signal,
        result.terminal_reason.as_deref(),
    )
    .expect("typed failure signal must have terminal reason")
}

fn terminal_disposition_finalize_request<'a>(
    input: &'a ResumeTerminalDispositionInput<'_, '_>,
    error_category: Option<&'a str>,
    terminal_reason: &'a str,
) -> oulipoly_runtime::services::InvocationLifecycleFinalizeRequest<'a> {
    mapper::finalize_request(
        &input.env.state,
        input.invocation_row_id,
        false,
        input.result.exit_code,
        error_category,
        Some(terminal_reason),
    )
}

fn emit_resume_terminal_failure_output(result: &oulipoly_runtime::executor::ExecutionResult) {
    formatter::emit_stderr(&result.stderr);
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
    let outcome = maybe_quota_action_outcome(zero_turn_action);
    finalize(maybe_quota_finalize_request(
        state,
        invocation_row_id,
        result,
        outcome.error_category(),
    ))?;
    match outcome {
        MaybeQuotaActionOutcome::Continue { .. } => Ok(ResumeLoopControl::Continue),
        MaybeQuotaActionOutcome::ReturnFailure { .. } => {
            emit_maybe_quota_failure_output(result);
            Ok(ResumeLoopControl::Return(failure_exit_code(
                result.exit_code,
            )))
        }
    }
}

enum MaybeQuotaActionOutcome {
    Continue {
        error_category: Option<&'static str>,
    },
    ReturnFailure {
        error_category: Option<&'static str>,
    },
}

impl MaybeQuotaActionOutcome {
    fn error_category(&self) -> Option<&'static str> {
        match self {
            MaybeQuotaActionOutcome::Continue { error_category }
            | MaybeQuotaActionOutcome::ReturnFailure { error_category } => *error_category,
        }
    }
}

fn maybe_quota_action_outcome(zero_turn_action: ZeroTurnAction) -> MaybeQuotaActionOutcome {
    match zero_turn_action {
        ZeroTurnAction::ConfirmedExhaustion => MaybeQuotaActionOutcome::Continue {
            error_category: Some("quota_exhausted"),
        },
        ZeroTurnAction::VerifySameProvider => MaybeQuotaActionOutcome::Continue {
            error_category: None,
        },
        ZeroTurnAction::Continue | ZeroTurnAction::Unclassified => {
            MaybeQuotaActionOutcome::ReturnFailure {
                error_category: None,
            }
        }
    }
}

fn emit_maybe_quota_failure_output(result: &oulipoly_runtime::executor::ExecutionResult) {
    formatter::emit_stderr(&result.stderr);
}

fn failure_exit_code(exit_code: i32) -> i32 {
    if exit_code == 0 { 1 } else { exit_code }
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
