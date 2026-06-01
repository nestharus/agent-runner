//! ## Declared roles
//! accessor

use super::registry_error_mapper;
use crate::provider_registry::ProviderRegistry;
use oulipoly_config::ModelConfig;

use super::super::ExternalRotationError;

pub(super) fn describe_external_model_provider(
    registry: &ProviderRegistry,
    model: &ModelConfig,
) -> Result<oulipoly_provider::generated::DescribeResult, ExternalRotationError> {
    registry
        .describe_model_provider(&model.name)
        .map_err(registry_error_mapper::map_registry_identity_error)
}

pub(super) fn describe_external_model_provider_for_dispatch(
    registry: &ProviderRegistry,
    model_name: &str,
) -> Result<oulipoly_provider::generated::DescribeResult, ExternalRotationError> {
    registry
        .describe_model_provider(model_name)
        .map_err(registry_error_mapper::map_registry_dispatch_error)
}

pub(super) fn enabled_artifact_for_model(
    registry: &ProviderRegistry,
    model_name: &str,
) -> Result<oulipoly_provider::resolver::ProviderArtifactRef, ExternalRotationError> {
    registry
        .enabled_artifact_for_model(model_name)
        .map_err(registry_error_mapper::map_registry_dispatch_error)
}
