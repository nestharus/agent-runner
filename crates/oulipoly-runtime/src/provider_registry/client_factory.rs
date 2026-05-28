use oulipoly_provider::client::{ProviderClient, ProviderClientOptions};
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
}
