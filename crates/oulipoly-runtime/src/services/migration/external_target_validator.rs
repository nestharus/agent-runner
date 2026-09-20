//! ## Declared roles
//! validator

use crate::rotation_domain::ExternalRotationError;
use crate::rotation_external_provider::error_formatter;
use crate::services::MigrationServiceRequest;

pub(super) fn select_external_rotation_target_provider(
    request: &MigrationServiceRequest<'_>,
    registry: &crate::provider_registry::ProviderRegistry,
) -> Result<String, ExternalRotationError> {
    if let Some(target) = request.manual_target {
        if target.is_empty() {
            let source_artifact = registry
                .artifact_key_for_account(&request.resolved.active_provider)
                .ok_or_else(|| {
                    error_formatter::missing_enabled_artifact(
                        "source account has no provider artifact",
                    )
                })?;
            let active_index = request
                .migration_model
                .providers
                .iter()
                .position(|provider| provider.name == request.resolved.active_provider);
            // Keep original pool indices: the persisted cursor belongs to this model.
            for _ in 0..request.migration_model.providers.len() {
                let index = crate::balancer::select_next_working_candidate(
                    request.state,
                    request.migration_model,
                    chrono::Utc::now(),
                    active_index,
                )
                .map_err(|error| error_formatter::host_apply_conflict(format!("{error:?}")))?;
                let Some(index) = index else { break };
                let candidate = &request.migration_model.providers[index];
                if registry.artifact_key_for_account(&candidate.name).as_ref()
                    == Some(&source_artifact)
                {
                    return Ok(candidate.name.clone());
                }
            }
            return Err(error_formatter::malformed_external_identity(
                "no other working provider is available for external rotation",
            ));
        }
        return validate_manual_external_target(request, target);
    }
    Err(error_formatter::malformed_external_identity(
        "external rotation target requires an explicit manual target",
    ))
}

fn validate_manual_external_target(
    request: &MigrationServiceRequest<'_>,
    target: &str,
) -> Result<String, ExternalRotationError> {
    if target == request.resolved.active_provider {
        return Err(error_formatter::malformed_external_identity(
            "manual external rotation target matches active provider",
        ));
    }
    if request
        .migration_model
        .providers
        .iter()
        .any(|provider| provider.name == target)
    {
        Ok(target.to_string())
    } else {
        Err(error_formatter::malformed_external_identity(
            "manual external rotation target is not in model pool",
        ))
    }
}
