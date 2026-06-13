//! ## Declared roles
//!
//! `orchestration`, `validator`, `accessor`, `mapper`, `predicate`, `formatter`

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use oulipoly_config::{ModelConfig, ProviderConfig};
use oulipoly_runtime::balancer;
use oulipoly_runtime::executor;
use oulipoly_runtime::services::{
    InvocationLifecycleServicePort, MigrationServiceOutput, MigrationServiceRequest,
    RoutingServicePort, RoutingServiceRequest, ServiceError,
};

use super::disposition::{
    ReplTerminalControl, ReplTerminalDispositionInput, handle_terminal_signal_disposition,
};
use super::finalization::{
    CompletedReplAttemptInput, finalize_completed_repl_attempt, finalize_spawn_error,
};
use super::resolution::resolve_optional_repl_resume;
use super::{formatter, mapper, validator};
use crate::error_emit::effective_model_for_execution;
use crate::invocation::finalize::FinalizerGuard;
use crate::invocation::result_envelope::should_emit_invocation_line;
use crate::migration_providers::load_resume_execution_environment;
use crate::quota_zero_turn::{
    apply_zero_turn_classification_to_signal_fields,
    host_observed_completion_from_interactive_result, zero_turn_classify_after_completion,
    zero_turn_record_baseline,
};
use crate::resume_cli::{format_resume_error, resume_execution_target, resume_migration_pool};
use crate::spawn_cwd::{effective_resume_spawn_cwd, effective_spawn_cwd};
use crate::terminal_outcome_adapter::{
    apply_age153_terminal_signal_fixture_override_to_fields, apply_terminal_signal_outcome,
};
use crate::wiring;

pub(crate) fn run_repl(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    model_name: Option<&str>,
    resume: Option<&str>,
    manual_migrate: Option<&str>,
    working_dir: Option<&Path>,
    models_dir_override: Option<&Path>,
) -> Result<i32, String> {
    if let Some(session_id) = resume {
        crate::run::resume::validate_resume_input(session_id)?;
    }

    let mut prepared = prepare_repl_execution(
        agent_runtime_services,
        model_name,
        resume,
        models_dir_override,
    )?;
    let in_flight = repl_in_flight();
    let ctx = repl_balance_context(&prepared.env, &in_flight);
    let parent_invocation_id = repl_parent_invocation_id(&prepared.env);
    let stderr_is_terminal = repl_stderr_is_terminal();
    let mut resume_spawn_cwd = None;
    let Some((provider_index, provider, resume_session_id)) =
        select_repl_provider(ReplProviderSelectionInput {
            agent_runtime_services,
            env: &prepared.env,
            model: &prepared.model,
            ctx: &ctx,
            resolved_resume: &mut prepared.resolved_resume,
            fallback_target: &mut prepared.fallback_target,
            resume,
            manual_migrate,
            working_dir,
            stderr_is_terminal,
            resume_spawn_cwd: &mut resume_spawn_cwd,
        })?
    else {
        return Ok(1);
    };
    validator::validate_provider_repl_capability(&provider)?;

    let mut attempt = start_selected_repl_invocation(StartSelectedReplInvocationInput {
        agent_runtime_services,
        env: &prepared.env,
        resolved_resume: prepared.resolved_resume.as_ref(),
        resume,
        model: &prepared.model,
        provider: &provider,
        provider_index,
        parent_invocation_id,
    })?;
    let invocation_env = serialize_repl_invocation_env(&attempt.invocation)?;

    bind_repl_resume_session(
        &prepared.env,
        attempt.invocation_row_id,
        resume,
        manual_migrate,
        resume_session_id.as_deref(),
    )?;

    emit_repl_invocation_line_if_needed(stderr_is_terminal, &attempt.invocation);

    execute_and_finalize_repl_attempt(ReplExecutionInput {
        agent_runtime_services,
        env: &prepared.env,
        invocation: &attempt.invocation,
        invocation_row_id: attempt.invocation_row_id,
        guard: &mut attempt.guard,
        provider: &provider,
        model: &prepared.model,
        resume,
        manual_migrate,
        working_dir,
        resume_spawn_cwd: resume_spawn_cwd.as_deref(),
        resume_session_id: resume_session_id.as_deref(),
        invocation_env: &invocation_env,
    })
}

