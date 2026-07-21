//! ## Declared roles
//!
//! `orchestration`, `mapper`

use oulipoly_runtime::services::{ResumeServiceOutput, ResumeServiceRequest};

use super::formatter;
use crate::migration_providers::ResumeExecutionEnvironment;
use crate::resume_cli::{format_resume_service_rejection, resume_model_pool_mismatch_message};
use crate::wiring;

pub(super) fn resolve_optional_repl_resume(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    env: &ResumeExecutionEnvironment,
    resume: Option<&str>,
    model_name: Option<&str>,
) -> Result<Option<oulipoly_state::ResolvedResume>, String> {
    let Some(session_id) = resume else {
        return Ok(None);
    };
    resolve_repl_resume(agent_runtime_services, env, session_id, model_name).map(Some)
}

fn resolve_repl_resume(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    env: &ResumeExecutionEnvironment,
    session_id: &str,
    model_name: Option<&str>,
) -> Result<oulipoly_state::ResolvedResume, String> {
    // Source guard marker: agent_runtime_services.resume_service.resolve_resume(ResumeServiceRequest)
    let output = agent_runtime_services
        .resume_service
        .resolve_resume(ResumeServiceRequest {
            state: &env.state,
            models: &env.models,
            providers_cfg: &env.providers_cfg,
            input: session_id,
            model_override: model_name,
        });
    match map_repl_resume_resolution(output) {
        ReplResumeResolution::Resolved(resolved) => Ok(resolved),
        ReplResumeResolution::ProviderModelMismatch { active_provider } => {
            Err(resume_model_pool_mismatch_message(
                &env.models,
                model_name.unwrap_or("<unknown>"),
                session_id,
                &active_provider,
            ))
        }
        ReplResumeResolution::Rejected(error) => Err(format_resume_service_rejection(error)),
        ReplResumeResolution::ServiceFailure(error) => {
            Err(formatter::resume_service_failure(&error))
        }
    }
}

enum ReplResumeResolution {
    Resolved(oulipoly_state::ResolvedResume),
    ProviderModelMismatch { active_provider: String },
    Rejected(oulipoly_runtime::services::ResumeServiceRejection),
    ServiceFailure(String),
}

fn map_repl_resume_resolution(
    output: Result<ResumeServiceOutput, oulipoly_runtime::services::ServiceError>,
) -> ReplResumeResolution {
    match output {
        Ok(ResumeServiceOutput::ResumeResolved { resolved }) => {
            ReplResumeResolution::Resolved(resolved)
        }
        Ok(ResumeServiceOutput::ResumeRejected {
            error:
                oulipoly_runtime::services::ResumeServiceRejection::State(
                    oulipoly_state::ResumeError::ProviderModelMismatch {
                        active_provider, ..
                    },
                ),
        }) => ReplResumeResolution::ProviderModelMismatch { active_provider },
        Ok(ResumeServiceOutput::ResumeRejected { error }) => ReplResumeResolution::Rejected(error),
        Err(err) => ReplResumeResolution::ServiceFailure(err.to_string()),
    }
}
