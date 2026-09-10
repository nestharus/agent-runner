//! ## Declared roles
//!
//! `predicate`, `orchestration`, `mapper`, `formatter`, `validator`

use oulipoly_state::{ResultEnvelopeFailureIdentity, ResultEnvelopeInput, result_envelope_payload};

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
    let mut stdout = std::io::stdout().lock();
    emit_result_envelope_to(
        &mut stdout,
        uuid,
        success,
        exit_code,
        error_category,
        terminal_reason,
        failure_identity,
    )
}

pub(crate) fn emit_result_envelope_to(
    output: &mut impl std::io::Write,
    uuid: &str,
    success: bool,
    exit_code: i32,
    error_category: Option<&str>,
    terminal_reason: Option<&str>,
    failure_identity: Option<&ResultEnvelopeFailureIdentity>,
) -> std::io::Result<()> {
    let finished_at = crate::dispatch::format_timestamp_rfc3339(crate::dispatch::utc_now());
    emit_result_envelope_at(
        output,
        ResultEnvelopeEmitInput {
            uuid,
            success,
            exit_code,
            error_category,
            terminal_reason,
            failure_identity,
            finished_at: &finished_at,
        },
    )
}

fn emit_result_envelope_at(
    output: &mut impl std::io::Write,
    input: ResultEnvelopeEmitInput<'_>,
) -> std::io::Result<()> {
    let uuid = input.uuid;
    let payload = result_envelope_payload_for_emit(input);
    emit_result_envelope_payload(output, uuid, &payload)
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

fn emit_result_envelope_payload(
    output: &mut impl std::io::Write,
    uuid: &str,
    payload: &serde_json::Value,
) -> std::io::Result<()> {
    let json = serialize_result_envelope_payload(payload).map_err(|error| {
        std::io::Error::other(format!(
            "failed to serialize result envelope for {uuid}: {error}"
        ))
    })?;
    emit_result_envelope_line(output, &json)
}

fn serialize_result_envelope_payload(payload: &serde_json::Value) -> Result<String, String> {
    serde_json::to_string(payload).map_err(|err| err.to_string())
}

fn emit_result_envelope_line(output: &mut impl std::io::Write, json: &str) -> std::io::Result<()> {
    emit_marker_line(output, "OULIPOLY_RESULT", json)
}

pub(crate) fn emit_pre_invocation_failure_line(json: &str) {
    if let Err(error) = emit_stdout_marker_line("OULIPOLY_FAILURE", json) {
        eprintln!("Warning: Failed to emit pre-invocation failure: {error}");
    }
}

fn emit_stdout_marker_line(marker: &str, json: &str) -> std::io::Result<()> {
    let mut stdout = std::io::stdout().lock();
    emit_marker_line(&mut stdout, marker, json)
}

fn emit_marker_line(
    output: &mut impl std::io::Write,
    marker: &str,
    json: &str,
) -> std::io::Result<()> {
    output.write_all(marker.as_bytes())?;
    output.write_all(b"=")?;
    output.write_all(json.as_bytes())?;
    output.write_all(b"\n")?;
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct RejectWrites;

    impl std::io::Write for RejectWrites {
        fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "fixture rejects result control record",
            ))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn stderr_emission_helper_emits_for_non_tty_stderr() {
        assert!(should_emit_invocation_line(false));
    }

    #[test]
    fn stderr_emission_helper_suppresses_for_tty_stderr() {
        assert!(!should_emit_invocation_line(true));
    }

    #[test]
    fn result_control_record_write_failure_is_returned() {
        let error = emit_result_envelope_to(
            &mut RejectWrites,
            "fixture-invocation",
            true,
            0,
            None,
            None,
            None,
        )
        .expect_err("control-record write must fail");

        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
    }
}
