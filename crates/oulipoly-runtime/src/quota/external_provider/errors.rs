//! ## Declared roles
//!
//! `mapper`, `accessor`

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExternalQuotaError {
    MissingQuotaCapability,
    RuntimeDisabledArtifact,
    RegistryLookup,
    ProviderTransport {
        category: String,
    },
    ProviderProtocol {
        category: String,
    },
    ProviderFailed {
        code: String,
    },
    ProbeUnavailable,
    ProbeEmptyWindows,
    ProbeRetryFailed {
        first: Box<ExternalQuotaError>,
        refresh_auth: Option<Box<ExternalQuotaError>>,
        retry: Box<ExternalQuotaError>,
    },
    Projection {
        reason: &'static str,
    },
    SchemaInvalidRequest,
}

impl ExternalQuotaError {
    pub(crate) fn missing_quota_capability() -> Self {
        Self::MissingQuotaCapability
    }

    pub(crate) fn runtime_disabled_artifact() -> Self {
        Self::RuntimeDisabledArtifact
    }

    pub(crate) fn registry_lookup() -> Self {
        Self::RegistryLookup
    }

    pub(crate) fn provider_transport(category: impl Into<String>) -> Self {
        Self::ProviderTransport {
            category: category.into(),
        }
    }

    pub(crate) fn provider_protocol(category: impl Into<String>) -> Self {
        Self::ProviderProtocol {
            category: category.into(),
        }
    }

    pub(crate) fn provider_failed(code: impl Into<String>) -> Self {
        Self::ProviderFailed { code: code.into() }
    }

    pub(crate) fn probe_unavailable() -> Self {
        Self::ProbeUnavailable
    }

    pub(crate) fn probe_empty_windows() -> Self {
        Self::ProbeEmptyWindows
    }

    pub(crate) fn probe_retry_failed(
        first: ExternalQuotaError,
        refresh_auth: Option<ExternalQuotaError>,
        retry: ExternalQuotaError,
    ) -> Self {
        Self::ProbeRetryFailed {
            first: Box::new(first),
            refresh_auth: refresh_auth.map(Box::new),
            retry: Box::new(retry),
        }
    }

    pub(crate) fn projection(reason: &'static str) -> Self {
        Self::Projection { reason }
    }

    pub(crate) fn schema_invalid_request() -> Self {
        Self::SchemaInvalidRequest
    }
}
