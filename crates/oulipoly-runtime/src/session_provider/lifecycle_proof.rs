use super::types::{NoRefProofOutput, NoRefProofRequest, SessionProviderError};
use crate::provider_registry::ProviderRegistryHandle;
use crate::services::{
    ProductionSessionLifecycleService, SessionLifecycleIngestMode, SessionLifecycleOutput,
    SessionLifecycleRequest, SessionLifecycleServicePort,
};
use oulipoly_config::SessionsConfig;

pub fn dispatch_aware_no_ref_lifecycle_proof(
    request: NoRefProofRequest<'_>,
) -> Result<NoRefProofOutput, SessionProviderError> {
    let lifecycle = run_no_ref_lifecycle_proof(&request)?;
    Ok(no_ref_proof_output(lifecycle))
}

fn run_no_ref_lifecycle_proof(
    request: &NoRefProofRequest<'_>,
) -> Result<NoRefLifecycleProof, SessionProviderError> {
    let mut stderr = Vec::new();
    let sessions_cfg = SessionsConfig::default();
    let lifecycle = no_ref_lifecycle_service(request.registry.clone())
        .ingest_session(no_ref_lifecycle_request(
            request,
            &sessions_cfg,
            &mut stderr,
        ))
        .map_err(|error| {
            SessionProviderError::new("no_ref_lifecycle_proof_failed", error.to_string())
        })?;
    Ok(NoRefLifecycleProof { stderr, lifecycle })
}

struct NoRefLifecycleProof {
    stderr: Vec<u8>,
    lifecycle: SessionLifecycleOutput,
}

fn no_ref_lifecycle_request<'a>(
    request: &'a NoRefProofRequest<'a>,
    sessions_cfg: &'a SessionsConfig,
    stderr: &'a mut Vec<u8>,
) -> SessionLifecycleRequest<'a> {
    SessionLifecycleRequest {
        state: request.state,
        sessions_cfg,
        providers_cfg: None,
        provider_name: request.provider_name,
        external_provider: None,
        invocation_row_id: request.invocation_row_id,
        invocation_uuid: request.invocation_uuid,
        effective_cwd: None,
        mode: SessionLifecycleIngestMode::Pinned {
            resume_target: request.session_id.to_string(),
        },
        stderr,
    }
}

fn no_ref_proof_output(lifecycle: NoRefLifecycleProof) -> NoRefProofOutput {
    NoRefProofOutput {
        lifecycle_stderr: lifecycle.stderr,
        lifecycle: lifecycle.lifecycle,
    }
}

fn no_ref_lifecycle_service(
    registry: Option<ProviderRegistryHandle>,
) -> ProductionSessionLifecycleService {
    registry.map_or_else(
        ProductionSessionLifecycleService::new,
        ProductionSessionLifecycleService::with_registry_handle,
    )
}
