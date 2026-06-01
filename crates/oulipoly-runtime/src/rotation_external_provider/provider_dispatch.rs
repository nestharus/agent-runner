//! ## Declared roles
//! orchestration, mapper, formatter

use super::{ExternalRotationError, error_formatter};
use oulipoly_provider::error::ProviderClientError;
use serde_json::Value;

pub(super) fn invoke_provider_contract<T: serde::de::DeserializeOwned>(
    client: &oulipoly_provider::client::ProviderClient,
    operation: &'static str,
    request: Value,
) -> Result<T, ExternalRotationError> {
    client
        .invoke_typed::<T, _>(operation, request, Vec::<(String, String)>::new())
        .map_err(map_provider_client_error)
}

fn map_provider_client_error(error: ProviderClientError) -> ExternalRotationError {
    match error {
        ProviderClientError::Transport { description, .. } => {
            error_formatter::provider_transport_failure(description)
        }
        ProviderClientError::Protocol { description, .. } => {
            error_formatter::protocol_invalid_response(description)
        }
        ProviderClientError::ProviderCapability(capability) => {
            error_formatter::capability_missing(capability.error().code.clone())
        }
    }
}
