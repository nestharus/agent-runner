//! Role: mapper, predicate.

use super::error_formatter::{
    format_external_dispatch_error, format_external_input_validation_error,
};
use super::errors::ExternalProviderDispatchError;
use crate::provider_registry::ProviderRegistryError;
use crate::services::ServiceError;
use oulipoly_provider::error::{HostErrorKind, ProviderClientError};
use oulipoly_provider::generated::ErrorCategory;

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
        ProviderClientError::ProviderCapability(error) => service_error(
            ExternalProviderDispatchError::policy_rejected(error.error().diagnostics.clone()),
        ),
    }
}

/// FIX #32: classify a provider-client failure as "rotatable" — a transient,
/// account-specific condition where another pool account may succeed — versus a
/// deterministic failure that would recur identically on every account.
///
/// Rotatable:
/// - host transport `host_timeout` (the provider artifact handshake/launch
///   exceeded the host budget under load — a sibling account spawns its own
///   process and may answer in time), and
/// - a provider-reported error whose category is `unavailable` or `timeout`
///   (the auth-expired / account-temporarily-unavailable class).
///
/// Everything else (schema/protocol violations, policy rejections, missing
/// capabilities, invalid requests, cancellation) is deterministic or
/// caller-driven and must terminal-fail fast rather than burn the pool.
pub(crate) fn provider_client_error_is_rotatable(error: &ProviderClientError) -> bool {
    match error {
        ProviderClientError::Transport { kind, .. } => host_error_kind_is_rotatable(kind),
        ProviderClientError::ProviderCapability(error) => {
            provider_category_is_rotatable(&error.error().category)
        }
        ProviderClientError::Protocol { .. } => false,
    }
}

fn host_error_kind_is_rotatable(kind: &HostErrorKind) -> bool {
    matches!(kind, HostErrorKind::Timeout)
}

fn provider_category_is_rotatable(category: &ErrorCategory) -> bool {
    matches!(
        category,
        ErrorCategory::Unavailable | ErrorCategory::Timeout
    )
}

pub(crate) fn protocol_service_error(category: &'static str) -> ServiceError {
    service_error(ExternalProviderDispatchError::provider_protocol_failure(
        category,
    ))
}

pub(crate) fn invalid_provider_input_error(message: String) -> ServiceError {
    ServiceError::InvalidRequest {
        message: format_external_input_validation_error(&message),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_provider::error::{ProviderCapabilityError, ProviderDiagnostics};
    use serde_json::json;

    fn transport(kind: HostErrorKind) -> ProviderClientError {
        ProviderClientError::host_transport(
            kind,
            "policy.evaluate",
            None,
            ProviderDiagnostics::default(),
        )
    }

    fn protocol(kind: HostErrorKind) -> ProviderClientError {
        ProviderClientError::protocol(kind, "launch", None, ProviderDiagnostics::default())
    }

    fn capability(category: &str) -> ProviderClientError {
        let envelope = json!({
            "contract": oulipoly_provider::generated::CONTRACT_VERSION,
            "request_id": "request-example-001",
            "ok": false,
            "error": {
                "code": "auth_expired",
                "category": category,
                "message": "account token expired",
                "retryable": true,
            },
        });
        let capability = ProviderCapabilityError::from_valid_envelope(
            "policy.evaluate",
            envelope,
            ProviderDiagnostics::default(),
            None,
        )
        .expect("error envelope should parse");
        ProviderClientError::from_capability(capability)
    }

    #[test]
    fn host_timeout_transport_is_rotatable() {
        assert!(provider_client_error_is_rotatable(&transport(
            HostErrorKind::Timeout
        )));
    }

    #[test]
    fn non_timeout_transport_classes_are_terminal() {
        for kind in [
            HostErrorKind::SpawnFailed,
            HostErrorKind::StdoutLimitExceeded,
            HostErrorKind::Cancelled,
            HostErrorKind::ProviderProcessNonzero,
            HostErrorKind::EmptyStdout,
        ] {
            assert!(
                !provider_client_error_is_rotatable(&transport(kind.clone())),
                "transport {kind:?} must not rotate"
            );
        }
    }

    #[test]
    fn protocol_failures_are_terminal() {
        assert!(!provider_client_error_is_rotatable(&protocol(
            HostErrorKind::SchemaInvalidResponse
        )));
        assert!(!provider_client_error_is_rotatable(&protocol(
            HostErrorKind::MismatchedContract
        )));
    }

    #[test]
    fn unavailable_and_timeout_capability_classes_are_rotatable() {
        assert!(
            provider_client_error_is_rotatable(&capability("unavailable")),
            "auth-expired / account-unavailable must rotate"
        );
        assert!(
            provider_client_error_is_rotatable(&capability("timeout")),
            "provider-reported timeout must rotate"
        );
    }

    #[test]
    fn deterministic_capability_classes_are_terminal() {
        for category in [
            "unsupported",
            "invalid_request",
            "invalid_settings",
            "conflict",
            "failed",
        ] {
            assert!(
                !provider_client_error_is_rotatable(&capability(category)),
                "capability {category} must not rotate"
            );
        }
    }
}