fn prepare_repl_execution(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    model_name: Option<&str>,
    resume: Option<&str>,
    models_dir_override: Option<&Path>,
) -> Result<PreparedReplExecution, String> {
    prepare_repl_model_and_resume(
        agent_runtime_services,
        model_name,
        resume,
        models_dir_override,
    )
}

fn repl_in_flight() -> oulipoly_runtime::quota::InFlight {
    oulipoly_runtime::quota::InFlight::new()
}

fn repl_balance_context<'a>(
    env: &'a crate::migration_providers::ResumeExecutionEnvironment,
    in_flight: &'a oulipoly_runtime::quota::InFlight,
) -> balancer::BalanceContext<'a> {
    balancer::BalanceContext {
        providers_cfg: &env.providers_cfg,
        sessions_cfg: &env.sessions_cfg,
        in_flight,
    }
}

fn repl_parent_invocation_id(
    env: &crate::migration_providers::ResumeExecutionEnvironment,
) -> Option<i64> {
    crate::dispatch::resolve_parent_invocation_id(&env.state)
}

fn repl_stderr_is_terminal() -> bool {
    std::io::stderr().is_terminal()
}

struct StartSelectedReplInvocationInput<'a, 'state> {
    agent_runtime_services: &'a wiring::AgentRuntimeServices,
    env: &'state crate::migration_providers::ResumeExecutionEnvironment,
    resolved_resume: Option<&'a oulipoly_state::ResolvedResume>,
    resume: Option<&'a str>,
    model: &'a ModelConfig,
    provider: &'a ProviderConfig,
    provider_index: usize,
    parent_invocation_id: Option<i64>,
}

fn start_selected_repl_invocation<'state>(
    input: StartSelectedReplInvocationInput<'_, 'state>,
) -> Result<ReplInvocationAttempt<'state>, String> {
    start_repl_invocation(
        input.agent_runtime_services,
        input.env,
        input.resolved_resume,
        input.resume,
        input.model,
        input.provider,
        input.provider_index,
        input.parent_invocation_id,
    )
}

struct PreparedReplExecution {
    env: crate::migration_providers::ResumeExecutionEnvironment,
    resolved_resume: Option<oulipoly_state::ResolvedResume>,
    fallback_target: Option<crate::resume_cli::ResumeExecutionTarget>,
    model: ModelConfig,
}

fn prepare_repl_model_and_resume(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    model_name: Option<&str>,
    resume: Option<&str>,
    models_dir_override: Option<&Path>,
) -> Result<PreparedReplExecution, String> {
    let env = load_resume_execution_environment(models_dir_override)?;
    let resolved_resume =
        resolve_optional_repl_resume(agent_runtime_services, &env, resume, model_name)?;
    let fallback_target = fallback_target_for_resume(&env, resolved_resume.as_ref())?;
    let model = resolve_repl_model(&env.models, model_name, fallback_target.as_ref())?;
    Ok(PreparedReplExecution {
        env,
        resolved_resume,
        fallback_target,
        model,
    })
}

fn fallback_target_for_resume(
    env: &crate::migration_providers::ResumeExecutionEnvironment,
    resolved_resume: Option<&oulipoly_state::ResolvedResume>,
) -> Result<Option<crate::resume_cli::ResumeExecutionTarget>, String> {
    resolved_resume
        .map(|resolved| {
            resume_execution_target(resolved, &env.providers_cfg).map_err(format_resume_error)
        })
        .transpose()
}

struct ReplInvocationAttempt<'state> {
    invocation: oulipoly_state::CompositeInvocationId,
    invocation_row_id: i64,
    guard: FinalizerGuard<'state>,
}

