use super::types::{SessionProviderError, SessionProviderIdentity};
use crate::provider_registry::{PinnedProviderEndpoint, ProviderRegistry, ProviderRegistryError};
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
    let endpoint = registry
        .preflight_account(&identity.provider_name)
        .map_err(map_registry_error)?;
    validate_endpoint_identity(endpoint.as_ref(), identity)?;
    require_session_capability(endpoint.capabilities())?;
    client_from_endpoint(registry, endpoint.client(), cancellation)
}

pub(super) fn session_enumerate_client(
    registry: &ProviderRegistry,
    identity: &SessionProviderIdentity,
) -> Result<ProviderClient, SessionProviderError> {
    let endpoint = registry
        .preflight_account(&identity.provider_name)
        .map_err(map_registry_error)?;
    validate_endpoint_identity(endpoint.as_ref(), identity)?;
    require_session_capability(endpoint.capabilities())?;
    require_session_enumerate_capability(endpoint.capabilities())?;
    client_from_endpoint(registry, endpoint.client(), None)
}

pub(super) fn session_page_client(
    registry: &ProviderRegistry,
    identity: &SessionProviderIdentity,
    cancellation: &CancellationToken,
    timeout: Duration,
) -> Result<ProviderClient, SessionProviderError> {
    let endpoint = registry
        .preflight_account(&identity.provider_name)
        .map_err(map_registry_error)?;
    validate_endpoint_identity(endpoint.as_ref(), identity)?;
    require_session_capability(endpoint.capabilities())?;
    require_session_turn_pages_capability(endpoint.capabilities())?;
    registry
        .client_factory()
        .client_from_pinned_with_cancellation_and_timeout(endpoint.client(), cancellation, timeout)
        .map_err(map_client_error)
}

fn validate_endpoint_identity(
    endpoint: &PinnedProviderEndpoint,
    identity: &SessionProviderIdentity,
) -> Result<(), SessionProviderError> {
    let expected_instance_id = format!("{}-instance", endpoint.capabilities().provider_id);
    if identity.provider_instance_id.as_deref() != Some(expected_instance_id.as_str()) {
        return Err(SessionProviderError::new(
            "session_provider_instance_identity_mismatch",
            "session provider instance identity does not match the selected account endpoint",
        ));
    }
    let settings_id = endpoint.settings_id().map_err(map_registry_error)?;
    if identity.settings_id != settings_id {
        return Err(SessionProviderError::new(
            "session_provider_settings_identity_mismatch",
            "session settings identity does not match the selected account endpoint",
        ));
    }
    Ok(())
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

fn client_from_endpoint(
    registry: &ProviderRegistry,
    pinned: &ProviderClient,
    cancellation: Option<&CancellationToken>,
) -> Result<ProviderClient, SessionProviderError> {
    match cancellation {
        Some(cancellation) => registry
            .client_factory()
            .client_from_pinned_with_cancellation(pinned, cancellation)
            .map_err(map_client_error),
        None => pinned
            .fork_from_pinned(pinned.options().clone())
            .map_err(map_client_error),
    }
}

pub(super) fn provider_client(
    registry: &ProviderRegistry,
    identity: &SessionProviderIdentity,
) -> Result<ProviderClient, SessionProviderError> {
    session_client(registry, identity)
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
        | ProviderRegistryError::AccountSettingsNotConfigured { .. }
        | ProviderRegistryError::FamilyImplementationNotConfigured { .. }
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
