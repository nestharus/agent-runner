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
    #[error("model has no configured provider endpoint: {model_name}")]
    ModelProviderNotConfigured { model_name: String },
    #[error("provider account has no explicit implementation endpoint: {account_name}")]
    AccountImplementationNotConfigured { account_name: String },
    #[error("provider account has no explicit settings identity: {account_name}")]
    AccountSettingsNotConfigured { account_name: String },
    #[error("provider family has no explicit bootstrap endpoint: {family}")]
    FamilyImplementationNotConfigured { family: String },
    #[error(
        "provider family {family} has conflicting executable authority from accounts {first_account} and {second_account}"
    )]
    FamilyImplementationConflict {
        family: String,
        first_account: String,
        second_account: String,
    },
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
