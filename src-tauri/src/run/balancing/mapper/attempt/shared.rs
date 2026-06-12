use oulipoly_runtime::executor;
use oulipoly_state::CompositeInvocationId;

use super::super::super::accessor::BalancedExecutionEnvironment;
use crate::invocation::finalize::FinalizerGuard;
use crate::terminal_outcome_adapter::TerminalSignalContext;
use crate::wiring;

pub(in crate::run::balancing) type AttemptLifecycleInput<'a, 'state> = (
    &'a wiring::AgentRuntimeServices,
    &'a BalancedExecutionEnvironment,
    &'a CompositeInvocationId,
    i64,
    &'a mut FinalizerGuard<'state>,
);

pub(in crate::run::balancing) type AttemptProviderInput<'a> = (&'a str, usize, Option<&'a str>);

pub(in crate::run::balancing) type AttemptTerminalInput<'a, 'ctx> = (
    &'a executor::ExecutionResult,
    &'a Option<executor::TerminalSignal>,
    &'a mut TerminalSignalContext<'ctx, std::io::Stderr>,
);

pub(in crate::run::balancing) type AttemptBudgetInput = (usize, usize);
