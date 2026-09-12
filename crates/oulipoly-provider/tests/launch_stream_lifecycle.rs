pub mod support {
    pub mod provider_client;
}

use oulipoly_provider::client::{CancellationToken, ProviderClient, ProviderClientOptions};
use oulipoly_provider::generated::ProcessStatus;
use oulipoly_provider::resolver::ProviderArtifactRef;
use oulipoly_provider::stream::DecodedLaunchEvent;
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
fn launch_cancellation_cleans_descendants_and_preserves_stderr_diagnostics() {
    let fake = FakeProvider::compile(fake_provider_source());
    let leak_probe = LeakProbe::new();
    let token = CancellationToken::new();
    token.cancel_after(Duration::from_millis(500));
    let client = ProviderClient::new(
        ProviderArtifactRef::Path { path: fake.path() },
        ProviderClientOptions::default()
            .with_timeout(Duration::from_millis(50))
            .with_cancellation(Some(token))
            .with_kill_after_grace(Duration::from_millis(25)),
    );

    let error = client
        .launch(
            launch_request(),
            FakeProviderMode::ChildGrandchild.env_with_probe(&leak_probe),
        )
        .expect_err("explicit launch cancellation should fail");

    assert_eq!(error.transport_kind(), "host_cancelled");
    assert_eq!(
        error.request_id(),
        None,
        "host cancellation has no response envelope"
    );
    assert!(error.diagnostics().stderr.captured_len <= error.diagnostics().stderr.limit);
    leak_probe.assert_no_descendants();
}

#[test]
fn launch_heartbeats_do_not_cap_total_turn_runtime() {
    let fake = FakeProvider::compile(fake_provider_source());
    let client = short_handshake_client(fake.path(), Duration::from_millis(120));

    let result = client
        .launch(
            launch_request(),
            FakeProviderMode::LaunchHeartbeatsThenExit.env(),
        )
        .expect("optional heartbeat events remain accepted without a total launch deadline");

    assert_eq!(result.exit.status, ProcessStatus::Exited { code: 0 });
    assert!(
        result
            .events
            .iter()
            .filter(|event| matches!(event, DecodedLaunchEvent::Heartbeat { .. }))
            .count()
            >= 4
    );
}

#[test]
fn explicit_cancellation_kills_process_tree_after_stream_stalls() {
    let fake = FakeProvider::compile(fake_provider_source());
    let leak_probe = LeakProbe::new();
    let token = CancellationToken::new();
    token.cancel_after(Duration::from_millis(500));
    let client = ProviderClient::new(
        ProviderArtifactRef::Path { path: fake.path() },
        ProviderClientOptions::default()
            .with_timeout(Duration::from_millis(50))
            .with_cancellation(Some(token)),
    );

    let error = client
        .launch(
            launch_request(),
            FakeProviderMode::LaunchHeartbeatThenChildGrandchildHang.env_with_probe(&leak_probe),
        )
        .expect_err("explicit cancellation should stop the stalled launch");

    assert_eq!(error.transport_kind(), "host_cancelled");
    assert_eq!(
        error.request_id(),
        Some(REQUEST_ID),
        "stream cancellation retains the launch request identity"
    );
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
            .with_kill_after_grace(Duration::from_millis(200))
            .with_cancellation(cancellation),
    )
}

fn short_handshake_client(
    path: impl Into<std::path::PathBuf>,
    timeout: Duration,
) -> ProviderClient {
    ProviderClient::new(
        ProviderArtifactRef::Path { path: path.into() },
        ProviderClientOptions::default()
            .with_timeout(timeout)
            .with_kill_after_grace(Duration::from_millis(50)),
    )
}

#[test]
fn quiet_launch_outlives_handshake_budget_and_waits_for_actual_exit() {
    let fake = FakeProvider::compile(fake_provider_source());
    let client = short_handshake_client(fake.path(), Duration::from_millis(50));
    let started = std::time::Instant::now();
    let result = client
        .launch(
            launch_request(),
            FakeProviderMode::LaunchQuietThenExit.env(),
        )
        .expect("initial silence, inter-event silence, and post-final silence are valid");
    assert_eq!(result.exit.status, ProcessStatus::Exited { code: 0 });
    assert!(started.elapsed() >= Duration::from_millis(600));
    assert!(
        !result
            .events
            .iter()
            .any(|event| matches!(event, DecodedLaunchEvent::Heartbeat { .. }))
    );
}
