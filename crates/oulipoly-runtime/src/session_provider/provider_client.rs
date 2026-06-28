use super::types::{SessionProviderError, SessionProviderIdentity};
use crate::provider_registry::{ProviderRegistry, ProviderRegistryError};
use oulipoly_provider::client::ProviderClient;
use oulipoly_provider::error::ProviderClientError;
use oulipoly_provider::generated::DescribeResult;
use serde_json::Value;

pub(super) fn session_client(
    registry: &ProviderRegistry,
    identity: &SessionProviderIdentity,
) -> Result<ProviderClient, SessionProviderError> {
    let describe = describe_session_provider(registry, identity)?;
    require_session_capability(&describe)?;
    enabled_provider_instance_client(registry, identity)
}

pub(super) fn session_enumerate_client(
    registry: &ProviderRegistry,
    identity: &SessionProviderIdentity,
) -> Result<ProviderClient, SessionProviderError> {
    let describe = describe_session_provider(registry, identity)?;
    require_session_capability(&describe)?;
    require_session_enumerate_capability(&describe)?;
    enabled_provider_instance_client(registry, identity)
}

fn describe_session_provider(
    registry: &ProviderRegistry,
    identity: &SessionProviderIdentity,
) -> Result<DescribeResult, SessionProviderError> {
    registry
        .describe_model_provider_instance(&identity.model_name, &identity.provider_name)
        .map_err(map_registry_error)
}

fn require_session_capability(describe: &DescribeResult) -> Result<(), SessionProviderError> {
    if describe.capabilities.session {
        Ok(())
    } else {
        Err(SessionProviderError::new(
            "session_capability_missing",
            "provider describe did not advertise session capability",
        ))
    }
}

fn require_session_enumerate_capability(
    describe: &DescribeResult,
) -> Result<(), SessionProviderError> {
    if describe.capabilities.session_enumerate {
        Ok(())
    } else {
        Err(SessionProviderError::new(
            "session_enumerate_capability_missing",
            "provider describe did not advertise session.enumerate capability",
        ))
    }
}

fn enabled_provider_instance_client(
    registry: &ProviderRegistry,
    identity: &SessionProviderIdentity,
) -> Result<ProviderClient, SessionProviderError> {
    let artifact = registry
        .enabled_artifact_for_model_provider(&identity.model_name, &identity.provider_name)
        .map_err(map_registry_error)?;
    Ok(registry.client_factory().client_for(artifact))
}

pub(super) fn provider_client(
    registry: &ProviderRegistry,
    identity: &SessionProviderIdentity,
) -> Result<ProviderClient, SessionProviderError> {
    let artifact = registry
        .enabled_artifact_for_model_provider(&identity.model_name, &identity.provider_name)
        .map_err(map_registry_error)?;
    Ok(registry.client_factory().client_for(artifact))
}

pub(super) fn invoke_session<T>(
    client: &ProviderClient,
    subcommand: &str,
    request: Value,
) -> Result<T, SessionProviderError>
where
    T: serde::de::DeserializeOwned,
{
    client
        .invoke_typed(subcommand, request, Vec::<(String, String)>::new())
        .map_err(map_client_error)
}

fn map_registry_error(error: ProviderRegistryError) -> SessionProviderError {
    SessionProviderError::new("provider_registry_error", error.to_string())
}

fn map_client_error(error: ProviderClientError) -> SessionProviderError {
    match error.provider_error_code() {
        Some(code) => SessionProviderError::new(code.to_string(), error.to_string()),
        None => SessionProviderError::new(error.transport_kind().to_string(), error.to_string()),
    }
}
