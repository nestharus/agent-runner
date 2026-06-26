//! ## Declared roles
//!
//! `orchestration`, `validator`, `accessor`, `mapper`, `filter`, `predicate`, `formatter`

use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::Path;
use std::sync::Arc;

use oulipoly_runtime::provider_registry::{ProviderRegistry, ProviderRegistryOptions};
use oulipoly_runtime::services::{
    ExecutorServiceRequest, InvocationLifecycleServicePort, MigrationServiceOutput,
    ResumeServiceOutput, ServiceError,
};
use oulipoly_runtime::{executor, sessions};
use sha2::{Digest, Sha256};

use super::disposition::{
    confirmed_zero_turn_maybe_quota, handle_terminal_signal_disposition, ResumeLoopControl,
    ResumeTerminalDispositionInput,
};
use super::finalization::{
    finalize_completed_attempt, CompletedAttemptControl, CompletedAttemptInput,
};
use super::{formatter, mapper, validator};
use crate::captured_child::emit_captured_child_marker_lines;
use crate::cli::inputs::resolve_resume_answer;
use crate::invocation::finalize::FinalizerGuard;
use crate::migration_providers::load_resume_execution_environment;
use crate::quota_zero_turn::{
    apply_zero_turn_classification_to_result, filter_quota_exhausted_migration_candidates,
    host_observed_completion_from_result, zero_turn_classification_for_action,
    zero_turn_classify_after_completion, zero_turn_record_baseline,
};
use crate::resume_cli::{
    format_resume_error, render_resume_model_pool_mismatch, renderable_resume_execution_target,
    resume_migration_pool,
};
use crate::spawn_cwd::effective_resume_spawn_cwd;
use crate::terminal_outcome_adapter::{
    apply_age153_terminal_signal_fixture_override, confirm_maybe_quota_exhausted,
    resume_terminal_signal_for_outcome, terminal_signal_reason, TerminalSignalDisposition,
};
use crate::wiring;
use crate::zero_turn_orchestration::{next_action, ZeroTurnAction, ZeroTurnConfirmationState};

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_resume(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    model_name: Option<&str>,
    session_id: &str,
    manual_migrate: Option<&str>,
    prompt: Option<&str>,
    file: Option<&Path>,
    working_dir: Option<&Path>,
    models_dir_override: Option<&Path>,
) -> Result<i32, String> {
    if let Some(exit_code) = reject_invalid_resume_input(session_id) {
        return Ok(exit_code);
    }
    if let Some(exit_code) = crate::wake_coordinator::validate_auto_wake_child(session_id)? {
        return Ok(exit_code);
    }
    if !crate::wake_coordinator::is_auto_wake_invocation() {
        crate::wake_coordinator::reset_manual_resume_wake_claim(session_id)?;
    }
    // Source guard marker: agent_runtime_services.resume_service.resolve_resume(ResumeServiceRequest)

    let mut prepared = match prepare_headless_resume_execution(
        agent_runtime_services,
        model_name,
        session_id,
        prompt,
        file,
        working_dir,
        models_dir_override,
    )? {
        Ok(prepared) => prepared,
        Err(exit_code) => {
            crate::wake_coordinator::release_current_auto_wake_claim_for_session(session_id);
            return Ok(exit_code);
        }
    };
    let result = run_resume_loop(ResumeLoopInput {
        agent_runtime_services,
        prepared: &mut prepared,
        manual_migrate,
        session_id,
        working_dir,
    });
    if failed_auto_wake_exit(&result) {
        let _ = crate::wake_coordinator::recheck_after_failed_auto_wake(session_id);
    }
    result
}

fn failed_auto_wake_exit(result: &Result<i32, String>) -> bool {
    crate::wake_coordinator::is_auto_wake_invocation() && !matches!(result, Ok(0))
}

fn reject_invalid_resume_input(session_id: &str) -> Option<i32> {
    match validator::validate_resume_input(session_id) {
        Ok(()) => None,
        Err(message) => {
            formatter::emit_stderr(&message);
            Some(1)
        }
    }
}

struct PreparedHeadlessResumeExecution {
    answer: Option<String>,
    mailbox_session_id: String,
    mailbox_delivery_seqs: Vec<i64>,
    mailbox_delivery_nonce: Option<String>,
    env: crate::migration_providers::ResumeExecutionEnvironment,
    resolved: oulipoly_state::ResolvedResume,
    effective_spawn_cwd: std::path::PathBuf,
    parent_invocation_id: Option<i64>,
    max_attempts: usize,
}

#[allow(clippy::too_many_arguments)]
fn prepare_headless_resume_execution(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    model_name: Option<&str>,
    session_id: &str,
    prompt: Option<&str>,
    file: Option<&Path>,
    working_dir: Option<&Path>,
    models_dir_override: Option<&Path>,
) -> Result<Result<PreparedHeadlessResumeExecution, i32>, String> {
    let answer = resolve_resume_answer(prompt, file)?;
    let env = load_resume_execution_environment(models_dir_override)?;
    refresh_resume_provider_registry(agent_runtime_services, &env)?;
    let resolved = match resolve_resume_for_headless_execution(
        agent_runtime_services,
        &env,
        session_id,
        model_name,
    ) {
        Ok(resolved) => resolved,
        Err(exit_code) => return Ok(Err(exit_code)),
    };
    if let Err(exit_code) = prepare_initial_headless_resume_target(
        &resolved,
        &env.providers_cfg,
        std::io::stderr().is_terminal(),
    ) {
        return Ok(Err(exit_code));
    }
    let effective_spawn_cwd = effective_resume_execution_cwd(&env, session_id, working_dir)?;
    let parent_invocation_id = crate::dispatch::resolve_parent_invocation_id(&env.state);
    let max_attempts = headless_resume_retry_budget(&resolved);
    let mailbox_delivery = crate::mailbox_delivery::prepare_headless_resume_delivery(
        &resolved,
        answer,
        Some(&env.models_dir),
    )?;
    Ok(Ok(PreparedHeadlessResumeExecution {
        answer: mailbox_delivery.answer,
        mailbox_session_id: mailbox_delivery.session_id,
        mailbox_delivery_seqs: mailbox_delivery.seqs,
        mailbox_delivery_nonce: mailbox_delivery.delivery_nonce,
        env,
        resolved,
        effective_spawn_cwd,
        parent_invocation_id,
        max_attempts,
    }))
}

fn refresh_resume_provider_registry(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    env: &crate::migration_providers::ResumeExecutionEnvironment,
) -> Result<(), String> {
    let models = env.models.values().cloned().collect::<Vec<_>>();
    let registry =
        ProviderRegistry::from_model_configs(&models, resume_provider_registry_options(env))
            .map_err(|err| format!("failed to build resume provider registry: {err}"))?;
    agent_runtime_services
        .provider_registry_handle
        .replace(Arc::new(registry));
    Ok(())
}

fn resume_provider_registry_options(
    env: &crate::migration_providers::ResumeExecutionEnvironment,
) -> ProviderRegistryOptions {
    ProviderRegistryOptions::default()
        .with_path_entries_from_process_path()
        .with_config_root(env.config_root.clone())
        .with_data_root(resume_provider_registry_data_root(env))
}

