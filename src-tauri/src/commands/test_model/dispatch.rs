//! ## Declared roles
//!
//! `accessor`, `mapper`, `orchestration`

use oulipoly_config::ModelConfig;
use oulipoly_runtime::executor::ExecutionResult;
use oulipoly_runtime::services::{
    DiagnosticsServiceOutput, DiagnosticsServicePort, DiagnosticsServiceRequest,
    ExecutorServicePort, ExecutorServiceRequest, RoutingServicePort, RoutingServiceRequest,
};
use oulipoly_state::StateDb;
use oulipoly_state::repositories::ProviderQuotaRepository;

pub(crate) fn select_test_model_route(
    routing_service: &dyn RoutingServicePort,
    model: &ModelConfig,
    db: &StateDb,
) -> Result<usize, String> {
    routing_service
        .select_route(RoutingServiceRequest {
            model,
            state: db,
            ctx: None,
            provider_pin: None,
        })
        .map(|output| output.provider_index)
        .map_err(|error| error.to_string())
}

pub(crate) fn execute_effective_request(
    executor_service: &dyn ExecutorServicePort,
    request: ExecutorServiceRequest,
) -> Result<ExecutionResult, String> {
    executor_service
        .execute(request)
        .map(|output| output.result)
        .map_err(|error| error.to_string())
}

pub(crate) fn diagnostics_output_for_result(
    diagnostics_service: &dyn DiagnosticsServicePort,
    input: String,
) -> Result<DiagnosticsServiceOutput, String> {
    diagnostics_service
        .diagnose(DiagnosticsServiceRequest::ClassifyExhaustion { stderr: input })
        .map_err(|error| error.to_string())
}

pub(crate) fn mark_effective_provider_exhausted(
    db: &StateDb,
    provider_name: &str,
) -> Result<(), String> {
    <StateDb as ProviderQuotaRepository>::mark_exhausted(db, provider_name)
}
