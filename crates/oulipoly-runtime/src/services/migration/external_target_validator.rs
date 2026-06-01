//! ## Declared roles
//! validator

use crate::rotation_domain::ExternalRotationError;
use crate::rotation_external_provider::error_formatter;
use crate::services::MigrationServiceRequest;

pub(super) fn select_external_rotation_target_provider(
    request: &MigrationServiceRequest<'_>,
) -> Result<String, ExternalRotationError> {
    if let Some(target) = request.manual_target {
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
