use oulipoly_runtime::executor;
use oulipoly_state::{CompositeInvocationId, ResultEnvelopeFailureIdentity, StateDb};

use super::super::accessor::BalancedExecutionEnvironment;
use crate::invocation::finalize::FinalizerGuard;
use crate::wiring;

pub(in crate::run::balancing) struct DiagnosticsFallbackInput {
    pub(in crate::run::balancing) diagnostic_input: String,
    pub(in crate::run::balancing) exit_code: i32,
}

pub(in crate::run::balancing) fn diagnostics_fallback_input(
    result: &executor::ExecutionResult,
) -> DiagnosticsFallbackInput {
    DiagnosticsFallbackInput {
        diagnostic_input: super::super::formatter::diagnostic_input(result),
        exit_code: result.exit_code,
    }
}

pub(in crate::run::balancing) fn result_failure_identity(
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

pub(in crate::run::balancing) struct FailureResultEnvelopeInput<'a> {
    pub(in crate::run::balancing) state: &'a StateDb,
    pub(in crate::run::balancing) invocation_id: &'a str,
    pub(in crate::run::balancing) provider_name: &'a str,
    pub(in crate::run::balancing) provider_session_id: Option<&'a str>,
    pub(in crate::run::balancing) exit_code: i32,
    pub(in crate::run::balancing) error_category: Option<&'a str>,
    pub(in crate::run::balancing) terminal_reason: Option<&'a str>,
}

pub(in crate::run::balancing) fn failure_result_envelope_input<'a>(
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

pub(in crate::run::balancing) fn completed_attempt_failure_result_envelope_input<'a>(
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

pub(in crate::run::balancing) struct ArtifactPersistFailureInput<'a, 'state> {
    pub(in crate::run::balancing) agent_runtime_services: &'a wiring::AgentRuntimeServices,
    pub(in crate::run::balancing) env: &'a BalancedExecutionEnvironment,
    pub(in crate::run::balancing) invocation_id: &'a str,
    pub(in crate::run::balancing) invocation_row_id: i64,
    pub(in crate::run::balancing) guard: &'a mut FinalizerGuard<'state>,
    pub(in crate::run::balancing) provider_name: &'a str,
    pub(in crate::run::balancing) provider_session_id: Option<&'a str>,
    pub(in crate::run::balancing) error: &'a str,
}

pub(in crate::run::balancing) struct ArtifactPersistFailureInputSource<'a, 'state> {
    pub(in crate::run::balancing) agent_runtime_services: &'a wiring::AgentRuntimeServices,
    pub(in crate::run::balancing) env: &'a BalancedExecutionEnvironment,
    pub(in crate::run::balancing) invocation: &'a CompositeInvocationId,
    pub(in crate::run::balancing) invocation_row_id: i64,
    pub(in crate::run::balancing) guard: &'a mut FinalizerGuard<'state>,
    pub(in crate::run::balancing) provider_name: &'a str,
    pub(in crate::run::balancing) provider_session_id: Option<&'a str>,
    pub(in crate::run::balancing) error: &'a str,
}

pub(in crate::run::balancing) fn artifact_persist_failure_input<'a, 'state>(
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
