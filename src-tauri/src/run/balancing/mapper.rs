//! mapper

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig, ProvidersConfig, SessionsConfig};
use oulipoly_runtime::balancer;
use oulipoly_runtime::executor;
use oulipoly_runtime::executor::terminal_signal::TerminalSignalKind;
use oulipoly_runtime::quota::InFlight;
use oulipoly_runtime::services::{
    ExecutorServiceRequest, InvocationLifecycleFinalizeRequest, InvocationLifecycleStartRequest,
    RoutingServiceRequest,
};
use oulipoly_state::{
    CompositeInvocationId, InvocationStart, ProviderSessionBinding, ResultEnvelopeFailureIdentity,
    StateDb,
};
use uuid::Uuid;

use super::accessor::BalancedExecutionEnvironment;
use super::disposition::{MaybeQuotaVerifyInput, TypedDispositionInput};
use super::finalization::CompletedAttemptInput;
use crate::invocation::finalize::FinalizerGuard;
use crate::session_ingest_cli::{ResumeIngestMode, SessionIngestRequest};
use crate::terminal_outcome_adapter::{TerminalSignalContext, spawn_error_terminal_signal};
use crate::wiring;
use crate::zero_turn_orchestration::ZeroTurnAction;

pub(super) struct BalancedConfigTomlPaths {
    pub(super) providers_path: PathBuf,
    pub(super) sessions_path: PathBuf,
}

pub(super) fn balanced_config_toml_paths(config_root: PathBuf) -> BalancedConfigTomlPaths {
    BalancedConfigTomlPaths {
        providers_path: config_root.join("providers.toml"),
        sessions_path: config_root.join("sessions.toml"),
    }
}

pub(super) fn composite_invocation_id(provider_name: &str) -> CompositeInvocationId {
    CompositeInvocationId {
        source: provider_name.to_string(),
        id: Uuid::new_v4().to_string(),
    }
}

pub(super) fn balanced_invocation_start(
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TerminalSignalBranch {
    QuotaExhaustedRetry,
    MaybeQuotaVerify,
    ProlongedSilenceFail,
    InteractiveFail,
    CompletedAttempt,
}

pub(super) fn terminal_signal_branch(
    signal: &Option<executor::TerminalSignal>,
) -> TerminalSignalBranch {
    let Some(signal) = signal else {
        return TerminalSignalBranch::CompletedAttempt;
    };
    match signal.kind {
        TerminalSignalKind::QuotaExhaustedInband => TerminalSignalBranch::QuotaExhaustedRetry,
        TerminalSignalKind::MaybeQuotaExhausted => TerminalSignalBranch::MaybeQuotaVerify,
        TerminalSignalKind::ProlongedSilence => TerminalSignalBranch::ProlongedSilenceFail,
        TerminalSignalKind::NonzeroExit
        | TerminalSignalKind::SignalExit
        | TerminalSignalKind::SpawnError
        | TerminalSignalKind::RateLimited
        | TerminalSignalKind::Unknown => TerminalSignalBranch::InteractiveFail,
        TerminalSignalKind::CleanExit => TerminalSignalBranch::CompletedAttempt,
    }
}

pub(super) fn diagnostic_exhaustion_category(input: &str) -> Option<String> {
    super::predicate::diagnostic_input_is_exhaustion(input)
        .then(crate::quota_zero_turn::quota_exhausted_category)
}

pub(super) fn balanced_execution_environment(
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

pub(super) fn balance_context<'a>(
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

pub(super) fn routing_service_request<'a>(
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

pub(super) fn quota_retry_budget(model: &ModelConfig) -> usize {
    model.providers.len().max(1) + 1
}

pub(super) fn invocation_lifecycle_start_request<'a>(
    state: &'a StateDb,
    start: &'a InvocationStart,
) -> InvocationLifecycleStartRequest<'a> {
    InvocationLifecycleStartRequest { state, start }
}

