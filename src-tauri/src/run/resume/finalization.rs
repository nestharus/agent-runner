//! ## Declared roles
//!
//! `orchestration`, `accessor`, `mapper`, `predicate`, `formatter`

use std::io::Write as _;
use std::path::Path;

use oulipoly_runtime::services::InvocationLifecycleServicePort;
use oulipoly_state::CompositeInvocationId;

use super::{formatter, mapper};
use crate::invocation::finalize::FinalizerGuard;
use crate::migration_providers::ResumeExecutionEnvironment;
use crate::quota_zero_turn::resume_result_error_category;
use crate::session_ingest_cli::{
    ResumeIngestMode, SessionIngestRequest, ingest_and_emit_session_id_resume_aware,
};
use crate::wiring;

pub(super) enum CompletedAttemptControl {
    Continue,
    Return(i32),
}

pub(super) struct CompletedAttemptInput<'a, 'state> {
    pub(super) agent_runtime_services: &'a wiring::AgentRuntimeServices,
    pub(super) env: &'a ResumeExecutionEnvironment,
    pub(super) invocation: &'a CompositeInvocationId,
    pub(super) invocation_row_id: i64,
    pub(super) guard: &'a mut FinalizerGuard<'state>,
    pub(super) provider_name: &'a str,
    pub(super) result: &'a oulipoly_runtime::executor::ExecutionResult,
    pub(super) working_dir: Option<&'a Path>,
    pub(super) manual_migrate: Option<&'a str>,
    pub(super) session_id: &'a str,
    pub(super) active_session_id: &'a str,
    pub(super) effective_spawn_cwd: &'a Path,
    pub(super) attempts: usize,
    pub(super) max_attempts: usize,
}

pub(super) fn finalize_completed_attempt(
    mut input: CompletedAttemptInput<'_, '_>,
) -> Result<CompletedAttemptControl, String> {
    let success = super::predicate::completed_attempt_success(input.result);
    let error_category = completed_attempt_error_category(&input);
    let quota_exhausted =
        super::predicate::completed_attempt_quota_exhausted(error_category.as_deref());

    if let Err(err) = persist_returned_artifacts(&input) {
        return Ok(handle_returned_artifacts_persist_failure(&mut input, err));
    }

    finalize_regular_completed_attempt(&mut input, success, error_category.as_deref())?;

    if success {
        return Ok(handle_completed_success(&input));
    }

    if quota_exhausted {
        return Ok(handle_quota_exhausted(&input));
    }

    Ok(handle_completed_failure(&input, error_category.as_deref()))
}

fn completed_attempt_error_category(input: &CompletedAttemptInput<'_, '_>) -> Option<String> {
    resume_result_error_category(
        input.agent_runtime_services,
        input.result,
        &input.env.models,
        input.working_dir,
    )
}

fn persist_returned_artifacts(input: &CompletedAttemptInput<'_, '_>) -> Result<(), String> {
    input
        .env
        .state
        .record_returned_artifacts(input.invocation_row_id, &input.result.returned_artifacts)
        .map_err(|err| err.to_string())
}

fn handle_returned_artifacts_persist_failure(
    input: &mut CompletedAttemptInput<'_, '_>,
    err: String,
) -> CompletedAttemptControl {
    formatter::emit_returned_artifacts_error(&err);
    input
        .agent_runtime_services
        .invocation_lifecycle_service
        .finalize_invocation(mapper::returned_artifacts_finalize_request(
            &input.env.state,
            input.invocation_row_id,
        ))
        .map(|_| ())
        .unwrap_or_else(formatter::emit_finalize_invocation_warning);
    input.guard.mark_finalized();
    CompletedAttemptControl::Return(1)
}

fn finalize_regular_completed_attempt(
    input: &mut CompletedAttemptInput<'_, '_>,
    success: bool,
    error_category: Option<&str>,
) -> Result<(), String> {
    input
        .agent_runtime_services
        .invocation_lifecycle_service
        .finalize_invocation(mapper::finalize_request(
            &input.env.state,
            input.invocation_row_id,
            success,
            input.result.exit_code,
            error_category,
            input.result.terminal_reason.as_deref(),
        ))
        .map_err(|err| err.to_string())?;
    input.guard.mark_finalized();
    Ok(())
}

fn handle_completed_success(input: &CompletedAttemptInput<'_, '_>) -> CompletedAttemptControl {
    ingest_and_emit_session_id_resume_aware(
        input.agent_runtime_services,
        SessionIngestRequest {
            state: &input.env.state,
            sessions_cfg: &input.env.sessions_cfg,
            providers_cfg: Some(&input.env.providers_cfg),
            provider_name: input.provider_name,
            invocation_row_id: input.invocation_row_id,
            invocation_uuid: &input.invocation.id,
            effective_cwd: Some(input.effective_spawn_cwd),
            mode: ResumeIngestMode::Pinned {
                resume_target: super::filter::resumed_session_target(
                    input.manual_migrate,
                    input.session_id,
                    input.active_session_id,
                ),
            },
        },
    );
    let _ = std::io::stdout().write_all(&input.result.stdout);
    CompletedAttemptControl::Return(input.result.exit_code)
}

fn handle_quota_exhausted(input: &CompletedAttemptInput<'_, '_>) -> CompletedAttemptControl {
    if super::predicate::retry_available(input.attempts, input.max_attempts) {
        formatter::emit_routing_retry(input.provider_name);
    }
    CompletedAttemptControl::Continue
}

fn handle_completed_failure(
    input: &CompletedAttemptInput<'_, '_>,
    error_category: Option<&str>,
) -> CompletedAttemptControl {
    formatter::emit_stderr(&input.result.stderr);
    if let Some(category) = error_category {
        formatter::emit_diagnostics_category(category);
    }
    CompletedAttemptControl::Return(input.result.exit_code)
}
