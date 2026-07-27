use oulipoly_provider::client::{ProcessSpawnObserver, ProviderClient, ProviderClientOptions};
use oulipoly_provider::resolver::ProviderArtifactRef;
use oulipoly_provider::stream::LaunchEventObserver;

#[derive(Debug, Clone)]
pub struct ProviderClientFactory {
    options: ProviderClientOptions,
}

impl ProviderClientFactory {
    pub fn new(options: ProviderClientOptions) -> Self {
        Self { options }
    }

    pub fn client_for(&self, artifact: ProviderArtifactRef) -> ProviderClient {
        ProviderClient::new(artifact, self.options.clone())
    }

    pub(crate) fn client_for_with_observers(
        &self,
        artifact: ProviderArtifactRef,
        spawn_observer: Option<ProcessSpawnObserver>,
        launch_event_observer: Option<LaunchEventObserver>,
    ) -> ProviderClient {
        ProviderClient::new(
            artifact,
            self.options
                .clone()
                .with_spawn_observer(spawn_observer)
                .with_launch_event_observer(launch_event_observer),
        )
    }
}