fn resume_provider_registry_data_root(
    env: &crate::migration_providers::ResumeExecutionEnvironment,
) -> std::path::PathBuf {
    oulipoly_state::paths::data_dir().unwrap_or_else(|_| env.config_root.clone())
}

fn effective_resume_execution_cwd(
    env: &crate::migration_providers::ResumeExecutionEnvironment,
    session_id: &str,
    working_dir: Option<&Path>,
) -> Result<std::path::PathBuf, String> {
    effective_resume_spawn_cwd(
        &env.state,
        &env.models,
        &env.providers_cfg,
        &env.sessions_cfg,
        session_id,
        working_dir,
    )
}

fn headless_resume_retry_budget(resolved: &oulipoly_state::ResolvedResume) -> usize {
    resolved
        .model
        .as_ref()
        .map(|model| model.providers.len())
        .unwrap_or(1)
        .max(1)
        + 1
}

struct ResumeLoopInput<'a> {
    agent_runtime_services: &'a wiring::AgentRuntimeServices,
    prepared: &'a mut PreparedHeadlessResumeExecution,
    manual_migrate: Option<&'a str>,
    session_id: &'a str,
    working_dir: Option<&'a Path>,
}

fn run_resume_loop(input: ResumeLoopInput<'_>) -> Result<i32, String> {
    let mut attempts = 0usize;
    let mut last_exit_code = 1;
    let mut zero_turn_confirmation = ZeroTurnConfirmationState::new();

    loop {
        if resume_attempts_exhausted(attempts, input.prepared.max_attempts) {
            return Ok(resume_attempts_exhausted_exit_code(last_exit_code));
        }
        attempts += 1;

        match run_resume_attempt(ResumeAttemptInput {
            agent_runtime_services: input.agent_runtime_services,
            env: &input.prepared.env,
            resolved: &mut input.prepared.resolved,
            answer: input.prepared.answer.as_deref(),
            mailbox_session_id: &input.prepared.mailbox_session_id,
            mailbox_delivery_seqs: &input.prepared.mailbox_delivery_seqs,
            mailbox_delivery_nonce: input.prepared.mailbox_delivery_nonce.as_deref(),
            manual_migrate: input.manual_migrate,
            session_id: input.session_id,
            working_dir: input.working_dir,
            attempts,
            max_attempts: input.prepared.max_attempts,
            parent_invocation_id: input.prepared.parent_invocation_id,
            effective_spawn_cwd: &input.prepared.effective_spawn_cwd,
            zero_turn_confirmation: &mut zero_turn_confirmation,
        })? {
            ResumeAttemptLoopControl::Continue(exit_code) => {
                last_exit_code = exit_code;
                continue;
            }
            ResumeAttemptLoopControl::Return(exit_code) => return Ok(exit_code),
        }
    }
}

fn resume_attempts_exhausted(attempts: usize, max_attempts: usize) -> bool {
    attempts >= max_attempts
}

fn resume_attempts_exhausted_exit_code(last_exit_code: i32) -> i32 {
    formatter::emit_stderr("BLOCKED:all-providers-exhausted");
    normalized_resume_exhausted_exit_code(last_exit_code)
}

fn normalized_resume_exhausted_exit_code(last_exit_code: i32) -> i32 {
    if last_exit_code == 0 {
        1
    } else {
        last_exit_code
    }
}

struct ResumeAttemptInput<'a> {
    agent_runtime_services: &'a wiring::AgentRuntimeServices,
    env: &'a crate::migration_providers::ResumeExecutionEnvironment,
    resolved: &'a mut oulipoly_state::ResolvedResume,
    answer: Option<&'a str>,
    mailbox_session_id: &'a str,
    mailbox_delivery_seqs: &'a [i64],
    mailbox_delivery_nonce: Option<&'a str>,
    manual_migrate: Option<&'a str>,
    session_id: &'a str,
    working_dir: Option<&'a Path>,
    attempts: usize,
    max_attempts: usize,
    parent_invocation_id: Option<i64>,
    effective_spawn_cwd: &'a Path,
    zero_turn_confirmation: &'a mut ZeroTurnConfirmationState,
}

enum ResumeAttemptLoopControl {
    Continue(i32),
    Return(i32),
}

fn run_resume_attempt(
    mut input: ResumeAttemptInput<'_>,
) -> Result<ResumeAttemptLoopControl, String> {
    let target = match prepare_resume_attempt_target(&mut input)? {
        Ok(target) => target,
        Err(exit_code) => return Ok(ResumeAttemptLoopControl::Return(exit_code)),
    };
    let provider_index = target.provider_index;
    let provider = target.provider;
    let strategy = match resume_attempt_strategy_for_target(input.resolved, &provider) {
        Ok(strategy) => strategy,
        Err(exit_code) => return Ok(ResumeAttemptLoopControl::Return(exit_code)),
    };

    let mut bound_attempt = setup_bound_resume_attempt(&input, &provider, provider_index)?;

    let mut result = match execute_resume_attempt_command(
        &input,
        &provider,
        provider_index,
        target.prompt_mode,
        &bound_attempt.invocation_env,
        strategy,
    ) {
        Ok(result) => result,
        Err(_spawn_err) => return handle_resume_attempt_spawn_error(&input, &mut bound_attempt),
    };

    handle_resume_attempt_result(&mut input, &mut bound_attempt, &provider, &mut result)
}

fn handle_resume_attempt_spawn_error(
    input: &ResumeAttemptInput<'_>,
    bound_attempt: &mut BoundResumeAttempt<'_>,
) -> Result<ResumeAttemptLoopControl, String> {
    record_failed_mailbox_delivery_attempt(input, "resume_spawn_error")?;
    finalize_resume_spawn_error(input, &mut bound_attempt.attempt)?;
    Ok(ResumeAttemptLoopControl::Return(1))
}

fn handle_resume_attempt_result(
    input: &mut ResumeAttemptInput<'_>,
    bound_attempt: &mut BoundResumeAttempt<'_>,
    provider: &oulipoly_config::ProviderConfig,
    result: &mut executor::ExecutionResult,
) -> Result<ResumeAttemptLoopControl, String> {
    apply_resume_attempt_classification(
        input.env,
        &provider.name,
        &bound_attempt.provider_session_id,
        &bound_attempt.zero_turn_baseline,
        input.zero_turn_confirmation,
        result,
    )
    .and_then(|zero_turn_action| {
        record_resume_acceptance_if_present(
            input,
            bound_attempt.attempt.invocation_row_id,
            result,
        )?;
        emit_captured_child_marker_lines(&result.captured_child_invocations);
        handle_resume_attempt_terminal_signal(
            input,
            &mut bound_attempt.attempt,
            provider,
            &bound_attempt.provider_session_id,
            result,
            zero_turn_action,
        )
    })
}

