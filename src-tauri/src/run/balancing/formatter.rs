//! Declared roles: formatter, mapper, predicate, orchestration

use std::fmt::Display;
use std::io::Write as _;

use oulipoly_config::ModelConfig;
use oulipoly_runtime::executor;
use oulipoly_state::{CompositeInvocationId, StateDb};

use crate::error_emit::emit_unknown_diagnostic;
use crate::invocation::result_envelope::emit_result_envelope;

use super::accessor::result_failure_chain_id;
use super::mapper::{FailureResultEnvelopeInput, result_failure_identity};

pub(super) fn emit_pool_exhausted_pre_invocation_failure(model: &ModelConfig, reason: &str) {
    crate::dispatch::emit_pre_invocation_failure(
        "pool_exhausted",
        Some(&model.name),
        None,
        super::mapper::model_provider_names(model),
        Some(reason),
    );
}

pub(super) fn invocation_env_serialization_error(error: serde_json::Error) -> String {
    format!("Failed to serialize invocation id: {error}")
}

pub(super) fn invocation_env(
    invocation: &CompositeInvocationId,
) -> Result<String, serde_json::Error> {
    serde_json::to_string(&invocation)
}

pub(super) fn emit_invocation_stderr_line(invocation: &CompositeInvocationId) {
    emit_stderr(&invocation.stderr_line());
}

pub(super) fn diagnostic_input(result: &executor::ExecutionResult) -> String {
    crate::redaction::diagnostic_input(&result.stderr, &result.stdout)
}

pub(super) fn spawn_error_terminal_reason(signal: &executor::TerminalSignal) -> &str {
    crate::terminal_outcome_adapter::typed_terminal_reason_fallback(signal).unwrap_or("spawn_error")
}

pub(super) fn exhausted_attempt_reason(
    route_error: Option<String>,
    model_name: &str,
    retry_budget: usize,
) -> String {
    route_error.unwrap_or_else(|| {
        crate::quota_zero_turn::format_quota_retry_budget_exhausted(model_name, retry_budget)
    })
}

pub(super) fn emit_provider_selection_pre_invocation_failure(
    model: &ModelConfig,
    reason: &str,
    pool_exhausted: bool,
) {
    if pool_exhausted {
        emit_pool_exhausted_pre_invocation_failure(model, reason);
        return;
    }
    crate::dispatch::emit_pre_invocation_failure(
        "provider_selection",
        Some(&model.name),
        None,
        Vec::new(),
        Some(reason),
    );
}

pub(super) fn emit_provider_resolution_pre_invocation_failure(
    model: &ModelConfig,
    provider_index: usize,
    reason: &str,
) {
    crate::dispatch::emit_pre_invocation_failure(
        "provider_resolution",
        Some(&model.name),
        Some(provider_index),
        vec![model.providers[provider_index].name.clone()],
        Some(reason),
    );
}

pub(super) fn emit_session_capture_failure(reason: &str) {
    eprintln!("[session-capture] {reason}");
}

pub(super) fn emit_session_capture_update_warning(error: impl Display) {
    eprintln!("Warning: Failed to update session capture: {error}");
}

pub(super) fn emit_quota_tick_warning(error: impl Display) {
    eprintln!("Warning: Failed to bump quota tick: {error}");
}

pub(super) fn emit_provider_session_binding_warning(error: impl Display) {
    eprintln!("Warning: Failed to bind provider session: {error}");
}

pub(super) fn emit_finalize_invocation_warning(error: impl Display) {
    eprintln!("Warning: Failed to finalize invocation: {error}");
}

pub(super) fn emit_returned_artifacts_error(error: impl Display) {
    eprintln!("Error: Failed to record returned artifacts: {error}");
}

pub(super) fn emit_routing_retry(provider_name: &str) {
    eprintln!(
        "[routing] provider {provider_name} returned quota_exhausted; retrying another provider"
    );
}

pub(super) fn emit_stderr(stderr: &str) {
    eprintln!("{stderr}");
}

pub(super) fn emit_success_output(
    invocation_id: &str,
    exit_code: i32,
    error_category: Option<&str>,
    terminal_reason: Option<&str>,
    stdout: &[u8],
) {
    let _ = std::io::stdout().write_all(stdout);
    emit_result_envelope(
        invocation_id,
        true,
        exit_code,
        error_category,
        terminal_reason,
        None,
    );
}

pub(super) fn emit_failure_output(input: FailureResultEnvelopeInput<'_>, stderr: &str) {
    emit_failure_result_envelope(input);
    emit_stderr(stderr);
}

pub(super) fn emit_failure_output_with_diagnostics(
    input: FailureResultEnvelopeInput<'_>,
    stderr: &str,
    error_category: Option<&str>,
) {
    emit_failure_output(input, stderr);
    if let Some(category) = error_category {
        eprintln!("[diagnostics: {category}]");
    }
}

pub(super) fn emit_failure_result_envelope(input: FailureResultEnvelopeInput<'_>) {
    let agent_runner_chain_id =
        result_failure_chain_id(input.state, input.provider_name, input.provider_session_id);
    let failure_identity = result_failure_identity(
        input.invocation_id,
        input.provider_name,
        input.provider_session_id,
        agent_runner_chain_id,
    );
    emit_result_envelope(
        input.invocation_id,
        false,
        input.exit_code,
        input.error_category,
        input.terminal_reason,
        Some(&failure_identity),
    );
}

pub(super) fn emit_spawn_error_failure_result_envelope(
    state: &StateDb,
    invocation_id: &str,
    provider_name: &str,
    provider_session_id: Option<&str>,
    terminal_reason: &str,
) {
    emit_failure_result_envelope(super::mapper::failure_result_envelope_input(
        state,
        invocation_id,
        provider_name,
        provider_session_id,
        -1,
        Some(terminal_reason),
        Some(terminal_reason),
    ));
}

pub(super) fn emit_unknown_diagnostic_if_settled_unknown(
    state: &StateDb,
    provider_name: &str,
    provider_index: usize,
    result: &executor::ExecutionResult,
    settled_unknown: bool,
    retry_rotation_disposition: &str,
) {
    if settled_unknown {
        emit_unknown_diagnostic(
            state,
            provider_name,
            provider_index,
            result,
            retry_rotation_disposition,
        );
    }
}
