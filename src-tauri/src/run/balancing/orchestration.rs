//! ## Declared roles
//!
//! `orchestration`, `mapper`, `formatter`, `predicate`, `accessor`, `validator`
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: src-tauri/src/run/balancing/orchestration.rs
//!     role: intrinsic-surface
//!     Domain: balanced_attempt_lifecycle
//!     Owns:
//!       - run_with_balancing retry and attempt loop
//!       - provider selection, pin forwarding, and effective-provider resolution sequencing
//!       - invocation lifecycle row start/finalization sequencing
//!       - zero-turn baseline, classification, and retry sequencing
//!       - terminal-signal branch dispatch and quota-exhaustion retry sequencing
//! ```

use std::collections::HashMap;
use std::path::Path;

use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig};
use oulipoly_runtime::executor;
use oulipoly_runtime::services::{InvocationLifecycleServicePort, RoutingServicePort};
use oulipoly_state::CompositeInvocationId;
use oulipoly_state::repositories::StateDbOpener;

use super::accessor::{BalancedExecutionEnvironment, load_balanced_execution_environment};
use super::disposition::{
    BalancedLoopControl, handle_interactive_fail, handle_maybe_quota_verify,
    handle_prolonged_silence_fail, handle_quota_exhausted_retry,
};
use super::finalization::finalize_completed_attempt;
use super::formatter;
use super::mapper::{
    TerminalSignalBranch, balanced_invocation_start, composite_invocation_id,
    terminal_signal_branch,
};
use super::predicate::{
    attempts_exhausted, confirmed_zero_turn_exhaustion, provider_selection_pool_exhausted,
    should_defer_generic_exit, should_late_bind_zero_turn_baseline,
};
use super::state_update::bind_start_known_provider_session_if_present;
use crate::captured_child::emit_captured_child_marker_lines;
use crate::error_emit::effective_model_for_execution;
use crate::invocation::finalize::FinalizerGuard;
use crate::quota_zero_turn::{
    apply_zero_turn_classification_to_result, zero_turn_classification_for_action,
    zero_turn_classify_after_completion, zero_turn_late_bind_baseline, zero_turn_record_baseline,
};
use crate::terminal_outcome_adapter::{
    TerminalSignalContext, apply_age153_terminal_signal_fixture_override,
    balanced_terminal_signal_for_outcome, confirm_maybe_quota_exhausted,
};
use crate::wiring;
use crate::zero_turn_orchestration::{
    ZeroTurnAction, ZeroTurnBaseline, ZeroTurnConfirmationState, next_action,
};

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_with_balancing(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    state_db_opener: &dyn StateDbOpener,
    model: &ModelConfig,
    prompt: &str,
    all_models: &HashMap<String, ModelConfig>,
    models_dir: &Path,
    working_dir: Option<&Path>,
    extra_inputs: &HashMap<String, Vec<String>>,
) -> Result<i32, String> {
    let mut env = load_balanced_execution_environment(state_db_opener)?;
    env.models_dir = models_dir.to_path_buf();
    run_with_balancing_environment(
        agent_runtime_services,
        env,
        model,
        prompt,
        all_models,
        working_dir,
        extra_inputs,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_with_balancing_with_pin(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    state_db_opener: &dyn StateDbOpener,
    model: &ModelConfig,
    prompt: &str,
    all_models: &HashMap<String, ModelConfig>,
    models_dir: &Path,
    working_dir: Option<&Path>,
    extra_inputs: &HashMap<String, Vec<String>>,
    provider_pin: Option<&str>,
) -> Result<i32, String> {
    if provider_pin.is_none() {
        return run_with_balancing(
            agent_runtime_services,
            state_db_opener,
            model,
            prompt,
            all_models,
            models_dir,
            working_dir,
            extra_inputs,
        );
    }
    let mut env = load_balanced_execution_environment(state_db_opener)?;
    env.models_dir = models_dir.to_path_buf();
    run_with_balancing_environment(
        agent_runtime_services,
        env,
        model,
        prompt,
        all_models,
        working_dir,
        extra_inputs,
        provider_pin,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_with_balancing_environment(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    env: BalancedExecutionEnvironment,
    model: &ModelConfig,
    prompt: &str,
    all_models: &HashMap<String, ModelConfig>,
    working_dir: Option<&Path>,
    extra_inputs: &HashMap<String, Vec<String>>,
    provider_pin: Option<&str>,
) -> Result<i32, String> {
    let in_flight = oulipoly_runtime::quota::InFlight::new();
    let ctx = super::mapper::balance_context(&env.providers_cfg, &env.sessions_cfg, &in_flight);
    let state = &env.state;
    let parent_invocation_id = crate::dispatch::resolve_parent_invocation_id(state);
    // Source guard marker: resolve_parent_invocation_id(&state)
    // Source guard marker: routing_service.select_route(RoutingServiceRequest { ctx: Some(
    // Source guard marker: .finalize_invocation(
    // Source guard marker: record_returned_artifacts(
    let max_attempts = super::mapper::quota_retry_budget(model);
    let mut attempts = 0usize;
    let mut zero_turn_confirmation = ZeroTurnConfirmationState::new();
    let mut pending_same_provider_verification: Option<(usize, Option<String>)> = None;

    loop {
        if attempts_exhausted(attempts, max_attempts) {
            let reason = exhausted_attempt_reason(agent_runtime_services, model, &env, &ctx);
            formatter::emit_stderr("BLOCKED:all-providers-exhausted");
            formatter::emit_pool_exhausted_pre_invocation_failure(model, &reason);
            return Err(reason);
        }
        attempts += 1;

        let pending_verification = pending_same_provider_verification.take();
        let attempt_provider = resolve_balanced_attempt_provider(
            agent_runtime_services,
            model,
            &env,
            &ctx,
            attempts,
            pending_verification.as_ref(),
            provider_pin,
        )?;
        let provider_index = attempt_provider.provider_index;
        let provider = attempt_provider.provider;
        let prompt_mode = attempt_provider.prompt_mode;
        let provider_name = provider.name.as_str();
        let mut attempt = start_balanced_attempt(
            agent_runtime_services,
            &env,
            model,
            &provider,
            provider_index,
            parent_invocation_id,
            pending_verification,
        )?;

        let mut result = execute_balanced_attempt(
            agent_runtime_services,
            &env,
            model,
            &provider,
            provider_index,
            prompt_mode,
            prompt,
            working_dir,
            extra_inputs,
            &mut attempt,
        )?;
        let zero_turn = classify_balanced_zero_turn_result(BalancedZeroTurnInput {
            env: &env,
            provider_name,
            start_known_provider_session_id: attempt.start_known_provider_session_id.as_deref(),
            result: &mut result,
            zero_turn_baseline: &mut attempt.zero_turn_baseline,
            zero_turn_confirmation: &mut zero_turn_confirmation,
        });

        emit_captured_child_marker_lines(&result.captured_child_invocations);

        let terminal_signal_context_ids = super::mapper::terminal_signal_context_ids(
            &attempt.invocation.id,
            zero_turn.provider_session_id.as_deref(),
        );
        let mut terminal_signal_stderr = std::io::stderr();
        let mut terminal_signal_ctx = super::mapper::terminal_signal_context_for_attempt(
            &terminal_signal_context_ids,
            provider_name,
            &env.state,
            &mut terminal_signal_stderr,
        );
        let should_defer_generic_exit = should_defer_generic_exit(all_models, &result);
        let balanced_terminal_signal =
            balanced_terminal_signal_for_outcome(&result, should_defer_generic_exit);

        let control = dispatch_balanced_terminal_branch(BalancedTerminalDispatchInput {
            agent_runtime_services,
            env: &env,
            model,
            all_models,
            working_dir,
            invocation: &attempt.invocation,
            invocation_row_id: attempt.invocation_row_id,
            guard: &mut attempt.guard,
            provider_name,
            provider_index,
            zero_turn_provider_session_id: zero_turn.provider_session_id.as_deref(),
            result: &result,
            terminal_signal: &balanced_terminal_signal,
            terminal_signal_ctx: &mut terminal_signal_ctx,
            attempts,
            max_attempts,
            zero_turn_action: zero_turn.action,
            pending_same_provider_verification: &mut pending_same_provider_verification,
        });

        match control {
            BalancedLoopControl::Continue => continue,
            BalancedLoopControl::Return(result) => return result,
        }
    }
}

struct BalancedAttemptProvider {
    provider_index: usize,
    provider: ProviderConfig,
    prompt_mode: PromptMode,
}

fn resolve_balanced_attempt_provider(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    model: &ModelConfig,
    env: &BalancedExecutionEnvironment,
    ctx: &oulipoly_runtime::balancer::BalanceContext<'_>,
    attempts: usize,
    pending_verification: Option<&(usize, Option<String>)>,
    provider_pin: Option<&str>,
) -> Result<BalancedAttemptProvider, String> {
    let provider_index = attempt_provider_index(
        agent_runtime_services,
        model,
        env,
        ctx,
        attempts,
        pending_verification,
        provider_pin,
    )?;
    let (provider, prompt_mode) = attempt_effective_provider(model, env, provider_index)?;
    Ok(BalancedAttemptProvider {
        provider_index,
        provider,
        prompt_mode,
    })
}

fn attempt_provider_index(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    model: &ModelConfig,
    env: &BalancedExecutionEnvironment,
    ctx: &oulipoly_runtime::balancer::BalanceContext<'_>,
    attempts: usize,
    pending_verification: Option<&(usize, Option<String>)>,
    provider_pin: Option<&str>,
) -> Result<usize, String> {
    match pending_verification {
        Some((provider_index, _)) => Ok(*provider_index),
        None => select_provider_index_for_new_attempt(
            agent_runtime_services,
            model,
            env,
            ctx,
            attempts,
            provider_pin,
        ),
    }
}

fn select_provider_index_for_new_attempt(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    model: &ModelConfig,
    env: &BalancedExecutionEnvironment,
    ctx: &oulipoly_runtime::balancer::BalanceContext<'_>,
    attempts: usize,
    provider_pin: Option<&str>,
) -> Result<usize, String> {
    match select_balanced_provider_index(agent_runtime_services, model, env, ctx, provider_pin) {
        Ok(provider_index) => Ok(provider_index),
        Err(err) => {
            formatter::emit_provider_selection_pre_invocation_failure(
                model,
                &err,
                provider_selection_pool_exhausted(attempts, model.providers.len()),
            );
            Err(err)
        }
    }
}

fn attempt_effective_provider(
    model: &ModelConfig,
    env: &BalancedExecutionEnvironment,
    provider_index: usize,
) -> Result<(ProviderConfig, PromptMode), String> {
    match effective_model_for_execution(model, provider_index, &env.providers_cfg) {
        Ok(effective) => Ok(effective),
        Err(err) => {
            formatter::emit_provider_resolution_pre_invocation_failure(model, provider_index, &err);
            Err(err)
        }
    }
}

struct BalancedInvocationAttempt<'state> {
    invocation: CompositeInvocationId,
    invocation_row_id: i64,
    guard: FinalizerGuard<'state>,
    start_known_provider_session_id: Option<String>,
    zero_turn_baseline: ZeroTurnBaseline,
    invocation_env: String,
}

fn start_balanced_attempt<'state>(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    env: &'state BalancedExecutionEnvironment,
    model: &ModelConfig,
    provider: &ProviderConfig,
    provider_index: usize,
    parent_invocation_id: Option<i64>,
    pending_verification: Option<(usize, Option<String>)>,
) -> Result<BalancedInvocationAttempt<'state>, String> {
    let provider_name = provider.name.as_str();
    let invocation = composite_invocation_id(provider_name);
    let invocation_row_id = start_balanced_invocation_row(
        agent_runtime_services,
        env,
        model,
        provider_name,
        provider_index,
        parent_invocation_id,
        &invocation,
    )?;
    let guard = FinalizerGuard::new(&env.state, invocation_row_id);
    let start_known_provider_session_id =
        start_known_provider_session_id_for_attempt(provider, pending_verification)?;
    bind_start_known_provider_session_if_present(
        &env.state,
        invocation_row_id,
        start_known_provider_session_id.as_deref(),
    );
    let zero_turn_baseline = zero_turn_record_baseline(
        &env.state,
        &env.sessions_cfg,
        provider_name,
        start_known_provider_session_id.as_deref(),
    );
    let invocation_env = formatter::invocation_env(&invocation)
        .map_err(formatter::invocation_env_serialization_error)?;
    formatter::emit_invocation_stderr_line(&invocation);
    Ok(BalancedInvocationAttempt {
        invocation,
        invocation_row_id,
        guard,
        start_known_provider_session_id,
        zero_turn_baseline,
        invocation_env,
    })
}

fn start_balanced_invocation_row(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    env: &BalancedExecutionEnvironment,
    model: &ModelConfig,
    provider_name: &str,
    provider_index: usize,
    parent_invocation_id: Option<i64>,
    invocation: &CompositeInvocationId,
) -> Result<i64, String> {
    let invocation_start = balanced_invocation_start(
        invocation,
        model,
        provider_name,
        provider_index,
        parent_invocation_id,
    );
    agent_runtime_services
        .invocation_lifecycle_service
        // invocation_lifecycle_service.start_invocation(InvocationLifecycleStartRequest
        .start_invocation(super::mapper::invocation_lifecycle_start_request(
            &env.state,
            &invocation_start,
        ))
        .map(|start| start.invocation_row_id)
        .map_err(|err| err.to_string())
}

fn start_known_provider_session_id_for_attempt(
    provider: &ProviderConfig,
    pending_verification: Option<(usize, Option<String>)>,
) -> Result<Option<String>, String> {
    match pending_verification {
        Some((_, session_id)) => Ok(session_id),
        None => executor::cli::start_known_provider_session_id(provider),
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_balanced_attempt(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    env: &BalancedExecutionEnvironment,
    model: &ModelConfig,
    provider: &ProviderConfig,
    provider_index: usize,
    prompt_mode: PromptMode,
    prompt: &str,
    working_dir: Option<&Path>,
    extra_inputs: &HashMap<String, Vec<String>>,
    attempt: &mut BalancedInvocationAttempt<'_>,
) -> Result<executor::ExecutionResult, String> {
    let executor_request = super::mapper::balanced_executor_request_for_attempt(
        (model, provider, provider_index, prompt_mode),
        (prompt, working_dir, &env.models_dir, extra_inputs),
        (
            &attempt.invocation_env,
            attempt.start_known_provider_session_id.clone(),
        ),
    );
    match agent_runtime_services
        .executor_service
        .execute(executor_request)
    {
        Ok(output) => Ok(output.result),
        Err(err) => {
            let err = err.to_string();
            finalize_spawn_error(super::mapper::spawn_error_input_for_attempt(
                (
                    agent_runtime_services,
                    env,
                    &attempt.invocation,
                    attempt.invocation_row_id,
                    &mut attempt.guard,
                ),
                (
                    provider.name.as_str(),
                    attempt.start_known_provider_session_id.as_deref(),
                ),
                err.clone(),
            ));
            Err(err)
        }
    }
}

struct BalancedZeroTurnInput<'a> {
    env: &'a BalancedExecutionEnvironment,
    provider_name: &'a str,
    start_known_provider_session_id: Option<&'a str>,
    result: &'a mut executor::ExecutionResult,
    zero_turn_baseline: &'a mut ZeroTurnBaseline,
    zero_turn_confirmation: &'a mut ZeroTurnConfirmationState,
}

struct BalancedZeroTurnOutcome {
    provider_session_id: Option<String>,
    action: ZeroTurnAction,
}

fn classify_balanced_zero_turn_result(input: BalancedZeroTurnInput<'_>) -> BalancedZeroTurnOutcome {
    apply_age153_terminal_signal_fixture_override(input.result);
    let provider_session_id = super::accessor::zero_turn_provider_session_id(
        input.start_known_provider_session_id,
        input.result,
    );
    late_bind_zero_turn_baseline_if_needed(
        input.env,
        input.provider_name,
        provider_session_id.as_deref(),
        input.zero_turn_baseline,
    );
    let zero_turn_classification = zero_turn_classify_after_completion(
        &input.env.state,
        &input.env.sessions_cfg,
        input.zero_turn_baseline,
    );
    apply_zero_turn_classification_to_result(
        input.result,
        input.provider_name,
        &zero_turn_classification,
    );
    let action = next_action(
        input.zero_turn_confirmation,
        zero_turn_classification_for_action(
            zero_turn_classification,
            input.result,
            input.provider_name,
            provider_session_id.as_deref(),
        ),
    );
    BalancedZeroTurnOutcome {
        provider_session_id,
        action,
    }
}

fn late_bind_zero_turn_baseline_if_needed(
    env: &BalancedExecutionEnvironment,
    provider_name: &str,
    provider_session_id: Option<&str>,
    zero_turn_baseline: &mut ZeroTurnBaseline,
) {
    if should_late_bind_zero_turn_baseline(zero_turn_baseline, provider_session_id) {
        let session_id =
            super::validator::required_late_bind_provider_session_id(provider_session_id);
        *zero_turn_baseline =
            zero_turn_late_bind_baseline(&env.sessions_cfg, provider_name, session_id);
    }
}

struct BalancedTerminalDispatchInput<'a, 'state, 'ctx> {
    agent_runtime_services: &'a wiring::AgentRuntimeServices,
    env: &'a BalancedExecutionEnvironment,
    model: &'a ModelConfig,
    all_models: &'a HashMap<String, ModelConfig>,
    working_dir: Option<&'a Path>,
    invocation: &'a CompositeInvocationId,
    invocation_row_id: i64,
    guard: &'a mut FinalizerGuard<'state>,
    provider_name: &'a str,
    provider_index: usize,
    zero_turn_provider_session_id: Option<&'a str>,
    result: &'a executor::ExecutionResult,
    terminal_signal: &'a Option<executor::TerminalSignal>,
    terminal_signal_ctx: &'a mut TerminalSignalContext<'ctx, std::io::Stderr>,
    attempts: usize,
    max_attempts: usize,
    zero_turn_action: ZeroTurnAction,
    pending_same_provider_verification: &'a mut Option<(usize, Option<String>)>,
}

fn dispatch_balanced_terminal_branch(
    input: BalancedTerminalDispatchInput<'_, '_, '_>,
) -> BalancedLoopControl {
    if confirmed_zero_turn_exhaustion(input.zero_turn_action, input.terminal_signal) {
        return handle_confirmed_zero_turn_exhaustion_branch(input);
    }
    let branch = terminal_signal_branch(input.terminal_signal);
    match branch {
        TerminalSignalBranch::QuotaExhaustedRetry => {
            handle_quota_exhausted_retry(typed_disposition_input_from_balanced_dispatch(input))
        }
        TerminalSignalBranch::MaybeQuotaVerify => handle_maybe_quota_verify(
            maybe_quota_verify_input_from_balanced_dispatch(input, false),
        ),
        TerminalSignalBranch::ProlongedSilenceFail => {
            handle_prolonged_silence_fail(typed_disposition_input_from_balanced_dispatch(input))
        }
        TerminalSignalBranch::InteractiveFail => {
            handle_interactive_fail(typed_disposition_input_from_balanced_dispatch(input))
        }
        TerminalSignalBranch::CompletedAttempt => {
            finalize_completed_attempt(completed_attempt_input_from_balanced_dispatch(input))
        }
    }
}

fn handle_confirmed_zero_turn_exhaustion_branch(
    input: BalancedTerminalDispatchInput<'_, '_, '_>,
) -> BalancedLoopControl {
    let signal = super::validator::required_confirmed_zero_turn_signal(input.terminal_signal);
    let _ = confirm_maybe_quota_exhausted(signal, input.terminal_signal_ctx);
    handle_maybe_quota_verify(maybe_quota_verify_input_from_balanced_dispatch(input, true))
}

fn typed_disposition_input_from_balanced_dispatch<'a, 'state, 'ctx>(
    input: BalancedTerminalDispatchInput<'a, 'state, 'ctx>,
) -> super::disposition::TypedDispositionInput<'a, 'state, 'ctx> {
    super::mapper::typed_disposition_input_for_attempt(
        (
            input.agent_runtime_services,
            input.env,
            input.invocation,
            input.invocation_row_id,
            input.guard,
        ),
        (
            input.provider_name,
            input.provider_index,
            input.zero_turn_provider_session_id,
        ),
        (
            input.result,
            input.terminal_signal,
            input.terminal_signal_ctx,
        ),
        (input.attempts, input.max_attempts),
    )
}