fn resume_attempt_strategy_for_target<'a>(
    resolved: &oulipoly_state::ResolvedResume,
    provider: &'a oulipoly_config::ProviderConfig,
) -> Result<Option<&'a oulipoly_config::ResumeStrategy>, i32> {
    if resolved_uses_provider_ref(resolved) {
        return Ok(None);
    }
    resume_attempt_strategy(provider).map(Some)
}

fn resume_attempt_strategy(
    provider: &oulipoly_config::ProviderConfig,
) -> Result<&oulipoly_config::ResumeStrategy, i32> {
    provider
        .resume
        .as_ref()
        .ok_or_else(|| missing_resume_block_exit_code(&provider.name))
}

fn missing_resume_block_exit_code(provider_name: &str) -> i32 {
    formatter::emit_missing_resume_block(provider_name);
    1
}

fn execute_resume_attempt_command(
    input: &ResumeAttemptInput<'_>,
    provider: &oulipoly_config::ProviderConfig,
    provider_index: usize,
    prompt_mode: oulipoly_config::PromptMode,
    invocation_env: &str,
    strategy: Option<&oulipoly_config::ResumeStrategy>,
) -> Result<executor::ExecutionResult, String> {
    if let Some(request) = provider_ref_resume_executor_request(
        input,
        provider,
        provider_index,
        prompt_mode,
        invocation_env,
    ) {
        return input
            .agent_runtime_services
            .executor_service
            .execute(request)
            .map(|output| output.result)
            .map_err(|err| err.to_string());
    }
    executor::cli::execute_resume_optional_prompt_with_model_identity(
        provider,
        provider_index,
        prompt_mode,
        input.answer,
        Some(input.effective_spawn_cwd),
        Some(invocation_env),
        executor::cli::ResumePayload {
            session_id: &input.resolved.active_session_id,
            strategy: strategy.expect("legacy resume target must have a resume strategy"),
        },
        input.resolved.model_name.as_deref().unwrap_or("<unknown>"),
        Some(&input.env.models_dir),
    )
}

fn provider_ref_resume_executor_request(
    input: &ResumeAttemptInput<'_>,
    provider: &oulipoly_config::ProviderConfig,
    provider_index: usize,
    prompt_mode: oulipoly_config::PromptMode,
    invocation_env: &str,
) -> Option<ExecutorServiceRequest> {
    let model = input.resolved.model.as_ref()?;
    model.provider.as_ref()?;
    Some(
        ExecutorServiceRequest::EffectiveWithStartKnownProviderSessionId {
            model: model.clone(),
            provider: provider.clone(),
            provider_index,
            prompt_mode,
            prompt: input.answer.unwrap_or_default().to_string(),
            working_dir: Some(input.effective_spawn_cwd.to_path_buf()),
            models_dir: Some(input.env.models_dir.clone()),
            extra_inputs: HashMap::new(),
            parent_invocation_env: Some(invocation_env.to_string()),
            start_known_provider_session_id: input.resolved.active_session_id.clone(),
        },
    )
}

fn finalize_resume_spawn_error(
    input: &ResumeAttemptInput<'_>,
    attempt: &mut ResumeInvocationAttempt<'_>,
) -> Result<(), String> {
    input
        .agent_runtime_services
        .invocation_lifecycle_service
        .finalize_invocation(mapper::spawn_error_finalize_request(
            &input.env.state,
            attempt.invocation_row_id,
        ))
        .map_err(|err| err.to_string())?;
    attempt.guard.mark_finalized();
    mark_resume_attempt_idle(
        &input.resolved.active_session_id,
        &attempt.invocation.id,
        Some(1),
    )?;
    Ok(())
}

fn prepare_resume_attempt_target(
    input: &mut ResumeAttemptInput<'_>,
) -> Result<Result<crate::resume_cli::ResumeExecutionTarget, i32>, String> {
    let mut target =
        match renderable_resume_execution_target(input.resolved, &input.env.providers_cfg) {
            Ok(target) => target,
            Err(exit_code) => return Ok(Err(exit_code)),
        };
    if let Err(exit_code) = migrate_resume_target(
        input.agent_runtime_services,
        input.env,
        input.resolved,
        &mut target,
        input.manual_migrate,
        input.attempts,
        input.effective_spawn_cwd,
    ) {
        return Ok(Err(exit_code));
    }
    Ok(Ok(target))
}

struct ResumeInvocationAttempt<'state> {
    invocation: oulipoly_state::CompositeInvocationId,
    invocation_row_id: i64,
    guard: FinalizerGuard<'state>,
}

struct BoundResumeAttempt<'state> {
    attempt: ResumeInvocationAttempt<'state>,
    provider_session_id: String,
    zero_turn_baseline: crate::zero_turn_orchestration::ZeroTurnBaseline,
    invocation_env: String,
}

fn setup_bound_resume_attempt<'state>(
    input: &ResumeAttemptInput<'state>,
    provider: &oulipoly_config::ProviderConfig,
    provider_index: usize,
) -> Result<BoundResumeAttempt<'state>, String> {
    let attempt = start_resume_invocation(input, provider, provider_index)?;
    let provider_session_id = input.resolved.active_session_id.clone();
    bind_resume_attempt_session(
        input,
        provider,
        attempt.invocation_row_id,
        &provider_session_id,
    )?;
    let zero_turn_baseline = zero_turn_record_baseline(
        &input.env.state,
        &input.env.sessions_cfg,
        &provider.name,
        Some(&provider_session_id),
    );
    let invocation_env = resume_invocation_env(&attempt.invocation)?;
    formatter::emit_stderr(&attempt.invocation.stderr_line());
    Ok(BoundResumeAttempt {
        attempt,
        provider_session_id,
        zero_turn_baseline,
        invocation_env,
    })
}

fn start_resume_invocation<'state>(
    input: &ResumeAttemptInput<'state>,
    provider: &oulipoly_config::ProviderConfig,
    provider_index: usize,
) -> Result<ResumeInvocationAttempt<'state>, String> {
    let invocation = mapper::composite_invocation_id(&provider.name);
    let invocation_start = mapper::resume_invocation_start(
        &invocation,
        input.resolved.model_name.as_deref(),
        &provider.name,
        provider_index,
        input.parent_invocation_id,
    );
    let invocation_row_id = input
        .agent_runtime_services
        .invocation_lifecycle_service
        .start_invocation(mapper::invocation_lifecycle_start_request(
            &input.env.state,
            &invocation_start,
        ))
        .map_err(|err| err.to_string())?
        .invocation_row_id;
    let guard = FinalizerGuard::new(&input.env.state, invocation_row_id);
    Ok(ResumeInvocationAttempt {
        invocation,
        invocation_row_id,
        guard,
    })
}

fn bind_resume_attempt_session(
    input: &ResumeAttemptInput<'_>,
    provider: &oulipoly_config::ProviderConfig,
    invocation_row_id: i64,
    provider_session_id: &str,
) -> Result<(), String> {
    input.env.state.bind_invocation_provider_session_start(
        invocation_row_id,
        &mapper::resumed_provider_session_binding(
            provider,
            provider_session_id,
            Some(input.session_id.to_string()),
        ),
    )?;
    if should_record_legacy_resume_input(input.manual_migrate) {
        record_legacy_resume_input_session_id(input, invocation_row_id)?;
    }
    Ok(())
}

