//! ## Declared roles
//!
//! `accessor`, `orchestration`

use std::path::{Path, PathBuf};

use oulipoly_config::{ModelConfig, ProviderConfig};

use super::execution;
use super::migration;
use super::terminal::{ReplExecutionInput, execute_and_finalize_repl_attempt};
use super::validator;
use crate::migration_providers::ResumeExecutionEnvironment;
use crate::resume_cli::ResumeExecutionTarget;
use crate::wiring;

type StartedReplInvocation<'state> = execution::ReplInvocationAttempt<'state>;

pub(crate) fn run_repl(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    model_name: Option<&str>,
    resume: Option<&str>,
    manual_migrate: Option<&str>,
    working_dir: Option<&Path>,
    models_dir_override: Option<&Path>,
) -> Result<i32, String> {
    crate::wake_coordinator::start_wake_reclaim_maintenance_driver();

    if let Some(session_id) = resume {
        crate::run::resume::validate_resume_input(session_id)?;
    }

    let mut prepared = execution::prepare_repl_execution(
        agent_runtime_services,
        model_name,
        resume,
        models_dir_override,
    )?;
    let stderr_is_terminal = execution::repl_stderr_is_terminal();
    let mut resume_spawn_cwd: Option<PathBuf> = None;
    let Some((provider_index, provider, resume_session_id)) = select_prepared_repl_provider(
        agent_runtime_services,
        &prepared.env,
        &prepared.model,
        &mut prepared.resolved_resume,
        &mut prepared.fallback_target,
        resume,
        manual_migrate,
        working_dir,
        stderr_is_terminal,
        &mut resume_spawn_cwd,
    )?
    else {
        return Ok(1);
    };
    validator::validate_provider_repl_capability(&provider)?;

    let mut attempt = start_repl_invocation(
        agent_runtime_services,
        &prepared.env,
        prepared.resolved_resume.as_ref(),
        resume,
        &prepared.model,
        &provider,
        provider_index,
    )?;
    let invocation_env = initialize_repl_invocation(
        &prepared.env,
        &attempt,
        resume,
        manual_migrate,
        resume_session_id.as_deref(),
    )?;
    emit_repl_invocation(stderr_is_terminal, &attempt.invocation);
    let _live_pty_retry_driver = crate::wake_coordinator::start_live_pty_retry_driver_for_owner();

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

#[allow(clippy::too_many_arguments)]
fn select_prepared_repl_provider(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    env: &ResumeExecutionEnvironment,
    model: &ModelConfig,
    resolved_resume: &mut Option<oulipoly_state::ResolvedResume>,
    fallback_target: &mut Option<ResumeExecutionTarget>,
    resume: Option<&str>,
    manual_migrate: Option<&str>,
    working_dir: Option<&Path>,
    stderr_is_terminal: bool,
    resume_spawn_cwd: &mut Option<PathBuf>,
) -> Result<Option<(usize, ProviderConfig, Option<String>)>, String> {
    let in_flight = execution::repl_in_flight();
    let ctx = execution::repl_balance_context(env, &in_flight);
    migration::select_repl_provider(migration::ReplProviderSelectionInput {
        agent_runtime_services,
        env,
        model,
        ctx: &ctx,
        resolved_resume,
        fallback_target,
        resume,
        manual_migrate,
        working_dir,
        stderr_is_terminal,
        resume_spawn_cwd,
    })
}

#[allow(clippy::too_many_arguments)]
fn start_repl_invocation<'state>(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    env: &'state ResumeExecutionEnvironment,
    resolved_resume: Option<&oulipoly_state::ResolvedResume>,
    resume: Option<&str>,
    model: &ModelConfig,
    provider: &ProviderConfig,
    provider_index: usize,
) -> Result<StartedReplInvocation<'state>, String> {
    execution::start_selected_repl_invocation(execution::StartSelectedReplInvocationInput {
        agent_runtime_services,
        env,
        resolved_resume,
        resume,
        model,
        provider,
        provider_index,
        parent_invocation_id: crate::dispatch::resolve_parent_invocation_id(&env.state),
    })
}

fn initialize_repl_invocation(
    env: &ResumeExecutionEnvironment,
    attempt: &StartedReplInvocation<'_>,
    resume: Option<&str>,
    manual_migrate: Option<&str>,
    resume_session_id: Option<&str>,
) -> Result<String, String> {
    let invocation_env = execution::serialize_repl_invocation_env(&attempt.invocation)?;
    execution::bind_repl_resume_session(
        env,
        attempt.invocation_row_id,
        resume,
        manual_migrate,
        resume_session_id,
    )?;
    Ok(invocation_env)
}

fn emit_repl_invocation(
    stderr_is_terminal: bool,
    invocation: &oulipoly_state::CompositeInvocationId,
) {
    execution::emit_repl_invocation_line_if_needed(stderr_is_terminal, invocation);
}
