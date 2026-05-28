use oulipoly_config::provider_implementation_ref::ProviderImplementationRefError;
use oulipoly_provider::error::ProviderClientError;
use oulipoly_provider::generated::ErrorCategory;
use oulipoly_provider::resolver::RuntimeDisabledArtifact;

#[derive(Debug, thiserror::Error)]
pub enum ProviderRegistryError {
    #[error("invalid provider implementation ref")]
    InvalidImplementationRef {
        source: ProviderImplementationRefError,
    },
    #[error("model has no configured provider implementation ref: {model_name}")]
    ModelProviderNotConfigured { model_name: String },
    #[error("provider artifact is runtime-disabled: {kind}")]
    RuntimeDisabledArtifact {
        kind: String,
        artifact: RuntimeDisabledArtifact,
    },
    #[error("provider transport failed: {kind}")]
    ProviderTransport {
        kind: String,
        source: Box<ProviderClientError>,
    },
    #[error("provider protocol failed: {kind}")]
    ProviderProtocol {
        kind: String,
        source: Box<ProviderClientError>,
    },
    #[error("provider describe failed: {code}")]
    ProviderDescribeFailed {
        category: ErrorCategory,
        code: String,
        source: Box<ProviderClientError>,
    },
}
