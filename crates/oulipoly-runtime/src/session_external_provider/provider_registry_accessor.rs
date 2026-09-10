//! Role: accessor.

use super::identity::ExternalSessionIdentity;
use crate::provider_registry::{PinnedProviderEndpoint, ProviderRegistry, ProviderRegistryError};
use std::sync::Arc;

pub(crate) fn preflight_provider(
    registry: &ProviderRegistry,
    identity: &ExternalSessionIdentity,
) -> Result<Arc<PinnedProviderEndpoint>, ProviderRegistryError> {
    registry.preflight_account(&identity.provider_name)
}