pub(super) fn provider_session_binding(provider_session_id: &str) -> ProviderSessionBinding {
    ProviderSessionBinding {
        provider_session_id: provider_session_id.to_string(),
        capture_method: "forced_flag_verified",
        resume_input_id: None,
        provider_session_resolved_account: None,
    }
}

pub(super) fn pending_same_provider_verification_session_id(
    provider_session_id: Option<&str>,
) -> Option<String> {
    provider_session_id.map(str::to_string)
}

pub(super) fn pending_same_provider_verification(
    provider_index: usize,
    provider_session_id: Option<&str>,
) -> (usize, Option<String>) {
    (
        provider_index,
        pending_same_provider_verification_session_id(provider_session_id),
    )
}

pub(super) struct BalancedExecutorRequestInput<'a> {
    pub(super) model: &'a ModelConfig,
    pub(super) provider: &'a ProviderConfig,
    pub(super) provider_index: usize,
    pub(super) prompt_mode: PromptMode,
    pub(super) prompt: &'a str,
    pub(super) working_dir: Option<&'a Path>,
    pub(super) extra_inputs: &'a HashMap<String, Vec<String>>,
    pub(super) invocation_env: &'a str,
    pub(super) start_known_provider_session_id: Option<String>,
}

pub(super) struct BalancedExecutorRequestInputSource<'a> {
    pub(super) model: &'a ModelConfig,
    pub(super) provider: &'a ProviderConfig,
    pub(super) provider_index: usize,
    pub(super) prompt_mode: PromptMode,
    pub(super) prompt: &'a str,
    pub(super) working_dir: Option<&'a Path>,
    pub(super) extra_inputs: &'a HashMap<String, Vec<String>>,
    pub(super) invocation_env: &'a str,
    pub(super) start_known_provider_session_id: Option<String>,
}

pub(super) fn balanced_executor_request_input(
    source: BalancedExecutorRequestInputSource<'_>,
) -> BalancedExecutorRequestInput<'_> {
    BalancedExecutorRequestInput {
        model: source.model,
        provider: source.provider,
        provider_index: source.provider_index,
        prompt_mode: source.prompt_mode,
        prompt: source.prompt,
        working_dir: source.working_dir,
        extra_inputs: source.extra_inputs,
        invocation_env: source.invocation_env,
        start_known_provider_session_id: source.start_known_provider_session_id,
    }
}

pub(super) fn balanced_executor_request(
    input: BalancedExecutorRequestInput<'_>,
) -> ExecutorServiceRequest {
    if let Some(start_known_provider_session_id) = input.start_known_provider_session_id {
        return ExecutorServiceRequest::EffectiveWithStartKnownProviderSessionId {
            model: input.model.clone(),
            provider: input.provider.clone(),
            provider_index: input.provider_index,
            prompt_mode: input.prompt_mode,
            prompt: input.prompt.to_string(),
            working_dir: input.working_dir.map(PathBuf::from),
            extra_inputs: input.extra_inputs.clone(),
            parent_invocation_env: Some(input.invocation_env.to_string()),
            start_known_provider_session_id,
        };
    }
    ExecutorServiceRequest::Effective {
        model: input.model.clone(),
        provider: input.provider.clone(),
        provider_index: input.provider_index,
        prompt_mode: input.prompt_mode,
        prompt: input.prompt.to_string(),
        working_dir: input.working_dir.map(PathBuf::from),
        extra_inputs: input.extra_inputs.clone(),
        parent_invocation_env: Some(input.invocation_env.to_string()),
    }
}

pub(super) type ExecutorModelInput<'a> = (&'a ModelConfig, &'a ProviderConfig, usize, PromptMode);
pub(super) type ExecutorPromptInput<'a> =
    (&'a str, Option<&'a Path>, &'a HashMap<String, Vec<String>>);
pub(super) type ExecutorInvocationInput<'a> = (&'a str, Option<String>);

