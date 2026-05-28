pub mod support {
    pub mod provider_client;
}

use oulipoly_provider::client::{CancellationToken, ProviderClient, ProviderClientOptions};
use oulipoly_provider::resolver::ProviderArtifactRef;
use std::time::Duration;
use support::provider_client::{
    REQUEST_ID, fake_provider_source, launch_request,
    testkit::{FakeProvider, FakeProviderMode, LeakProbe},
};

#[test]
fn launch_cancellation_with_provider_emitted_cancelled_exit_uses_final_event() {
    let fake = FakeProvider::compile(fake_provider_source());
    let token = CancellationToken::new();
    let client = launch_client(fake.path(), Some(token.clone()));
    token.cancel_after(Duration::from_millis(75));

    let result = client
        .launch(
            launch_request(),
            FakeProviderMode::LaunchCancelledFinalEvent.env(),
        )
        .expect("provider cancelled final exit should be authoritative");

    assert_eq!(result.exit.terminal_signal.kind.as_str(), "cancelled");
    assert!(result.diagnostics.host_cancellation_requested);
}

#[test]
fn launch_forced_kill_without_final_event_is_host_cancellation_not_missing_final() {
    let fake = FakeProvider::compile(fake_provider_source());
    let token = CancellationToken::new();
    let client = launch_client(fake.path(), Some(token.clone()));
    token.cancel_after(Duration::from_millis(75));

    let error = client
        .launch(launch_request(), FakeProviderMode::LaunchPartialHang.env())
        .expect_err("forced kill with no final event should be host cancellation");

    assert_eq!(error.transport_kind(), "host_cancelled");
    assert_eq!(error.request_id(), Some(REQUEST_ID));
    assert_ne!(error.transport_kind(), "missing_final_exit");
}

#[test]
fn launch_timeout_cleans_descendants_and_preserves_stderr_diagnostics() {
    let fake = FakeProvider::compile(fake_provider_source());
    let leak_probe = LeakProbe::new();
    let client = ProviderClient::new(
        ProviderArtifactRef::Path { path: fake.path() },
        ProviderClientOptions::default()
            .with_timeout(Duration::from_millis(150))
            .with_kill_after_grace(Duration::from_millis(25)),
    );

    let error = client
        .launch(
            launch_request(),
            FakeProviderMode::ChildGrandchild.env_with_probe(&leak_probe),
        )
        .expect_err("launch timeout should fail");

    assert_eq!(error.transport_kind(), "host_timeout");
    assert_eq!(error.request_id(), Some(REQUEST_ID));
    assert!(error.diagnostics().stderr.captured_len <= error.diagnostics().stderr.limit);
    leak_probe.assert_no_descendants();
}

fn launch_client(
    path: impl Into<std::path::PathBuf>,
    cancellation: Option<CancellationToken>,
) -> ProviderClient {
    ProviderClient::new(
        ProviderArtifactRef::Path { path: path.into() },
        ProviderClientOptions::default()
            .with_timeout(Duration::from_secs(5))
            .with_kill_after_grace(Duration::from_millis(25))
            .with_cancellation(cancellation),
    )
}
