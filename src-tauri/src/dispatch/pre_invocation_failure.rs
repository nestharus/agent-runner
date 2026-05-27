//! Declared roles: orchestration, mapper, formatter

use crate::invocation::result_envelope::emit_pre_invocation_failure_line;

pub(crate) fn emit_pre_invocation_failure(
    stage: &str,
    model_name: Option<&str>,
    provider_index: Option<usize>,
    attempted_providers: Vec<String>,
    reason: Option<&str>,
) {
    let payload = payload(
        stage,
        model_name,
        provider_index,
        attempted_providers,
        reason,
    );
    emit_payload_or_warn(&payload);
}

fn payload(
    stage: &str,
    model_name: Option<&str>,
    provider_index: Option<usize>,
    attempted_providers: Vec<String>,
    reason: Option<&str>,
) -> serde_json::Value {
    super::formatter::pre_invocation_failure_payload(
        super::formatter::PreInvocationFailurePayloadInput {
            stage,
            model_name,
            provider_index,
            attempted_providers,
            reason,
            finished_at: pre_invocation_finished_at(),
            message: super::formatter::format_stage_reason(stage, reason),
        },
    )
}

fn pre_invocation_finished_at() -> String {
    super::formatter::format_timestamp_rfc3339(super::clock::utc_now())
}

fn emit_payload_or_warn(payload: &serde_json::Value) {
    match super::formatter::serialize_json(payload) {
        Ok(json) => emit_pre_invocation_failure_line(&json),
        Err(err) => {
            super::formatter::emit_json_serialization_warning("pre-invocation failure", &err)
        }
    }
}