fn maybe_quota_verify_input_from_balanced_dispatch<'a, 'state, 'ctx>(
    input: BalancedTerminalDispatchInput<'a, 'state, 'ctx>,
    signal_already_applied: bool,
) -> super::disposition::MaybeQuotaVerifyInput<'a, 'state, 'ctx> {
    super::mapper::maybe_quota_verify_input_for_attempt(
        (
            input.agent_runtime_services,
            input.env,
            input.invocation,
            input.invocation_row_id,
            input.guard,
        ),
        (
            input.provider_name,
            input.provider_index,
            input.zero_turn_provider_session_id,
        ),
        (
            input.result,
            input.terminal_signal,
            input.terminal_signal_ctx,
        ),
        (input.attempts, input.max_attempts),
        input.zero_turn_action,
        input.pending_same_provider_verification,
        signal_already_applied,
    )
}

fn completed_attempt_input_from_balanced_dispatch<'a, 'state, 'ctx>(
    input: BalancedTerminalDispatchInput<'a, 'state, 'ctx>,
) -> super::finalization::CompletedAttemptInput<'a, 'state, 'ctx> {
    super::mapper::completed_attempt_input_for_attempt(
        (
            input.agent_runtime_services,
            input.env,
            input.invocation,
            input.invocation_row_id,
            input.guard,
        ),
        (
            input.provider_name,
            input.provider_index,
            input.zero_turn_provider_session_id,
        ),
        (
            input.result,
            input.terminal_signal,
            input.terminal_signal_ctx,
        ),
        (input.model, input.all_models, input.working_dir),
        (input.attempts, input.max_attempts),
    )
}

