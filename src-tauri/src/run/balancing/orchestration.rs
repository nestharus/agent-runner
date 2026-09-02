//! ## Declared roles
//!
//! `orchestration`, `mapper`, `formatter`, `predicate`, `accessor`, `validator`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/run/balancing/orchestration.rs
//!     role: adapter
//!     Translates:
//!       - balancing-run-loop-contract
//!       - routing-service-selection-contract
//!       - invocation-lifecycle-contract
//!       - executor-dispatch-contract
//!       - terminal-zero-turn-disposition-contract
//! ```

use std::collections::HashMap;
use std::path::Path;

use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig};
use oulipoly_runtime::executor;
use oulipoly_runtime::services::{
    InvocationLifecycleServicePort, ProviderSessionStartMode, RoutingServicePort,
};
use oulipoly_runtime::session_authority::SessionAuthorityExpectation;
use oulipoly_state::CompositeInvocationId;
use oulipoly_state::repositories::StateDbOpener;

use super::accessor::{BalancedExecutionEnvironment, load_balanced_execution_environment};
use super::disposition::{
    BalancedLoopControl, handle_interactive_fail, handle_maybe_quota_verify,
    handle_prolonged_silence_fail, handle_quota_exhausted_retry,
};
use super::finalization::finalize_completed_attempt;
use super::formatter;
use super::mapper::{TerminalSignalBranch, balanced_invocation_start, terminal_signal_branch};
use super::predicate::{
    attempts_exhausted, confirmed_zero_turn_exhaustion, provider_selection_pool_exhausted,
    should_defer_generic_exit, should_late_bind_zero_turn_baseline,
};
use super::state_update::{
    BalancedSessionAuthorityCommitRequest, bind_start_known_provider_session_if_present,
    commit_balanced_session_authority,
};
use crate::captured_child::emit_captured_child_marker_lines;
use crate::error_emit::effective_model_for_execution;
use crate::invocation::finalize::FinalizerGuard;
use crate::quota_zero_turn::{
    apply_zero_turn_classification_to_result, host_observed_completion_from_result,
    zero_turn_classification_for_action, zero_turn_classify_after_completion_with_recovery,
    zero_turn_late_bind_baseline, zero_turn_record_baseline,
};
use crate::run::reservation::ReservedRun;
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
    run_with_balancing_plan(
        agent_runtime_services,
        state_db_opener,
        model,
        prompt,
        all_models,
        models_dir,
        working_dir,
        extra_inputs,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub(in crate::run) fn run_reserved_with_balancing(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    state_db_opener: &dyn StateDbOpener,
    model: &ModelConfig,
    prompt: &str,
    all_models: &HashMap<String, ModelConfig>,
    models_dir: &Path,
    working_dir: Option<&Path>,
    extra_inputs: &HashMap<String, Vec<String>>,
    reservation: &ReservedRun,
) -> Result<i32, String> {
    run_with_balancing_plan(
        agent_runtime_services,
        state_db_opener,
        model,
        prompt,
        all_models,
        models_dir,
        working_dir,
        extra_inputs,
        Some(reservation),
    )
}

#[allow(clippy::too_many_arguments)]
fn run_with_balancing_plan(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    state_db_opener: &dyn StateDbOpener,
    model: &ModelConfig,
    prompt: &str,
    all_models: &HashMap<String, ModelConfig>,
    models_dir: &Path,
    working_dir: Option<&Path>,
    extra_inputs: &HashMap<String, Vec<String>>,
    reservation: Option<&ReservedRun>,
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
        reservation,
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
    reservation: Option<&ReservedRun>,
) -> Result<i32, String> {
    let in_flight = oulipoly_runtime::quota::InFlight::new();
    let ctx = super::mapper::balance_context(&env.providers_cfg, &in_flight);
    let state = &env.state;
    let parent_invocation_id = super::parent_invocation_row_id(
        crate::dispatch::resolve_parent_invocation_id(state),
        reservation,
    );
    // Source guard marker: resolve_parent_invocation_id(&state)
    // Source guard marker: routing_service.select_route(RoutingServiceRequest { ctx: Some(
    // Source guard marker: .finalize_invocation(
    // Source guard marker: record_returned_artifacts(
    let max_attempts = super::max_attempts(super::mapper::quota_retry_budget(model), reservation);
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
            reservation,
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
        if agent_runtime_services
            .provider_registry_handle
            .current()
            .has_account_endpoint(provider_name)
        {
            let observed_provider_name = result_provider_name(model, &result)?;
            commit_balanced_session_authority(BalancedSessionAuthorityCommitRequest {
                state: &env.state,
                invocation_row_id: attempt.invocation_row_id,
                invocation_uuid: &attempt.invocation.id,
                expectation: SessionAuthorityExpectation {
                    account_name: provider_name,
                    provider_session_id: attempt.start_known_provider_session_id.as_deref(),
                },
                observed_provider_name,
                start_mode: attempt.start_known_provider_session_mode,
                working_dir,
                result: &result,
            })?;
        }
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
        let balanced_terminal_signal = if zero_turn.recovered_generic_nonzero {
            result.terminal_signal.clone()
        } else {
            balanced_terminal_signal_for_outcome(&result, should_defer_generic_exit)
        };

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
            recovered_generic_nonzero: zero_turn.recovered_generic_nonzero,
            pending_same_provider_verification: &mut pending_same_provider_verification,
        });

        match control {
            BalancedLoopControl::Continue => continue,
            BalancedLoopControl::Return(result) => return result,
        }
    }
}

