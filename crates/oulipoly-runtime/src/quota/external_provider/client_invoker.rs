//! ## Declared roles
//!
//! `accessor`

use oulipoly_provider::client::ProviderClient;
use oulipoly_provider::error::ProviderClientError;
use oulipoly_provider::generated::{QuotaProbeResult, QuotaRefreshAuthResult, QuotaSourceResult};
use serde_json::Value;

pub(crate) fn invoke_quota_source(
    client: &ProviderClient,
    request: Value,
) -> Result<QuotaSourceResult, ProviderClientError> {
    client.invoke_typed::<QuotaSourceResult, _>(
        "quota.source",
        request,
        Vec::<(String, String)>::new(),
    )
}

pub(crate) fn invoke_quota_probe(
    client: &ProviderClient,
    request: Value,
) -> Result<QuotaProbeResult, ProviderClientError> {
    client.invoke_typed::<QuotaProbeResult, _>(
        "quota.probe",
        request,
        Vec::<(String, String)>::new(),
    )
}

pub(crate) fn invoke_quota_refresh_auth(
    client: &ProviderClient,
    request: Value,
) -> Result<QuotaRefreshAuthResult, ProviderClientError> {
    client.invoke_typed::<QuotaRefreshAuthResult, _>(
        "quota.refresh_auth",
        request,
        Vec::<(String, String)>::new(),
    )
}
