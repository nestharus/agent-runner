//! ## Declared roles
//!
//! `predicate`, `orchestration`, `mapper`, `formatter`, `validator`

use oulipoly_state::{ResultEnvelopeFailureIdentity, ResultEnvelopeInput, result_envelope_payload};
use std::io::Write as _;

pub(crate) fn should_emit_invocation_line(is_terminal: bool) -> bool {
    !is_terminal
}

pub(crate) fn emit_result_envelope(
    uuid: &str,
    success: bool,
    exit_code: i32,
    error_category: Option<&str>,
    terminal_reason: Option<&str>,
    failure_identity: Option<&ResultEnvelopeFailureIdentity>,
) -> std::io::Result<()> {
    let finished_at = crate::dispatch::format_timestamp_rfc3339(crate::dispatch::utc_now());
    emit_result_envelope_at(ResultEnvelopeEmitInput {
        uuid,
        success,
        exit_code,
        error_category,
        terminal_reason,
        failure_identity,
        finished_at: &finished_at,
    })
}

fn emit_result_envelope_at(input: ResultEnvelopeEmitInput<'_>) -> std::io::Result<()> {
    let uuid = input.uuid;
    let payload = result_envelope_payload_for_emit(input);
    emit_result_envelope_payload(uuid, &payload)
}

struct ResultEnvelopeEmitInput<'a> {
    uuid: &'a str,
    success: bool,
    exit_code: i32,
    error_category: Option<&'a str>,
    terminal_reason: Option<&'a str>,
    failure_identity: Option<&'a ResultEnvelopeFailureIdentity>,
    finished_at: &'a str,
}

fn result_envelope_payload_for_emit(input: ResultEnvelopeEmitInput<'_>) -> serde_json::Value {
    result_envelope_payload(result_envelope_input_for_emit(input))
}

fn result_envelope_input_for_emit(input: ResultEnvelopeEmitInput<'_>) -> ResultEnvelopeInput<'_> {
    ResultEnvelopeInput {
        id: input.uuid,
        success: input.success,
        exit_code: input.exit_code,
        error_category: input.error_category,
        terminal_reason: input.terminal_reason,
        finished_at: input.finished_at,
        failure_identity: input.failure_identity,
    }
}

fn emit_result_envelope_payload(uuid: &str, payload: &serde_json::Value) -> std::io::Result<()> {
    let json = serialize_result_envelope_payload(payload).map_err(|error| {
        std::io::Error::other(format!(
            "failed to serialize result envelope for {uuid}: {error}"
        ))
    })?;
    emit_result_envelope_line(&json)
}

fn serialize_result_envelope_payload(payload: &serde_json::Value) -> Result<String, String> {
    serde_json::to_string(payload).map_err(|err| err.to_string())
}

fn emit_result_envelope_line(json: &str) -> std::io::Result<()> {
    emit_stdout_marker_line("OULIPOLY_RESULT", json)
}

pub(crate) fn emit_pre_invocation_failure_line(json: &str) {
    if let Err(error) = emit_stdout_marker_line("OULIPOLY_FAILURE", json) {
        eprintln!("Warning: Failed to emit pre-invocation failure: {error}");
    }
}

fn emit_stdout_marker_line(marker: &str, json: &str) -> std::io::Result<()> {
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(marker.as_bytes())?;
    stdout.write_all(b"=")?;
    stdout.write_all(json.as_bytes())?;
    stdout.write_all(b"\n")?;
    stdout.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stderr_emission_helper_emits_for_non_tty_stderr() {
        assert!(should_emit_invocation_line(false));
    }

    #[test]
    fn stderr_emission_helper_suppresses_for_tty_stderr() {
        assert!(!should_emit_invocation_line(true));
    }
}
