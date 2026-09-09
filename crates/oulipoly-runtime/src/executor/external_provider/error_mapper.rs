//! Role: mapper, predicate.

use super::error_formatter::{
    format_external_dispatch_error, format_external_input_validation_error,
    format_transport_diagnostic,
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
        ProviderRegistryError::ProviderTransport { kind, source } => service_error(
            ExternalProviderDispatchError::provider_transport_diagnostic(
                kind,
                format_transport_diagnostic(&source),
            ),
        ),
        ProviderRegistryError::ProviderProtocol { kind, .. } => service_error(
            ExternalProviderDispatchError::provider_protocol_failure(kind),
        ),
        ProviderRegistryError::ProviderDescribeFailed { code, .. } => service_error(
            ExternalProviderDispatchError::provider_protocol_failure(code),
        ),
        ProviderRegistryError::InvalidImplementationRef { .. }
        | ProviderRegistryError::ModelProviderNotConfigured { .. }
        | ProviderRegistryError::AccountImplementationNotConfigured { .. }
        | ProviderRegistryError::AccountSettingsNotConfigured { .. }
        | ProviderRegistryError::FamilyImplementationNotConfigured { .. }
        | ProviderRegistryError::FamilyImplementationConflict { .. } => service_error(
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
            ExternalProviderDispatchError::provider_transport_diagnostic(
                kind.as_str(),
                format_transport_diagnostic(&error),
            ),
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
        ProviderClientError::ProviderCapability(error) => {
            let provider_error = error.error();
            service_error(ExternalProviderDispatchError::provider_failure(
                error.subcommand(),
                &provider_error.code,
                &provider_error.message,
                provider_error.diagnostics.clone(),
            ))
        }
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

    #[test]
    fn provider_capability_error_preserves_operation_code_and_message() {
        let error = map_provider_client_error(capability("invalid_request"));

        assert_eq!(
            error.to_string(),
            "external provider policy.evaluate failed: auth_expired: account token expired"
        );
    }
    #[test]
    fn transport_projection_redacts_sensitive_values_and_preserves_absence() {
        use oulipoly_provider::generated::ProcessStatus;
        use sha2::{Digest, Sha256};
        let secret = "credential=SECRET\n--argv-provider-output";
        let mut diagnostics = ProviderDiagnostics::with_description(format!(
            "first_failure: owned_wait: {secret}; errno=10; collection: {secret}"
        ));
        diagnostics.stdout.bytes = secret.as_bytes().to_vec();
        diagnostics.stderr.bytes = secret.as_bytes().to_vec();
        diagnostics.process_was_reaped = true;
        diagnostics.process_was_force_killed = true;
        diagnostics.provider_exit_code = Some(7);
        let error = ProviderClientError::host_transport(
            HostErrorKind::WaitFailed,
            secret,
            Some(secret.into()),
            diagnostics.clone(),
        )
        .with_process_context(
            diagnostics,
            ProcessStatus::SpawnError {
                reason: secret.into(),
            },
        );
        let output = map_provider_client_error(error.clone()).to_string();
        assert!(!output.contains("SECRET"));
        assert!(!output.contains("--argv"));
        assert!(output.contains("operation=redacted"));
        assert!(output.contains(&format!(
            "observed_request_id=sha256:{:x}",
            Sha256::digest(secret.as_bytes())
        )));
        assert!(output.contains("first_failure: owned_wait; errno=10; detail=redacted"));
        assert!(output.contains("collection: status=spawn_error:reason_redacted reaped=true force_killed=true nonzero=false exit_code=7"));
        assert!(output.len() < 700);
        assert_eq!(
            output,
            map_registry_error(ProviderRegistryError::ProviderTransport {
                kind: "wait_failed".into(),
                source: Box::new(error)
            })
            .to_string()
        );
        let absent = map_provider_client_error(transport(HostErrorKind::WaitFailed)).to_string();
        assert!(absent.contains("operation=policy.evaluate; observed_request_id=absent; first_failure: absent; collection: status=absent"));
        assert!(absent.contains("exit_code=absent"));
        assert_eq!(
            map_provider_client_error(transport(HostErrorKind::Cancelled)).to_string(),
            "external provider launch cancelled before final event"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn real_provider_wait_failure_projects_first_errno_and_missing_identity() {
        use oulipoly_provider::process::{
            ProcessCommand, ProcessLimits, ProcessRunner, ProcessSpawnObserver,
        };
        let limits = ProcessLimits {
            // Deliberate negative control: consume this exact child's status before
            // the provider owner polls. This is NOT a historical-cause claim.
            spawn_observer: Some(ProcessSpawnObserver::new(|pid| {
                let mut status = 0;
                assert_eq!(
                    unsafe { libc::waitpid(pid as i32, &mut status, 0) },
                    pid as i32
                );
                Ok(())
            })),
            ..ProcessLimits::default()
        };
        let error = ProcessRunner::new(limits)
            .run(
                ProcessCommand::new("/bin/true").arg("launch"),
                vec![],
                std::iter::empty::<(&str, &str)>(),
            )
            .expect_err("reaped child must expose real ECHILD");
        assert_eq!(error.transport_kind(), "wait_failed");
        assert!(!provider_client_error_is_rotatable(&error));
        let output = map_provider_client_error(error).to_string();
        assert!(
            output.contains(&format!(
                "first_failure: waitid_wnowait; errno={}; detail=redacted",
                libc::ECHILD
            )),
            "{output}"
        );
        assert!(output.contains("operation=launch; observed_request_id=absent"));
    }
    #[test]
    fn observed_wire_identity_hashes_verbatim_prefix_and_empty_is_not_absent() {
        use sha2::{Digest, Sha256};
        let ids = [
            None,
            Some(""),
            Some("external-provider-launch-same"),
            Some("external-provider-policy-same"),
        ];
        let outputs: Vec<_> = ids
            .iter()
            .map(|id| {
                map_provider_client_error(ProviderClientError::host_transport(
                    HostErrorKind::WaitFailed,
                    "launch",
                    id.map(str::to_owned),
                    ProviderDiagnostics::default(),
                ))
                .to_string()
            })
            .collect();
        for (index, id) in ids.iter().enumerate().skip(1) {
            assert!(outputs[index].contains(&format!(
                "sha256:{:x}",
                Sha256::digest(id.unwrap().as_bytes())
            )));
            assert_ne!(outputs[0], outputs[index]);
        }
        assert_ne!(outputs[2], outputs[3]);
    }
}
