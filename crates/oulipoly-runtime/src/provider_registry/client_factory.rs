use crate::executor::cli::spawn_identity::PARENT_INVOCATION_ENV;
use oulipoly_core::AutoWakeEnvironmentVariable;
use oulipoly_provider::client::{
    CancellationToken, ProcessSpawnObserver, ProviderClient, ProviderClientOptions,
};
use oulipoly_provider::resolver::ProviderArtifactRef;
use oulipoly_provider::stream::LaunchEventObserver;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct ProviderClientFactory {
    options: ProviderClientOptions,
}

impl ProviderClientFactory {
    pub fn new(options: ProviderClientOptions) -> Self {
        Self {
            options: options.with_environment_removals(provider_process_environment_removals()),
        }
    }

    pub fn client_for(&self, artifact: ProviderArtifactRef) -> ProviderClient {
        ProviderClient::new(artifact, self.options.clone())
    }

    pub(crate) fn client_from_pinned_with_observers(
        &self,
        pinned: &ProviderClient,
        spawn_observer: Option<ProcessSpawnObserver>,
        launch_event_observer: Option<LaunchEventObserver>,
    ) -> Result<ProviderClient, oulipoly_provider::error::ProviderClientError> {
        pinned.fork_from_pinned(
            self.options
                .clone()
                .with_spawn_observer(spawn_observer)
                .with_launch_event_observer(launch_event_observer),
        )
    }

    pub(crate) fn client_from_pinned_with_cancellation(
        &self,
        pinned: &ProviderClient,
        cancellation: &CancellationToken,
    ) -> Result<ProviderClient, oulipoly_provider::error::ProviderClientError> {
        pinned.fork_from_pinned(
            self.options
                .clone()
                .with_cancellation(Some(cancellation.clone())),
        )
    }

    pub(crate) fn client_from_pinned_with_cancellation_and_timeout(
        &self,
        pinned: &ProviderClient,
        cancellation: &CancellationToken,
        timeout: Duration,
    ) -> Result<ProviderClient, oulipoly_provider::error::ProviderClientError> {
        pinned.fork_from_pinned(
            self.options
                .clone()
                .with_cancellation(Some(cancellation.clone()))
                .with_timeout(timeout),
        )
    }
}

fn provider_process_environment_removals() -> impl Iterator<Item = &'static str> {
    AutoWakeEnvironmentVariable::ALL
        .into_iter()
        .map(AutoWakeEnvironmentVariable::name)
        .chain([
            oulipoly_state::COMPLETION_REGISTRATION_AUTHORITY_ENV,
            PARENT_INVOCATION_ENV,
        ])
}
