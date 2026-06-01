//! Role: validator.

use super::provider_error::{ExternalSessionProviderError, map_registry_missing_error};
use crate::provider_registry::ProviderRegistryHandle;

pub(crate) fn export_registry_handle(
    handle: Option<&ProviderRegistryHandle>,
) -> Result<&ProviderRegistryHandle, ExternalSessionProviderError> {
    handle.ok_or_else(map_registry_missing_error)
}

pub(crate) fn replace_registry_handle(
    handle: Option<&ProviderRegistryHandle>,
) -> Result<&ProviderRegistryHandle, ExternalSessionProviderError> {
    handle.ok_or_else(map_registry_missing_error)
}