#[allow(clippy::too_many_arguments)]
fn start_repl_invocation<'state>(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    env: &'state crate::migration_providers::ResumeExecutionEnvironment,
    resolved_resume: Option<&oulipoly_state::ResolvedResume>,
    resume: Option<&str>,
    model: &ModelConfig,
    provider: &ProviderConfig,
    provider_index: usize,
    parent_invocation_id: Option<i64>,
) -> Result<ReplInvocationAttempt<'state>, String> {
    let invocation = mapper::composite_invocation_id(&provider.name);
    let invocation_start = mapper::invocation_start(
        &invocation,
        invocation_model_name(resolved_resume, resume, model),
        &provider.name,
        provider_index,
        parent_invocation_id,
    );
    let invocation_row_id = agent_runtime_services
        .invocation_lifecycle_service
        .start_invocation(mapper::invocation_lifecycle_start_request(
            &env.state,
            &invocation_start,
        ))
        .map_err(|err| err.to_string())?
        .invocation_row_id;
    let guard = FinalizerGuard::new(&env.state, invocation_row_id);
    Ok(ReplInvocationAttempt {
        invocation,
        invocation_row_id,
        guard,
    })
}

fn serialize_repl_invocation_env(
    invocation: &oulipoly_state::CompositeInvocationId,
) -> Result<String, String> {
    serde_json::to_string(invocation).map_err(|e| format!("Failed to serialize invocation id: {e}"))
}

fn emit_repl_invocation_line_if_needed(
    stderr_is_terminal: bool,
    invocation: &oulipoly_state::CompositeInvocationId,
) {
    if should_emit_invocation_line(stderr_is_terminal) {
        formatter::emit_stderr(&invocation.stderr_line());
    }
}

struct ReplExecutionInput<'a, 'state> {
    agent_runtime_services: &'a wiring::AgentRuntimeServices,
    env: &'a crate::migration_providers::ResumeExecutionEnvironment,
    invocation: &'a oulipoly_state::CompositeInvocationId,
    invocation_row_id: i64,
    guard: &'a mut FinalizerGuard<'state>,
    provider: &'a ProviderConfig,
    model: &'a ModelConfig,
    resume: Option<&'a str>,
    manual_migrate: Option<&'a str>,
    working_dir: Option<&'a Path>,
    resume_spawn_cwd: Option<&'a Path>,
    resume_session_id: Option<&'a str>,
    invocation_env: &'a str,
}

fn execute_and_finalize_repl_attempt(input: ReplExecutionInput<'_, '_>) -> Result<i32, String> {
    let interactive_effective_cwd =
        repl_interactive_effective_cwd(input.resume_spawn_cwd, input.working_dir)?;

    let resume_payload = repl_resume_payload(input.provider, input.resume_session_id);
    let zero_turn_baseline = zero_turn_record_baseline(
        &input.env.state,
        &input.env.sessions_cfg,
        &input.provider.name,
        input.resume_session_id,
    );

    match executor::cli::execute_interactive_with_result_and_model_identity(
        input.provider,
        repl_execution_cwd(input.resume_spawn_cwd, input.working_dir),
        Some(input.invocation_env),
        resume_payload,
        Some(&input.model.name),
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

fn repl_resume_payload<'a>(
    provider: &'a ProviderConfig,
    resume_session_id: Option<&'a str>,
) -> Option<executor::cli::ResumePayload<'a>> {
    resume_session_id.map(|session_id| mapper::resume_payload(provider, session_id))
}

fn repl_execution_cwd<'a>(
    resume_spawn_cwd: Option<&'a Path>,
    working_dir: Option<&'a Path>,
) -> Option<&'a Path> {
    resume_spawn_cwd.or(working_dir)
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

fn repl_interactive_effective_cwd(
    resume_spawn_cwd: Option<&Path>,
    working_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    resume_spawn_cwd
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(|| effective_spawn_cwd(working_dir))
}

fn clear_repl_session_capture_for_unpinned(
    env: &crate::migration_providers::ResumeExecutionEnvironment,
    invocation_row_id: i64,
    resume: Option<&str>,
) -> Result<(), String> {
    if resume.is_none() {
        env.state
            .update_session_capture(invocation_row_id, None, "none")?;
    }
    Ok(())
}

fn resolve_repl_model(
    models: &std::collections::HashMap<String, ModelConfig>,
    model_name: Option<&str>,
    fallback_target: Option<&crate::resume_cli::ResumeExecutionTarget>,
) -> Result<ModelConfig, String> {
    fallback_model(fallback_target)
        .map(Ok)
        .unwrap_or_else(|| direct_or_default_repl_model(models, model_name, fallback_target))
}

