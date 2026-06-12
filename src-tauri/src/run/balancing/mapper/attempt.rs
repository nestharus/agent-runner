use std::collections::HashMap;
use std::path::Path;

use oulipoly_config::ModelConfig;
use oulipoly_runtime::executor;
use oulipoly_state::CompositeInvocationId;

use super::super::accessor::BalancedExecutionEnvironment;
use super::super::disposition::{MaybeQuotaVerifyInput, TypedDispositionInput};
use super::super::finalization::CompletedAttemptInput;
use crate::invocation::finalize::FinalizerGuard;
use crate::terminal_outcome_adapter::TerminalSignalContext;
use crate::wiring;
use crate::zero_turn_orchestration::ZeroTurnAction;

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

pub(in crate::run::balancing) fn maybe_quota_verify_input<'a, 'state, 'ctx>(
    typed: TypedDispositionInput<'a, 'state, 'ctx>,
    zero_turn_action: ZeroTurnAction,
    pending_same_provider_verification: &'a mut Option<(usize, Option<String>)>,
    signal_already_applied: bool,
) -> MaybeQuotaVerifyInput<'a, 'state, 'ctx> {
    MaybeQuotaVerifyInput {
        typed,
        zero_turn_action,
        pending_same_provider_verification,
        signal_already_applied,
    }
}

pub(in crate::run::balancing) fn maybe_quota_verify_input_for_attempt<'a, 'state, 'ctx>(
    lifecycle: AttemptLifecycleInput<'a, 'state>,
    provider: AttemptProviderInput<'a>,
    terminal: AttemptTerminalInput<'a, 'ctx>,
    budget: AttemptBudgetInput,
    zero_turn_action: ZeroTurnAction,
    pending_same_provider_verification: &'a mut Option<(usize, Option<String>)>,
    signal_already_applied: bool,
) -> MaybeQuotaVerifyInput<'a, 'state, 'ctx> {
    maybe_quota_verify_input(
        typed_disposition_input_for_attempt(lifecycle, provider, terminal, budget),
        zero_turn_action,
        pending_same_provider_verification,
        signal_already_applied,
    )
}

pub(in crate::run::balancing) struct CompletedAttemptInputSource<'a, 'state, 'ctx> {
    pub(in crate::run::balancing) agent_runtime_services: &'a wiring::AgentRuntimeServices,
    pub(in crate::run::balancing) env: &'a BalancedExecutionEnvironment,
    pub(in crate::run::balancing) invocation: &'a CompositeInvocationId,
    pub(in crate::run::balancing) invocation_row_id: i64,
    pub(in crate::run::balancing) guard: &'a mut FinalizerGuard<'state>,
    pub(in crate::run::balancing) provider_name: &'a str,
    pub(in crate::run::balancing) model: &'a ModelConfig,
    pub(in crate::run::balancing) provider_index: usize,
    pub(in crate::run::balancing) result: &'a executor::ExecutionResult,
    pub(in crate::run::balancing) terminal_signal: &'a Option<executor::TerminalSignal>,
    pub(in crate::run::balancing) terminal_signal_ctx:
        &'a mut TerminalSignalContext<'ctx, std::io::Stderr>,
    pub(in crate::run::balancing) all_models: &'a HashMap<String, ModelConfig>,
    pub(in crate::run::balancing) working_dir: Option<&'a Path>,
    pub(in crate::run::balancing) zero_turn_provider_session_id: Option<&'a str>,
    pub(in crate::run::balancing) attempts: usize,
    pub(in crate::run::balancing) max_attempts: usize,
}

pub(in crate::run::balancing) fn completed_attempt_input<'a, 'state, 'ctx>(
    source: CompletedAttemptInputSource<'a, 'state, 'ctx>,
) -> CompletedAttemptInput<'a, 'state, 'ctx> {
    CompletedAttemptInput {
        agent_runtime_services: source.agent_runtime_services,
        env: source.env,
        invocation: source.invocation,
        invocation_row_id: source.invocation_row_id,
        guard: source.guard,
        provider_name: source.provider_name,
        model: source.model,
        provider_index: source.provider_index,
        result: source.result,
        terminal_signal: source.terminal_signal,
        terminal_signal_ctx: source.terminal_signal_ctx,
        all_models: source.all_models,
        working_dir: source.working_dir,
        zero_turn_provider_session_id: source.zero_turn_provider_session_id,
        attempts: source.attempts,
        max_attempts: source.max_attempts,
    }
}

pub(in crate::run::balancing) type CompletedAttemptRunInput<'a> = (
    &'a ModelConfig,
    &'a HashMap<String, ModelConfig>,
    Option<&'a Path>,
);

pub(in crate::run::balancing) fn completed_attempt_input_for_attempt<'a, 'state, 'ctx>(
    lifecycle: AttemptLifecycleInput<'a, 'state>,
    provider: AttemptProviderInput<'a>,
    terminal: AttemptTerminalInput<'a, 'ctx>,
    run_input: CompletedAttemptRunInput<'a>,
    budget: AttemptBudgetInput,
) -> CompletedAttemptInput<'a, 'state, 'ctx> {
    let (agent_runtime_services, env, invocation, invocation_row_id, guard) = lifecycle;
    let (provider_name, provider_index, zero_turn_provider_session_id) = provider;
    let (result, terminal_signal, terminal_signal_ctx) = terminal;
    let (model, all_models, working_dir) = run_input;
    let (attempts, max_attempts) = budget;
    completed_attempt_input(CompletedAttemptInputSource {
        agent_runtime_services,
        env,
        invocation,
        invocation_row_id,
        guard,
        provider_name,
        model,
        provider_index,
        result,
        terminal_signal,
        terminal_signal_ctx,
        all_models,
        working_dir,
        zero_turn_provider_session_id,
        attempts,
        max_attempts,
    })
}