fn result_provider_name<'a>(
    model: &'a ModelConfig,
    result: &executor::ExecutionResult,
) -> Result<&'a str, String> {
    model
        .providers
        .get(result.provider_index)
        .map(|provider| provider.name.as_str())
        .ok_or_else(|| {
            format!(
                "executor returned provider index {} outside model {} pool",
                result.provider_index, model.name
            )
        })
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
) -> Result<BalancedAttemptProvider, String> {
    let provider_index = attempt_provider_index(
        agent_runtime_services,
        model,
        env,
        ctx,
        attempts,
        pending_verification,
    )?;
    let effective_provider = attempt_effective_provider(model, env, provider_index)?;
    Ok(balanced_attempt_provider(
        provider_index,
        effective_provider,
    ))
}

fn balanced_attempt_provider(
    provider_index: usize,
    effective_provider: (ProviderConfig, PromptMode),
) -> BalancedAttemptProvider {
    let (provider, prompt_mode) = effective_provider;
    BalancedAttemptProvider {
        provider_index,
        provider,
        prompt_mode,
    }
}

fn attempt_provider_index(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    model: &ModelConfig,
    env: &BalancedExecutionEnvironment,
    ctx: &oulipoly_runtime::balancer::BalanceContext<'_>,
    attempts: usize,
    pending_verification: Option<&(usize, Option<String>)>,
) -> Result<usize, String> {
    match pending_verification {
        Some((provider_index, _)) => Ok(*provider_index),
        None => {
            select_provider_index_for_new_attempt(agent_runtime_services, model, env, ctx, attempts)
        }
    }
}

