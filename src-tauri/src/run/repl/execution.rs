//! ## Declared roles
//!
//! `accessor`, `formatter`, `mapper`, `orchestration`, `predicate`, `validator`

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use oulipoly_config::{ModelConfig, ProviderConfig};
use oulipoly_runtime::balancer;
use oulipoly_runtime::executor;
use oulipoly_runtime::services::{
    InvocationLifecycleServicePort, RoutingServicePort, RoutingServiceRequest,
};

use super::resolution::resolve_optional_repl_resume;
use super::{formatter, mapper};
use crate::error_emit::effective_model_for_execution;
use crate::invocation::finalize::FinalizerGuard;
use crate::invocation::result_envelope::should_emit_invocation_line;
use crate::migration_providers::{ResumeExecutionEnvironment, load_resume_execution_environment};
use crate::resume_cli::{
    ResumeExecutionTarget, format_resume_error, interactive_resume_execution_target,
};
use crate::spawn_cwd::effective_spawn_cwd;
use crate::wiring;

pub(super) struct PreparedReplExecution {
    pub(super) env: ResumeExecutionEnvironment,
    pub(super) resolved_resume: Option<oulipoly_state::ResolvedResume>,
    pub(super) fallback_target: Option<ResumeExecutionTarget>,
    pub(super) model: ModelConfig,
}

pub(super) fn prepare_repl_execution(
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
    Ok(mapper::prepared_repl_execution(
        env,
        resolved_resume,
        fallback_target,
        model,
    ))
}

fn fallback_target_for_resume(
    env: &ResumeExecutionEnvironment,
    resolved_resume: Option<&oulipoly_state::ResolvedResume>,
) -> Result<Option<ResumeExecutionTarget>, String> {
    resolved_resume
        .map(|resolved| {
            interactive_resume_execution_target(resolved, &env.providers_cfg)
                .map_err(format_resume_error)
        })
        .transpose()
}

pub(super) fn repl_in_flight() -> oulipoly_runtime::quota::InFlight {
    oulipoly_runtime::quota::InFlight::new()
}

pub(super) fn repl_balance_context<'a>(
    env: &'a ResumeExecutionEnvironment,
    in_flight: &'a oulipoly_runtime::quota::InFlight,
) -> balancer::BalanceContext<'a> {
    balancer::BalanceContext {
        providers_cfg: &env.providers_cfg,
        in_flight,
    }
}

pub(super) fn repl_stderr_is_terminal() -> bool {
    std::io::stderr().is_terminal()
}

pub(super) struct StartSelectedReplInvocationInput<'a, 'state> {
    pub(super) agent_runtime_services: &'a wiring::AgentRuntimeServices,
    pub(super) env: &'state ResumeExecutionEnvironment,
    pub(super) resolved_resume: Option<&'a oulipoly_state::ResolvedResume>,
    pub(super) resume: Option<&'a str>,
    pub(super) model: &'a ModelConfig,
    pub(super) provider: &'a ProviderConfig,
    pub(super) provider_index: usize,
    pub(super) parent_invocation_id: Option<i64>,
}

pub(super) struct ReplInvocationAttempt<'state> {
    pub(super) invocation: oulipoly_state::CompositeInvocationId,
    pub(super) invocation_row_id: i64,
    pub(super) completion_registration_authority: oulipoly_state::CompletionRegistrationAuthority,
    pub(super) guard: FinalizerGuard<'state>,
}

pub(super) fn start_selected_repl_invocation<'state>(
    input: StartSelectedReplInvocationInput<'_, 'state>,
) -> Result<ReplInvocationAttempt<'state>, String> {
    let invocation = mapper::composite_invocation_id(&input.provider.name);
    let invocation_start = mapper::invocation_start(
        &invocation,
        invocation_model_name(input.resolved_resume, input.resume, input.model),
        &input.provider.name,
        input.provider_index,
        input.parent_invocation_id,
    );
    let invocation_start = input
        .agent_runtime_services
        .invocation_lifecycle_service
        .start_invocation(mapper::invocation_lifecycle_start_request(
            &input.env.state,
            &invocation_start,
        ))
        .map_err(|err| err.to_string())?;
    let invocation_row_id = invocation_start.invocation_row_id;
    let guard = FinalizerGuard::new(&input.env.state, invocation_row_id);
    Ok(mapper::repl_invocation_attempt(
        invocation,
        invocation_row_id,
        invocation_start.completion_registration_authority,
        guard,
    ))
}

