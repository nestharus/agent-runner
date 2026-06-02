//! Role: orchestration.

use oulipoly_provider::client::ProviderClient;
use oulipoly_provider::error::ProviderClientError;
use oulipoly_provider::generated::{SessionExportResult, SessionReplaceResult};
use serde_json::Value;

pub(crate) fn invoke_export(
    client: &ProviderClient,
    request: Value,
) -> Result<SessionExportResult, ProviderClientError> {
    client.invoke_typed("session.export", request, Vec::<(String, String)>::new())
}

pub(crate) fn invoke_replace(
    client: &ProviderClient,
    request: Value,
) -> Result<SessionReplaceResult, ProviderClientError> {
    client.invoke_typed("session.replace", request, Vec::<(String, String)>::new())
}