fn fallback_model(
    fallback_target: Option<&crate::resume_cli::ResumeExecutionTarget>,
) -> Option<ModelConfig> {
    fallback_target.and_then(|target| target.model.clone())
}

fn direct_or_default_repl_model(
    models: &std::collections::HashMap<String, ModelConfig>,
    model_name: Option<&str>,
    fallback_target: Option<&crate::resume_cli::ResumeExecutionTarget>,
) -> Result<ModelConfig, String> {
    match repl_model_source(model_name, fallback_target)? {
        ReplModelSource::ProviderDefault => Ok(mapper::provider_default_model()),
        ReplModelSource::Named(model_name) => lookup_repl_model(models, model_name),
    }
}

enum ReplModelSource<'a> {
    ProviderDefault,
    Named(&'a str),
}

fn repl_model_source<'a>(
    model_name: Option<&'a str>,
    fallback_target: Option<&crate::resume_cli::ResumeExecutionTarget>,
) -> Result<ReplModelSource<'a>, String> {
    if fallback_target.is_some() {
        return Ok(ReplModelSource::ProviderDefault);
    }
    Ok(ReplModelSource::Named(required_repl_model_name(
        model_name,
    )?))
}

fn required_repl_model_name(model_name: Option<&str>) -> Result<&str, String> {
    model_name.ok_or_else(|| "model is required unless --resume is present".to_string())
}

fn lookup_repl_model(
    models: &std::collections::HashMap<String, ModelConfig>,
    model_name: &str,
) -> Result<ModelConfig, String> {
    models
        .get(model_name)
        .cloned()
        .ok_or_else(|| unknown_repl_model_message(model_name))
}

fn unknown_repl_model_message(model_name: &str) -> String {
    format!("Unknown model: {model_name}")
}

struct ReplProviderSelectionInput<'a, 'ctx> {
    agent_runtime_services: &'a wiring::AgentRuntimeServices,
    env: &'a crate::migration_providers::ResumeExecutionEnvironment,
    model: &'a ModelConfig,
    ctx: &'a balancer::BalanceContext<'ctx>,
    resolved_resume: &'a mut Option<oulipoly_state::ResolvedResume>,
    fallback_target: &'a mut Option<crate::resume_cli::ResumeExecutionTarget>,
    resume: Option<&'a str>,
    manual_migrate: Option<&'a str>,
    working_dir: Option<&'a Path>,
    stderr_is_terminal: bool,
    resume_spawn_cwd: &'a mut Option<PathBuf>,
}

fn select_repl_provider(
    mut input: ReplProviderSelectionInput<'_, '_>,
) -> Result<Option<(usize, ProviderConfig, Option<String>)>, String> {
    if input.resolved_resume.is_some() {
        let mut resolved = input
            .resolved_resume
            .take()
            .expect("resolved resume checked above");
        let selected = select_repl_resume_provider(&mut input, &mut resolved)?;
        *input.resolved_resume = Some(resolved);
        Ok(selected)
    } else {
        select_repl_direct_provider(
            input.agent_runtime_services,
            input.model,
            input.env,
            input.ctx,
        )
        .map(Some)
    }
}

fn select_repl_resume_provider(
    input: &mut ReplProviderSelectionInput<'_, '_>,
    resolved: &mut oulipoly_state::ResolvedResume,
) -> Result<Option<(usize, ProviderConfig, Option<String>)>, String> {
    let selected_provider = &resolved.active_provider;
    emit_selected_repl_resume_provider(input.stderr_is_terminal, selected_provider);
    if !validate_repl_resume_target(input.fallback_target.as_ref(), selected_provider) {
        return Ok(None);
    }

    if !migrate_repl_resume_provider(input, resolved)? {
        return Ok(None);
    }
    selected_repl_resume_provider_tuple(input.fallback_target.as_ref(), resolved)
}

fn emit_selected_repl_resume_provider(stderr_is_terminal: bool, selected_provider: &str) {
    if crate::dispatch::should_emit_resume_short_line(stderr_is_terminal) {
        formatter::emit_stderr(&format!("[resume] -> {selected_provider}"));
    }
}

