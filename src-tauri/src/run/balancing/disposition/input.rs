//! Declared roles: mapper

use oulipoly_runtime::executor;
use oulipoly_state::CompositeInvocationId;

use super::super::accessor::BalancedExecutionEnvironment;
use crate::invocation::finalize::FinalizerGuard;
use crate::terminal_outcome_adapter::TerminalSignalContext;
use crate::wiring;
use crate::zero_turn_orchestration::ZeroTurnAction;

pub(in crate::run::balancing) struct TypedDispositionInput<'a, 'state, 'ctx> {
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

pub(in crate::run::balancing) struct MaybeQuotaVerifyInput<'a, 'state, 'ctx> {
    pub(in crate::run::balancing) typed: TypedDispositionInput<'a, 'state, 'ctx>,
    pub(in crate::run::balancing) zero_turn_action: ZeroTurnAction,
    pub(in crate::run::balancing) pending_same_provider_verification:
        &'a mut Option<(usize, Option<String>)>,
    pub(in crate::run::balancing) signal_already_applied: bool,
}