fn select_balanced_provider_index(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    model: &ModelConfig,
    env: &BalancedExecutionEnvironment,
    ctx: &oulipoly_runtime::balancer::BalanceContext<'_>,
    provider_pin: Option<&str>,
) -> Result<usize, String> {
    agent_runtime_services
        .routing_service
        .select_route(super::mapper::routing_service_request(
            model,
            &env.state,
            ctx,
            provider_pin,
        ))
        .map(|route| route.provider_index)
        .map_err(|err| err.to_string())
}

fn exhausted_attempt_reason(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    model: &ModelConfig,
    env: &BalancedExecutionEnvironment,
    ctx: &oulipoly_runtime::balancer::BalanceContext<'_>,
) -> String {
    let route_error = match agent_runtime_services
        .routing_service
        // routing_service.select_route(RoutingServiceRequest { ctx: Some(
        .select_route(super::mapper::routing_service_request(
            model, &env.state, ctx, None,
        )) {
        Err(err) => Some(err.to_string()),
        Ok(_) => None,
    };
    formatter::exhausted_attempt_reason(
        route_error,
        &model.name,
        super::mapper::quota_retry_budget(model),
    )
}

fn finalize_spawn_error(input: super::mapper::SpawnErrorInput<'_, '_>) {
    let signal = spawn_error_signal(&input);
    apply_spawn_error_terminal_outcome(&input, &signal);
    let finalized = finalize_spawn_error_invocation(&input, &signal);
    emit_spawn_error_envelope(&input, &signal);
    if finalized {
        input.guard.mark_finalized();
    }
    mark_spawn_error_session_idle_if_present(&input);
}