fn validate_repl_resume_target(
    fallback_target: Option<&crate::resume_cli::ResumeExecutionTarget>,
    selected_provider: &str,
) -> bool {
    if repl_resume_target_missing_resume(fallback_target) {
        emit_repl_missing_resume_block(selected_provider);
        return false;
    }
    true
}

fn repl_resume_target_missing_resume(
    fallback_target: Option<&crate::resume_cli::ResumeExecutionTarget>,
) -> bool {
    fallback_target
        .as_ref()
        .is_some_and(|target| target.provider.resume.is_none())
}

fn selected_repl_resume_provider_tuple(
    fallback_target: Option<&crate::resume_cli::ResumeExecutionTarget>,
    resolved: &oulipoly_state::ResolvedResume,
) -> Result<Option<(usize, ProviderConfig, Option<String>)>, String> {
    let target = fallback_target.expect("resume target must be resolved before spawn");
    let provider = target.provider.clone();
    if provider_missing_repl_resume_block(&provider) {
        emit_repl_missing_resume_block(&provider.name);
        return Ok(None);
    }
    Ok(Some(selected_repl_provider_tuple(
        target.provider_index,
        provider,
        resolved,
    )))
}

fn provider_missing_repl_resume_block(provider: &ProviderConfig) -> bool {
    provider.resume.is_none()
}

fn emit_repl_missing_resume_block(provider_name: &str) {
    formatter::emit_stderr(&format!(
        "provider {provider_name} has no [providers.resume] block; cannot resume"
    ));
}

fn selected_repl_provider_tuple(
    provider_index: usize,
    provider: ProviderConfig,
    resolved: &oulipoly_state::ResolvedResume,
) -> (usize, ProviderConfig, Option<String>) {
    (
        provider_index,
        provider,
        Some(resolved.active_session_id.clone()),
    )
}

fn migrate_repl_resume_provider(
    input: &mut ReplProviderSelectionInput<'_, '_>,
    resolved: &mut oulipoly_state::ResolvedResume,
) -> Result<bool, String> {
    let migration = prepare_repl_migration(input, resolved)?;
    *input.resume_spawn_cwd = Some(migration.effective_spawn_cwd.clone());
    match dispatch_repl_migration(
        input,
        resolved,
        &migration.model,
        &migration.effective_spawn_cwd,
    ) {
        Ok(MigrationServiceOutput::Migrated { segment: migrated })
        | Ok(MigrationServiceOutput::AutoRotated {
            segment: migrated, ..
        }) => apply_repl_migrated_segment(input, resolved, migrated)?,
        Ok(MigrationServiceOutput::Stay) => {}
        Ok(MigrationServiceOutput::RotationFailed { reason }) => {
            return Ok(render_repl_rotation_failed(&reason));
        }
        Err(ServiceError::Dependency { message }) => {
            return Ok(render_repl_migration_dependency_failure(&message));
        }
        Err(err) => return Err(format!("migration service failed: {err}")),
    }
    Ok(true)
}

struct PreparedReplMigration {
    model: ModelConfig,
    effective_spawn_cwd: PathBuf,
}

fn prepare_repl_migration(
    input: &ReplProviderSelectionInput<'_, '_>,
    resolved: &oulipoly_state::ResolvedResume,
) -> Result<PreparedReplMigration, String> {
    Ok(PreparedReplMigration {
        model: repl_migration_model(input, resolved),
        effective_spawn_cwd: repl_migration_effective_cwd(input)?,
    })
}

fn repl_migration_model(
    input: &ReplProviderSelectionInput<'_, '_>,
    resolved: &oulipoly_state::ResolvedResume,
) -> ModelConfig {
    resume_migration_pool(resolved, &input.env.providers_cfg)
}

fn repl_migration_effective_cwd(
    input: &ReplProviderSelectionInput<'_, '_>,
) -> Result<PathBuf, String> {
    effective_resume_spawn_cwd(
        &input.env.state,
        &input.env.models,
        &input.env.providers_cfg,
        &input.env.sessions_cfg,
        input
            .resume
            .expect("resume input must exist for resolved resume"),
        input.working_dir,
    )
}

