//! Role: formatter.

use super::errors::ExternalProviderDispatchError;

pub(crate) fn format_external_dispatch_error(error: ExternalProviderDispatchError) -> String {
    match error {
        ExternalProviderDispatchError::MissingCapability { capability } => {
            format!("external provider missing required capability: {capability}")
        }
        ExternalProviderDispatchError::RuntimeDisabledCrate => {
            "external provider artifact is runtime-disabled: runtime_disabled".to_string()
        }
        ExternalProviderDispatchError::ProviderTransport { category } => {
            format!("external provider transport failed: {category}")
        }
        ExternalProviderDispatchError::ProviderProtocol { category } => {
            format!("external provider protocol failed: {category}")
        }
        ExternalProviderDispatchError::CancellationFallback { reason } => {
            let _ = reason;
            "external provider launch cancelled before final event".to_string()
        }
        ExternalProviderDispatchError::PolicyRejected => {
            "external provider policy rejected launch".to_string()
        }
    }
}
