//! Role: orchestration.
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/external_provider/dispatch.rs
//!     role: intrinsic-surface
//!     Domain: external-provider dispatch orchestration
//!     Owns:
//!       - provider artifact lookup and capability gate sequence
//!       - sibling error_mapper coupling
//!       - sibling request_builder coupling
//!       - terminal-classify handoff hook
//!       - launch-result mapper handoff
//! ```

use super::capability_gate::gate_required_capabilities;
use super::client_invoker::{invoke_provider_launch, invoke_provider_policy};
use super::context::ExternalProviderDispatchContext;
use super::error_mapper::{
    invalid_provider_input_error, map_provider_client_error, map_registry_error,
    protocol_service_error, service_error,
};
use super::launch_result_mapper::map_launch_result_with_terminal_classification;
use super::policy_transform::apply_policy_transform;
use super::request_builder::{build_launch_candidate, build_launch_request, build_policy_request};
use super::terminal_classify_handoff::classify_after_launch_success;
use crate::executor::ExecutionResult;
use crate::provider_registry::ProviderRegistry;
use crate::services::ServiceError;

pub(crate) fn dispatch(
    registry: &ProviderRegistry,
    context: ExternalProviderDispatchContext,
) -> Result<ExecutionResult, ServiceError> {
    let artifact = registry
        .enabled_artifact_for_model(&context.model.name)
        .map_err(map_registry_error)?;
    let describe = registry
        .describe_model_provider(&context.model.name)
        .map_err(map_registry_error)?;
    gate_required_capabilities(&describe).map_err(service_error)?;

    let client = registry.client_factory().client_for(artifact);
    let candidate = build_launch_candidate(&context).map_err(invalid_provider_input_error)?;
    let policy_request = build_policy_request(&context, &candidate)
        .map_err(|_| protocol_service_error("schema_invalid_request"))?;
    let policy_result =
        invoke_provider_policy(&client, policy_request).map_err(map_provider_client_error)?;
    let candidate = apply_policy_transform(candidate, policy_result).map_err(service_error)?;
    let launch_request = build_launch_request(&context, &candidate)
        .map_err(|_| protocol_service_error("schema_invalid_request"))?;
    let launch_result =
        invoke_provider_launch(&client, launch_request).map_err(map_provider_client_error)?;
    let classification = classify_after_launch_success(registry, &context, &launch_result);

    Ok(map_launch_result_with_terminal_classification(
        launch_result,
        context.provider_index,
        &context.provider.name,
        classification,
    ))
}
