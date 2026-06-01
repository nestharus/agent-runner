//! ## Declared roles
//! mapper

use super::super::ExternalRotationIdentity;
use oulipoly_config::ModelConfig;
use oulipoly_state::ResolvedResume;

pub(super) fn map_external_rotation_identity(
    model: &ModelConfig,
    resolved: &ResolvedResume,
    target_provider: &str,
    describe: oulipoly_provider::generated::DescribeResult,
) -> ExternalRotationIdentity {
    ExternalRotationIdentity {
        model_name: model.name.clone(),
        source_provider: resolved.active_provider.clone(),
        source_session_id: resolved.active_session_id.clone(),
        target_provider: target_provider.to_string(),
        provider_instance_id: Some(format!("{}-instance", describe.provider_id)),
        settings_id: describe
            .settings_schema_id
            .unwrap_or_else(|| "default-settings".to_string()),
    }
}
