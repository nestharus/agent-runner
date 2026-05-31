//! ## Declared roles
//!
//! `formatter`

use super::errors::ExternalQuotaError;

pub(crate) fn format_external_quota_error(error: ExternalQuotaError) -> String {
    match error {
        ExternalQuotaError::MissingQuotaCapability => {
            "external provider missing required capability: quota".to_string()
        }
        ExternalQuotaError::RuntimeDisabledArtifact => {
            "external provider artifact is runtime-disabled: runtime_disabled".to_string()
        }
        ExternalQuotaError::RegistryLookup => {
            "external provider registry lookup failed: model_provider_not_configured".to_string()
        }
        ExternalQuotaError::ProviderTransport { category } => {
            format!("external provider quota transport failed: {category}")
        }
        ExternalQuotaError::ProviderProtocol { category } => {
            format!("external provider quota protocol failed: {category}")
        }
        ExternalQuotaError::ProviderFailed { code } => {
            format!("external provider quota failed: {code}")
        }
        ExternalQuotaError::ProbeUnavailable => {
            "external provider quota probe reported unavailable".to_string()
        }
        ExternalQuotaError::ProbeEmptyWindows => {
            "external provider quota probe returned empty windows".to_string()
        }
        ExternalQuotaError::ProbeRetryFailed {
            first,
            refresh_auth,
            retry,
        } => format_probe_retry_failed(*first, refresh_auth.map(|error| *error), *retry),
        ExternalQuotaError::Projection { reason } => {
            format!("external provider quota projection failed: {reason}")
        }
        ExternalQuotaError::SchemaInvalidRequest => {
            "external provider quota request failed schema validation".to_string()
        }
    }
}

fn format_probe_retry_failed(
    first: ExternalQuotaError,
    refresh_auth: Option<ExternalQuotaError>,
    retry: ExternalQuotaError,
) -> String {
    let first = format_external_quota_error(first);
    let retry = format_external_quota_error(retry);
    match refresh_auth {
        Some(refresh_auth) => {
            let refresh_auth = format_external_quota_error(refresh_auth);
            format!(
                "external provider quota retry failed: first={first}; refresh_auth={refresh_auth}; retry={retry}"
            )
        }
        None => {
            format!("external provider quota retry failed: first={first}; retry={retry}")
        }
    }
}