fn should_record_legacy_resume_input(manual_migrate: Option<&str>) -> bool {
    manual_migrate.is_some()
}

fn record_legacy_resume_input_session_id(
    input: &ResumeAttemptInput<'_>,
    invocation_row_id: i64,
) -> Result<(), String> {
    input
        .env
        .state
        .record_legacy_resume_input_session_id(invocation_row_id, input.session_id)
}

fn resume_invocation_env(
    invocation: &oulipoly_state::CompositeInvocationId,
) -> Result<String, String> {
    serde_json::to_string(invocation).map_err(|e| format!("Failed to serialize invocation id: {e}"))
}

fn apply_resume_attempt_classification(
    env: &crate::migration_providers::ResumeExecutionEnvironment,
    provider_name: &str,
    provider_session_id: &str,
    zero_turn_baseline: &crate::zero_turn_orchestration::ZeroTurnBaseline,
    zero_turn_confirmation: &mut ZeroTurnConfirmationState,
    result: &mut executor::ExecutionResult,
) -> Result<crate::zero_turn_orchestration::ZeroTurnAction, String> {
    apply_age153_terminal_signal_fixture_override(result);
    let zero_turn_classification = zero_turn_classify_after_completion(
        &env.state,
        &env.sessions_cfg,
        zero_turn_baseline,
        host_observed_completion_from_result(result),
    );
    apply_zero_turn_classification_to_result(result, provider_name, &zero_turn_classification);
    Ok(next_action(
        zero_turn_confirmation,
        zero_turn_classification_for_action(
            zero_turn_classification,
            result,
            provider_name,
            Some(provider_session_id),
        ),
    ))
}

fn record_resume_acceptance_if_present(
    input: &ResumeAttemptInput<'_>,
    invocation_row_id: i64,
    result: &executor::ExecutionResult,
) -> Result<(), String> {
    let Some(acceptance) = resume_acceptance_result(result) else {
        return Ok(());
    };
    record_resume_acceptance(input, invocation_row_id, acceptance)
}

fn resume_acceptance_result(
    result: &executor::ExecutionResult,
) -> Option<&oulipoly_runtime::executor::ResumeAcceptanceResult> {
    result.resume_acceptance.as_ref()
}

fn record_resume_acceptance(
    input: &ResumeAttemptInput<'_>,
    invocation_row_id: i64,
    acceptance: &oulipoly_runtime::executor::ResumeAcceptanceResult,
) -> Result<(), String> {
    input
        .agent_runtime_services
        .resume_service
        .record_acceptance(mapper::resume_acceptance_request(
            &input.env.state,
            invocation_row_id,
            acceptance,
        ))
        .map_err(format_resume_acceptance_service_error)?;
    Ok(())
}

fn format_resume_acceptance_service_error(err: oulipoly_runtime::services::ServiceError) -> String {
    format!("resume acceptance service failed: {err}")
}

fn handle_resume_attempt_terminal_signal(
    input: &ResumeAttemptInput<'_>,
    attempt: &mut ResumeInvocationAttempt<'_>,
    provider: &oulipoly_config::ProviderConfig,
    provider_session_id: &str,
    result: &executor::ExecutionResult,
    zero_turn_action: crate::zero_turn_orchestration::ZeroTurnAction,
) -> Result<ResumeAttemptLoopControl, String> {
    let terminal_signal_disposition = terminal_signal_disposition_for_result(
        &input.env.state,
        &attempt.invocation.id,
        &provider.name,
        provider_session_id,
        result,
        zero_turn_action,
    );

    let disposition_control = handle_terminal_signal_disposition(ResumeTerminalDispositionInput {
        agent_runtime_services: input.agent_runtime_services,
        env: input.env,
        invocation_row_id: attempt.invocation_row_id,
        guard: &mut attempt.guard,
        result,
        terminal_signal_disposition,
        zero_turn_action,
    })?;
    if let Some(control) = apply_resume_terminal_disposition_outcome(
        input,
        attempt,
        provider_session_id,
        result,
        terminal_disposition_outcome(disposition_control, result),
    )? {
        return Ok(control);
    }

    ingest_mailbox_delivery_confirmation_turn_if_needed(input, provider, result, zero_turn_action);

    if let Some(control) = handle_unconfirmed_mailbox_delivery_if_needed(
        input,
        attempt,
        provider,
        provider_session_id,
        result,
        zero_turn_action,
    )? {
        return Ok(control);
    }

    finalize_completed_attempt_for_resume(input, attempt, provider, provider_session_id, result)
}

fn ingest_mailbox_delivery_confirmation_turn_if_needed(
    input: &ResumeAttemptInput<'_>,
    provider: &oulipoly_config::ProviderConfig,
    result: &executor::ExecutionResult,
    zero_turn_action: ZeroTurnAction,
) {
    if !mailbox_delivery_requires_turn_confirmation(input, &provider.name, result) {
        return;
    }
    if mailbox_delivery_turn_confirmed(input, &provider.name, result, zero_turn_action) {
        return;
    }
    let report = sessions::scan_provider_session(
        &provider.name,
        &input.env.sessions_cfg,
        &input.env.state,
        &input.resolved.active_session_id,
    );
    emit_pre_confirmation_session_scan_errors(&provider.name, report.errors);
}

fn emit_pre_confirmation_session_scan_errors(provider_name: &str, errors: Vec<String>) {
    for error in errors {
        formatter::emit_stderr(&format!(
            "Warning: Session ingest failed for {provider_name}: {error}"
        ));
    }
}

enum ResumeTerminalDispositionOutcome {
    Continue(i32),
    Return(i32),
    CompletedAttempt,
}

fn terminal_disposition_outcome(
    control: ResumeLoopControl,
    result: &executor::ExecutionResult,
) -> ResumeTerminalDispositionOutcome {
    match control {
        ResumeLoopControl::Continue => ResumeTerminalDispositionOutcome::Continue(result.exit_code),
        ResumeLoopControl::Return(exit_code) => ResumeTerminalDispositionOutcome::Return(exit_code),
        ResumeLoopControl::CompletedAttempt => ResumeTerminalDispositionOutcome::CompletedAttempt,
    }
}

fn apply_resume_terminal_disposition_outcome(
    input: &ResumeAttemptInput<'_>,
    attempt: &ResumeInvocationAttempt<'_>,
    provider_session_id: &str,
    result: &executor::ExecutionResult,
    outcome: ResumeTerminalDispositionOutcome,
) -> Result<Option<ResumeAttemptLoopControl>, String> {
    match outcome {
        ResumeTerminalDispositionOutcome::Continue(exit_code) => {
            record_failed_mailbox_delivery_attempt(
                input,
                &failed_delivery_error(result, "resume_retry"),
            )?;
            mark_resume_attempt_idle(provider_session_id, &attempt.invocation.id, Some(exit_code))?;
            Ok(Some(ResumeAttemptLoopControl::Continue(exit_code)))
        }
        ResumeTerminalDispositionOutcome::Return(exit_code) => {
            record_failed_mailbox_delivery_attempt(
                input,
                &failed_delivery_error(result, "resume_failed"),
            )?;
            mark_resume_attempt_idle(provider_session_id, &attempt.invocation.id, Some(exit_code))?;
            Ok(Some(ResumeAttemptLoopControl::Return(exit_code)))
        }
        ResumeTerminalDispositionOutcome::CompletedAttempt => Ok(None),
    }
}

