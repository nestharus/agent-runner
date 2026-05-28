pub mod support {
    pub mod provider_client;
}

use oulipoly_provider::client::{ProviderClient, ProviderClientOptions, ProviderOutputLimits};
use oulipoly_provider::resolver::ProviderArtifactRef;
use std::time::Duration;
use support::provider_client::{
    describe_request, fake_provider_source,
    testkit::{FakeProvider, FakeProviderMode},
};

#[test]
fn client_applies_stdout_and_stderr_caps_with_truncation_metadata() {
    let fake = FakeProvider::compile(fake_provider_source());
    let client = ProviderClient::new(
        ProviderArtifactRef::Path { path: fake.path() },
        ProviderClientOptions {
            output_limits: ProviderOutputLimits {
                stdout_bytes: 4096,
                stderr_bytes: 2048,
            },
            timeout: Duration::from_secs(3),
            ..ProviderClientOptions::default()
        },
    );

    let error = client
        .invoke_json(
            "describe",
            describe_request(),
            FakeProviderMode::LargeStdoutStderr.env(),
        )
        .expect_err("over-limit stdout should be a bounded protocol error");

    assert_eq!(error.transport_kind(), "stdout_limit_exceeded");
    assert_eq!(error.diagnostics().stdout.captured_len, 4096);
    assert!(error.diagnostics().stdout.truncated);
    assert_eq!(error.diagnostics().stderr.captured_len, 2048);
    assert!(error.diagnostics().stderr.truncated);
}

#[test]
fn client_does_not_deadlock_when_output_exceeds_limits_while_stdin_is_written() {
    let fake = FakeProvider::compile(fake_provider_source());
    let client = ProviderClient::new(
        ProviderArtifactRef::Path { path: fake.path() },
        ProviderClientOptions {
            output_limits: ProviderOutputLimits {
                stdout_bytes: 8192,
                stderr_bytes: 8192,
            },
            timeout: Duration::from_secs(3),
            ..ProviderClientOptions::default()
        },
    );

    let error = client
        .invoke_json(
            "describe",
            describe_request(),
            FakeProviderMode::PipePressure.env(),
        )
        .expect_err("pipe pressure should complete with a bounded error");

    assert_ne!(error.transport_kind(), "host_timeout");
    assert!(error.diagnostics().stdout.truncated);
    assert!(error.diagnostics().stderr.truncated);
}
