pub mod support {
    pub mod provider_client;
}

use oulipoly_provider::error::{
    HostErrorKind, ProviderCapabilityError, ProviderClientError, ProviderDiagnostics,
};
use oulipoly_provider::generated::{ErrorCategory, ProcessStatus};
use support::provider_client::{REQUEST_ID, describe_error_response};

#[test]
fn provider_capability_error_preserves_contract_error_object() {
    let envelope = describe_error_response("timeout", "example_timeout");
    let error = ProviderCapabilityError::from_valid_envelope(
        "describe",
        envelope,
        ProviderDiagnostics::default(),
        Some(ProcessStatus::Exited { code: 7 }),
    )
    .expect("valid provider error envelope should map to capability error");

    assert_eq!(error.request_id(), REQUEST_ID);
    assert_eq!(error.subcommand(), "describe");
    assert_eq!(error.error().category, ErrorCategory::Timeout);
    assert_eq!(error.error().code, "example_timeout");
    assert!(error.error().retryable);
    assert_eq!(
        error.process_status(),
        Some(&ProcessStatus::Exited { code: 7 })
    );
}

#[test]
fn provider_timeout_category_is_not_host_timeout() {
    let provider_error = ProviderClientError::from_capability(
        ProviderCapabilityError::from_valid_envelope(
            "describe",
            describe_error_response("timeout", "example_timeout"),
            ProviderDiagnostics::default(),
            Some(ProcessStatus::Exited { code: 0 }),
        )
        .expect("provider timeout envelope should be valid"),
    );

    assert!(provider_error.is_provider_capability());
    assert_eq!(
        provider_error.provider_category(),
        Some(ErrorCategory::Timeout)
    );
    assert_ne!(provider_error.transport_kind(), "host_timeout");
}

#[test]
fn host_timeout_is_transport_error_without_provider_category() {
    let error = ProviderClientError::host_transport(
        HostErrorKind::Timeout,
        "describe",
        Some(REQUEST_ID.to_owned()),
        ProviderDiagnostics::default(),
    );

    assert!(error.is_host_transport_or_protocol());
    assert_eq!(error.transport_kind(), "host_timeout");
    assert_eq!(error.provider_category(), None);
}

#[test]
fn host_cancellation_is_transport_error_without_provider_category() {
    let error = ProviderClientError::host_transport(
        HostErrorKind::Cancelled,
        "launch",
        Some(REQUEST_ID.to_owned()),
        ProviderDiagnostics::default(),
    );

    assert!(error.is_host_transport_or_protocol());
    assert_eq!(error.transport_kind(), "host_cancelled");
    assert_eq!(error.provider_category(), None);
}

#[test]
fn valid_error_envelope_takes_precedence_over_nonzero_exit() {
    let error = ProviderClientError::classify_non_launch(
        "describe",
        Some(describe_error_response("failed", "example_failed")),
        ProcessStatus::Exited { code: 9 },
        ProviderDiagnostics::default(),
        None,
    )
    .expect_err("ok=false envelope should surface as provider capability error result");

    assert!(error.is_provider_capability());
    assert_eq!(error.provider_category(), Some(ErrorCategory::Failed));
    assert_eq!(
        error.process_status(),
        Some(&ProcessStatus::Exited { code: 9 })
    );
}

#[test]
fn missing_valid_error_envelope_with_nonzero_exit_is_transport_error() {
    let error = ProviderClientError::classify_non_launch(
        "describe",
        None,
        ProcessStatus::Exited { code: 9 },
        ProviderDiagnostics::default(),
        None,
    )
    .expect_err("nonzero without valid error envelope should be transport failure");

    assert!(error.is_host_transport_or_protocol());
    assert_eq!(error.transport_kind(), "provider_process_nonzero");
}
