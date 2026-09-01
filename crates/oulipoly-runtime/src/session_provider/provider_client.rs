use super::types::{SessionProviderError, SessionProviderIdentity};
use crate::provider_registry::{ProviderRegistry, ProviderRegistryError};
use oulipoly_provider::client::{CancellationToken, ProviderClient};
use oulipoly_provider::error::ProviderClientError;
use oulipoly_provider::generated::DescribeResult;
use serde_json::Value;
use std::time::Duration;

pub(super) fn session_client(
    registry: &ProviderRegistry,
    identity: &SessionProviderIdentity,
) -> Result<ProviderClient, SessionProviderError> {
    session_client_inner(registry, identity, None)
}

pub(super) fn session_client_with_cancellation(
    registry: &ProviderRegistry,
    identity: &SessionProviderIdentity,
    cancellation: &CancellationToken,
) -> Result<ProviderClient, SessionProviderError> {
    session_client_inner(registry, identity, Some(cancellation))
}

fn session_client_inner(
    registry: &ProviderRegistry,
    identity: &SessionProviderIdentity,
    cancellation: Option<&CancellationToken>,
) -> Result<ProviderClient, SessionProviderError> {
    let describe = match cancellation {
        Some(cancellation) => registry.describe_model_provider_instance_with_cancellation(
            &identity.model_name,
            &identity.provider_name,
            cancellation,
        ),
        None => {
            registry.describe_model_provider_instance(&identity.model_name, &identity.provider_name)
        }
    }
    .map_err(map_registry_error)?;
    require_session_capability(&describe)?;
    enabled_provider_instance_client(registry, identity, cancellation)
}

pub(super) fn session_enumerate_client(
    registry: &ProviderRegistry,
    identity: &SessionProviderIdentity,
) -> Result<ProviderClient, SessionProviderError> {
    let describe = describe_session_provider(registry, identity)?;
    require_session_capability(&describe)?;
    require_session_enumerate_capability(&describe)?;
    enabled_provider_instance_client(registry, identity, None)
}

pub(super) fn session_page_client(
    registry: &ProviderRegistry,
    identity: &SessionProviderIdentity,
    cancellation: &CancellationToken,
    timeout: Duration,
) -> Result<ProviderClient, SessionProviderError> {
    let describe = registry
        .describe_model_provider_instance_with_cancellation(
            &identity.model_name,
            &identity.provider_name,
            cancellation,
        )
        .map_err(map_registry_error)?;
    require_session_capability(&describe)?;
    require_session_turn_pages_capability(&describe)?;
    let artifact = registry
        .enabled_artifact_for_model_provider(&identity.model_name, &identity.provider_name)
        .map_err(map_registry_error)?;
    Ok(registry
        .client_factory()
        .client_for_with_cancellation_and_timeout(artifact, cancellation, timeout))
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

fn require_session_turn_pages_capability(
    describe: &DescribeResult,
) -> Result<(), SessionProviderError> {
    if describe.capabilities.session_turn_pages_v1 {
        Ok(())
    } else {
        Err(SessionProviderError::new(
            "session_turn_pages_capability_missing",
            "provider describe did not advertise selected session_turn_pages_v1 capability",
        ))
    }
}

fn enabled_provider_instance_client(
    registry: &ProviderRegistry,
    identity: &SessionProviderIdentity,
    cancellation: Option<&CancellationToken>,
) -> Result<ProviderClient, SessionProviderError> {
    let artifact = registry
        .enabled_artifact_for_model_provider(&identity.model_name, &identity.provider_name)
        .map_err(map_registry_error)?;
    Ok(match cancellation {
        Some(cancellation) => registry
            .client_factory()
            .client_for_with_cancellation(artifact, cancellation),
        None => registry.client_factory().client_for(artifact),
    })
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
    let token = match &error {
        ProviderRegistryError::ProviderTransport { .. }
        | ProviderRegistryError::ProviderProtocol { .. }
        | ProviderRegistryError::ProviderDescribeFailed { .. }
        | ProviderRegistryError::RuntimeDisabledArtifact { .. }
        | ProviderRegistryError::ModelProviderNotConfigured { .. }
        | ProviderRegistryError::AccountImplementationNotConfigured { .. }
        | ProviderRegistryError::FamilyImplementationConflict { .. } => {
            "session_provider_describe_unavailable"
        }
        ProviderRegistryError::InvalidImplementationRef { .. } => "provider_registry_error",
    };
    SessionProviderError::new(token, error.to_string())
}

fn map_client_error(error: ProviderClientError) -> SessionProviderError {
    match error.provider_error_code() {
        Some(code) => SessionProviderError::new(code.to_string(), error.to_string()),
        None => SessionProviderError::new(error.transport_kind().to_string(), error.to_string()),
    }
}
