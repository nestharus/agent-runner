use std::path::PathBuf;

use oulipoly_config::{ModelConfig, ProvidersConfig, SessionsConfig};
use oulipoly_runtime::balancer;
use oulipoly_runtime::quota::InFlight;
use oulipoly_runtime::services::{InvocationLifecycleStartRequest, RoutingServiceRequest};
use oulipoly_state::{CompositeInvocationId, InvocationStart, ProviderSessionBinding, StateDb};
use uuid::Uuid;

use super::super::accessor::BalancedExecutionEnvironment;

pub(in crate::run::balancing) struct BalancedConfigTomlPaths {
    pub(in crate::run::balancing) providers_path: PathBuf,
    pub(in crate::run::balancing) sessions_path: PathBuf,
}

pub(in crate::run::balancing) fn balanced_config_toml_paths(
    config_root: PathBuf,
) -> BalancedConfigTomlPaths {
    BalancedConfigTomlPaths {
        providers_path: config_root.join("providers.toml"),
        sessions_path: config_root.join("sessions.toml"),
    }
}

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

pub(in crate::run::balancing) fn diagnostic_exhaustion_category(input: &str) -> Option<String> {
    super::super::predicate::diagnostic_input_is_exhaustion(input)
        .then(crate::quota_zero_turn::quota_exhausted_category)
}

pub(in crate::run::balancing) fn balanced_execution_environment(
    state: StateDb,
    providers_cfg: ProvidersConfig,
    sessions_cfg: SessionsConfig,
    models_dir: PathBuf,
) -> BalancedExecutionEnvironment {
    BalancedExecutionEnvironment {
        state,
        providers_cfg,
        sessions_cfg,
        models_dir,
    }
}

pub(in crate::run::balancing) fn balance_context<'a>(
    providers_cfg: &'a ProvidersConfig,
    sessions_cfg: &'a SessionsConfig,
    in_flight: &'a InFlight,
) -> balancer::BalanceContext<'a> {
    balancer::BalanceContext {
        providers_cfg,
        sessions_cfg,
        in_flight,
    }
}

pub(in crate::run::balancing) fn routing_service_request<'a>(
    model: &'a ModelConfig,
    state: &'a StateDb,
    ctx: &'a balancer::BalanceContext<'a>,
) -> RoutingServiceRequest<'a> {
    RoutingServiceRequest {
        model,
        state,
        ctx: Some(ctx),
    }
}

pub(in crate::run::balancing) fn quota_retry_budget(model: &ModelConfig) -> usize {
    model.providers.len().max(1) + 1
}

pub(in crate::run::balancing) fn invocation_lifecycle_start_request<'a>(
    state: &'a StateDb,
    start: &'a InvocationStart,
) -> InvocationLifecycleStartRequest<'a> {
    InvocationLifecycleStartRequest { state, start }
}

pub(in crate::run::balancing) fn provider_session_binding(
    provider_session_id: &str,
) -> ProviderSessionBinding {
    ProviderSessionBinding {
        provider_session_id: provider_session_id.to_string(),
        capture_method: "forced_flag_verified",
        resume_input_id: None,
        provider_session_resolved_account: None,
    }
}

pub(in crate::run::balancing) fn pending_same_provider_verification_session_id(
    provider_session_id: Option<&str>,
) -> Option<String> {
    provider_session_id.map(str::to_string)
}

pub(in crate::run::balancing) fn pending_same_provider_verification(
    provider_index: usize,
    provider_session_id: Option<&str>,
) -> (usize, Option<String>) {
    (
        provider_index,
        pending_same_provider_verification_session_id(provider_session_id),
    )
}

pub(in crate::run::balancing) fn model_provider_names(model: &ModelConfig) -> Vec<String> {
    model
        .providers
        .iter()
        .map(|provider| provider.name.clone())
        .collect()
}