pub(super) fn balanced_executor_request_for_attempt(
    model_input: ExecutorModelInput<'_>,
    prompt_input: ExecutorPromptInput<'_>,
    invocation_input: ExecutorInvocationInput<'_>,
) -> ExecutorServiceRequest {
    let (model, provider, provider_index, prompt_mode) = model_input;
    let (prompt, working_dir, extra_inputs) = prompt_input;
    let (invocation_env, start_known_provider_session_id) = invocation_input;
    balanced_executor_request(balanced_executor_request_input(
        BalancedExecutorRequestInputSource {
            model,
            provider,
            provider_index,
            prompt_mode,
            prompt,
            working_dir,
            extra_inputs,
            invocation_env,
            start_known_provider_session_id,
        },
    ))
}

pub(super) fn model_provider_names(model: &ModelConfig) -> Vec<String> {
    model
        .providers
        .iter()
        .map(|provider| provider.name.clone())
        .collect()
}

pub(super) struct DiagnosticsFallbackInput {
    pub(super) diagnostic_input: String,
    pub(super) exit_code: i32,
}

pub(super) fn diagnostics_fallback_input(
    result: &executor::ExecutionResult,
) -> DiagnosticsFallbackInput {
    DiagnosticsFallbackInput {
        diagnostic_input: super::formatter::diagnostic_input(result),
        exit_code: result.exit_code,
    }
}

pub(super) fn result_failure_identity(
    invocation_id: &str,
    provider_name: &str,
    provider_session_id: Option<&str>,
    agent_runner_chain_id: Option<String>,
) -> ResultEnvelopeFailureIdentity {
    ResultEnvelopeFailureIdentity {
        agent_runner_invocation_id: invocation_id.to_string(),
        provider_name: Some(provider_name.to_string()),
        provider_session_id: provider_session_id.map(str::to_string),
        agent_runner_chain_id,
    }
}

pub(super) struct FailureResultEnvelopeInput<'a> {
    pub(super) state: &'a StateDb,
    pub(super) invocation_id: &'a str,
    pub(super) provider_name: &'a str,
    pub(super) provider_session_id: Option<&'a str>,
    pub(super) exit_code: i32,
    pub(super) error_category: Option<&'a str>,
    pub(super) terminal_reason: Option<&'a str>,
}

pub(super) fn failure_result_envelope_input<'a>(
    state: &'a StateDb,
    invocation_id: &'a str,
    provider_name: &'a str,
    provider_session_id: Option<&'a str>,
    exit_code: i32,
    error_category: Option<&'a str>,
    terminal_reason: Option<&'a str>,
) -> FailureResultEnvelopeInput<'a> {
    FailureResultEnvelopeInput {
        state,
        invocation_id,
        provider_name,
        provider_session_id,
        exit_code,
        error_category,
        terminal_reason,
    }
}

pub(super) fn completed_attempt_failure_result_envelope_input<'a>(
    state: &'a StateDb,
    invocation_id: &'a str,
    provider_name: &'a str,
    provider_session_id: Option<&'a str>,
    result: &'a executor::ExecutionResult,
    error_category: Option<&'a str>,
) -> FailureResultEnvelopeInput<'a> {
    failure_result_envelope_input(
        state,
        invocation_id,
        provider_name,
        provider_session_id,
        result.exit_code,
        error_category,
        result.terminal_reason.as_deref(),
    )
}

pub(super) struct ArtifactPersistFailureInput<'a, 'state> {
    pub(super) agent_runtime_services: &'a wiring::AgentRuntimeServices,
    pub(super) env: &'a BalancedExecutionEnvironment,
    pub(super) invocation_id: &'a str,
    pub(super) invocation_row_id: i64,
    pub(super) guard: &'a mut FinalizerGuard<'state>,
    pub(super) provider_name: &'a str,
    pub(super) provider_session_id: Option<&'a str>,
    pub(super) error: &'a str,
}

