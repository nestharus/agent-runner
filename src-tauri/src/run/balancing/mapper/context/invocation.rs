use oulipoly_config::ModelConfig;
use oulipoly_runtime::services::InvocationLifecycleStartRequest;
use oulipoly_state::{CompositeInvocationId, InvocationStart, StateDb};
use uuid::Uuid;

pub(in crate::run::balancing) fn composite_invocation_id(
    provider_name: &str,
) -> CompositeInvocationId {
    CompositeInvocationId {
        source: provider_name.to_string(),
        id: Uuid::new_v4().to_string(),
    }
}

pub(in crate::run::balancing) fn balanced_invocation_start(
    invocation: &CompositeInvocationId,
    model: &ModelConfig,
    provider_name: &str,
    provider_index: usize,
    parent_invocation_id: Option<i64>,
) -> InvocationStart {
    InvocationStart {
        invocation_uuid: invocation.id.clone(),
        model_name: model.name.clone(),
        provider_name: provider_name.to_string(),
        provider_index,
        parent_invocation_id,
    }
}

pub(in crate::run::balancing) fn invocation_lifecycle_start_request<'a>(
    state: &'a StateDb,
    start: &'a InvocationStart,
) -> InvocationLifecycleStartRequest<'a> {
    InvocationLifecycleStartRequest { state, start }
}
