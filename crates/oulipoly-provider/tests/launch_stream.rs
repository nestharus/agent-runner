//! ## Declared roles
//!
//! Roles: validator, orchestration, mapper, accessor, predicate.
//!
//! - validator: launch-stream integration tests assert valid stream decoding,
//!   request-id correlation, model/provider nonzero handling, truncation,
//!   protocol diagnostics, and exact launch argv/stdin behavior.
//! - orchestration: each test compiles the fake provider, creates a
//!   `ProviderClient`, invokes `ProviderClient::launch`, and inspects the
//!   resulting success or transport error.
//! - mapper: `launch_client` maps a fixture binary path and provider-client
//!   options into the configured launch client under test.
//! - accessor: tests read `LaunchResult` event, exit, diagnostic, stdout/stderr,
//!   recorded-invocation, and last-argv surfaces.
//! - predicate: `matches!`, `contains`, and nonzero/truncation assertions
//!   classify expected event kinds and diagnostic states.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-provider/tests/launch_stream.rs
//!     role: adapter
//!     Translates:
//!       - fake-provider-fixture-contract
//!       - provider-client-options-contract
//!       - provider-cli-subprocess-contract
//!       - launch-jsonl-stream-contract
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-provider/tests/launch_stream.rs
//!     role: intrinsic-surface
//!     Domain: launch stream provider-client integration test suite
//!     Owns:
//!       - valid launch JSONL decoding and binary payload ordering coverage
//!       - request-id echo and exact argv/stdin invocation coverage
//!       - model exit versus provider process exit behavior coverage
//!       - launch stdout truncation precedence coverage
//!       - malformed protocol diagnostic and process-status preservation coverage
//! ```

pub mod support {
    pub mod provider_client;
}

use oulipoly_provider::client::{ProviderClient, ProviderClientOptions, ProviderOutputLimits};
use oulipoly_provider::error::ProviderClientError;
use oulipoly_provider::generated::{ProcessStatus, TerminalSignalKind};
use oulipoly_provider::resolver::ProviderArtifactRef;
use oulipoly_provider::stream::DecodedLaunchEvent;
use std::time::Duration;
use support::provider_client::{
    REQUEST_ID, fake_provider_source, launch_request, read_recorded_invocation, temp_fixture_dir,
    testkit::{FakeProvider, FakeProviderMode},
};

#[test]
fn launch_accepts_valid_jsonl_and_preserves_decoded_binary_order() {
    let fake = FakeProvider::compile(fake_provider_source());
    let client = launch_client(fake.path());
    let result = client
        .launch(launch_request(), FakeProviderMode::LaunchValid.env())
        .expect("valid launch event stream should succeed");

    assert_eq!(result.events.len(), 5);
    assert_eq!(
        result.events[0],
        DecodedLaunchEvent::Stdout {
            seq: 1,
            data: vec![0x00, 0x01, 0xff],
        }
    );
    assert_eq!(
        result.events[1],
        DecodedLaunchEvent::Stderr {
            seq: 2,
            data: b"err".to_vec(),
        }
    );
    assert!(matches!(
        result.events[2],
        DecodedLaunchEvent::Marker { seq: 3, .. }
    ));
    assert!(matches!(
        result.events[3],
        DecodedLaunchEvent::Heartbeat { seq: 4, .. }
    ));
    assert_eq!(result.exit.status, ProcessStatus::Exited { code: 0 });
}

#[test]
fn launch_fake_provider_echoes_host_supplied_request_id() {
    let fake = FakeProvider::compile(fake_provider_source());
    let mut request = launch_request();
    request["request_id"] = "request-host-generated-217".into();

    launch_client(fake.path())
        .launch(request, FakeProviderMode::LaunchValid.env())
        .expect("launch events should correlate with the host request id");
}

#[test]
fn launch_model_nonzero_final_exit_is_outcome_not_provider_transport_failure() {
    let fake = FakeProvider::compile(fake_provider_source());
    let result = launch_client(fake.path())
        .launch(launch_request(), FakeProviderMode::LaunchModelNonzero.env())
        .expect("model nonzero exit event is valid launch outcome");

    assert_eq!(result.exit.status, ProcessStatus::Exited { code: 9 });
    assert_eq!(
        result.exit.terminal_signal.kind,
        TerminalSignalKind::NonzeroExit
    );
    assert!(!result.diagnostics.provider_process_nonzero);
}

#[test]
fn launch_maps_provider_error_envelope_to_capability_error() {
    let fake = FakeProvider::compile(fake_provider_source());
    let error = launch_client(fake.path())
        .launch(
            launch_request(),
            FakeProviderMode::LaunchProviderError.env(),
        )
        .expect_err("launch provider error envelope should remain actionable");

    let ProviderClientError::ProviderCapability(error) = error else {
        panic!("launch provider error envelope must not collapse to a stream protocol error");
    };
    assert_eq!(error.subcommand(), "launch");
    assert_eq!(error.error().code, "launch_conflict");
    assert_eq!(error.error().message, "conflict from fake-provider");
    assert_eq!(
        error.process_status(),
        Some(&ProcessStatus::Exited { code: 2 })
    );
    assert!(error.diagnostics().provider_process_nonzero);
}