pub(super) struct ArtifactPersistFailureInputSource<'a, 'state> {
    pub(super) agent_runtime_services: &'a wiring::AgentRuntimeServices,
    pub(super) env: &'a BalancedExecutionEnvironment,
    pub(super) invocation: &'a CompositeInvocationId,
    pub(super) invocation_row_id: i64,
    pub(super) guard: &'a mut FinalizerGuard<'state>,
    pub(super) provider_name: &'a str,
    pub(super) provider_session_id: Option<&'a str>,
    pub(super) error: &'a str,
}

pub(super) fn artifact_persist_failure_input<'a, 'state>(
    source: ArtifactPersistFailureInputSource<'a, 'state>,
) -> ArtifactPersistFailureInput<'a, 'state> {
    ArtifactPersistFailureInput {
        agent_runtime_services: source.agent_runtime_services,
        env: source.env,
        invocation_id: &source.invocation.id,
        invocation_row_id: source.invocation_row_id,
        guard: source.guard,
        provider_name: source.provider_name,
        provider_session_id: source.provider_session_id,
        error: source.error,
    }
}

pub(super) fn spawn_error_finalize_request<'a>(
    state: &'a StateDb,
    invocation_row_id: i64,
    terminal_reason: &'a str,
) -> InvocationLifecycleFinalizeRequest<'a> {
    InvocationLifecycleFinalizeRequest {
        state,
        invocation_row_id,
        success: false,
        exit_code: -1,
        error_category: Some(terminal_reason),
        terminal_reason: Some(terminal_reason),
    }
}

pub(super) fn spawn_error_signal(provider_name: &str, error: String) -> executor::TerminalSignal {
    spawn_error_terminal_signal(provider_name, error)
}

pub(super) struct TerminalSignalContextIds {
    pub(super) invocation_uuid: Uuid,
    pub(super) session_uuid: Option<Uuid>,
}

pub(super) fn terminal_signal_context_ids(
    invocation_id: &str,
    provider_session_id: Option<&str>,
) -> TerminalSignalContextIds {
    TerminalSignalContextIds {
        invocation_uuid: super::parser::parse_invocation_uuid(invocation_id),
        session_uuid: crate::dispatch::provider_session_marker_uuid(provider_session_id),
    }
}

pub(super) struct TerminalSignalContextInput<'a, W: std::io::Write> {
    pub(super) ids: &'a TerminalSignalContextIds,
    pub(super) provider_name: &'a str,
    pub(super) state: &'a StateDb,
    pub(super) stderr: &'a mut W,
}

pub(super) fn terminal_signal_context<'a, W: std::io::Write>(
    input: TerminalSignalContextInput<'a, W>,
) -> TerminalSignalContext<'a, W> {
    TerminalSignalContext {
        invocation_id: &input.ids.invocation_uuid,
        session_id: input.ids.session_uuid.as_ref(),
        provider: input.provider_name,
        state_db: input.state,
        stderr: input.stderr,
    }
}

pub(super) fn terminal_signal_context_for_attempt<'a, W: std::io::Write>(
    ids: &'a TerminalSignalContextIds,
    provider_name: &'a str,
    state: &'a StateDb,
    stderr: &'a mut W,
) -> TerminalSignalContext<'a, W> {
    terminal_signal_context(TerminalSignalContextInput {
        ids,
        provider_name,
        state,
        stderr,
    })
}

pub(super) struct TypedDispositionInputSource<'a, 'state, 'ctx> {
    pub(super) agent_runtime_services: &'a wiring::AgentRuntimeServices,
    pub(super) env: &'a BalancedExecutionEnvironment,
    pub(super) invocation: &'a CompositeInvocationId,
    pub(super) invocation_row_id: i64,
    pub(super) guard: &'a mut FinalizerGuard<'state>,
    pub(super) provider_name: &'a str,
    pub(super) provider_index: usize,
    pub(super) result: &'a executor::ExecutionResult,
    pub(super) terminal_signal: &'a Option<executor::TerminalSignal>,
    pub(super) terminal_signal_ctx: &'a mut TerminalSignalContext<'ctx, std::io::Stderr>,
    pub(super) zero_turn_provider_session_id: Option<&'a str>,
    pub(super) attempts: usize,
    pub(super) max_attempts: usize,
}

