//! Role: mapper.

use super::error_formatter::format_external_dispatch_error;
use super::errors::ExternalProviderDispatchError;
use crate::provider_registry::ProviderRegistryError;
use crate::services::ServiceError;
use oulipoly_provider::error::{HostErrorKind, ProviderClientError};

pub(crate) fn map_registry_error(error: ProviderRegistryError) -> ServiceError {
    match error {
        ProviderRegistryError::RuntimeDisabledArtifact { .. } => {
            service_error(ExternalProviderDispatchError::runtime_disabled_artifact())
        }
        ProviderRegistryError::ProviderTransport { kind, .. } => service_error(
            ExternalProviderDispatchError::provider_transport_failure(kind),
        ),
        ProviderRegistryError::ProviderProtocol { kind, .. } => service_error(
            ExternalProviderDispatchError::provider_protocol_failure(kind),
        ),
        ProviderRegistryError::ProviderDescribeFailed { code, .. } => service_error(
            ExternalProviderDispatchError::provider_protocol_failure(code),
        ),
        ProviderRegistryError::InvalidImplementationRef { .. }
        | ProviderRegistryError::ModelProviderNotConfigured { .. } => service_error(
            ExternalProviderDispatchError::provider_protocol_failure("registry_lookup"),
        ),
    }
}

pub(crate) fn map_provider_client_error(error: ProviderClientError) -> ServiceError {
    match &error {
        ProviderClientError::Transport { kind, .. } if matches!(kind, HostErrorKind::Cancelled) => {
            service_error(ExternalProviderDispatchError::cancellation_fallback(
                kind.as_str(),
            ))
        }
        ProviderClientError::Transport { kind, .. } => service_error(
            ExternalProviderDispatchError::provider_transport_failure(kind.as_str()),
        ),
        ProviderClientError::Protocol {
            kind,
            process_status,
            diagnostics,
            ..
        } if provider_nonzero_before_final(kind, process_status.as_deref(), diagnostics) => {
            service_error(ExternalProviderDispatchError::provider_transport_failure(
                "provider_nonzero_before_final",
            ))
        }
        ProviderClientError::Protocol { kind, .. } => service_error(
            ExternalProviderDispatchError::provider_protocol_failure(kind.as_str()),
        ),
        ProviderClientError::ProviderCapability(_) => {
            service_error(ExternalProviderDispatchError::policy_rejected())
        }
    }
}

pub(crate) fn protocol_service_error(category: &'static str) -> ServiceError {
    service_error(ExternalProviderDispatchError::provider_protocol_failure(
        category,
    ))
}

pub(crate) fn service_error(error: ExternalProviderDispatchError) -> ServiceError {
    ServiceError::Dependency {
        message: format_external_dispatch_error(error),
    }
}

fn provider_nonzero_before_final(
    kind: &HostErrorKind,
    status: Option<&oulipoly_provider::generated::ProcessStatus>,
    diagnostics: &oulipoly_provider::error::ProviderDiagnostics,
) -> bool {
    matches!(kind, HostErrorKind::MissingFinalExit)
        && diagnostics.provider_process_nonzero
        && matches!(
            status,
            Some(oulipoly_provider::generated::ProcessStatus::Exited { code }) if *code != 0
        )
}
