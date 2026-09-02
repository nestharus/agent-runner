//! ## Declared roles
//!
//! `mapper`, `orchestration`, `predicate`

use std::path::Path;

use oulipoly_config::{ModelConfig, ProviderConfig};
use oulipoly_runtime::executor;
use oulipoly_runtime::session_provider::SessionProviderIdentity;

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
    let provider_registry = input
        .agent_runtime_services
        .provider_registry_handle
        .current();
    // Endpoint-backed PTY launches bind the provider's real session before its first model turn.
    let live_session_binding = if provider_registry.has_account_endpoint(&input.provider.name) {
        let endpoint = provider_registry
            .preflight_account(&input.provider.name)
            .map_err(|error| {
                format!("Failed to preflight provider endpoint for live session binding: {error}")
            })?;
        let settings_id = endpoint.settings_id().map_err(|error| error.to_string())?;
        Some(executor::cli::InteractiveLiveSessionBinding {
            endpoint: endpoint.clone(),
            registry: provider_registry.clone(),
            identity: SessionProviderIdentity {
                model_name: input.model.name.clone(),
                provider_name: input.provider.name.clone(),
                provider_instance_id: Some(format!(
                    "{}-instance",
                    endpoint.capabilities().provider_id
                )),
                settings_id: settings_id.to_string(),
            },
            state_db_path: input.env.state.path().to_path_buf(),
            invocation_row_id: input.invocation_row_id,
            invocation_uuid: input.invocation.id.clone(),
            expected_provider_session_id: input.resume_session_id.map(str::to_string),
            effective_cwd: Some(interactive_effective_cwd.clone()),
        })
    } else {
        None
    };
    let zero_turn_baseline = zero_turn_record_baseline(
        &input.env.state,
        &input.env.sessions_cfg,
        &input.provider.name,
        input.resume_session_id,
    );

    let execution_result = match live_session_binding {
        Some(binding) => {
            executor::cli::execute_interactive_with_result_and_model_config_and_live_session_binding(
                input.provider,
                repl_execution_cwd(input.resume_spawn_cwd, input.working_dir),
                Some(input.invocation_env),
                resume_payload,
                input.model,
                binding,
            )
        }
        None => executor::cli::execute_interactive_with_result_and_model_config(
            input.provider,
            repl_execution_cwd(input.resume_spawn_cwd, input.working_dir),
            Some(input.invocation_env),
            resume_payload,
            input.model,
            provider_registry,
        ),
    };

    match execution_result {
        Ok(mut result) => {
            classify_repl_result(
                input.env,
                &input.provider.name,
                &zero_turn_baseline,
                &mut result,
            );
            if let Err(err) = clear_repl_session_capture_for_unpinned(
                input.env,
                input.invocation_row_id,
                input.resume,
            ) {
                handoff_repl_pty_delivery(&input, result.exit_code);
                return Err(err);
            }
            finalize_repl_execution_result(input, &interactive_effective_cwd, &result)
        }
        Err(_spawn_err) => {
            let clear_result = clear_repl_session_capture_for_unpinned(
                input.env,
                input.invocation_row_id,
                input.resume,
            );
            handoff_repl_pty_delivery(&input, 1);
            clear_result?;
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
    let session_id = input.resume_session_id;
    let invocation_uuid = input.invocation.id.as_str();
    let finalization = match handle_terminal_signal_disposition(ReplTerminalDispositionInput {
        agent_runtime_services: input.agent_runtime_services,
        env: input.env,
        invocation_row_id: input.invocation_row_id,
        guard: input.guard,
        result,
        terminal_signal_disposition,
    }) {
        Ok(ReplTerminalControl::Return(exit_code)) => Ok(exit_code),
        Err(err) => Err(err),
        Ok(ReplTerminalControl::Completed) => {
            finalize_completed_repl_execution(input, interactive_effective_cwd, result)
        }
    };
    if let Err(err) = crate::mailbox_delivery::finalize_pty_mailbox_delivery_handoff(
        session_id,
        invocation_uuid,
        result.exit_code,
    ) {
        tracing::warn!(
            session_id,
            invocation_uuid,
            "Failed to hand off REPL PTY mailbox delivery: {err}"
        );
    }
    finalization
}

fn handoff_repl_pty_delivery(input: &ReplExecutionInput<'_, '_>, exit_code: i32) {
    if let Err(err) = crate::mailbox_delivery::finalize_pty_mailbox_delivery_handoff(
        input.resume_session_id,
        &input.invocation.id,
        exit_code,
    ) {
        tracing::warn!(
            session_id = input.resume_session_id,
            invocation_uuid = input.invocation.id,
            "Failed to hand off REPL PTY mailbox delivery: {err}"
        );
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
