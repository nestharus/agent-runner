//! Shared `OULIPOLY_RESULT` payload construction for stream markers and raw
//! invocation result artifacts.

use serde_json::Value;

/// Identity fields attached to failed `OULIPOLY_RESULT` payloads.
///
/// These fields connect a reported failure back to the runner
/// invocation, provider account, provider session, and chain when those values
/// are known. Provider-derived fields are `Some` after execution has reached a
/// concrete provider and session metadata has been captured or inferred; they
/// remain `None` for pre-provider or otherwise unavailable identities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultEnvelopeFailureIdentity {
    /// Runner-generated invocation UUID for the failed invocation.
    pub agent_runner_invocation_id: String,
    /// Provider account name used for the invocation, when provider selection
    /// reached a concrete account.
    pub provider_name: Option<String>,
    /// Provider-native session identifier associated with the failure, when a
    /// session was captured or supplied.
    pub provider_session_id: Option<String>,
    /// Runner chain identifier linked from the provider session, when known.
    pub agent_runner_chain_id: Option<String>,
}

/// Input used to build a stream/result-artifact `OULIPOLY_RESULT` payload.
///
/// Successful payloads include the common result fields only. Failed payloads
/// include those common fields plus failure identity fields, using
/// `failure_identity` when supplied and falling back to `id` for
/// `agent_runner_invocation_id` when it is absent.
#[derive(Debug, Clone, Copy)]
pub struct ResultEnvelopeInput<'a> {
    /// Invocation UUID emitted as the payload `id`.
    pub id: &'a str,
    /// Whether the invocation succeeded.
    pub success: bool,
    /// Process exit code recorded for the invocation.
    pub exit_code: i32,
    /// Settled failure category for failed invocations, or `None` when no
    /// category applies.
    pub error_category: Option<&'a str>,
    /// Terminal signal or lifecycle reason associated with the invocation, when
    /// one was classified.
    pub terminal_reason: Option<&'a str>,
    /// Completion timestamp formatted as RFC 3339 text.
    pub finished_at: &'a str,
    /// Optional failure identity fields included only in failed payloads.
    pub failure_identity: Option<&'a ResultEnvelopeFailureIdentity>,
}

/// Build the JSON payload for an `OULIPOLY_RESULT` marker or result artifact.
///
/// The payload schema differs by outcome: success payloads contain only the
/// common result fields, while failure payloads also contain
/// `agent_runner_invocation_id`, `provider_name`, `provider_session_id`, and
/// `agent_runner_chain_id`.
pub fn result_envelope_payload(input: ResultEnvelopeInput<'_>) -> Value {
    if input.success {
        success_result_envelope_payload(input)
    } else {
        failure_result_envelope_payload(input)
    }
}

fn success_result_envelope_payload(input: ResultEnvelopeInput<'_>) -> Value {
    serde_json::json!({
        "id": input.id,
        "status": "succeeded",
        "success": true,
        "exit_code": input.exit_code,
        "terminal_reason": input.terminal_reason,
        "error_category": input.error_category,
        "finished_at": input.finished_at,
    })
}

fn failure_result_envelope_payload(input: ResultEnvelopeInput<'_>) -> Value {
    let identity = input.failure_identity;
    let agent_runner_invocation_id = identity
        .map(|identity| identity.agent_runner_invocation_id.as_str())
        .unwrap_or(input.id);
    let provider_name = identity.and_then(|identity| identity.provider_name.as_deref());
    let provider_session_id = identity.and_then(|identity| identity.provider_session_id.as_deref());
    let agent_runner_chain_id =
        identity.and_then(|identity| identity.agent_runner_chain_id.as_deref());

    serde_json::json!({
        "id": input.id,
        "status": "failed",
        "success": false,
        "exit_code": input.exit_code,
        "terminal_reason": input.terminal_reason,
        "error_category": input.error_category,
        "finished_at": input.finished_at,
        "agent_runner_invocation_id": agent_runner_invocation_id,
        "provider_name": provider_name,
        "provider_session_id": provider_session_id,
        "agent_runner_chain_id": agent_runner_chain_id,
    })
}
