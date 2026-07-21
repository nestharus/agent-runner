//! ## Declared roles
//!
//! `mapper`, `orchestration`, `predicate`

use std::path::Path;

use oulipoly_config::{ModelConfig, ProviderConfig};
use oulipoly_runtime::executor;

use super::disposition::{
    ReplTerminalControl, ReplTerminalDispositionInput, handle_terminal_signal_disposition,
};
use super::execution::{
    clear_repl_session_capture_for_unpinned, repl_execution_cwd, repl_interactive_effective_cwd,
    repl_resume_payload,
};
use super::finalization::{
    CompletedReplAttemptInput, finalize_completed_repl_attempt, finalize_spawn_error,
};
use super::mapper;
use crate::invocation::finalize::FinalizerGuard;
use crate::migration_providers::ResumeExecutionEnvironment;
use crate::quota_zero_turn::{
    apply_zero_turn_classification_to_signal_fields,
    host_observed_completion_from_interactive_result, zero_turn_classify_after_completion,
    zero_turn_record_baseline,
};
use crate::terminal_outcome_adapter::{
    apply_age153_terminal_signal_fixture_override_to_fields, apply_terminal_signal_outcome,
};
use crate::wiring;

pub(super) struct ReplExecutionInput<'a, 'state> {
    pub(super) agent_runtime_services: &'a wiring::AgentRuntimeServices,
    pub(super) env: &'a ResumeExecutionEnvironment,
    pub(super) invocation: &'a oulipoly_state::CompositeInvocationId,
    pub(super) invocation_row_id: i64,
    pub(super) guard: &'a mut FinalizerGuard<'state>,
    pub(super) provider: &'a ProviderConfig,
    pub(super) model: &'a ModelConfig,
    pub(super) resume: Option<&'a str>,
    pub(super) manual_migrate: Option<&'a str>,
    pub(super) working_dir: Option<&'a Path>,
    pub(super) resume_spawn_cwd: Option<&'a Path>,
    pub(super) resume_session_id: Option<&'a str>,
    pub(super) invocation_env: &'a str,
}

pub(super) fn execute_and_finalize_repl_attempt(
    input: ReplExecutionInput<'_, '_>,
) -> Result<i32, String> {
    let interactive_effective_cwd =
        repl_interactive_effective_cwd(input.resume_spawn_cwd, input.working_dir)?;
    let resume_payload = repl_resume_payload(input.provider, input.resume_session_id);
    let zero_turn_baseline = zero_turn_record_baseline(
        &input.env.state,
        &input.env.sessions_cfg,
        &input.provider.name,
        input.resume_session_id,
    );

    match executor::cli::execute_interactive_with_result_and_model_config(
        input.provider,
        repl_execution_cwd(input.resume_spawn_cwd, input.working_dir),
        Some(input.invocation_env),
        resume_payload,
        input.model,
        input
            .agent_runtime_services
            .provider_registry_handle
            .current(),
    ) {
        Ok(mut result) => {
            classify_repl_result(
                input.env,
                &input.provider.name,
                &zero_turn_baseline,
                &mut result,
            );
            clear_repl_session_capture_for_unpinned(
                input.env,
                input.invocation_row_id,
                input.resume,
            )?;
            finalize_repl_execution_result(input, &interactive_effective_cwd, &result)
        }
        Err(_spawn_err) => {
            clear_repl_session_capture_for_unpinned(
                input.env,
                input.invocation_row_id,
                input.resume,
            )?;
            finalize_repl_spawn_error(input)
        }
    }
}

fn finalize_repl_execution_result(
    input: ReplExecutionInput<'_, '_>,
    interactive_effective_cwd: &Path,
    result: &oulipoly_runtime::executor::cli::InteractiveExecutionResult,
) -> Result<i32, String> {
    let terminal_signal_disposition = terminal_signal_disposition_for_result(
        input.env,
        &input.invocation.id,
        &input.provider.name,
        input.resume_session_id,
        result,
    );
    match handle_terminal_signal_disposition(ReplTerminalDispositionInput {
        agent_runtime_services: input.agent_runtime_services,
        env: input.env,
        invocation_row_id: input.invocation_row_id,
        guard: input.guard,
        result,
        terminal_signal_disposition,
    })? {
        ReplTerminalControl::Return(exit_code) => Ok(exit_code),
        ReplTerminalControl::Completed => {
            finalize_completed_repl_execution(input, interactive_effective_cwd, result)
        }
    }
}

fn finalize_completed_repl_execution(
    input: ReplExecutionInput<'_, '_>,
    interactive_effective_cwd: &Path,
    result: &oulipoly_runtime::executor::cli::InteractiveExecutionResult,
) -> Result<i32, String> {
    finalize_completed_repl_attempt(CompletedReplAttemptInput {
        agent_runtime_services: input.agent_runtime_services,
        env: input.env,
        invocation: input.invocation,
        invocation_row_id: input.invocation_row_id,
        guard: input.guard,
        provider_name: &input.provider.name,
        model: input.model,
        result,
        resume: input.resume,
        resume_session_id: input.resume_session_id,
        manual_migrate: input.manual_migrate,
        interactive_effective_cwd,
    })
}

fn finalize_repl_spawn_error(input: ReplExecutionInput<'_, '_>) -> Result<i32, String> {
    finalize_spawn_error(
        input.agent_runtime_services,
        input.env,
        input.invocation_row_id,
        input.guard,
    )
}

fn classify_repl_result(
    env: &ResumeExecutionEnvironment,
    provider_name: &str,
    zero_turn_baseline: &crate::zero_turn_orchestration::ZeroTurnBaseline,
    result: &mut oulipoly_runtime::executor::cli::InteractiveExecutionResult,
) {
    apply_age153_terminal_signal_fixture_override_to_fields(
        &mut result.terminal_signal,
        &mut result.terminal_reason,
    );
    let zero_turn_classification = zero_turn_classify_after_completion(
        &env.state,
        &env.sessions_cfg,
        zero_turn_baseline,
        host_observed_completion_from_interactive_result(result),
    );
    apply_zero_turn_classification_to_signal_fields(
        &mut result.terminal_signal,
        &mut result.terminal_reason,
        provider_name,
        &zero_turn_classification,
    );
}

fn terminal_signal_disposition_for_result(
    env: &ResumeExecutionEnvironment,
    invocation_id: &str,
    provider_name: &str,
    resume_session_id: Option<&str>,
    result: &oulipoly_runtime::executor::cli::InteractiveExecutionResult,
) -> crate::terminal_outcome_adapter::TerminalSignalDisposition {
    let ids = mapper::terminal_signal_context_ids(invocation_id, resume_session_id);
    let mut terminal_signal_stderr = std::io::stderr();
    let mut terminal_signal_ctx = mapper::terminal_signal_context_for_repl(
        &ids,
        provider_name,
        &env.state,
        &mut terminal_signal_stderr,
    );
    // AGE-153 source guard: marker emission routes through emit_terminal_signal_marker.
    apply_terminal_signal_outcome(&result.terminal_signal, &mut terminal_signal_ctx)
}
