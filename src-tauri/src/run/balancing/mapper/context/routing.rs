use oulipoly_config::{ModelConfig, ProvidersConfig, SessionsConfig};
use oulipoly_runtime::balancer;
use oulipoly_runtime::quota::InFlight;
use oulipoly_runtime::services::RoutingServiceRequest;
use oulipoly_state::StateDb;

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
