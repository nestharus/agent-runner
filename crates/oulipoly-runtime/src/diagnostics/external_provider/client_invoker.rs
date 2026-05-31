//! Role: accessor.

use oulipoly_provider::client::ProviderClient;
use oulipoly_provider::error::ProviderClientError;
use oulipoly_provider::generated::TerminalClassifyResult;
use serde_json::Value;

pub(crate) fn invoke_terminal_classify(
    client: &ProviderClient,
    request: Value,
) -> Result<TerminalClassifyResult, ProviderClientError> {
    client.invoke_typed::<TerminalClassifyResult, _>(
        "terminal.classify",
        request,
        Vec::<(String, String)>::new(),
    )
}
