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

use crate::provider_registry::{ProviderRegistry, describe_provider_client};
use crate::services::{ServiceError, TerminalClassification, TerminalClassifyServiceRequest};
use oulipoly_provider::client::ProviderClient;
use oulipoly_provider::generated::DescribeResult;

pub(crate) use reason_format::fixed_reason_for_kind;

pub(crate) fn classify_terminal(
    registry: &ProviderRegistry,
    request: TerminalClassifyServiceRequest,
) -> Result<TerminalClassification, ServiceError> {
    let artifact = registry
        .enabled_artifact_for_model(&request.model_name)
        .map_err(error_mapper::registry_error)?;
    let client = registry.client_factory().client_for(artifact);
    let describe = describe_provider_client(&client, registry.host_options())
        .map_err(error_mapper::registry_error)?;
    classify_terminal_with_client(registry, &client, &describe, request)
}

pub(crate) fn classify_terminal_with_client(
    registry: &ProviderRegistry,
    client: &ProviderClient,
    describe: &DescribeResult,
    request: TerminalClassifyServiceRequest,
) -> Result<TerminalClassification, ServiceError> {
    capability_gate::validate_terminal_capability(describe)
        .map_err(error_mapper::classify_error)?;

    let provider_request =
        request_builder::build_terminal_classify_request(&request, registry.host_options())
            .map_err(error_mapper::projection_error)?;
    let result = client_invoker::invoke_terminal_classify(client, provider_request)
        .map_err(error_mapper::client_error)?;
    result_mapper::map_terminal_classify_result(&request, result)
        .map_err(error_mapper::classify_error)
}
