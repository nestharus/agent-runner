//! Role: accessor.

use super::identity::ExternalSessionIdentity;
use crate::provider_registry::{ProviderRegistry, ProviderRegistryError};
use oulipoly_provider::client::ProviderClient;
use oulipoly_provider::generated::DescribeResult;

pub(crate) fn describe_provider(
    registry: &ProviderRegistry,
    identity: &ExternalSessionIdentity,
) -> Result<DescribeResult, ProviderRegistryError> {
    registry.describe_model_provider(&identity.model_name)
}

pub(crate) fn provider_client_for_model(
    registry: &ProviderRegistry,
    identity: &ExternalSessionIdentity,
) -> Result<ProviderClient, ProviderRegistryError> {
    let artifact = registry.enabled_artifact_for_model(&identity.model_name)?;
    Ok(registry.client_factory().client_for(artifact))
}
