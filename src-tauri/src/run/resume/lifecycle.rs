//! ## Declared roles
//!
//! `formatter`, `mapper`, `orchestration`, `predicate`

use oulipoly_runtime::services::InvocationLifecycleServicePort;

use super::finalization::{
    CompletedAttemptControl, CompletedAttemptInput, finalize_completed_attempt,
};
use super::orchestration::{ResumeAttemptInput, ResumeAttemptLoopControl};
use super::{formatter, mapper, wake};
use crate::invocation::finalize::FinalizerGuard;
use crate::quota_zero_turn::zero_turn_record_baseline;

pub(super) struct ResumeInvocationAttempt<'state> {
    pub(super) invocation: oulipoly_state::CompositeInvocationId,
    pub(super) invocation_row_id: i64,
    pub(super) guard: FinalizerGuard<'state>,
}

pub(super) struct BoundResumeAttempt<'state> {
    pub(super) attempt: ResumeInvocationAttempt<'state>,
    pub(super) provider_session_id: String,
    pub(super) zero_turn_baseline: crate::zero_turn_orchestration::ZeroTurnBaseline,
    pub(super) invocation_env: String,
}

pub(super) fn setup_bound_resume_attempt<'state>(
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
    Ok(mapper::bound_resume_attempt(
        attempt,
        provider_session_id,
        zero_turn_baseline,
        invocation_env,
    ))
}

fn resume_invocation_env(
    invocation: &oulipoly_state::CompositeInvocationId,
) -> Result<String, String> {
    serde_json::to_string(invocation).map_err(|e| format!("Failed to serialize invocation id: {e}"))
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
    Ok(mapper::resume_invocation_attempt(
        invocation,
        invocation_row_id,
        guard,
    ))
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
        input
            .env
            .state
            .record_legacy_resume_input_session_id(invocation_row_id, input.session_id)?;
    }
    Ok(())
}

fn should_record_legacy_resume_input(manual_migrate: Option<&str>) -> bool {
    manual_migrate.is_some()
}

pub(super) fn finalize_resume_spawn_error(
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
    wake::mark_resume_attempt_idle(
        &input.resolved.active_session_id,
        &attempt.invocation.id,
        Some(1),
    )
}

pub(super) fn record_resume_acceptance_if_present(
    input: &ResumeAttemptInput<'_>,
    invocation_row_id: i64,
    result: &oulipoly_runtime::executor::ExecutionResult,
) -> Result<(), String> {
    let Some(acceptance) = result.resume_acceptance.as_ref() else {
        return Ok(());
    };
    input
        .agent_runtime_services
        .resume_service
        .record_acceptance(mapper::resume_acceptance_request(
            &input.env.state,
            invocation_row_id,
            acceptance,
        ))
        .map_err(formatter::resume_acceptance_service_failure)?;
    Ok(())
}

pub(super) fn finalize_completed_attempt_for_resume(
    input: &ResumeAttemptInput<'_>,
    attempt: &mut ResumeInvocationAttempt<'_>,
    provider: &oulipoly_config::ProviderConfig,
    provider_session_id: &str,
    result: &oulipoly_runtime::executor::ExecutionResult,
    recovered_generic_nonzero: bool,
    terminal_completion_confirmed: bool,
) -> Result<ResumeAttemptLoopControl, String> {
    match finalize_completed_attempt(CompletedAttemptInput {
        agent_runtime_services: input.agent_runtime_services,
        env: input.env,
        invocation: &attempt.invocation,
        invocation_row_id: attempt.invocation_row_id,
        guard: &mut attempt.guard,
        provider_name: &provider.name,
        provider_session_id,
        model: input.resolved.model.as_ref(),
        result,
        working_dir: input.working_dir,
        manual_migrate: input.manual_migrate,
        session_id: input.session_id,
        active_session_id: &input.resolved.active_session_id,
        effective_spawn_cwd: input.effective_spawn_cwd,
        attempts: input.attempts,
        max_attempts: input.max_attempts,
        recovered_generic_nonzero,
        terminal_completion_confirmed,
    })? {
        CompletedAttemptControl::Continue => {
            finalize_retrying_resume(input, attempt, provider_session_id, result)
        }
        CompletedAttemptControl::Return(exit_code) if exit_code == 0 => finalize_successful_resume(
            input,
            attempt,
            provider_session_id,
            exit_code,
            result.exit_code,
        ),
        CompletedAttemptControl::Return(exit_code) => {
            finalize_failed_resume(input, attempt, provider_session_id, result, exit_code)
        }
    }
}

fn finalize_retrying_resume(
    input: &ResumeAttemptInput<'_>,
    attempt: &ResumeInvocationAttempt<'_>,
    provider_session_id: &str,
    result: &oulipoly_runtime::executor::ExecutionResult,
) -> Result<ResumeAttemptLoopControl, String> {
    wake::record_failed_mailbox_delivery_attempt(
        input,
        &wake::failed_delivery_error(result, "quota_exhausted"),
    )?;
    wake::mark_resume_attempt_idle(
        provider_session_id,
        &attempt.invocation.id,
        Some(result.exit_code),
    )?;
    Ok(ResumeAttemptLoopControl::Continue(result.exit_code))
}

fn finalize_successful_resume(
    input: &ResumeAttemptInput<'_>,
    attempt: &ResumeInvocationAttempt<'_>,
    provider_session_id: &str,
    exit_code: i32,
    physical_exit_code: i32,
) -> Result<ResumeAttemptLoopControl, String> {
    wake::complete_successful_mailbox_delivery(
        input,
        provider_session_id,
        &attempt.invocation.id,
        physical_exit_code,
    )?;
    Ok(ResumeAttemptLoopControl::Return(exit_code))
}

fn finalize_failed_resume(
    input: &ResumeAttemptInput<'_>,
    attempt: &ResumeInvocationAttempt<'_>,
    provider_session_id: &str,
    result: &oulipoly_runtime::executor::ExecutionResult,
    exit_code: i32,
) -> Result<ResumeAttemptLoopControl, String> {
    wake::record_failed_mailbox_delivery_attempt(
        input,
        &wake::failed_delivery_error(result, "resume_failed"),
    )?;
    wake::mark_resume_attempt_idle(provider_session_id, &attempt.invocation.id, Some(exit_code))?;
    Ok(ResumeAttemptLoopControl::Return(exit_code))
}
