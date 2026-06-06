use oulipoly_provider::client::{ProcessSpawnObserver, ProviderClient, ProviderClientOptions};
use oulipoly_provider::resolver::ProviderArtifactRef;

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

    pub(crate) fn client_for_with_spawn_observer(
        &self,
        artifact: ProviderArtifactRef,
        observer: Option<ProcessSpawnObserver>,
    ) -> ProviderClient {
        ProviderClient::new(artifact, self.options.clone().with_spawn_observer(observer))
    }
}