pub(super) fn typed_disposition_input<'a, 'state, 'ctx>(
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

pub(super) type AttemptLifecycleInput<'a, 'state> = (
    &'a wiring::AgentRuntimeServices,
    &'a BalancedExecutionEnvironment,
    &'a CompositeInvocationId,
    i64,
    &'a mut FinalizerGuard<'state>,
);
pub(super) type AttemptProviderInput<'a> = (&'a str, usize, Option<&'a str>);
pub(super) type AttemptTerminalInput<'a, 'ctx> = (
    &'a executor::ExecutionResult,
    &'a Option<executor::TerminalSignal>,
    &'a mut TerminalSignalContext<'ctx, std::io::Stderr>,
);
pub(super) type AttemptBudgetInput = (usize, usize);

pub(super) struct SpawnErrorInput<'a, 'state> {
    pub(super) agent_runtime_services: &'a wiring::AgentRuntimeServices,
    pub(super) env: &'a BalancedExecutionEnvironment,
    pub(super) invocation_id: &'a str,
    pub(super) invocation_row_id: i64,
    pub(super) guard: &'a mut FinalizerGuard<'state>,
    pub(super) provider_name: &'a str,
    pub(super) provider_session_id: Option<&'a str>,
    pub(super) error: String,
}

pub(super) fn spawn_error_input_for_attempt<'a, 'state>(
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

pub(super) fn typed_disposition_input_for_attempt<'a, 'state, 'ctx>(
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

pub(super) fn maybe_quota_verify_input<'a, 'state, 'ctx>(
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

pub(super) fn maybe_quota_verify_input_for_attempt<'a, 'state, 'ctx>(
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

pub(super) struct CompletedAttemptInputSource<'a, 'state, 'ctx> {
    pub(super) agent_runtime_services: &'a wiring::AgentRuntimeServices,
    pub(super) env: &'a BalancedExecutionEnvironment,
    pub(super) invocation: &'a CompositeInvocationId,
    pub(super) invocation_row_id: i64,
    pub(super) guard: &'a mut FinalizerGuard<'state>,
    pub(super) provider_name: &'a str,
    pub(super) model: &'a ModelConfig,
    pub(super) provider_index: usize,
    pub(super) result: &'a executor::ExecutionResult,
    pub(super) terminal_signal: &'a Option<executor::TerminalSignal>,
    pub(super) terminal_signal_ctx: &'a mut TerminalSignalContext<'ctx, std::io::Stderr>,
    pub(super) all_models: &'a HashMap<String, ModelConfig>,
    pub(super) working_dir: Option<&'a Path>,
    pub(super) zero_turn_provider_session_id: Option<&'a str>,
    pub(super) attempts: usize,
    pub(super) max_attempts: usize,
}

pub(super) fn completed_attempt_input<'a, 'state, 'ctx>(
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

pub(super) type CompletedAttemptRunInput<'a> = (
    &'a ModelConfig,
    &'a HashMap<String, ModelConfig>,
    Option<&'a Path>,
);

pub(super) fn completed_attempt_input_for_attempt<'a, 'state, 'ctx>(
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

pub(super) fn maybe_quota_finalize_request<'a>(
    state: &'a StateDb,
    invocation_row_id: i64,
    exit_code: i32,
    confirmed: bool,
    terminal_reason: &'a str,
) -> InvocationLifecycleFinalizeRequest<'a> {
    InvocationLifecycleFinalizeRequest {
        state,
        invocation_row_id,
        success: false,
        exit_code,
        error_category: confirmed.then_some("quota_exhausted"),
        terminal_reason: Some(terminal_reason),
    }
}

