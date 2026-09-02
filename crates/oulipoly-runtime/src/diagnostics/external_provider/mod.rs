//! Role: orchestration.

mod capability_gate;
mod client_invoker;
mod error_format;
mod error_mapper;
mod errors;
mod reason_format;
mod request_builder;
mod result_mapper;
mod status_projection;

use crate::provider_registry::ProviderRegistry;
use crate::services::{ServiceError, TerminalClassification, TerminalClassifyServiceRequest};
use oulipoly_provider::client::ProviderClient;
use oulipoly_provider::generated::DescribeResult;

pub(crate) use reason_format::fixed_reason_for_kind;

pub(crate) fn classify_terminal(
    registry: &ProviderRegistry,
    request: TerminalClassifyServiceRequest,
) -> Result<TerminalClassification, ServiceError> {
    let endpoint = registry
        .preflight_account(&request.provider_name)
        .map_err(error_mapper::registry_error)?;
    let settings_id = endpoint
        .settings_id()
        .map_err(error_mapper::registry_error)?;
    if settings_id != request.settings_id {
        return Err(error_mapper::settings_identity_mismatch());
    }
    classify_terminal_with_client(
        registry,
        endpoint.client(),
        endpoint.capabilities(),
        request,
    )
}

pub(crate) fn classify_terminal_with_client(
    registry: &ProviderRegistry,
    client: &ProviderClient,
    describe: &DescribeResult,
    request: TerminalClassifyServiceRequest,
) -> Result<TerminalClassification, ServiceError> {
    capability_gate::validate_terminal_capability(describe)
        .map_err(error_mapper::classify_error)?;

    let provider_request = request_builder::build_terminal_classify_request(
        &request,
        &format!("{}-instance", describe.provider_id),
        registry.host_options(),
    )
    .map_err(error_mapper::projection_error)?;
    let result = client_invoker::invoke_terminal_classify(client, provider_request)
        .map_err(error_mapper::client_error)?;
    result_mapper::map_terminal_classify_result(&request, result)
        .map_err(error_mapper::classify_error)
}
