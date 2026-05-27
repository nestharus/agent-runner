//! ## Declared roles
//!
//! `orchestration`, `mapper`, `formatter`

use oulipoly_runtime::services::{ResumeServiceOutput, ResumeServiceRequest};

use crate::migration_providers::ResumeExecutionEnvironment;
use crate::resume_cli::{format_resume_error, resume_model_pool_mismatch_message};
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
    match agent_runtime_services
        .resume_service
        .resolve_resume(ResumeServiceRequest {
            state: &env.state,
            models: &env.models,
            input: session_id,
            model_override: model_name,
        }) {
        Ok(ResumeServiceOutput::ResumeResolved { resolved }) => Ok(resolved),
        Ok(ResumeServiceOutput::ResumeRejected {
            error:
                oulipoly_state::ResumeError::ProviderModelMismatch {
                    active_provider, ..
                },
        }) => Err(resume_model_pool_mismatch_message(
            &env.models,
            model_name.unwrap_or("<unknown>"),
            session_id,
            &active_provider,
        )),
        Ok(ResumeServiceOutput::ResumeRejected { error }) => Err(format_resume_error(error)),
        Err(err) => Err(format!("resume service failed: {err}")),
    }
}