pub(super) fn quota_exhausted_finalize_request<'a>(
    state: &'a StateDb,
    invocation_row_id: i64,
    exit_code: i32,
    terminal_reason: &'a str,
) -> InvocationLifecycleFinalizeRequest<'a> {
    InvocationLifecycleFinalizeRequest {
        state,
        invocation_row_id,
        success: false,
        exit_code,
        error_category: Some("quota_exhausted"),
        terminal_reason: Some(terminal_reason),
    }
}

pub(super) fn terminal_failure_finalize_request<'a>(
    state: &'a StateDb,
    invocation_row_id: i64,
    result: &'a executor::ExecutionResult,
    terminal_reason: &'a str,
) -> InvocationLifecycleFinalizeRequest<'a> {
    InvocationLifecycleFinalizeRequest {
        state,
        invocation_row_id,
        success: false,
        exit_code: result.exit_code,
        error_category: terminal_failure_error_category(result, terminal_reason),
        terminal_reason: Some(terminal_reason),
    }
}

pub(super) fn returned_artifacts_finalize_request(
    state: &StateDb,
    invocation_row_id: i64,
) -> InvocationLifecycleFinalizeRequest<'_> {
    InvocationLifecycleFinalizeRequest {
        state,
        invocation_row_id,
        success: false,
        exit_code: 1,
        error_category: Some("returned_artifacts"),
        terminal_reason: Some("returned_artifacts_persist_failed"),
    }
}

pub(super) fn completed_finalize_request<'a>(
    state: &'a StateDb,
    invocation_row_id: i64,
    success: bool,
    exit_code: i32,
    error_category: Option<&'a str>,
    terminal_reason: Option<&'a str>,
) -> InvocationLifecycleFinalizeRequest<'a> {
    InvocationLifecycleFinalizeRequest {
        state,
        invocation_row_id,
        success,
        exit_code,
        error_category,
        terminal_reason,
    }
}

pub(super) fn terminal_failure_error_category<'a>(
    result: &'a executor::ExecutionResult,
    terminal_reason: &'a str,
) -> Option<&'a str> {
    crate::terminal_outcome_adapter::terminal_signal_error_category(
        &result.terminal_signal,
        terminal_reason,
    )
}

pub(super) struct CompletedSessionIngestRequestInput<'a> {
    pub(super) agent_runtime_services: &'a wiring::AgentRuntimeServices,
    pub(super) env: &'a BalancedExecutionEnvironment,
    pub(super) model: &'a ModelConfig,
    pub(super) provider_name: &'a str,
    pub(super) invocation_row_id: i64,
    pub(super) invocation_id: &'a str,
    pub(super) effective_cwd: &'a Path,
}

pub(super) fn completed_session_ingest_request<'a>(
    input: CompletedSessionIngestRequestInput<'a>,
) -> SessionIngestRequest<'a> {
    SessionIngestRequest {
        state: &input.env.state,
        sessions_cfg: &input.env.sessions_cfg,
        providers_cfg: Some(&input.env.providers_cfg),
        provider_name: input.provider_name,
        external_provider: crate::session_ingest_cli::session_external_provider_identity(
            input.agent_runtime_services,
            Some(input.model),
            input.provider_name,
        ),
        invocation_row_id: input.invocation_row_id,
        invocation_uuid: input.invocation_id,
        effective_cwd: Some(input.effective_cwd),
        mode: ResumeIngestMode::Unpinned {
            capture_method: "turn_script",
        },
    }
}

pub(super) fn completed_session_ingest_request_for_attempt<'a>(
    input: &'a CompletedAttemptInput<'_, '_, '_>,
    effective_cwd: &'a Path,
) -> SessionIngestRequest<'a> {
    completed_session_ingest_request(CompletedSessionIngestRequestInput {
        agent_runtime_services: input.agent_runtime_services,
        env: input.env,
        model: input.model,
        provider_name: input.provider_name,
        invocation_row_id: input.invocation_row_id,
        invocation_id: &input.invocation.id,
        effective_cwd,
    })
}