fn handle_unconfirmed_mailbox_delivery_if_needed(
    input: &ResumeAttemptInput<'_>,
    attempt: &mut ResumeInvocationAttempt<'_>,
    provider: &oulipoly_config::ProviderConfig,
    provider_session_id: &str,
    result: &executor::ExecutionResult,
    zero_turn_action: ZeroTurnAction,
) -> Result<Option<ResumeAttemptLoopControl>, String> {
    if !mailbox_delivery_unconfirmed(input, &provider.name, result, zero_turn_action) {
        return Ok(None);
    }
    record_failed_mailbox_delivery_attempt(input, "mailbox_delivery_unconfirmed")?;
    finalize_unconfirmed_mailbox_delivery(input, attempt, result)?;
    mark_resume_attempt_idle(provider_session_id, &attempt.invocation.id, Some(1))?;
    Ok(Some(ResumeAttemptLoopControl::Return(1)))
}

fn finalize_completed_attempt_for_resume(
    input: &ResumeAttemptInput<'_>,
    attempt: &mut ResumeInvocationAttempt<'_>,
    provider: &oulipoly_config::ProviderConfig,
    provider_session_id: &str,
    result: &executor::ExecutionResult,
) -> Result<ResumeAttemptLoopControl, String> {
    match finalize_completed_attempt(CompletedAttemptInput {
        agent_runtime_services: input.agent_runtime_services,
        env: input.env,
        invocation: &attempt.invocation,
        invocation_row_id: attempt.invocation_row_id,
        guard: &mut attempt.guard,
        provider_name: &provider.name,
        model: input.resolved.model.as_ref(),
        result,
        working_dir: input.working_dir,
        manual_migrate: input.manual_migrate,
        session_id: input.session_id,
        active_session_id: &input.resolved.active_session_id,
        effective_spawn_cwd: input.effective_spawn_cwd,
        attempts: input.attempts,
        max_attempts: input.max_attempts,
    })? {
        CompletedAttemptControl::Continue => {
            record_failed_mailbox_delivery_attempt(
                input,
                &failed_delivery_error(result, "quota_exhausted"),
            )?;
            mark_resume_attempt_idle(
                provider_session_id,
                &attempt.invocation.id,
                Some(result.exit_code),
            )?;
            Ok(ResumeAttemptLoopControl::Continue(result.exit_code))
        }
        CompletedAttemptControl::Return(exit_code) => {
            if exit_code == 0 {
                crate::mailbox_delivery::mark_headless_resume_delivered(
                    input.mailbox_session_id,
                    input.mailbox_delivery_seqs,
                    &attempt.invocation.id,
                )?;
                let _ = crate::wake_coordinator::mark_successful_turn_idle_and_recheck(
                    provider_session_id,
                    &attempt.invocation.id,
                    exit_code,
                )?;
            } else {
                record_failed_mailbox_delivery_attempt(
                    input,
                    &failed_delivery_error(result, "resume_failed"),
                )?;
                mark_resume_attempt_idle(
                    provider_session_id,
                    &attempt.invocation.id,
                    Some(exit_code),
                )?;
            }
            Ok(ResumeAttemptLoopControl::Return(exit_code))
        }
    }
}

fn mailbox_delivery_unconfirmed(
    input: &ResumeAttemptInput<'_>,
    provider_name: &str,
    result: &executor::ExecutionResult,
    zero_turn_action: ZeroTurnAction,
) -> bool {
    mailbox_delivery_requires_turn_confirmation(input, provider_name, result)
        && !mailbox_delivery_turn_confirmed(input, provider_name, result, zero_turn_action)
}

fn mailbox_delivery_requires_turn_confirmation(
    input: &ResumeAttemptInput<'_>,
    provider_name: &str,
    result: &executor::ExecutionResult,
) -> bool {
    result.exit_code == 0
        && !input.mailbox_delivery_seqs.is_empty()
        && input.answer.is_some_and(|answer| !answer.trim().is_empty())
        && input.env.sessions_cfg.get(provider_name).is_some()
}

fn mailbox_delivery_turn_confirmed(
    input: &ResumeAttemptInput<'_>,
    provider_name: &str,
    result: &executor::ExecutionResult,
    zero_turn_action: ZeroTurnAction,
) -> bool {
    matches!(zero_turn_action, ZeroTurnAction::Continue)
        || submitted_user_turn_confirms_mailbox_delivery(input, result)
        || ingested_user_turn_confirms_mailbox_delivery(input, provider_name)
}

fn submitted_user_turn_confirms_mailbox_delivery(
    input: &ResumeAttemptInput<'_>,
    result: &executor::ExecutionResult,
) -> bool {
    let Some(submitted) = result.submitted_user_turn.as_ref() else {
        return false;
    };
    let Some(answer) = input.answer else {
        return false;
    };
    submitted.provider_session_id == input.resolved.active_session_id
        && submitted_user_turn_payload_confirms_mailbox_delivery(input, submitted, answer)
}

fn submitted_user_turn_payload_confirms_mailbox_delivery(
    input: &ResumeAttemptInput<'_>,
    submitted: &executor::SubmittedUserTurn,
    answer: &str,
) -> bool {
    if let Some(delivery_nonce) = input.mailbox_delivery_nonce {
        return submitted.delivery_nonce.as_deref() == Some(delivery_nonce);
    }
    submitted.prompt_sha256 == sha256_hex(answer.as_bytes())
}

