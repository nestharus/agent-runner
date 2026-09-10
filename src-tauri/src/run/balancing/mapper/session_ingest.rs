use std::path::Path;

use oulipoly_config::ModelConfig;

use super::super::accessor::BalancedExecutionEnvironment;
use super::super::finalization::CompletedAttemptInput;
use crate::session_ingest_cli::{ResumeIngestMode, SessionIngestRequest};
use crate::wiring;

pub(in crate::run::balancing) struct CompletedSessionIngestRequestInput<'a> {
    pub(in crate::run::balancing) agent_runtime_services: &'a wiring::AgentRuntimeServices,
    pub(in crate::run::balancing) env: &'a BalancedExecutionEnvironment,
    pub(in crate::run::balancing) model: &'a ModelConfig,
    pub(in crate::run::balancing) provider_name: &'a str,
    pub(in crate::run::balancing) invocation_row_id: i64,
    pub(in crate::run::balancing) invocation_id: &'a str,
    pub(in crate::run::balancing) effective_cwd: &'a Path,
}

pub(in crate::run::balancing) fn completed_session_ingest_request<'a>(
    input: CompletedSessionIngestRequestInput<'a>,
) -> SessionIngestRequest<'a> {
    SessionIngestRequest {
        state: &input.env.state,
        sessions_cfg: &input.env.sessions_cfg,
        providers_cfg: Some(&input.env.providers_cfg),
        provider_name: input.provider_name,
        external_provider: crate::session_ingest_cli::configured_session_external_provider_identity(
            input.agent_runtime_services,
            Some(input.model),
            input.provider_name,
        ),
        invocation_row_id: input.invocation_row_id,
        invocation_uuid: input.invocation_id,
        effective_cwd: Some(input.effective_cwd),
        mode: ResumeIngestMode::Unpinned {
            capture_method: "turn_script",
        },
    }
}

pub(in crate::run::balancing) fn completed_session_ingest_request_for_attempt<'a>(
    input: &'a CompletedAttemptInput<'_, '_, '_>,
    effective_cwd: &'a Path,
) -> SessionIngestRequest<'a> {
    completed_session_ingest_request(CompletedSessionIngestRequestInput {
        agent_runtime_services: input.agent_runtime_services,
        env: input.env,
        model: input.model,
        provider_name: input.provider_name,
        invocation_row_id: input.invocation_row_id,
        invocation_id: &input.invocation.id,
        effective_cwd,
    })
}