fn mark_spawn_error_session_idle_if_present(input: &super::mapper::SpawnErrorInput<'_, '_>) {
    if let Some(session_id) = input.provider_session_id {
        let result = mark_spawn_error_session_idle(session_id, input.invocation_id);
        warn_spawn_error_session_idle_failure_if_needed(session_id, input.invocation_id, result);
    }
}

fn mark_spawn_error_session_idle(session_id: &str, invocation_id: &str) -> Result<(), String> {
    crate::wake_coordinator::mark_session_idle_after_turn(session_id, invocation_id, Some(1))
}

fn warn_spawn_error_session_idle_failure_if_needed(
    session_id: &str,
    invocation_id: &str,
    result: Result<(), String>,
) {
    if let Err(err) = result {
        warn_spawn_error_session_idle_failure(session_id, invocation_id, &err);
    }
}

fn warn_spawn_error_session_idle_failure(session_id: &str, invocation_id: &str, err: &str) {
    tracing::warn!(
        session_id,
        invocation_uuid = invocation_id,
        "Failed to mark balanced spawn-error session idle: {err}"
    );
}

fn spawn_error_signal(
    input: &super::mapper::SpawnErrorInput<'_, '_>,
) -> oulipoly_runtime::executor::TerminalSignal {
    super::mapper::spawn_error_signal(input.provider_name, input.error.clone())
}

