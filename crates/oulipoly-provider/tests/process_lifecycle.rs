pub mod support {
    pub mod provider_client;
}

use oulipoly_provider::client::{
    CancellationToken, ProviderClient, ProviderClientOptions, ProviderOutputLimits,
};
use oulipoly_provider::resolver::ProviderArtifactRef;
use std::time::Duration;
use support::provider_client::{
    REQUEST_ID, describe_request, fake_provider_source, launch_request,
    testkit::{FakeProvider, FakeProviderMode, LeakProbe},
};

#[test]
fn client_process_substrate_closes_stdin_and_drains_stderr() {
    let fake = FakeProvider::compile(fake_provider_source());
    let client = client_for(fake.path());

    client
        .invoke_json(
            "describe",
            describe_request(),
            FakeProviderMode::StdinEof.env(),
        )
        .expect("stdin eof fixture should complete");

    assert!(
        client
            .last_diagnostics()
            .stderr_text()
            .contains("observed stdin eof")
    );
}

#[test]
fn client_process_substrate_does_not_deadlock_under_pipe_pressure() {
    let fake = FakeProvider::compile(fake_provider_source());
    let client = ProviderClient::new(
        ProviderArtifactRef::Path { path: fake.path() },
        ProviderClientOptions {
            output_limits: ProviderOutputLimits {
                stdout_bytes: 64 * 1024,
                stderr_bytes: 64 * 1024,
            },
            ..ProviderClientOptions::default().with_timeout(Duration::from_secs(3))
        },
    );

    let error = client
        .invoke_json(
            "describe",
            describe_request(),
            FakeProviderMode::PipePressure.env(),
        )
        .expect_err("large pipe-pressure stdout should hit the stdout cap after draining");

    assert_eq!(error.transport_kind(), "stdout_limit_exceeded");
    assert!(error.diagnostics().stdout.truncated);
    assert!(error.diagnostics().stderr.truncated);
}

#[test]
fn client_timeout_kills_descendants_observed_by_probe() {
    let fake = FakeProvider::compile(fake_provider_source());
    let leak_probe = LeakProbe::new();
    let client = ProviderClient::new(
        ProviderArtifactRef::Path { path: fake.path() },
        ProviderClientOptions::default()
            .with_timeout(Duration::from_millis(150))
            .with_kill_after_grace(Duration::from_millis(50)),
    );

    let error = client
        .invoke_json(
            "describe",
            describe_request(),
            FakeProviderMode::ChildGrandchild.env_with_probe(&leak_probe),
        )
        .expect_err("sleeping process tree should time out");

    assert_eq!(error.transport_kind(), "host_timeout");
    assert_eq!(error.request_id(), Some(REQUEST_ID));
    leak_probe.assert_no_descendants();
}

#[test]
fn client_cancellation_kills_descendants_observed_by_probe() {
    let fake = FakeProvider::compile(fake_provider_source());
    let leak_probe = LeakProbe::new();
    let token = CancellationToken::new();
    let client = ProviderClient::new(
        ProviderArtifactRef::Path { path: fake.path() },
        ProviderClientOptions::default()
            .with_timeout(Duration::from_secs(30))
            .with_kill_after_grace(Duration::from_millis(50))
            .with_cancellation(Some(token.clone())),
    );
    let probe_for_cancellation = leak_probe.clone();
    let cancellation = std::thread::spawn(move || {
        probe_for_cancellation.wait_for_descendants();
        token.cancel();
    });

    let error = client
        .launch(
            launch_request(),
            FakeProviderMode::SigtermResistantChildGrandchild.env_with_probe(&leak_probe),
        )
        .expect_err("cancelled process tree should return cancellation");
    cancellation
        .join()
        .expect("cancellation thread should complete");

    assert_eq!(error.transport_kind(), "host_cancelled");
    assert_eq!(error.request_id(), Some(REQUEST_ID));
    assert!(error.diagnostics().process_was_force_killed);
    assert!(error.diagnostics().process_was_reaped);
    leak_probe.assert_no_descendants();
}

#[test]
fn client_cancellation_gracefully_reaps_sigterm_respecting_provider() {
    let fake = FakeProvider::compile(fake_provider_source());
    let token = CancellationToken::new();
    let client = ProviderClient::new(
        ProviderArtifactRef::Path { path: fake.path() },
        ProviderClientOptions::default()
            .with_timeout(Duration::from_secs(30))
            .with_kill_after_grace(Duration::from_millis(200))
            .with_cancellation(Some(token.clone())),
    );
    token.cancel_after(Duration::from_millis(100));

    let error = client
        .invoke_json(
            "describe",
            describe_request(),
            FakeProviderMode::Sleep.env(),
        )
        .expect_err("cancelled SIGTERM-respecting process should return cancellation");

    assert_eq!(error.transport_kind(), "host_cancelled");
    assert_eq!(error.request_id(), Some(REQUEST_ID));
    assert!(!error.diagnostics().process_was_force_killed);
    assert!(error.diagnostics().process_was_reaped);
}

fn client_for(path: impl Into<std::path::PathBuf>) -> ProviderClient {
    ProviderClient::new(
        ProviderArtifactRef::Path { path: path.into() },
        ProviderClientOptions::default().with_timeout(Duration::from_secs(3)),
    )
}
