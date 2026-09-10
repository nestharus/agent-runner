//! ## Declared roles
//!
//! `mapper`

use super::error_format::format_external_quota_error;
use super::errors::ExternalQuotaError;
use crate::provider_registry::ProviderRegistryError;
use crate::quota::RefreshOutcome;
use oulipoly_provider::error::ProviderClientError;
use oulipoly_provider::generated::QuotaProbeResult;

pub(crate) fn registry_error(error: ProviderRegistryError) -> ExternalQuotaError {
    match error {
        ProviderRegistryError::RuntimeDisabledArtifact { .. } => {
            ExternalQuotaError::runtime_disabled_artifact()
        }
        ProviderRegistryError::ProviderTransport { kind, .. } => {
            ExternalQuotaError::provider_transport(kind)
        }
        ProviderRegistryError::ProviderProtocol { kind, .. } => {
            ExternalQuotaError::provider_protocol(kind)
        }
        ProviderRegistryError::ProviderDescribeFailed { code, .. } => {
            ExternalQuotaError::provider_failed(code)
        }
        ProviderRegistryError::InvalidImplementationRef { .. }
        | ProviderRegistryError::ModelProviderNotConfigured { .. }
        | ProviderRegistryError::AccountImplementationNotConfigured { .. }
        | ProviderRegistryError::AccountSettingsNotConfigured { .. }
        | ProviderRegistryError::FamilyImplementationNotConfigured { .. }
        | ProviderRegistryError::FamilyImplementationConflict { .. } => {
            ExternalQuotaError::registry_lookup()
        }
    }
}

pub(crate) fn provider_client_error(error: ProviderClientError) -> ExternalQuotaError {
    match error {
        ProviderClientError::Transport { kind, .. } => {
            ExternalQuotaError::provider_transport(kind.as_str())
        }
        ProviderClientError::Protocol { kind, .. } => {
            ExternalQuotaError::provider_protocol(kind.as_str())
        }
        ProviderClientError::ProviderCapability(error) => {
            ExternalQuotaError::provider_failed(error.error().code.clone())
        }
    }
}

pub(crate) fn failed_outcome(error: ExternalQuotaError) -> RefreshOutcome {
    RefreshOutcome::Failed(format_external_quota_error(error))
}

pub(crate) fn probe_retry_error(
    first: Result<QuotaProbeResult, ExternalQuotaError>,
    refresh_auth: Option<ExternalQuotaError>,
    retry: ExternalQuotaError,
) -> ExternalQuotaError {
    let first = first_probe_retry_cause(first);
    ExternalQuotaError::probe_retry_failed(first, refresh_auth, retry)
}

fn first_probe_retry_cause(
    first: Result<QuotaProbeResult, ExternalQuotaError>,
) -> ExternalQuotaError {
    match first {
        Err(error) => error,
        Ok(result) if !result.available => ExternalQuotaError::probe_unavailable(),
        Ok(result) if result.windows.is_empty() => ExternalQuotaError::probe_empty_windows(),
        Ok(_) => ExternalQuotaError::probe_empty_windows(),
    }
}