pub(super) fn serialize_repl_invocation_env(
    invocation: &oulipoly_state::CompositeInvocationId,
    authority: &oulipoly_state::CompletionRegistrationAuthority,
) -> Result<String, String> {
    authority.invocation_launch_environment(invocation)
}

pub(super) fn emit_repl_invocation_line_if_needed(
    stderr_is_terminal: bool,
    invocation: &oulipoly_state::CompositeInvocationId,
) {
    if should_emit_invocation_line(stderr_is_terminal) {
        formatter::emit_stderr(&invocation.stderr_line());
    }
}

pub(super) fn repl_resume_payload<'a>(
    provider: &'a ProviderConfig,
    resume_session_id: Option<&'a str>,
) -> Option<executor::cli::ResumePayload<'a>> {
    resume_session_id.map(|session_id| mapper::resume_payload(provider, session_id))
}

pub(super) fn repl_execution_cwd<'a>(
    resume_spawn_cwd: Option<&'a Path>,
    working_dir: Option<&'a Path>,
) -> Option<&'a Path> {
    resume_spawn_cwd.or(working_dir)
}

pub(super) fn repl_interactive_effective_cwd(
    resume_spawn_cwd: Option<&Path>,
    working_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    resume_spawn_cwd
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(|| effective_spawn_cwd(working_dir))
}

pub(super) fn clear_repl_session_capture_for_unpinned(
    env: &ResumeExecutionEnvironment,
    invocation_row_id: i64,
    resume: Option<&str>,
) -> Result<(), String> {
    if repl_session_capture_is_unpinned(resume) {
        env.state
            .update_session_capture(invocation_row_id, None, "none")?;
    }
    Ok(())
}

fn repl_session_capture_is_unpinned(resume: Option<&str>) -> bool {
    resume.is_none()
}

fn resolve_repl_model(
    models: &std::collections::HashMap<String, ModelConfig>,
    model_name: Option<&str>,
    fallback_target: Option<&ResumeExecutionTarget>,
) -> Result<ModelConfig, String> {
    fallback_model(fallback_target)
        .map(Ok)
        .unwrap_or_else(|| direct_or_default_repl_model(models, model_name, fallback_target))
}

fn fallback_model(fallback_target: Option<&ResumeExecutionTarget>) -> Option<ModelConfig> {
    fallback_target.and_then(|target| target.model.clone())
}

fn direct_or_default_repl_model(
    models: &std::collections::HashMap<String, ModelConfig>,
    model_name: Option<&str>,
    fallback_target: Option<&ResumeExecutionTarget>,
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
    fallback_target: Option<&ResumeExecutionTarget>,
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
    cloned_repl_model(models, model_name).ok_or_else(|| formatter::unknown_model_error(model_name))
}

fn cloned_repl_model(
    models: &std::collections::HashMap<String, ModelConfig>,
    model_name: &str,
) -> Option<ModelConfig> {
    models.get(model_name).cloned()
}

pub(super) fn select_repl_direct_provider(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    model: &ModelConfig,
    env: &ResumeExecutionEnvironment,
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
    Ok(mapper::selected_repl_provider_tuple(
        provider_index,
        provider,
        None,
    ))
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

pub(super) fn bind_repl_resume_session(
    env: &ResumeExecutionEnvironment,
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
    env: &ResumeExecutionEnvironment,
    invocation_row_id: i64,
    resume: Option<&str>,
) -> Result<(), String> {
    env.state.record_legacy_resume_input_session_id(
        invocation_row_id,
        resume.expect("resume checked before recording legacy input"),
    )
}