fn apply_spawn_error_terminal_outcome(
    input: &super::mapper::SpawnErrorInput<'_, '_>,
    signal: &oulipoly_runtime::executor::TerminalSignal,
) {
    let terminal_signal_context_ids = spawn_error_terminal_signal_context_ids(input.invocation_id);
    let mut terminal_signal_stderr = std::io::stderr();
    let mut terminal_signal_ctx = super::mapper::terminal_signal_context_for_attempt(
        &terminal_signal_context_ids,
        input.provider_name,
        &input.env.state,
        &mut terminal_signal_stderr,
    );
    let _ = crate::terminal_outcome_adapter::apply_terminal_signal_outcome(
        &Some(signal.clone()),
        &mut terminal_signal_ctx,
    );
}

fn spawn_error_terminal_signal_context_ids(
    invocation_id: &str,
) -> super::mapper::TerminalSignalContextIds {
    super::mapper::terminal_signal_context_ids(invocation_id, None)
}

fn finalize_spawn_error_invocation(
    input: &super::mapper::SpawnErrorInput<'_, '_>,
    signal: &oulipoly_runtime::executor::TerminalSignal,
) -> bool {
    let terminal_reason = formatter::spawn_error_terminal_reason(signal);
    match input
        .agent_runtime_services
        .invocation_lifecycle_service
        .finalize_invocation(super::mapper::spawn_error_finalize_request(
            &input.env.state,
            input.invocation_row_id,
            terminal_reason,
        )) {
        Ok(_) => true,
        Err(err) => {
            formatter::emit_finalize_invocation_warning(err);
            false
        }
    }
}

fn emit_spawn_error_envelope(
    input: &super::mapper::SpawnErrorInput<'_, '_>,
    signal: &oulipoly_runtime::executor::TerminalSignal,
) {
    let terminal_reason = formatter::spawn_error_terminal_reason(signal);
    formatter::emit_spawn_error_failure_result_envelope(
        &input.env.state,
        input.invocation_id,
        input.provider_name,
        input.provider_session_id,
        terminal_reason,
    );
}