fn select_provider_index_for_new_attempt(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    model: &ModelConfig,
    env: &BalancedExecutionEnvironment,
    ctx: &oulipoly_runtime::balancer::BalanceContext<'_>,
    attempts: usize,
) -> Result<usize, String> {
    match select_balanced_provider_index(agent_runtime_services, model, env, ctx) {
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
    start_known_provider_session_mode: Option<ProviderSessionStartMode>,
    zero_turn_baseline: ZeroTurnBaseline,
    invocation_env: String,
}

struct StartKnownProviderSession {
    id: Option<String>,
    mode: Option<ProviderSessionStartMode>,
}

#[allow(clippy::too_many_arguments)]
fn start_balanced_attempt<'state>(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    env: &'state BalancedExecutionEnvironment,
    model: &ModelConfig,
    provider: &ProviderConfig,
    provider_index: usize,
    parent_invocation_id: Option<i64>,
    pending_verification: Option<(usize, Option<String>)>,
    reservation: Option<&ReservedRun>,
) -> Result<BalancedInvocationAttempt<'state>, String> {
    let provider_name = provider.name.as_str();
    let invocation = super::composite_invocation_id(provider_name, reservation);
    let invocation_start = start_balanced_invocation_row(
        agent_runtime_services,
        env,
        model,
        provider_name,
        provider_index,
        parent_invocation_id,
        &invocation,
    )?;
    let invocation_row_id = invocation_start.invocation_row_id;
    let guard = FinalizerGuard::new(&env.state, invocation_row_id);
    let start_known_provider_session =
        start_known_provider_session_for_attempt(provider, pending_verification)?;
    if !agent_runtime_services
        .provider_registry_handle
        .current()
        .has_account_endpoint(provider_name)
    {
        bind_start_known_provider_session_if_present(
            &env.state,
            invocation_row_id,
            start_known_provider_session.id.as_deref(),
        );
    }
    let zero_turn_baseline = zero_turn_record_baseline(
        &env.state,
        &env.sessions_cfg,
        provider_name,
        start_known_provider_session.id.as_deref(),
    );
    let invocation_env = invocation_start
        .completion_registration_authority
        .invocation_launch_environment(&invocation)?;
    formatter::emit_invocation_stderr_line(&invocation);
    Ok(balanced_invocation_attempt(
        invocation,
        invocation_row_id,
        guard,
        start_known_provider_session,
        zero_turn_baseline,
        invocation_env,
    ))
}

fn balanced_invocation_attempt<'state>(
    invocation: CompositeInvocationId,
    invocation_row_id: i64,
    guard: FinalizerGuard<'state>,
    start_known_provider_session: StartKnownProviderSession,
    zero_turn_baseline: ZeroTurnBaseline,
    invocation_env: String,
) -> BalancedInvocationAttempt<'state> {
    BalancedInvocationAttempt {
        invocation,
        invocation_row_id,
        guard,
        start_known_provider_session_id: start_known_provider_session.id,
        start_known_provider_session_mode: start_known_provider_session.mode,
        zero_turn_baseline,
        invocation_env,
    }
}

fn start_balanced_invocation_row(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    env: &BalancedExecutionEnvironment,
    model: &ModelConfig,
    provider_name: &str,
    provider_index: usize,
    parent_invocation_id: Option<i64>,
    invocation: &CompositeInvocationId,
) -> Result<oulipoly_runtime::services::InvocationLifecycleStartOutput, String> {
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
        .map_err(|err| err.to_string())
}

