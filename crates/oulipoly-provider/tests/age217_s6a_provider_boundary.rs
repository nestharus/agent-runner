pub mod support {
    pub mod provider_client;
}

use oulipoly_provider::client::{CancellationToken, ProviderClient, ProviderClientOptions};
use oulipoly_provider::resolver::ProviderArtifactRef;
use std::time::Duration;
use support::provider_client::{
    REQUEST_ID, fake_provider_source, launch_request,
    testkit::{FakeProvider, FakeProviderMode},
};

#[test]
fn external_provider_launch_provider_client_token_cancellation_without_final_event_finalizes_at_adapter_seam()
 {
    let fake = FakeProvider::compile(fake_provider_source());
    let token = CancellationToken::new();
    let client = ProviderClient::new(
        ProviderArtifactRef::Path { path: fake.path() },
        ProviderClientOptions::default()
            .with_timeout(Duration::from_secs(5))
            .with_kill_after_grace(Duration::from_millis(25))
            .with_cancellation(Some(token.clone())),
    );
    token.cancel_after(Duration::from_millis(75));

    let error = client
        .launch(launch_request(), FakeProviderMode::LaunchPartialHang.env())
        .expect_err("host cancellation without final event should stay distinct");

    assert_eq!(error.transport_kind(), "host_cancelled");
    assert_eq!(error.request_id(), Some(REQUEST_ID));
    assert_ne!(error.transport_kind(), "missing_final_exit");
}
