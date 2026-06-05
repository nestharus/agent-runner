//! OpenCode terminal-signal recognizer.
//!
//! OpenCode runs through the OpenAI-compatible launch path, but its
//! `--format json` stream has provider-specific `error` events. Keep generic
//! `openai_compat` substring matching disabled and classify only structured
//! OpenCode error JSON here.

use crate::executor::terminal_signal::{
    TERMINAL_SIGNAL_EVIDENCE_MAX_LEN, TerminalSignal, TerminalSignalEvidence, TerminalSignalKind,
    TerminalSignalRecognizer, bounded_excerpt, post_quota_terminal_signal_kind,
    pre_quota_terminal_signal_kind, terminal_signal, terminal_status_evidence,
};
use serde_json::Value;
use std::borrow::Cow;

#[derive(Clone, Copy, Debug, Default)]
pub struct Recognizer;

impl TerminalSignalRecognizer for Recognizer {
    fn recognize(&self, evidence: &TerminalSignalEvidence<'_>) -> TerminalSignal {
        if let Some(kind) = pre_quota_terminal_signal_kind(&evidence.terminal_status) {
            let signal_evidence = terminal_status_evidence(&evidence.terminal_status)
                .unwrap_or_else(|| "unknown".to_string());
            return terminal_signal(evidence, kind, signal_evidence);
        }

        if let Some((kind, signal_evidence)) = opencode_json_error_signal(evidence) {
            return terminal_signal(evidence, kind, signal_evidence);
        }

        if let Some(kind) = post_quota_terminal_signal_kind(&evidence.terminal_status) {
            let signal_evidence = terminal_status_evidence(&evidence.terminal_status)
                .unwrap_or_else(|| "unknown".to_string());
            return terminal_signal(evidence, kind, signal_evidence);
        }

        terminal_signal(evidence, TerminalSignalKind::Unknown, "unknown".to_string())
    }
}

fn opencode_json_error_signal(
    evidence: &TerminalSignalEvidence<'_>,
) -> Option<(TerminalSignalKind, String)> {
    json_error_signal_from_stream(evidence.stdout)
        .or_else(|| json_error_signal_from_stream(evidence.stderr))
}

fn json_error_signal_from_stream(bytes: &[u8]) -> Option<(TerminalSignalKind, String)> {
    let text = stream_text(bytes);
    for line in non_empty_stream_lines(text.as_ref()) {
        let Some(kind) = json_error_line_kind(line) else {
            continue;
        };
        return Some((kind, json_error_line_evidence(line)));
    }
    None
}

fn stream_text(bytes: &[u8]) -> Cow<'_, str> {
    String::from_utf8_lossy(bytes)
}

fn non_empty_stream_lines(text: &str) -> Vec<&str> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect()
}

fn json_error_line_evidence(line: &str) -> String {
    bounded_excerpt(line, TERMINAL_SIGNAL_EVIDENCE_MAX_LEN)
}

fn json_error_line_kind(line: &str) -> Option<TerminalSignalKind> {
    let value = parse_json_error_line(line)?;
    let error = json_error_event_error(&value)?;
    let lower_message = normalized_error_message(error);
    terminal_signal_kind_from_json_error(error, &lower_message)
}

fn parse_json_error_line(line: &str) -> Option<Value> {
    serde_json::from_str(line).ok()
}

fn json_error_event_error(value: &Value) -> Option<&Value> {
    if value.get("type").and_then(Value::as_str) != Some("error") {
        return None;
    }
    value.get("error")
}

fn normalized_error_message(error: &Value) -> String {
    error_message(error).to_lowercase()
}

fn terminal_signal_kind_from_json_error(
    error: &Value,
    lower_message: &str,
) -> Option<TerminalSignalKind> {
    if error_reports_rate_limit(error, lower_message) {
        return Some(TerminalSignalKind::RateLimited);
    }
    if error_reports_persistent_quota(lower_message) {
        return Some(TerminalSignalKind::QuotaExhaustedInband);
    }
    None
}

fn error_status_code(error: &Value) -> Option<i64> {
    value_at_paths(
        error,
        &[
            "/data/statusCode",
            "/data/status_code",
            "/statusCode",
            "/status",
        ],
    )
    .and_then(number_or_numeric_string)
}

fn error_message(error: &Value) -> String {
    value_at_paths(error, &["/data/message", "/message"])
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn value_at_paths<'a>(value: &'a Value, paths: &[&str]) -> Option<&'a Value> {
    paths.iter().find_map(|path| value.pointer(path))
}

fn number_or_numeric_string(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|text| text.parse::<i64>().ok()))
}

fn error_reports_rate_limit(error: &Value, message: &str) -> bool {
    error_status_code(error) == Some(429) || message_reports_rate_limit(message)
}

fn message_reports_rate_limit(message: &str) -> bool {
    message.contains("rate limit exceeded") || message.contains("too many requests")
}

fn error_reports_persistent_quota(message: &str) -> bool {
    (message.contains("quota")
        && (message.contains("exceeded")
            || message.contains("exhausted")
            || message.contains("insufficient")))
        || (message.contains("usage limit")
            && (message.contains("reached") || message.contains("exceeded")))
}

#[cfg(test)]
mod tests {
    use super::Recognizer;
    use crate::executor::terminal_signal::{
        TerminalSignalEvidence, TerminalSignalKind, TerminalSignalRecognizer,
        TerminalStatusEvidence,
    };
    use std::time::SystemTime;

    fn evidence(stdout: &'static [u8], stderr: &'static [u8]) -> TerminalSignalEvidence<'static> {
        TerminalSignalEvidence {
            provider_name: "opencode",
            stdout,
            stderr,
            terminal_status: TerminalStatusEvidence::Exited { code: 1 },
            observed_at: SystemTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn status_code_429_maps_to_rate_limited() {
        let signal = Recognizer.recognize(&evidence(
            br#"{"type":"error","error":{"data":{"message":"anything","statusCode":429}}}"#,
            b"",
        ));

        assert_eq!(signal.kind, TerminalSignalKind::RateLimited);
    }

    #[test]
    fn persistent_quota_message_maps_to_quota_exhausted() {
        let signal = Recognizer.recognize(&evidence(
            b"",
            br#"{"type":"error","error":{"data":{"message":"quota exhausted for account"}}}"#,
        ));

        assert_eq!(signal.kind, TerminalSignalKind::QuotaExhaustedInband);
    }

    #[test]
    fn unrelated_error_falls_back_to_nonzero_exit() {
        let signal = Recognizer.recognize(&evidence(
            br#"{"type":"error","error":{"data":{"message":"syntax error"}}}"#,
            b"",
        ));

        assert_eq!(signal.kind, TerminalSignalKind::NonzeroExit);
    }
}
