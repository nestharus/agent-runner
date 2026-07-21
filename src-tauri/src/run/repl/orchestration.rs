//! ## Declared roles
//!
//! `orchestration`

use std::path::{Path, PathBuf};

use super::execution::{
    StartSelectedReplInvocationInput, bind_repl_resume_session,
    emit_repl_invocation_line_if_needed, prepare_repl_execution, repl_balance_context,
    repl_in_flight, repl_parent_invocation_id, repl_stderr_is_terminal,
    serialize_repl_invocation_env, start_selected_repl_invocation,
};
use super::migration::{ReplProviderSelectionInput, select_repl_provider};
use super::terminal::{ReplExecutionInput, execute_and_finalize_repl_attempt};
use super::validator;
use crate::wiring;

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
    let mut resume_spawn_cwd: Option<PathBuf> = None;
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
