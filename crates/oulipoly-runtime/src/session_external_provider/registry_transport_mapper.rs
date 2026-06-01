//! Role: mapper.

use crate::provider_registry::ProviderRegistryError;
use oulipoly_provider::error::{HostErrorKind, ProviderClientError, ProviderDiagnostics};

pub(crate) fn registry_error_as_transport(error: ProviderRegistryError) -> ProviderClientError {
    ProviderClientError::host_transport(
        HostErrorKind::Other("provider_transport_failure".to_string()),
        "session",
        None,
        ProviderDiagnostics::with_description(error.to_string()),
    )
}
