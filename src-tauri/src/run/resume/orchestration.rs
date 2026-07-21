//! ## Declared roles
//!
//! `orchestration`

use std::path::Path;

use super::{execution, lifecycle, terminal, wake};
use crate::migration_providers::ResumeExecutionEnvironment;
use crate::wiring;
use crate::zero_turn_orchestration::ZeroTurnConfirmationState;

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
    if let Some(exit_code) = execution::reject_invalid_resume_input(session_id) {
        return Ok(exit_code);
    }
    if let Some(exit_code) = wake::validate_auto_wake_child(session_id)? {
        return Ok(exit_code);
    }
    wake::reset_manual_resume_wake_claim(session_id)?;
    // Source guard marker: agent_runtime_services.resume_service.resolve_resume(ResumeServiceRequest)

    let mut prepared = match execution::prepare_headless_resume_execution(
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
            wake::release_current_auto_wake_claim(session_id);
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
    wake::recheck_after_failed_auto_wake(session_id, &result);
    result
}

struct ResumeLoopInput<'a> {
    agent_runtime_services: &'a wiring::AgentRuntimeServices,
    prepared: &'a mut execution::PreparedHeadlessResumeExecution,
    manual_migrate: Option<&'a str>,
    session_id: &'a str,
    working_dir: Option<&'a Path>,
}

fn run_resume_loop(input: ResumeLoopInput<'_>) -> Result<i32, String> {
    let mut attempts = 0usize;
    let mut last_exit_code = 1;
    let mut zero_turn_confirmation = ZeroTurnConfirmationState::new();

    loop {
        if terminal::resume_attempts_exhausted(attempts, input.prepared.max_attempts) {
            return Ok(terminal::resume_attempts_exhausted_exit_code(
                last_exit_code,
            ));
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
            ResumeAttemptLoopControl::Continue(exit_code) => last_exit_code = exit_code,
            ResumeAttemptLoopControl::Return(exit_code) => return Ok(exit_code),
        }
    }
}

pub(super) struct ResumeAttemptInput<'a> {
    pub(super) agent_runtime_services: &'a wiring::AgentRuntimeServices,
    pub(super) env: &'a ResumeExecutionEnvironment,
    pub(super) resolved: &'a mut oulipoly_state::ResolvedResume,
    pub(super) answer: Option<&'a str>,
    pub(super) mailbox_session_id: &'a str,
    pub(super) mailbox_delivery_seqs: &'a [i64],
    pub(super) mailbox_delivery_nonce: Option<&'a str>,
    pub(super) manual_migrate: Option<&'a str>,
    pub(super) session_id: &'a str,
    pub(super) working_dir: Option<&'a Path>,
    pub(super) attempts: usize,
    pub(super) max_attempts: usize,
    pub(super) parent_invocation_id: Option<i64>,
    pub(super) effective_spawn_cwd: &'a Path,
    pub(super) zero_turn_confirmation: &'a mut ZeroTurnConfirmationState,
}

pub(super) enum ResumeAttemptLoopControl {
    Continue(i32),
    Return(i32),
}

fn run_resume_attempt(
    mut input: ResumeAttemptInput<'_>,
) -> Result<ResumeAttemptLoopControl, String> {
    let target = match execution::prepare_resume_attempt_target(&mut input)? {
        Ok(target) => target,
        Err(exit_code) => return Ok(ResumeAttemptLoopControl::Return(exit_code)),
    };
    let provider_index = target.provider_index;
    let provider = target.provider;
    let strategy = match execution::resume_attempt_strategy_for_target(input.resolved, &provider) {
        Ok(strategy) => strategy,
        Err(exit_code) => return Ok(ResumeAttemptLoopControl::Return(exit_code)),
    };
    let mut bound_attempt =
        lifecycle::setup_bound_resume_attempt(&input, &provider, provider_index)?;

    let mut result = match execution::execute_resume_attempt_command(
        &input,
        &provider,
        provider_index,
        target.prompt_mode,
        &bound_attempt.invocation_env,
        strategy,
    ) {
        Ok(result) => result,
        Err(_spawn_err) => {
            wake::record_failed_mailbox_delivery_attempt(&input, "resume_spawn_error")?;
            lifecycle::finalize_resume_spawn_error(&input, &mut bound_attempt.attempt)?;
            return Ok(ResumeAttemptLoopControl::Return(1));
        }
    };

    terminal::handle_resume_attempt_result(&mut input, &mut bound_attempt, &provider, &mut result)
}
