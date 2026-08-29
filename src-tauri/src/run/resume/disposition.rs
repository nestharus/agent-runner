//! ## Declared roles
//!
//! `accessor`, `formatter`, `mapper`, `orchestration`, `predicate`

use oulipoly_runtime::services::InvocationLifecycleServicePort;

use super::finalization::{ConfirmedDeliverySettlement, finalize_confirmed_delivery};
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
    pub(super) invocation_id: &'a str,
    pub(super) invocation_row_id: i64,
    pub(super) guard: &'a mut FinalizerGuard<'state>,
    pub(super) provider_name: &'a str,
    pub(super) provider_session_id: &'a str,
    pub(super) result: &'a oulipoly_runtime::executor::ExecutionResult,
    pub(super) terminal_signal_disposition: TerminalSignalDisposition,
    pub(super) zero_turn_action: ZeroTurnAction,
    pub(super) recovered_generic_nonzero: bool,
    pub(super) confirmed_delivery: Option<ConfirmedDeliverySettlement<'a>>,
}

pub(super) fn handle_terminal_signal_disposition(
    mut input: ResumeTerminalDispositionInput<'_, '_>,
) -> Result<ResumeLoopControl, String> {
    if recovered_generic_nonzero_completed_attempt(&input) {
        return Ok(ResumeLoopControl::CompletedAttempt);
    }
    match input.terminal_signal_disposition {
        TerminalSignalDisposition::MaybeQuotaVerify => handle_maybe_quota_verify(input),
        TerminalSignalDisposition::QuotaExhaustedRetry => {
            let terminal_reason = quota_retry_terminal_reason(input.result);
            finalize_terminal_disposition(&mut input, Some("quota_exhausted"), terminal_reason)?;
            Ok(ResumeLoopControl::Continue)
        }
        TerminalSignalDisposition::ProlongedSilenceFail
        | TerminalSignalDisposition::InteractiveFail => {
            let terminal_reason = typed_failure_terminal_reason(input.result);
            let error_category =
                terminal_signal_error_category(&input.result.terminal_signal, terminal_reason);
            finalize_terminal_disposition(&mut input, error_category, terminal_reason)?;
            emit_resume_terminal_failure_output(&input, error_category, terminal_reason);
            Ok(ResumeLoopControl::Return(mapper::failure_exit_code(
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

fn emit_resume_terminal_failure_output(
    input: &ResumeTerminalDispositionInput<'_, '_>,
    error_category: Option<&str>,
    terminal_reason: &str,
) {
    formatter::emit_resume_failure_output(formatter::ResumeFailureOutputInput {
        state: &input.env.state,
        invocation_id: input.invocation_id,
        provider_name: input.provider_name,
        provider_session_id: input.provider_session_id,
        exit_code: input.result.exit_code,
        error_category,
        terminal_reason: Some(terminal_reason),
        stderr: &input.result.stderr,
    });
}

fn handle_maybe_quota_verify(
    mut input: ResumeTerminalDispositionInput<'_, '_>,
) -> Result<ResumeLoopControl, String> {
    let outcome = maybe_quota_action_outcome(input.zero_turn_action);
    let error_category = outcome.error_category();
    let terminal_reason = maybe_quota_terminal_reason(input.result);
    finalize_terminal_disposition(&mut input, error_category, terminal_reason)?;
    match outcome {
        MaybeQuotaActionOutcome::Continue { .. } => Ok(ResumeLoopControl::Continue),
        MaybeQuotaActionOutcome::ReturnFailure { .. } => {
            emit_resume_terminal_failure_output(&input, error_category, terminal_reason);
            Ok(ResumeLoopControl::Return(mapper::failure_exit_code(
                input.result.exit_code,
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

fn maybe_quota_terminal_reason(result: &oulipoly_runtime::executor::ExecutionResult) -> &str {
    crate::terminal_outcome_adapter::terminal_signal_reason(
        &result.terminal_signal,
        result.terminal_reason.as_deref(),
    )
    .unwrap_or("maybe_quota_exhausted")
}

fn finalize_terminal_disposition(
    input: &mut ResumeTerminalDispositionInput<'_, '_>,
    error_category: Option<&str>,
    terminal_reason: &str,
) -> Result<(), String> {
    if let Some(settlement) = input.confirmed_delivery {
        finalize_confirmed_delivery(
            &input.env.state,
            input.invocation_row_id,
            input.result,
            false,
            error_category,
            Some(terminal_reason),
            settlement,
        )?;
    } else {
        input
            .agent_runtime_services
            .invocation_lifecycle_service
            .finalize_invocation(mapper::finalize_request(
                &input.env.state,
                input.invocation_row_id,
                false,
                input.result.exit_code,
                error_category,
                Some(terminal_reason),
            ))
            .map_err(|err| err.to_string())?;
    }
    input.guard.mark_finalized();
    Ok(())
}
