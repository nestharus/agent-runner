//! Role: formatter.

use crate::provider_registry::ProviderRegistryError;

pub(crate) fn registry_error_message(error: &ProviderRegistryError) -> String {
    format!("provider_transport_failure: {error}")
}
