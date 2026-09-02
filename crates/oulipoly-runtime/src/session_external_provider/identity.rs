//! Role: mapper.

use crate::services::SessionServiceExternalProviderIdentity;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExternalSessionIdentity {
    pub(crate) model_name: String,
    pub(crate) provider_name: String,
    pub(crate) provider_instance_id: Option<String>,
    pub(crate) settings_id: String,
}

pub(crate) fn map_identity(
    identity: SessionServiceExternalProviderIdentity,
) -> ExternalSessionIdentity {
    ExternalSessionIdentity {
        model_name: identity.model_name,
        provider_name: identity.provider_name,
        provider_instance_id: identity.provider_instance_id,
        settings_id: identity.settings_id,
    }
}

pub(crate) fn map_described_identity(
    identity: ExternalSessionIdentity,
    provider_instance_id: String,
    settings_id: String,
) -> ExternalSessionIdentity {
    ExternalSessionIdentity {
        model_name: identity.model_name,
        provider_name: identity.provider_name,
        provider_instance_id: Some(provider_instance_id),
        settings_id,
    }
}

pub(crate) fn provider_instance_id(
    identity: &ExternalSessionIdentity,
) -> Result<&str, super::provider_error::ExternalSessionProviderError> {
    identity
        .provider_instance_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(super::provider_error::map_instance_identity_missing_error)
}