fn ingested_user_turn_confirms_mailbox_delivery(
    input: &ResumeAttemptInput<'_>,
    provider_name: &str,
) -> bool {
    if let Some(delivery_nonce) = input.mailbox_delivery_nonce {
        return input
            .env
            .state
            .has_session_user_turn_containing(
                provider_name,
                &input.resolved.active_session_id,
                delivery_nonce,
            )
            .unwrap_or(false);
    }

    let Some(answer) = input.answer else {
        return false;
    };
    input
        .env
        .state
        .has_session_user_text_turn(provider_name, &input.resolved.active_session_id, answer)
        .unwrap_or(false)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn finalize_unconfirmed_mailbox_delivery(
    input: &ResumeAttemptInput<'_>,
    attempt: &mut ResumeInvocationAttempt<'_>,
    result: &executor::ExecutionResult,
) -> Result<(), String> {
    formatter::emit_stderr(
        "resume mailbox delivery was not confirmed by a new session turn; leaving mailbox rows pending",
    );
    input
        .agent_runtime_services
        .invocation_lifecycle_service
        .finalize_invocation(mapper::finalize_request(
            &input.env.state,
            attempt.invocation_row_id,
            false,
            1,
            Some("mailbox_delivery_unconfirmed"),
            result.terminal_reason.as_deref(),
        ))
        .map_err(|err| err.to_string())?;
    attempt.guard.mark_finalized();
    Ok(())
}

fn record_failed_mailbox_delivery_attempt(
    input: &ResumeAttemptInput<'_>,
    delivery_error: &str,
) -> Result<(), String> {
    crate::mailbox_delivery::mark_headless_resume_delivery_failed(
        input.mailbox_session_id,
        input.mailbox_delivery_seqs,
        delivery_error,
    )
}

fn failed_delivery_error(result: &executor::ExecutionResult, fallback: &str) -> String {
    terminal_signal_reason(&result.terminal_signal, result.terminal_reason.as_deref())
        .or(result.terminal_reason.as_deref())
        .unwrap_or(fallback)
        .to_string()
}

fn mark_resume_attempt_idle(
    provider_session_id: &str,
    invocation_uuid: &str,
    exit_code: Option<i32>,
) -> Result<(), String> {
    crate::wake_coordinator::mark_session_idle_after_turn(
        provider_session_id,
        invocation_uuid,
        exit_code,
    )
}

fn resolve_resume_for_headless_execution(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    env: &crate::migration_providers::ResumeExecutionEnvironment,
    session_id: &str,
    model_name: Option<&str>,
) -> Result<oulipoly_state::ResolvedResume, i32> {
    let output = agent_runtime_services
        .resume_service
        .resolve_resume(mapper::resume_service_request(env, session_id, model_name))
        .map_err(|err| err.to_string());
    headless_resume_resolution_result(output, env, session_id, model_name)
}

fn headless_resume_resolution_result(
    output: Result<ResumeServiceOutput, String>,
    env: &crate::migration_providers::ResumeExecutionEnvironment,
    session_id: &str,
    model_name: Option<&str>,
) -> Result<oulipoly_state::ResolvedResume, i32> {
    match map_headless_resume_resolution(output) {
        HeadlessResumeResolution::Resolved(resolved) => Ok(resolved),
        resolution => {
            render_headless_resume_resolution_error(resolution, env, session_id, model_name);
            Err(1)
        }
    }
}

enum HeadlessResumeResolution {
    Resolved(oulipoly_state::ResolvedResume),
    ProviderModelMismatch { active_provider: String },
    Rejected(oulipoly_state::ResumeError),
    ServiceFailure(String),
}

fn map_headless_resume_resolution(
    output: Result<ResumeServiceOutput, String>,
) -> HeadlessResumeResolution {
    match output {
        Ok(ResumeServiceOutput::ResumeResolved { resolved }) => {
            HeadlessResumeResolution::Resolved(resolved)
        }
        Ok(ResumeServiceOutput::ResumeRejected {
            error:
                oulipoly_state::ResumeError::ProviderModelMismatch {
                    active_provider, ..
                },
        }) => HeadlessResumeResolution::ProviderModelMismatch { active_provider },
        Ok(ResumeServiceOutput::ResumeRejected { error }) => {
            HeadlessResumeResolution::Rejected(error)
        }
        Err(err) => HeadlessResumeResolution::ServiceFailure(err),
    }
}

fn render_headless_resume_resolution_error(
    resolution: HeadlessResumeResolution,
    env: &crate::migration_providers::ResumeExecutionEnvironment,
    session_id: &str,
    model_name: Option<&str>,
) {
    match resolution {
        HeadlessResumeResolution::ProviderModelMismatch { active_provider } => {
            render_resume_model_pool_mismatch(
                &env.models,
                model_name.unwrap_or("<unknown>"),
                session_id,
                &active_provider,
            );
        }
        HeadlessResumeResolution::Rejected(error) => render_resume_error(error),
        HeadlessResumeResolution::ServiceFailure(err) => render_resume_service_failure(&err),
        HeadlessResumeResolution::Resolved(_) => {}
    }
}

fn render_resume_error(error: oulipoly_state::ResumeError) {
    formatter::emit_stderr(&format_resume_error(error));
}

fn render_resume_service_failure(error: &str) {
    formatter::emit_stderr(&format!("resume service failed: {error}"));
}

fn prepare_initial_headless_resume_target(
    resolved: &oulipoly_state::ResolvedResume,
    providers_cfg: &oulipoly_config::ProvidersConfig,
    stderr_is_terminal: bool,
) -> Result<(), i32> {
    let target = renderable_resume_execution_target(resolved, providers_cfg)?;
    render_resume_short_line_if_needed(stderr_is_terminal, &resolved.active_provider);
    validate_headless_resume_target(resolved, &target, &resolved.active_provider)
}

fn render_resume_short_line_if_needed(stderr_is_terminal: bool, selected_provider: &str) {
    if crate::dispatch::should_emit_resume_short_line(stderr_is_terminal) {
        formatter::emit_resume_short_line(selected_provider);
    }
}

fn validate_headless_resume_target(
    resolved: &oulipoly_state::ResolvedResume,
    target: &crate::resume_cli::ResumeExecutionTarget,
    selected_provider: &str,
) -> Result<(), i32> {
    if resolved_uses_provider_ref(resolved) {
        return validate_provider_ref_headless_resume_target(resolved, target, selected_provider)
            .map_err(|message| provider_ref_resume_block_exit_code(&message));
    }
    if headless_resume_target_has_resume(target) {
        Ok(())
    } else {
        Err(headless_missing_resume_block_exit_code(selected_provider))
    }
}

fn validate_provider_ref_headless_resume_target(
    resolved: &oulipoly_state::ResolvedResume,
    target: &crate::resume_cli::ResumeExecutionTarget,
    selected_provider: &str,
) -> Result<(), String> {
    let model = require_provider_ref_resolved_model(
        provider_ref_resolved_model(resolved),
        selected_provider,
    )?;
    require_provider_ref_model_implementation(model, selected_provider)?;
    let target_member_name = require_provider_ref_target_member_name(
        provider_ref_target_member_name(model, target.provider_index),
        selected_provider,
        target.provider_index,
    )?;
    require_provider_ref_resolved_provider(selected_provider, target_member_name)?;
    require_provider_ref_loaded_provider(
        selected_provider,
        provider_ref_loaded_provider_name(target),
    )
}

fn require_provider_ref_resolved_model<'a>(
    model: Option<&'a oulipoly_config::ModelConfig>,
    selected_provider: &str,
) -> Result<&'a oulipoly_config::ModelConfig, String> {
    model.ok_or_else(|| provider_ref_missing_model_message(selected_provider))
}

fn require_provider_ref_model_implementation(
    model: &oulipoly_config::ModelConfig,
    selected_provider: &str,
) -> Result<(), String> {
    if provider_ref_model_has_implementation(model) {
        Ok(())
    } else {
        Err(provider_ref_missing_implementation_message(
            selected_provider,
        ))
    }
}