fn dispatch_repl_migration(
    input: &mut ReplProviderSelectionInput<'_, '_>,
    resolved: &oulipoly_state::ResolvedResume,
    migration_model: &ModelConfig,
    effective_spawn_cwd: &Path,
) -> Result<MigrationServiceOutput, ServiceError> {
    let mut migration_stderr = std::io::stderr();
    input
        .agent_runtime_services
        .migration_service
        .migrate(MigrationServiceRequest {
            state: &input.env.state,
            sessions_cfg: &input.env.sessions_cfg,
            resolved,
            manual_target: input.manual_migrate,
            active_exhausted: false,
            migration_model,
            effective_cwd: effective_spawn_cwd,
            stderr: &mut migration_stderr,
        })
}

fn apply_repl_migrated_segment(
    input: &mut ReplProviderSelectionInput<'_, '_>,
    resolved: &mut oulipoly_state::ResolvedResume,
    migrated: oulipoly_runtime::migration::MigratedSegment,
) -> Result<(), String> {
    resolved.active_provider = migrated.target_provider;
    resolved.active_session_id = migrated.target_session_id;
    *input.fallback_target = Some(
        resume_execution_target(resolved, &input.env.providers_cfg).map_err(format_resume_error)?,
    );
    Ok(())
}

fn render_repl_rotation_failed(reason: &oulipoly_runtime::services::RotationFailedReason) -> bool {
    formatter::emit_stderr(&formatter::rotation_failed_reason(reason));
    false
}

fn render_repl_migration_dependency_failure(message: &str) -> bool {
    formatter::emit_stderr(&format!("migration failed: {message}"));
    false
}

fn select_repl_direct_provider(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    model: &ModelConfig,
    env: &crate::migration_providers::ResumeExecutionEnvironment,
    ctx: &balancer::BalanceContext<'_>,
) -> Result<(usize, ProviderConfig, Option<String>), String> {
    let provider_index = agent_runtime_services
        .routing_service
        .select_route(RoutingServiceRequest {
            model,
            state: &env.state,
            ctx: Some(ctx),
        })
        .map_err(|err| err.to_string())?
        .provider_index;
    let (provider, _) = effective_model_for_execution(model, provider_index, &env.providers_cfg)?;
    Ok((provider_index, provider, None))
}

fn invocation_model_name(
    resolved_resume: Option<&oulipoly_state::ResolvedResume>,
    resume: Option<&str>,
    model: &ModelConfig,
) -> String {
    resolved_resume
        .and_then(|resolved| resolved.model_name.clone())
        .unwrap_or_else(|| {
            if resume.is_some() {
                "<unknown>".to_string()
            } else {
                model.name.clone()
            }
        })
}

fn bind_repl_resume_session(
    env: &crate::migration_providers::ResumeExecutionEnvironment,
    invocation_row_id: i64,
    resume: Option<&str>,
    manual_migrate: Option<&str>,
    resume_session_id: Option<&str>,
) -> Result<(), String> {
    let Some(active_session_id) = resume_session_id else {
        return Ok(());
    };
    env.state.bind_invocation_provider_session_start(
        invocation_row_id,
        &mapper::resumed_provider_session_binding(active_session_id, resume.map(str::to_string)),
    )?;
    if should_record_repl_legacy_resume_input(resume, manual_migrate) {
        record_repl_legacy_resume_input(env, invocation_row_id, resume)?;
    }
    Ok(())
}

fn should_record_repl_legacy_resume_input(
    resume: Option<&str>,
    manual_migrate: Option<&str>,
) -> bool {
    resume.is_some() && manual_migrate.is_some()
}

fn record_repl_legacy_resume_input(
    env: &crate::migration_providers::ResumeExecutionEnvironment,
    invocation_row_id: i64,
    resume: Option<&str>,
) -> Result<(), String> {
    env.state.record_legacy_resume_input_session_id(
        invocation_row_id,
        resume.expect("resume checked before recording legacy input"),
    )
}

fn classify_repl_result(
    env: &crate::migration_providers::ResumeExecutionEnvironment,
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
    env: &crate::migration_providers::ResumeExecutionEnvironment,
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
