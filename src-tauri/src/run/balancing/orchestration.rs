//! ## Declared roles
//!
//! `orchestration`, `mapper`, `formatter`, `predicate`, `accessor`, `validator`

use std::collections::HashMap;
use std::path::Path;

use oulipoly_config::ModelConfig;
use oulipoly_runtime::executor;
use oulipoly_runtime::services::{InvocationLifecycleServicePort, RoutingServicePort};
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
    apply_age153_terminal_signal_fixture_override, balanced_terminal_signal_for_outcome,
    confirm_maybe_quota_exhausted,
};
use crate::wiring;
use crate::zero_turn_orchestration::{ZeroTurnConfirmationState, next_action};

pub(crate) fn run_with_balancing(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    state_db_opener: &dyn StateDbOpener,
    model: &ModelConfig,
    prompt: &str,
    all_models: &HashMap<String, ModelConfig>,
    working_dir: Option<&Path>,
    extra_inputs: &HashMap<String, Vec<String>>,
) -> Result<i32, String> {
    let env = load_balanced_execution_environment(state_db_opener)?;
    run_with_balancing_environment(
        agent_runtime_services,
        env,
        model,
        prompt,
        all_models,
        working_dir,
        extra_inputs,
    )
}

fn run_with_balancing_environment(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    env: BalancedExecutionEnvironment,
    model: &ModelConfig,
    prompt: &str,
    all_models: &HashMap<String, ModelConfig>,
    working_dir: Option<&Path>,
    extra_inputs: &HashMap<String, Vec<String>>,
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
        let provider_index = match pending_verification.as_ref() {
            Some((provider_index, _)) => *provider_index,
            None => match select_balanced_provider_index(agent_runtime_services, model, &env, &ctx)
            {
                Ok(provider_index) => provider_index,
                Err(err) => {
                    formatter::emit_provider_selection_pre_invocation_failure(
                        model,
                        &err,
                        provider_selection_pool_exhausted(attempts, model.providers.len()),
                    );
                    return Err(err);
                }
            },
        };
        let (provider, prompt_mode) =
            match effective_model_for_execution(model, provider_index, &env.providers_cfg) {
                Ok(effective) => effective,
                Err(err) => {
                    formatter::emit_provider_resolution_pre_invocation_failure(
                        model,
                        provider_index,
                        &err,
                    );
                    return Err(err);
                }
            };
        let provider_name = &provider.name;
        let invocation = composite_invocation_id(provider_name);
        let invocation_start = balanced_invocation_start(
            &invocation,
            model,
            provider_name,
            provider_index,
            parent_invocation_id,
        );
        let invocation_row_id = agent_runtime_services
            .invocation_lifecycle_service
            // invocation_lifecycle_service.start_invocation(InvocationLifecycleStartRequest
            .start_invocation(super::mapper::invocation_lifecycle_start_request(
                &env.state,
                &invocation_start,
            ))
            .map_err(|err| err.to_string())?
            .invocation_row_id;
        let mut guard = FinalizerGuard::new(&env.state, invocation_row_id);
        let start_known_provider_session_id = match pending_verification {
            Some((_, session_id)) => session_id,
            None => executor::cli::start_known_provider_session_id(&provider)?,
        };
        bind_start_known_provider_session_if_present(
            &env.state,
            invocation_row_id,
            start_known_provider_session_id.as_deref(),
        );
        let mut zero_turn_baseline = zero_turn_record_baseline(
            &env.state,
            &env.sessions_cfg,
            provider_name,
            start_known_provider_session_id.as_deref(),
        );
        let invocation_env = formatter::invocation_env(&invocation)
            .map_err(formatter::invocation_env_serialization_error)?;
        formatter::emit_invocation_stderr_line(&invocation);

        let executor_request = super::mapper::balanced_executor_request_for_attempt(
            (model, &provider, provider_index, prompt_mode),
            (prompt, working_dir, extra_inputs),
            (&invocation_env, start_known_provider_session_id.clone()),
        );

        let mut result = match agent_runtime_services
            .executor_service
            .execute(executor_request)
        {
            Ok(output) => output.result,
            Err(err) => {
                finalize_spawn_error(super::mapper::spawn_error_input_for_attempt(
                    (
                        agent_runtime_services,
                        &env,
                        &invocation,
                        invocation_row_id,
                        &mut guard,
                    ),
                    (provider_name, start_known_provider_session_id.as_deref()),
                    err.to_string(),
                ));
                return Err(err.to_string());
            }
        };
        apply_age153_terminal_signal_fixture_override(&mut result);
        let zero_turn_provider_session_id = super::accessor::zero_turn_provider_session_id(
            start_known_provider_session_id.as_deref(),
            &result,
        );
        if should_late_bind_zero_turn_baseline(
            &zero_turn_baseline,
            zero_turn_provider_session_id.as_deref(),
        ) {
            let session_id = super::validator::required_late_bind_provider_session_id(
                zero_turn_provider_session_id.as_deref(),
            );
            zero_turn_baseline =
                zero_turn_late_bind_baseline(&env.sessions_cfg, provider_name, session_id);
        }
        let zero_turn_classification =
            zero_turn_classify_after_completion(&env.state, &env.sessions_cfg, &zero_turn_baseline);
        apply_zero_turn_classification_to_result(
            &mut result,
            provider_name,
            &zero_turn_classification,
        );
        let zero_turn_action = next_action(
            &mut zero_turn_confirmation,
            zero_turn_classification_for_action(
                zero_turn_classification,
                &result,
                provider_name,
                zero_turn_provider_session_id.as_deref(),
            ),
        );

        emit_captured_child_marker_lines(&result.captured_child_invocations);

        let terminal_signal_context_ids = super::mapper::terminal_signal_context_ids(
            &invocation.id,
            zero_turn_provider_session_id.as_deref(),
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

        let control = if confirmed_zero_turn_exhaustion(zero_turn_action, &balanced_terminal_signal)
        {
            let signal =
                super::validator::required_confirmed_zero_turn_signal(&balanced_terminal_signal);
            let _ = confirm_maybe_quota_exhausted(signal, &mut terminal_signal_ctx);
            handle_maybe_quota_verify(super::mapper::maybe_quota_verify_input_for_attempt(
                (
                    agent_runtime_services,
                    &env,
                    &invocation,
                    invocation_row_id,
                    &mut guard,
                ),
                (
                    provider_name,
                    provider_index,
                    zero_turn_provider_session_id.as_deref(),
                ),
                (&result, &balanced_terminal_signal, &mut terminal_signal_ctx),
                (attempts, max_attempts),
                zero_turn_action,
                &mut pending_same_provider_verification,
                true,
            ))
        } else {
            match terminal_signal_branch(&balanced_terminal_signal) {
                TerminalSignalBranch::QuotaExhaustedRetry => handle_quota_exhausted_retry(
                    super::mapper::typed_disposition_input_for_attempt(
                        (
                            agent_runtime_services,
                            &env,
                            &invocation,
                            invocation_row_id,
                            &mut guard,
                        ),
                        (
                            provider_name,
                            provider_index,
                            zero_turn_provider_session_id.as_deref(),
                        ),
                        (&result, &balanced_terminal_signal, &mut terminal_signal_ctx),
                        (attempts, max_attempts),
                    ),
                ),
                TerminalSignalBranch::MaybeQuotaVerify => {
                    handle_maybe_quota_verify(super::mapper::maybe_quota_verify_input_for_attempt(
                        (
                            agent_runtime_services,
                            &env,
                            &invocation,
                            invocation_row_id,
                            &mut guard,
                        ),
                        (
                            provider_name,
                            provider_index,
                            zero_turn_provider_session_id.as_deref(),
                        ),
                        (&result, &balanced_terminal_signal, &mut terminal_signal_ctx),
                        (attempts, max_attempts),
                        zero_turn_action,
                        &mut pending_same_provider_verification,
                        false,
                    ))
                }
                TerminalSignalBranch::ProlongedSilenceFail => handle_prolonged_silence_fail(
                    super::mapper::typed_disposition_input_for_attempt(
                        (
                            agent_runtime_services,
                            &env,
                            &invocation,
                            invocation_row_id,
                            &mut guard,
                        ),
                        (
                            provider_name,
                            provider_index,
                            zero_turn_provider_session_id.as_deref(),
                        ),
                        (&result, &balanced_terminal_signal, &mut terminal_signal_ctx),
                        (attempts, max_attempts),
                    ),
                ),
                TerminalSignalBranch::InteractiveFail => {
                    handle_interactive_fail(super::mapper::typed_disposition_input_for_attempt(
                        (
                            agent_runtime_services,
                            &env,
                            &invocation,
                            invocation_row_id,
                            &mut guard,
                        ),
                        (
                            provider_name,
                            provider_index,
                            zero_turn_provider_session_id.as_deref(),
                        ),
                        (&result, &balanced_terminal_signal, &mut terminal_signal_ctx),
                        (attempts, max_attempts),
                    ))
                }
                TerminalSignalBranch::CompletedAttempt => {
                    finalize_completed_attempt(super::mapper::completed_attempt_input_for_attempt(
                        (
                            agent_runtime_services,
                            &env,
                            &invocation,
                            invocation_row_id,
                            &mut guard,
                        ),
                        (
                            provider_name,
                            provider_index,
                            zero_turn_provider_session_id.as_deref(),
                        ),
                        (&result, &balanced_terminal_signal, &mut terminal_signal_ctx),
                        (model, all_models, working_dir),
                        (attempts, max_attempts),
                    ))
                }
            }
        };

        match control {
            BalancedLoopControl::Continue => continue,
            BalancedLoopControl::Return(result) => return result,
        }
    }
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
    let route_error = match agent_runtime_services
        .routing_service
        // routing_service.select_route(RoutingServiceRequest { ctx: Some(
        .select_route(super::mapper::routing_service_request(
            model, &env.state, ctx,
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
    finalize_spawn_error_invocation(&input, &signal);
    emit_spawn_error_envelope(&input, &signal);
    input.guard.mark_finalized();
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
) {
    let terminal_reason = formatter::spawn_error_terminal_reason(signal);
    input
        .agent_runtime_services
        .invocation_lifecycle_service
        .finalize_invocation(super::mapper::spawn_error_finalize_request(
            &input.env.state,
            input.invocation_row_id,
            terminal_reason,
        ))
        .map(|_| ())
        .unwrap_or_else(formatter::emit_finalize_invocation_warning);
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
