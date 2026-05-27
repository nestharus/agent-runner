//! Declared roles: formatter

pub(crate) fn format_timestamp_rfc3339(timestamp: chrono::DateTime<chrono::Utc>) -> String {
    timestamp.to_rfc3339()
}

pub(super) fn emit_stderr_line(line: &str) {
    eprintln!("{line}");
}

pub(super) fn serialize_json(payload: &serde_json::Value) -> Result<String, serde_json::Error> {
    serde_json::to_string(payload)
}

pub(super) fn emit_json_serialization_warning(context: &str, err: &serde_json::Error) {
    eprintln!("Warning: Failed to serialize {context}: {err}");
}

pub(super) fn format_stage_reason(stage: &str, reason: Option<&str>) -> String {
    match reason {
        Some(reason) => format!("{stage}: {reason}"),
        None => stage.to_string(),
    }
}

pub(super) fn format_resume_agent_file_incompatible_error() -> String {
    "--resume is incompatible with --agent-file.".to_string()
}

pub(super) struct PreInvocationFailurePayloadInput<'a> {
    pub(super) stage: &'a str,
    pub(super) model_name: Option<&'a str>,
    pub(super) provider_index: Option<usize>,
    pub(super) attempted_providers: Vec<String>,
    pub(super) reason: Option<&'a str>,
    pub(super) finished_at: String,
    pub(super) message: String,
}

pub(super) fn pre_invocation_failure_payload(
    input: PreInvocationFailurePayloadInput<'_>,
) -> serde_json::Value {
    serde_json::json!({
        "failure_kind": "pre_invocation",
        "stage": input.stage,
        "status": "failed",
        "success": false,
        "exit_code": serde_json::Value::Null,
        "terminal_reason": "pre_invocation_failure",
        "error_category": serde_json::Value::Null,
        "finished_at": input.finished_at,
        "message": input.message,
        "detail": {
            "model_name": input.model_name,
            "provider_index": input.provider_index,
            "attempted_providers": input.attempted_providers,
            "reason": input.reason,
        },
        "agent_runner_invocation_id": serde_json::Value::Null,
        "provider_name": serde_json::Value::Null,
        "provider_session_id": serde_json::Value::Null,
        "agent_runner_chain_id": serde_json::Value::Null,
    })
}