fn start_known_provider_session_for_attempt(
    provider: &ProviderConfig,
    pending_verification: Option<(usize, Option<String>)>,
) -> Result<StartKnownProviderSession, String> {
    match pending_verification {
        Some((_, session_id)) => Ok(StartKnownProviderSession {
            mode: session_id
                .as_ref()
                .map(|_| ProviderSessionStartMode::Resume),
            id: session_id,
        }),
        None => {
            let id = executor::cli::start_known_provider_session_id(provider)?;
            Ok(StartKnownProviderSession {
                mode: id.as_ref().map(|_| ProviderSessionStartMode::Create),
                id,
            })
        }
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
    let _admission = crate::wake_coordinator::admit_session_launch(
        &attempt.invocation.id,
        attempt.start_known_provider_session_id.as_deref(),
    )?;
    let executor_request = super::mapper::balanced_executor_request_for_attempt(
        (model, provider, provider_index, prompt_mode),
        (prompt, working_dir, &env.models_dir, extra_inputs),
        (
            &attempt.invocation_env,
            attempt.start_known_provider_session_id.clone(),
            attempt.start_known_provider_session_mode,
        ),
    );
    let _live_pty_retry_driver = start_live_pty_retry_driver_if_applicable(provider);
    match agent_runtime_services
        .executor_service
        .execute(executor_request)
    {
        Ok(output) => Ok(output.result),
        Err(err) => {
            let error = executor_error_message(err);
            dispatch_balanced_spawn_error(
                agent_runtime_services,
                env,
                provider.name.as_str(),
                attempt,
                error.clone(),
            );
            Err(error)
        }
    }
}

fn executor_error_message(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn dispatch_balanced_spawn_error(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    env: &BalancedExecutionEnvironment,
    provider_name: &str,
    attempt: &mut BalancedInvocationAttempt<'_>,
    error: String,
) {
    finalize_spawn_error(super::mapper::spawn_error_input_for_attempt(
        (
            agent_runtime_services,
            env,
            &attempt.invocation,
            attempt.invocation_row_id,
            &mut attempt.guard,
        ),
        (
            provider_name,
            attempt.start_known_provider_session_id.as_deref(),
        ),
        error,
    ));
}

fn start_live_pty_retry_driver_if_applicable(
    provider: &ProviderConfig,
) -> Option<crate::wake_coordinator::LivePtyRetryDriverGuard> {
    provider
        .interactive_args
        .as_ref()
        .and_then(|_| crate::wake_coordinator::start_live_pty_retry_driver_for_owner())
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
    recovered_generic_nonzero: bool,
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
    let completion = zero_turn_classify_after_completion_with_recovery(
        &input.env.state,
        &input.env.sessions_cfg,
        input.zero_turn_baseline,
        host_observed_completion_from_result(input.result),
        input.result,
    );
    let zero_turn_classification = completion.classification;
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
    balanced_zero_turn_outcome(
        provider_session_id,
        action,
        completion.recovered_generic_nonzero,
    )
}

fn balanced_zero_turn_outcome(
    provider_session_id: Option<String>,
    action: ZeroTurnAction,
    recovered_generic_nonzero: bool,
) -> BalancedZeroTurnOutcome {
    BalancedZeroTurnOutcome {
        provider_session_id,
        action,
        recovered_generic_nonzero,
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
    recovered_generic_nonzero: bool,
    pending_same_provider_verification: &'a mut Option<(usize, Option<String>)>,
}

fn dispatch_balanced_terminal_branch(
    input: BalancedTerminalDispatchInput<'_, '_, '_>,
) -> BalancedLoopControl {
    if confirmed_zero_turn_exhaustion(input.zero_turn_action, input.terminal_signal) {
        return handle_confirmed_zero_turn_exhaustion_branch(input);
    }
    let branch = terminal_signal_branch(input.terminal_signal, input.recovered_generic_nonzero);
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
        input.recovered_generic_nonzero,
    )
}

fn select_balanced_provider_index(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    model: &ModelConfig,
    env: &BalancedExecutionEnvironment,
    ctx: &oulipoly_runtime::balancer::BalanceContext<'_>,
) -> Result<usize, String> {
    agent_runtime_services
        .routing_service
        .select_route(super::mapper::routing_service_request(
            model, &env.state, ctx,
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
    let route_result = agent_runtime_services
        .routing_service
        // routing_service.select_route(RoutingServiceRequest { ctx: Some(
        .select_route(super::mapper::routing_service_request(
            model, &env.state, ctx,
        ));
    let route_error = route_probe_error(route_result).map(route_probe_error_message);
    formatter::exhausted_attempt_reason(
        route_error,
        &model.name,
        super::mapper::quota_retry_budget(model),
    )
}

fn route_probe_error<T, E>(result: Result<T, E>) -> Option<E> {
    result.err()
}

fn route_probe_error_message(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn finalize_spawn_error(input: super::mapper::SpawnErrorInput<'_, '_>) {
    let signal = spawn_error_signal(&input);
    apply_spawn_error_terminal_outcome(&input, &signal);
    let finalized = finalize_spawn_error_invocation(&input, &signal);
    emit_spawn_error_envelope(&input, &signal);
    mark_spawn_error_guard_finalized(input.guard, finalized);
    let handed_off = finalize_spawn_error_mailbox_handoff(&input);
    mark_spawn_error_session_idle_if_needed(&input, handed_off);
}

fn mark_spawn_error_guard_finalized(guard: &mut FinalizerGuard<'_>, finalized: bool) {
    if finalized {
        guard.mark_finalized();
    }
}

struct SpawnErrorHandoffStatus<E> {
    handed_off: bool,
    error: Option<E>,
}

fn spawn_error_handoff_status<E>(result: Result<bool, E>) -> SpawnErrorHandoffStatus<E> {
    match result {
        Ok(handed_off) => SpawnErrorHandoffStatus {
            handed_off,
            error: None,
        },
        Err(error) => SpawnErrorHandoffStatus {
            handed_off: false,
            error: Some(error),
        },
    }
}

fn finalize_spawn_error_mailbox_handoff(input: &super::mapper::SpawnErrorInput<'_, '_>) -> bool {
    let result = crate::mailbox_delivery::finalize_pty_mailbox_delivery_handoff(
        input.provider_session_id,
        input.invocation_id,
        1,
    );
    let status = spawn_error_handoff_status(result);
    if let Some(error) = status.error {
        emit_spawn_error_handoff_warning(input, error);
    }
    status.handed_off
}

fn emit_spawn_error_handoff_warning(
    input: &super::mapper::SpawnErrorInput<'_, '_>,
    error: impl std::fmt::Display,
) {
    tracing::warn!(
        session_id = input.provider_session_id,
        invocation_uuid = input.invocation_id,
        "Failed to hand off balanced spawn-error PTY mailbox delivery: {error}"
    );
}

fn should_mark_spawn_error_session_idle(
    handed_off: bool,
    provider_session_id: Option<&str>,
) -> bool {
    !handed_off && provider_session_id.is_some()
}

fn mark_spawn_error_session_idle_if_needed(
    input: &super::mapper::SpawnErrorInput<'_, '_>,
    handed_off: bool,
) {
    if !should_mark_spawn_error_session_idle(handed_off, input.provider_session_id) {
        return;
    }
    let session_id = input
        .provider_session_id
        .expect("idle eligibility requires a provider session id");
    let result = crate::wake_coordinator::mark_session_idle_after_turn(
        session_id,
        input.invocation_id,
        Some(1),
    );
    if let Err(error) = result {
        emit_spawn_error_idle_warning(session_id, input.invocation_id, error);
    }
}

fn emit_spawn_error_idle_warning(
    session_id: &str,
    invocation_id: &str,
    error: impl std::fmt::Display,
) {
    tracing::warn!(
        session_id,
        invocation_uuid = invocation_id,
        "Failed to mark balanced spawn-error session idle: {error}"
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
    let result = input
        .agent_runtime_services
        .invocation_lifecycle_service
        .finalize_invocation(super::mapper::spawn_error_finalize_request(
            &input.env.state,
            input.invocation_row_id,
            terminal_reason,
        ));
    let status = spawn_error_finalization_status(result);
    if let Some(error) = status.error {
        formatter::emit_finalize_invocation_warning(error);
    }
    status.finalized
}

struct SpawnErrorFinalizationStatus<E> {
    finalized: bool,
    error: Option<E>,
}

fn spawn_error_finalization_status<T, E>(result: Result<T, E>) -> SpawnErrorFinalizationStatus<E> {
    match result {
        Ok(_) => SpawnErrorFinalizationStatus {
            finalized: true,
            error: None,
        },
        Err(error) => SpawnErrorFinalizationStatus {
            finalized: false,
            error: Some(error),
        },
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