fn require_provider_ref_target_member_name<'a>(
    target_member_name: Option<&'a str>,
    selected_provider: &str,
    provider_index: usize,
) -> Result<&'a str, String> {
    target_member_name
        .ok_or_else(|| provider_ref_invalid_index_message(selected_provider, provider_index))
}

fn require_provider_ref_resolved_provider(
    selected_provider: &str,
    target_member_name: &str,
) -> Result<(), String> {
    if target_member_name == selected_provider {
        Ok(())
    } else {
        Err(provider_ref_resolved_provider_message(
            selected_provider,
            target_member_name,
        ))
    }
}

fn require_provider_ref_loaded_provider(
    selected_provider: &str,
    loaded_provider_name: &str,
) -> Result<(), String> {
    if loaded_provider_name == selected_provider {
        Ok(())
    } else {
        Err(provider_ref_loaded_provider_message(
            selected_provider,
            loaded_provider_name,
        ))
    }
}

fn provider_ref_resolved_model(
    resolved: &oulipoly_state::ResolvedResume,
) -> Option<&oulipoly_config::ModelConfig> {
    resolved.model.as_ref()
}

fn provider_ref_model_has_implementation(model: &oulipoly_config::ModelConfig) -> bool {
    model.provider.is_some()
}

fn provider_ref_target_member_name(
    model: &oulipoly_config::ModelConfig,
    provider_index: usize,
) -> Option<&str> {
    model
        .providers
        .get(provider_index)
        .map(|provider| provider.name.as_str())
}

fn provider_ref_loaded_provider_name(target: &crate::resume_cli::ResumeExecutionTarget) -> &str {
    &target.provider.name
}

fn provider_ref_missing_model_message(selected_provider: &str) -> String {
    format!("provider-ref resume target {selected_provider} has no model config")
}

fn provider_ref_missing_implementation_message(selected_provider: &str) -> String {
    format!("provider-ref resume target {selected_provider} has no provider implementation")
}

fn provider_ref_invalid_index_message(selected_provider: &str, provider_index: usize) -> String {
    format!(
        "provider-ref resume target {selected_provider} has invalid provider index {provider_index}"
    )
}

fn provider_ref_resolved_provider_message(
    selected_provider: &str,
    resolved_provider: &str,
) -> String {
    format!("provider-ref resume target {selected_provider} resolved provider {resolved_provider}")
}

fn provider_ref_loaded_provider_message(selected_provider: &str, loaded_provider: &str) -> String {
    format!("provider-ref resume target {selected_provider} loaded provider {loaded_provider}")
}

fn headless_resume_target_has_resume(target: &crate::resume_cli::ResumeExecutionTarget) -> bool {
    target.provider.resume.is_some()
}

fn headless_missing_resume_block_exit_code(selected_provider: &str) -> i32 {
    formatter::emit_missing_resume_block(selected_provider);
    1
}

fn provider_ref_resume_block_exit_code(message: &str) -> i32 {
    formatter::emit_stderr(message);
    1
}

fn migrate_resume_target(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    env: &crate::migration_providers::ResumeExecutionEnvironment,
    resolved: &mut oulipoly_state::ResolvedResume,
    target: &mut crate::resume_cli::ResumeExecutionTarget,
    manual_migrate: Option<&str>,
    attempts: usize,
    effective_spawn_cwd: &Path,
) -> Result<(), i32> {
    if validate_provider_ref_default_migration_skip(resolved, target, manual_migrate)? {
        return apply_provider_ref_bound_resume_segment(env, resolved, target, effective_spawn_cwd);
    }
    let migration_model = migration_model_for_attempt(env, resolved, manual_migrate, attempts);
    let migration_result = dispatch_resume_migration(
        agent_runtime_services,
        env,
        resolved,
        manual_migrate,
        attempts,
        effective_spawn_cwd,
        &migration_model,
    );
    apply_resume_migration_result(env, resolved, target, migration_result)
}

fn validate_provider_ref_default_migration_skip(
    resolved: &oulipoly_state::ResolvedResume,
    target: &crate::resume_cli::ResumeExecutionTarget,
    manual_migrate: Option<&str>,
) -> Result<bool, i32> {
    if !should_skip_provider_ref_default_migration(resolved, manual_migrate) {
        return Ok(false);
    }
    validate_provider_ref_headless_resume_target(resolved, target, &resolved.active_provider)
        .map_err(|message| provider_ref_resume_block_exit_code(&message))?;
    Ok(true)
}

fn apply_provider_ref_bound_resume_segment(
    env: &crate::migration_providers::ResumeExecutionEnvironment,
    resolved: &mut oulipoly_state::ResolvedResume,
    target: &mut crate::resume_cli::ResumeExecutionTarget,
    effective_spawn_cwd: &Path,
) -> Result<(), i32> {
    let model = migration_model_pool(env, resolved);
    let mut migration_stderr = std::io::stderr();
    let mut fresh_session_id = || uuid::Uuid::new_v4().to_string();
    match oulipoly_runtime::migration::bound_provider_ref_resume_segment(
        &env.state,
        &env.sessions_cfg,
        &model,
        resolved,
        effective_spawn_cwd,
        &mut fresh_session_id,
        &mut migration_stderr,
    ) {
        Ok(oulipoly_runtime::migration::ProviderRefBoundOutcome::Rotated(segment)) => {
            apply_migrated_segment(env, resolved, target, segment)
        }
        Ok(
            oulipoly_runtime::migration::ProviderRefBoundOutcome::NoBoundary
            | oulipoly_runtime::migration::ProviderRefBoundOutcome::BoundaryNotFound
            | oulipoly_runtime::migration::ProviderRefBoundOutcome::AlreadyBounded,
        ) => Ok(()),
        Err(
            oulipoly_runtime::migration::MigrationError::SourceMissingStorage { .. }
            | oulipoly_runtime::migration::MigrationError::TargetMissingStorage { .. },
        ) => Ok(()),
        Err(err) => {
            emit_migration_service_error(MigrationServiceErrorMessage::Dependency(format!(
                "{err:?}"
            )));
            Err(1)
        }
    }
}

fn apply_resume_migration_result(
    env: &crate::migration_providers::ResumeExecutionEnvironment,
    resolved: &mut oulipoly_state::ResolvedResume,
    target: &mut crate::resume_cli::ResumeExecutionTarget,
    migration_result: Result<MigrationServiceOutput, ServiceError>,
) -> Result<(), i32> {
    match migration_result {
        Ok(MigrationServiceOutput::Migrated { segment: migrated })
        | Ok(MigrationServiceOutput::AutoRotated {
            segment: migrated, ..
        }) => apply_migrated_segment(env, resolved, target, migrated),
        Ok(MigrationServiceOutput::Stay) => Ok(()),
        Ok(MigrationServiceOutput::RotationFailed { reason }) => fail_rotation(reason),
        Err(err) => fail_migration_service(err),
    }
}

fn should_skip_provider_ref_default_migration(
    resolved: &oulipoly_state::ResolvedResume,
    manual_migrate: Option<&str>,
) -> bool {
    manual_migrate.is_none() && resolved_uses_provider_ref(resolved)
}

