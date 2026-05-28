pub mod support {
    pub mod provider_client;
}

use oulipoly_provider::client::{CancellationToken, ProviderClient, ProviderClientOptions};
use oulipoly_provider::generated::ErrorCategory;
use oulipoly_provider::resolver::ProviderArtifactRef;
use std::time::Duration;
use support::provider_client::{
    describe_request, fake_provider_source,
    testkit::{FakeProvider, FakeProviderMode},
};

#[test]
fn client_error_precedence_table_matches_contract() {
    let rows = [
        (
            "exit_0_ok_true",
            FakeProviderMode::Success,
            PrecedenceExpectation::Success,
        ),
        (
            "exit_0_ok_false",
            FakeProviderMode::ProviderError,
            PrecedenceExpectation::Capability(ErrorCategory::Failed),
        ),
        (
            "nonzero_ok_false",
            FakeProviderMode::ProviderErrorNonzero,
            PrecedenceExpectation::Capability(ErrorCategory::Failed),
        ),
        (
            "nonzero_no_envelope",
            FakeProviderMode::ExitNonzeroNoEnvelope,
            PrecedenceExpectation::Transport("provider_process_nonzero"),
        ),
        (
            "nonzero_ok_true",
            FakeProviderMode::SuccessThenNonzero,
            PrecedenceExpectation::Transport("provider_process_nonzero_with_success"),
        ),
        (
            "invalid_envelope",
            FakeProviderMode::SchemaInvalidSuccess,
            PrecedenceExpectation::Transport("schema_invalid_response"),
        ),
        (
            "empty_stdout",
            FakeProviderMode::EmptyStdout,
            PrecedenceExpectation::Transport("empty_stdout"),
        ),
        (
            "invalid_json",
            FakeProviderMode::InvalidJson,
            PrecedenceExpectation::Transport("invalid_json"),
        ),
        (
            "host_timeout",
            FakeProviderMode::Sleep,
            PrecedenceExpectation::Transport("host_timeout"),
        ),
        (
            "host_cancellation",
            FakeProviderMode::SleepWithCancellation,
            PrecedenceExpectation::Transport("host_cancelled"),
        ),
        (
            "stdin_failure_ok_true",
            FakeProviderMode::EarlyStdinSuccess,
            PrecedenceExpectation::Success,
        ),
        (
            "stdin_failure_ok_false",
            FakeProviderMode::EarlyStdinError,
            PrecedenceExpectation::Capability(ErrorCategory::Failed),
        ),
        (
            "stdin_failure_no_envelope",
            FakeProviderMode::EarlyStdinEmpty,
            PrecedenceExpectation::Transport("provider_closed_stdin_early"),
        ),
        (
            "provider_side_timeout_category",
            FakeProviderMode::ProviderTimeoutError,
            PrecedenceExpectation::Capability(ErrorCategory::Timeout),
        ),
    ];

    for (label, mode, expected) in rows {
        let fake = FakeProvider::compile(fake_provider_source());
        let mut options = ProviderClientOptions::default()
            .with_timeout(Duration::from_millis(150))
            .with_kill_after_grace(Duration::from_millis(25));
        if label == "host_cancellation" {
            let token = CancellationToken::new();
            token.cancel_after(Duration::from_millis(25));
            options = options.with_cancellation(Some(token));
        }
        let client = ProviderClient::new(ProviderArtifactRef::Path { path: fake.path() }, options);
        let result = client.invoke_json("describe", describe_request(), mode.env());
        expected.assert_result(label, result);
    }
}

enum PrecedenceExpectation {
    Success,
    Capability(ErrorCategory),
    Transport(&'static str),
}

impl PrecedenceExpectation {
    fn assert_result(
        self,
        label: &str,
        result: Result<serde_json::Value, oulipoly_provider::error::ProviderClientError>,
    ) {
        match self {
            Self::Success => {
                let value = result.unwrap_or_else(|error| {
                    panic!("{label} expected success, got {error:?}");
                });
                assert_eq!(value["ok"], true, "{label}");
            }
            Self::Capability(category) => {
                let error = result.expect_err("row should produce an error");
                assert!(error.is_provider_capability(), "{label}: {error:?}");
                assert_eq!(error.provider_category(), Some(category), "{label}");
            }
            Self::Transport(kind) => {
                let error = result.expect_err("row should produce an error");
                assert!(error.is_host_transport_or_protocol(), "{label}: {error:?}");
                assert_eq!(error.transport_kind(), kind, "{label}");
            }
        }
    }
}
