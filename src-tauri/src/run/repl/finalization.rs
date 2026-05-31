//! ## Declared roles
//!
//! `orchestration`, `mapper`

use std::path::Path;

use oulipoly_config::ModelConfig;
use oulipoly_runtime::services::InvocationLifecycleServicePort;
use oulipoly_state::CompositeInvocationId;

use super::mapper;
use crate::invocation::finalize::FinalizerGuard;
use crate::migration_providers::ResumeExecutionEnvironment;
use crate::session_ingest_cli::{SessionIngestRequest, ingest_and_emit_session_id_resume_aware};
use crate::wiring;

pub(super) struct CompletedReplAttemptInput<'a, 'state> {
    pub(super) agent_runtime_services: &'a wiring::AgentRuntimeServices,
    pub(super) env: &'a ResumeExecutionEnvironment,
    pub(super) invocation: &'a CompositeInvocationId,
    pub(super) invocation_row_id: i64,
    pub(super) guard: &'a mut FinalizerGuard<'state>,
    pub(super) provider_name: &'a str,
    pub(super) model: &'a ModelConfig,
    pub(super) result: &'a oulipoly_runtime::executor::cli::InteractiveExecutionResult,
    pub(super) resume: Option<&'a str>,
    pub(super) resume_session_id: Option<&'a str>,
    pub(super) manual_migrate: Option<&'a str>,
    pub(super) interactive_effective_cwd: &'a Path,
}

pub(super) fn finalize_completed_repl_attempt(
    input: CompletedReplAttemptInput<'_, '_>,
) -> Result<i32, String> {
    input
        .agent_runtime_services
        .invocation_lifecycle_service
        .finalize_invocation(mapper::finalize_request(
            &input.env.state,
            input.invocation_row_id,
            input.result.exit_code == 0,
            input.result.exit_code,
            None,
            input.result.terminal_reason.as_deref(),
        ))
        .map_err(|err| err.to_string())?;
    input.guard.mark_finalized();
    if input.result.exit_code == 0 {
        ingest_successful_repl_session(&input);
    }
    Ok(input.result.exit_code)
}

fn ingest_successful_repl_session(input: &CompletedReplAttemptInput<'_, '_>) {
    ingest_and_emit_session_id_resume_aware(
        input.agent_runtime_services,
        SessionIngestRequest {
            state: &input.env.state,
            sessions_cfg: &input.env.sessions_cfg,
            providers_cfg: Some(&input.env.providers_cfg),
            provider_name: input.provider_name,
            external_provider: crate::session_ingest_cli::session_external_provider_identity(
                input.agent_runtime_services,
                Some(input.model),
                input.provider_name,
            ),
            invocation_row_id: input.invocation_row_id,
            invocation_uuid: &input.invocation.id,
            effective_cwd: Some(input.interactive_effective_cwd),
            mode: mapper::repl_ingest_mode(
                input.resume,
                input.manual_migrate,
                input.resume_session_id,
            ),
        },
    );
}

pub(super) fn finalize_spawn_error(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    env: &ResumeExecutionEnvironment,
    invocation_row_id: i64,
    guard: &mut FinalizerGuard<'_>,
) -> Result<i32, String> {
    agent_runtime_services
        .invocation_lifecycle_service
        .finalize_invocation(mapper::spawn_error_finalize_request(
            &env.state,
            invocation_row_id,
        ))
        .map_err(|err| err.to_string())?;
    guard.mark_finalized();
    Ok(1)
}