fn resolved_uses_provider_ref(resolved: &oulipoly_state::ResolvedResume) -> bool {
    resolved
        .model
        .as_ref()
        .is_some_and(|model| model.provider.is_some())
}

fn migration_model_for_attempt(
    env: &crate::migration_providers::ResumeExecutionEnvironment,
    resolved: &oulipoly_state::ResolvedResume,
    manual_migrate: Option<&str>,
    attempts: usize,
) -> oulipoly_config::ModelConfig {
    let mut migration_model = migration_model_pool(env, resolved);
    apply_migration_candidate_filter(
        env,
        resolved,
        manual_migrate,
        attempts,
        &mut migration_model,
    );
    migration_model
}

fn apply_migration_candidate_filter(
    env: &crate::migration_providers::ResumeExecutionEnvironment,
    resolved: &oulipoly_state::ResolvedResume,
    manual_migrate: Option<&str>,
    attempts: usize,
    migration_model: &mut oulipoly_config::ModelConfig,
) {
    if should_filter_migration_candidates(manual_migrate, attempts) {
        filter_quota_exhausted_migration_model(env, resolved, migration_model);
    }
}

fn migration_model_pool(
    env: &crate::migration_providers::ResumeExecutionEnvironment,
    resolved: &oulipoly_state::ResolvedResume,
) -> oulipoly_config::ModelConfig {
    resume_migration_pool(resolved, &env.providers_cfg)
}

fn filter_quota_exhausted_migration_model(
    env: &crate::migration_providers::ResumeExecutionEnvironment,
    resolved: &oulipoly_state::ResolvedResume,
    migration_model: &mut oulipoly_config::ModelConfig,
) {
    filter_quota_exhausted_migration_candidates(
        &env.state,
        migration_model,
        &resolved.active_provider,
    );
}

fn should_filter_migration_candidates(manual_migrate: Option<&str>, attempts: usize) -> bool {
    manual_migrate.is_none() || attempts > 1
}

fn dispatch_resume_migration(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    env: &crate::migration_providers::ResumeExecutionEnvironment,
    resolved: &oulipoly_state::ResolvedResume,
    manual_migrate: Option<&str>,
    attempts: usize,
    effective_spawn_cwd: &Path,
    migration_model: &oulipoly_config::ModelConfig,
) -> Result<MigrationServiceOutput, ServiceError> {
    let mut migration_stderr = std::io::stderr();
    agent_runtime_services
        .migration_service
        .migrate(mapper::migration_service_request(
            mapper::ResumeMigrationRequestInput {
                env,
                resolved,
                manual_target: super::filter::first_attempt_manual_migrate(
                    attempts,
                    manual_migrate,
                ),
                active_exhausted: false,
                migration_model,
                effective_cwd: effective_spawn_cwd,
                stderr: &mut migration_stderr,
                _stderr_lifetime: std::marker::PhantomData,
            },
        ))
}

fn apply_migrated_segment(
    env: &crate::migration_providers::ResumeExecutionEnvironment,
    resolved: &mut oulipoly_state::ResolvedResume,
    target: &mut crate::resume_cli::ResumeExecutionTarget,
    migrated: oulipoly_runtime::migration::MigratedSegment,
) -> Result<(), i32> {
    resolved.active_provider = migrated.target_provider;
    resolved.active_session_id = migrated.target_session_id;
    *target = renderable_resume_execution_target(resolved, &env.providers_cfg)?;
    Ok(())
}

fn fail_rotation(reason: oulipoly_runtime::services::RotationFailedReason) -> Result<(), i32> {
    formatter::emit_stderr(&formatter::rotation_failed_reason(&reason));
    Err(1)
}

fn fail_migration_service(err: ServiceError) -> Result<(), i32> {
    emit_migration_service_error(migration_service_error_message(err));
    Err(1)
}

enum MigrationServiceErrorMessage {
    Dependency(String),
    Service(ServiceError),
}

fn migration_service_error_message(err: ServiceError) -> MigrationServiceErrorMessage {
    match err {
        ServiceError::Dependency { message } => MigrationServiceErrorMessage::Dependency(message),
        err => MigrationServiceErrorMessage::Service(err),
    }
}

fn emit_migration_service_error(message: MigrationServiceErrorMessage) {
    match message {
        MigrationServiceErrorMessage::Dependency(message) => {
            formatter::emit_migration_dependency_failure(&message);
        }
        MigrationServiceErrorMessage::Service(err) => {
            formatter::emit_migration_service_failure(&err);
        }
    }
}

fn terminal_signal_disposition_for_result(
    state_db: &oulipoly_state::StateDb,
    invocation_id: &str,
    provider_name: &str,
    provider_session_id: &str,
    result: &executor::ExecutionResult,
    zero_turn_action: crate::zero_turn_orchestration::ZeroTurnAction,
) -> TerminalSignalDisposition {
    let context_ids = mapper::terminal_signal_context_ids(invocation_id, Some(provider_session_id));
    let mut terminal_signal_stderr = std::io::stderr();
    let mut terminal_signal_ctx = mapper::terminal_signal_context_for_attempt(
        &context_ids,
        provider_name,
        state_db,
        &mut terminal_signal_stderr,
    );
    let terminal_signal = resume_terminal_signal_for_outcome(&result.terminal_signal);

    resume_terminal_signal_disposition(zero_turn_action, &terminal_signal, &mut terminal_signal_ctx)
}

fn resume_terminal_signal_disposition(
    zero_turn_action: crate::zero_turn_orchestration::ZeroTurnAction,
    terminal_signal: &Option<oulipoly_runtime::executor::TerminalSignal>,
    terminal_signal_ctx: &mut crate::terminal_outcome_adapter::TerminalSignalContext<
        '_,
        std::io::Stderr,
    >,
) -> TerminalSignalDisposition {
    match confirmed_maybe_quota_signal(zero_turn_action, terminal_signal) {
        Some(signal) => {
            let _ = confirm_maybe_quota_exhausted(signal, terminal_signal_ctx);
            TerminalSignalDisposition::MaybeQuotaVerify
        }
        None => apply_resume_terminal_signal_outcome(terminal_signal, terminal_signal_ctx),
    }
}

fn confirmed_maybe_quota_signal(
    zero_turn_action: crate::zero_turn_orchestration::ZeroTurnAction,
    terminal_signal: &Option<oulipoly_runtime::executor::TerminalSignal>,
) -> Option<&oulipoly_runtime::executor::TerminalSignal> {
    confirmed_zero_turn_maybe_quota(zero_turn_action, terminal_signal)
        .then_some(terminal_signal.as_ref())
        .flatten()
}

fn apply_resume_terminal_signal_outcome(
    terminal_signal: &Option<oulipoly_runtime::executor::TerminalSignal>,
    terminal_signal_ctx: &mut crate::terminal_outcome_adapter::TerminalSignalContext<
        '_,
        std::io::Stderr,
    >,
) -> TerminalSignalDisposition {
    crate::terminal_outcome_adapter::apply_terminal_signal_outcome(
        terminal_signal,
        terminal_signal_ctx,
    )
}
