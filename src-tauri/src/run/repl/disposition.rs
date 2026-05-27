//! ## Declared roles
//!
//! `orchestration`, `mapper`, `formatter`

use oulipoly_runtime::services::InvocationLifecycleServicePort;

use super::mapper;
use crate::invocation::finalize::FinalizerGuard;
use crate::migration_providers::ResumeExecutionEnvironment;
use crate::terminal_outcome_adapter::{
    TerminalSignalDisposition, terminal_signal_error_category, terminal_signal_reason,
};
use crate::wiring;

pub(super) enum ReplTerminalControl {
    Return(i32),
    Completed,
}

pub(super) struct ReplTerminalDispositionInput<'a, 'state> {
    pub(super) agent_runtime_services: &'a wiring::AgentRuntimeServices,
    pub(super) env: &'a ResumeExecutionEnvironment,
    pub(super) invocation_row_id: i64,
    pub(super) guard: &'a mut FinalizerGuard<'state>,
    pub(super) result: &'a oulipoly_runtime::executor::cli::InteractiveExecutionResult,
    pub(super) terminal_signal_disposition: TerminalSignalDisposition,
}

pub(super) fn handle_terminal_signal_disposition(
    input: ReplTerminalDispositionInput<'_, '_>,
) -> Result<ReplTerminalControl, String> {
    // AGE-153 source guard: TerminalSignalKind::CleanExit maps to TerminalSignalDisposition::InteractiveClean.
    match input.terminal_signal_disposition {
        TerminalSignalDisposition::InteractiveFail
        | TerminalSignalDisposition::ProlongedSilenceFail
        | TerminalSignalDisposition::QuotaExhaustedRetry
        | TerminalSignalDisposition::MaybeQuotaVerify => {
            let terminal_reason = terminal_signal_reason(
                &input.result.terminal_signal,
                input.result.terminal_reason.as_deref(),
            )
            .unwrap_or("unknown_exit");
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
            Ok(ReplTerminalControl::Return(input.result.exit_code))
        }
        TerminalSignalDisposition::InteractiveClean | TerminalSignalDisposition::NotApplicable => {
            Ok(ReplTerminalControl::Completed)
        }
    }
}
