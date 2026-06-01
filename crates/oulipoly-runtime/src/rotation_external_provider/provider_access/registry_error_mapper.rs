//! ## Declared roles
//! mapper, formatter

use super::super::{ExternalRotationError, error_formatter};
use crate::provider_registry::ProviderRegistryError;

pub(super) fn map_registry_identity_error(error: ProviderRegistryError) -> ExternalRotationError {
    match error {
        ProviderRegistryError::ModelProviderNotConfigured { model_name } => {
            error_formatter::missing_enabled_artifact(model_name)
        }
        other => map_registry_dispatch_error(other),
    }
}

pub(super) fn map_registry_dispatch_error(error: ProviderRegistryError) -> ExternalRotationError {
    match error {
        ProviderRegistryError::RuntimeDisabledArtifact { kind, .. } => {
            error_formatter::disabled_artifact(kind)
        }
        ProviderRegistryError::ProviderTransport { kind, .. } => {
            error_formatter::provider_transport_failure(kind)
        }
        ProviderRegistryError::ProviderProtocol { kind, .. } => {
            error_formatter::protocol_invalid_response(kind)
        }
        ProviderRegistryError::ProviderDescribeFailed { code, .. } => {
            error_formatter::describe_failure(code)
        }
        ProviderRegistryError::ModelProviderNotConfigured { model_name } => {
            error_formatter::missing_enabled_artifact(model_name)
        }
        ProviderRegistryError::InvalidImplementationRef { source } => {
            error_formatter::malformed_external_identity(source.to_string())
        }
    }
}
