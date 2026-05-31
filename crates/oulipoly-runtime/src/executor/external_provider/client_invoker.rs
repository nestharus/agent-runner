//! Role: accessor.

use oulipoly_provider::client::ProviderClient;
use oulipoly_provider::error::ProviderClientError;
use oulipoly_provider::generated::PolicyEvaluateResult;
use oulipoly_provider::stream::LaunchResult;
use serde_json::Value;

pub(crate) fn invoke_provider_policy(
    client: &ProviderClient,
    request: Value,
) -> Result<PolicyEvaluateResult, ProviderClientError> {
    client.invoke_typed::<PolicyEvaluateResult, _>(
        "policy.evaluate",
        request,
        Vec::<(String, String)>::new(),
    )
}

pub(crate) fn invoke_provider_launch(
    client: &ProviderClient,
    request: Value,
) -> Result<LaunchResult, ProviderClientError> {
    client.launch(request, Vec::<(String, String)>::new())
}
