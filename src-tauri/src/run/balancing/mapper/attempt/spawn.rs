use super::super::super::accessor::BalancedExecutionEnvironment;
use super::shared::AttemptLifecycleInput;
use crate::invocation::finalize::FinalizerGuard;
use crate::wiring;

pub(in crate::run::balancing) struct SpawnErrorInput<'a, 'state> {
    pub(in crate::run::balancing) agent_runtime_services: &'a wiring::AgentRuntimeServices,
    pub(in crate::run::balancing) env: &'a BalancedExecutionEnvironment,
    pub(in crate::run::balancing) invocation_id: &'a str,
    pub(in crate::run::balancing) invocation_row_id: i64,
    pub(in crate::run::balancing) guard: &'a mut FinalizerGuard<'state>,
    pub(in crate::run::balancing) provider_name: &'a str,
    pub(in crate::run::balancing) provider_session_id: Option<&'a str>,
    pub(in crate::run::balancing) error: String,
}

pub(in crate::run::balancing) fn spawn_error_input_for_attempt<'a, 'state>(
    lifecycle: AttemptLifecycleInput<'a, 'state>,
    provider: (&'a str, Option<&'a str>),
    error: String,
) -> SpawnErrorInput<'a, 'state> {
    let (agent_runtime_services, env, invocation, invocation_row_id, guard) = lifecycle;
    let (provider_name, provider_session_id) = provider;
    SpawnErrorInput {
        agent_runtime_services,
        env,
        invocation_id: &invocation.id,
        invocation_row_id,
        guard,
        provider_name,
        provider_session_id,
        error,
    }
}
