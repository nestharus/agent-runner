use oulipoly_runtime::executor;
use oulipoly_state::CompositeInvocationId;

use super::super::super::accessor::BalancedExecutionEnvironment;
use super::super::super::disposition::TypedDispositionInput;
use super::shared::{
    AttemptBudgetInput, AttemptLifecycleInput, AttemptProviderInput, AttemptTerminalInput,
};
use crate::invocation::finalize::FinalizerGuard;
use crate::terminal_outcome_adapter::TerminalSignalContext;
use crate::wiring;

pub(in crate::run::balancing) struct TypedDispositionInputSource<'a, 'state, 'ctx> {
    pub(in crate::run::balancing) agent_runtime_services: &'a wiring::AgentRuntimeServices,
    pub(in crate::run::balancing) env: &'a BalancedExecutionEnvironment,
    pub(in crate::run::balancing) invocation: &'a CompositeInvocationId,
    pub(in crate::run::balancing) invocation_row_id: i64,
    pub(in crate::run::balancing) guard: &'a mut FinalizerGuard<'state>,
    pub(in crate::run::balancing) provider_name: &'a str,
    pub(in crate::run::balancing) provider_index: usize,
    pub(in crate::run::balancing) result: &'a executor::ExecutionResult,
    pub(in crate::run::balancing) terminal_signal: &'a Option<executor::TerminalSignal>,
    pub(in crate::run::balancing) terminal_signal_ctx:
        &'a mut TerminalSignalContext<'ctx, std::io::Stderr>,
    pub(in crate::run::balancing) zero_turn_provider_session_id: Option<&'a str>,
    pub(in crate::run::balancing) attempts: usize,
    pub(in crate::run::balancing) max_attempts: usize,
}

pub(in crate::run::balancing) fn typed_disposition_input<'a, 'state, 'ctx>(
    source: TypedDispositionInputSource<'a, 'state, 'ctx>,
) -> TypedDispositionInput<'a, 'state, 'ctx> {
    TypedDispositionInput {
        agent_runtime_services: source.agent_runtime_services,
        env: source.env,
        invocation: source.invocation,
        invocation_row_id: source.invocation_row_id,
        guard: source.guard,
        provider_name: source.provider_name,
        provider_index: source.provider_index,
        result: source.result,
        terminal_signal: source.terminal_signal,
        terminal_signal_ctx: source.terminal_signal_ctx,
        zero_turn_provider_session_id: source.zero_turn_provider_session_id,
        attempts: source.attempts,
        max_attempts: source.max_attempts,
    }
}

pub(in crate::run::balancing) fn typed_disposition_input_for_attempt<'a, 'state, 'ctx>(
    lifecycle: AttemptLifecycleInput<'a, 'state>,
    provider: AttemptProviderInput<'a>,
    terminal: AttemptTerminalInput<'a, 'ctx>,
    budget: AttemptBudgetInput,
) -> TypedDispositionInput<'a, 'state, 'ctx> {
    let (agent_runtime_services, env, invocation, invocation_row_id, guard) = lifecycle;
    let (provider_name, provider_index, zero_turn_provider_session_id) = provider;
    let (result, terminal_signal, terminal_signal_ctx) = terminal;
    let (attempts, max_attempts) = budget;
    typed_disposition_input(TypedDispositionInputSource {
        agent_runtime_services,
        env,
        invocation,
        invocation_row_id,
        guard,
        provider_name,
        provider_index,
        result,
        terminal_signal,
        terminal_signal_ctx,
        zero_turn_provider_session_id,
        attempts,
        max_attempts,
    })
}