#[test]
fn launch_provider_nonzero_after_valid_final_exit_is_nonfatal_diagnostic() {
    let fake = FakeProvider::compile(fake_provider_source());
    let result = launch_client(fake.path())
        .launch(
            launch_request(),
            FakeProviderMode::LaunchProviderNonzeroAfterFinal.env(),
        )
        .expect("final exit event should remain authoritative");

    assert_eq!(result.exit.status, ProcessStatus::Exited { code: 0 });
    assert!(result.diagnostics.provider_process_nonzero);
    assert_eq!(result.diagnostics.provider_exit_code, Some(6));
}

#[test]
fn launch_accepts_valid_stream_larger_than_transport_capture_limit() {
    let fake = FakeProvider::compile(fake_provider_source());
    let result = launch_client(fake.path())
        .launch(
            launch_request(),
            FakeProviderMode::LaunchLongValidStream.env(),
        )
        .expect("valid launch streams must not fail on accumulated stdout volume");

    assert_eq!(result.exit.status, ProcessStatus::Exited { code: 0 });
    assert!(
        result.diagnostics.stdout.truncated,
        "diagnostics should record that bounded transport evidence was retained"
    );
}

#[test]
fn launch_provider_nonzero_without_final_exit_is_transport_error() {
    let fake = FakeProvider::compile(fake_provider_source());
    let error = launch_client(fake.path())
        .launch(
            launch_request(),
            FakeProviderMode::LaunchProviderNonzeroNoFinal.env(),
        )
        .expect_err("provider nonzero without final exit should fail transport");

    assert_eq!(error.transport_kind(), "missing_final_exit");
    assert!(error.diagnostics().provider_process_nonzero);
}

#[test]
fn launch_stdout_truncation_takes_precedence_over_parseable_exit_prefix() {
    let fake = FakeProvider::compile(fake_provider_source());
    let client = ProviderClient::new(
        ProviderArtifactRef::Path { path: fake.path() },
        ProviderClientOptions {
            output_limits: ProviderOutputLimits {
                stdout_bytes: 512,
                stderr_bytes: 64 * 1024,
            },
            ..ProviderClientOptions::default()
                .with_timeout(Duration::from_secs(3))
                .with_kill_after_grace(Duration::from_millis(50))
        },
    );

    let error = client
        .launch(
            launch_request(),
            FakeProviderMode::LaunchExitThenLargeStdout.env(),
        )
        .expect_err("truncated launch stdout must not be accepted as success");

    assert_eq!(error.transport_kind(), "stdout_limit_exceeded");
    assert!(error.diagnostics().stdout.truncated);
    assert!(error.diagnostics().process_was_reaped);
    assert!(error.process_status().is_some());
}

#[test]
fn launch_protocol_error_preserves_process_status_and_stderr_diagnostics() {
    let fake = FakeProvider::compile(fake_provider_source());
    let error = launch_client(fake.path())
        .launch(
            launch_request(),
            FakeProviderMode::LaunchMalformedLineStderr.env(),
        )
        .expect_err("malformed launch JSONL should fail protocol");

    assert_eq!(error.transport_kind(), "malformed_line");
    assert_eq!(
        error.process_status(),
        Some(&ProcessStatus::Exited { code: 0 })
    );
    assert!(error.diagnostics().stderr_text().contains("diagnostic"));
}

#[test]
fn launch_malformed_protocol_is_not_masked_by_provider_nonzero_exit() {
    let fake = FakeProvider::compile(fake_provider_source());
    let error = launch_client(fake.path())
        .launch(
            launch_request(),
            FakeProviderMode::LaunchMalformedLineNonzero.env(),
        )
        .expect_err("malformed launch JSONL should remain the primary error");

    assert_eq!(error.transport_kind(), "malformed_line");
    assert!(error.diagnostics().provider_process_nonzero);
    assert_eq!(
        error.process_status(),
        Some(&ProcessStatus::Exited { code: 8 })
    );
}

#[test]
fn launch_uses_exact_launch_argv_and_request_stdin_only() {
    let fake = FakeProvider::compile(fake_provider_source());
    let record = temp_fixture_dir("launch-argv-record").join("record.txt");
    let client = launch_client(fake.path());

    client
        .launch(
            launch_request(),
            FakeProviderMode::RecordArgvStdin.env_with_record(&record),
        )
        .expect("recording launch invocation should succeed");

    let recorded = read_recorded_invocation(record);
    assert_eq!(
        client.last_invocation_argv(),
        vec![fake.path().into_os_string(), "launch".into()]
    );
    assert_eq!(recorded.argv.len(), 2);
    assert_eq!(recorded.argv[1], "launch");
    assert!(!recorded.argv.iter().any(|arg| arg.contains(REQUEST_ID)));
    assert!(
        recorded
            .stdin
            .contains("\"request_id\":\"request-example-001\"")
    );
}

fn launch_client(path: impl Into<std::path::PathBuf>) -> ProviderClient {
    ProviderClient::new(
        ProviderArtifactRef::Path { path: path.into() },
        ProviderClientOptions::default()
            .with_timeout(Duration::from_secs(3))
            .with_kill_after_grace(Duration::from_millis(50)),
    )
}
