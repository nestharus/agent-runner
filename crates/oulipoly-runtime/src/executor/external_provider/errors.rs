//! Role: formatter.

use oulipoly_provider::generated::Diagnostic;

#[derive(Debug, Clone)]
pub(crate) enum ExternalProviderDispatchError {
    MissingCapability { capability: &'static str },
    RuntimeDisabledCrate,
    ProviderTransport { category: String },
    ProviderProtocol { category: String },
    CancellationFallback { reason: String },
    PolicyRejected { diagnostics: Vec<Diagnostic> },
}

impl ExternalProviderDispatchError {
    pub(crate) fn missing_required_capability(capability: &'static str) -> Self {
        Self::MissingCapability { capability }
    }

    pub(crate) fn runtime_disabled_artifact() -> Self {
        Self::RuntimeDisabledCrate
    }

    pub(crate) fn provider_transport_failure(category: impl Into<String>) -> Self {
        Self::ProviderTransport {
            category: category.into(),
        }
    }

    pub(crate) fn provider_protocol_failure(category: impl Into<String>) -> Self {
        Self::ProviderProtocol {
            category: category.into(),
        }
    }

    pub(crate) fn cancellation_fallback(reason: impl Into<String>) -> Self {
        Self::CancellationFallback {
            reason: reason.into(),
        }
    }

    pub(crate) fn policy_rejected(diagnostics: Vec<Diagnostic>) -> Self {
        Self::PolicyRejected { diagnostics }
    }
}
