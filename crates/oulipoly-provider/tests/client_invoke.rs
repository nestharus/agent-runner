pub mod support {
    pub mod provider_client;
}

use oulipoly_provider::client::{ProviderClient, ProviderClientOptions, ProviderOutputLimits};
use oulipoly_provider::error::ProviderClientError;
use oulipoly_provider::generated::DescribeResult;
use oulipoly_provider::resolver::ProviderArtifactRef;
use std::time::Duration;
use support::provider_client::{
    REQUEST_ID, describe_request, executable_script, fake_provider_source,
    read_recorded_invocation, temp_fixture_dir,
    testkit::{FakeProvider, FakeProviderMode},
};

#[test]
fn invoke_success_returns_typed_result_and_keeps_stderr_diagnostics() {
    let fake = FakeProvider::compile(fake_provider_source());
    let client = client_for(fake.path());
    let result: DescribeResult = client
        .invoke_typed(
            "describe",
            describe_request(),
            FakeProviderMode::SuccessStderr.env(),
        )
        .expect("valid success envelope should return typed result");

    assert_eq!(result.provider_id, "fake-provider");
    assert!(
        client
            .last_diagnostics()
            .stderr_text()
            .contains("diagnostic")
    );
}

#[test]
fn invoke_writes_request_on_stdin_not_argv() {
    let fake = FakeProvider::compile(fake_provider_source());
    let record = temp_fixture_dir("argv-record").join("record.txt");
    let client = client_for(fake.path());

    client
        .invoke_json(
            "describe",
            describe_request(),
            FakeProviderMode::RecordArgvStdin.env_with_record(&record),
        )
        .expect("recording invocation should succeed");

    let recorded = read_recorded_invocation(record);
    assert_eq!(
        client.last_invocation_argv(),
        vec![fake.path().into_os_string(), "describe".into()]
    );
    assert_eq!(recorded.argv.len(), 2);
    assert_eq!(recorded.argv[1], "describe");
    assert!(!recorded.argv.iter().any(|arg| arg.contains(REQUEST_ID)));
    assert!(
        recorded
            .stdin
            .contains("\"request_id\":\"request-example-001\"")
    );
}

#[test]
fn invoke_rejects_unknown_subcommand_before_spawn() {
    let fake = FakeProvider::compile(fake_provider_source());
    let client = client_for(fake.path());
    let error = client
        .invoke_json(
            "unknown.command",
            describe_request(),
            FakeProviderMode::Success.env(),
        )
        .expect_err("unknown subcommand should reject before spawn");

    assert_eq!(error.transport_kind(), "unknown_subcommand");
    assert!(!fake.was_spawned());
}

#[test]
fn invoke_rejects_schema_invalid_request_before_spawn() {
    let fake = FakeProvider::compile(fake_provider_source());
    let mut request = describe_request();
    request
        .as_object_mut()
        .expect("request should be object")
        .remove("contract");
    let client = client_for(fake.path());

    let error = client
        .invoke_json("describe", request, FakeProviderMode::Success.env())
        .expect_err("schema-invalid request should reject before spawn");

    assert_eq!(error.transport_kind(), "schema_invalid_request");
    assert!(!fake.was_spawned());
}

#[test]
fn invoke_rejects_invalid_stdout_protocol_shapes() {
    let fake = FakeProvider::compile(fake_provider_source());
    let client = client_for(fake.path());
    let cases = [
        (FakeProviderMode::EmptyStdout, "empty_stdout"),
        (FakeProviderMode::InvalidUtf8, "invalid_utf8"),
        (FakeProviderMode::NonObjectArray, "non_object_json"),
        (FakeProviderMode::NonObjectString, "non_object_json"),
        (FakeProviderMode::NonObjectNumber, "non_object_json"),
        (FakeProviderMode::MissingOk, "schema_invalid_response"),
        (FakeProviderMode::InvalidJson, "invalid_json"),
        (FakeProviderMode::MultipleJson, "multiple_json_objects"),
        (FakeProviderMode::LeadingLog, "leading_stdout_text"),
        (FakeProviderMode::TrailingJunk, "trailing_non_whitespace"),
        (FakeProviderMode::StderrEnvelopeOnly, "empty_stdout"),
    ];

    for (mode, expected_kind) in cases {
        let error = client
            .invoke_json("describe", describe_request(), mode.env())
            .expect_err("invalid stdout protocol shape should fail");
        assert_eq!(error.transport_kind(), expected_kind, "mode {mode:?}");
    }
}

#[test]
fn invoke_rejects_mismatched_contract_and_request_id() {
    let fake = FakeProvider::compile(fake_provider_source());
    let client = client_for(fake.path());
    for mode in [
        FakeProviderMode::MismatchedContract,
        FakeProviderMode::MismatchedRequestId,
    ] {
        let error = client
            .invoke_json("describe", describe_request(), mode.env())
            .expect_err("correlation mismatch should be protocol error");
        assert!(matches!(
            error,
            ProviderClientError::Protocol { .. } | ProviderClientError::Transport { .. }
        ));
        assert!(matches!(
            error.transport_kind(),
            "mismatched_contract" | "mismatched_request_id"
        ));
    }
}

#[test]
fn invoke_early_stdin_close_valid_ok_true_wins() {
    let fake = FakeProvider::compile(fake_provider_source());
    let client = client_for(fake.path());
    let result = client
        .invoke_json(
            "describe",
            describe_request(),
            FakeProviderMode::EarlyStdinSuccess.env(),
        )
        .expect("valid success envelope should win over broken stdin");

    assert_eq!(result["ok"], true);
    assert!(client.last_diagnostics().stdin_closed_early);
}

#[test]
fn invoke_early_stdin_close_valid_ok_false_wins_as_capability_error() {
    let fake = FakeProvider::compile(fake_provider_source());
    let client = client_for(fake.path());
    let error = client
        .invoke_json(
            "describe",
            describe_request(),
            FakeProviderMode::EarlyStdinError.env(),
        )
        .expect_err("valid provider error should win over broken stdin");

    assert!(error.is_provider_capability());
    assert_eq!(error.provider_error_code(), Some("example_early_stdin"));
}

#[test]
fn invoke_early_stdin_close_without_valid_envelope_is_transport_error() {
    let fake = FakeProvider::compile(fake_provider_source());
    let client = client_for(fake.path());
    let error = client
        .invoke_json(
            "describe",
            describe_request(),
            FakeProviderMode::EarlyStdinEmpty.env(),
        )
        .expect_err("early stdin close without envelope should be transport error");

    assert_eq!(error.transport_kind(), "provider_closed_stdin_early");
}

#[test]
fn invoke_script_uses_artifact_then_single_subcommand_arg() {
    let client = ProviderClient::new(
        ProviderArtifactRef::Script {
            path: executable_script(),
        },
        ProviderClientOptions::default(),
    );
    let result = client
        .invoke_json("describe", describe_request(), [])
        .expect("direct executable script should be invokable");

    assert_eq!(result["ok"], true);
    assert_eq!(client.last_invocation_argv().len(), 2);
    assert_eq!(client.last_invocation_argv()[1], "describe");
}

fn client_for(path: impl Into<std::path::PathBuf>) -> ProviderClient {
    ProviderClient::new(
        ProviderArtifactRef::Path { path: path.into() },
        ProviderClientOptions {
            output_limits: ProviderOutputLimits {
                stdout_bytes: 256 * 1024,
                stderr_bytes: 64 * 1024,
            },
            timeout: Duration::from_secs(3),
            ..ProviderClientOptions::default()
        },
    )
}
