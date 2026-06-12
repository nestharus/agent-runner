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

        if let Some(signal_evidence) = stderr_storage_contention_evidence(evidence.stderr) {
            return terminal_signal(
                evidence,
                TerminalSignalKind::ProviderStorageContention,
                signal_evidence,
            );
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

/// Detect an OpenCode session-store contention crash reported as plain stderr
/// text (e.g. `Failed query: insert into "project" ...`) rather than a
/// structured JSON error event. Scans stderr only — assistant content is on
/// stdout — to keep ordinary output from being misclassified.
fn stderr_storage_contention_evidence(stderr: &[u8]) -> Option<String> {
    let text = stream_text(stderr);
    let line = non_empty_stream_lines(text.as_ref())
        .into_iter()
        .find(|line| message_reports_storage_contention(&line.to_lowercase()))?;
    Some(bounded_excerpt(line, TERMINAL_SIGNAL_EVIDENCE_MAX_LEN))
}

fn json_error_signal_from_stream(bytes: &[u8]) -> Option<(TerminalSignalKind, String)> {
    let text = stream_text(bytes);
    let lines = non_empty_stream_lines(text.as_ref());
    let line = lines.last()?;
    let kind = json_error_line_kind(line)?;
    Some((kind, json_error_line_evidence(line)))
}

fn stream_text(bytes: &[u8]) -> Cow<'_, str> {
    String::from_utf8_lossy(bytes)
}

fn non_empty_stream_lines(text: &str) -> Vec<&str> {
    retain_non_empty_lines(trimmed_stream_lines(text))
}

fn trimmed_stream_lines(text: &str) -> Vec<&str> {
    text.lines().map(str::trim).collect()
}

fn retain_non_empty_lines(lines: Vec<&str>) -> Vec<&str> {
    lines.into_iter().filter(|line| !line.is_empty()).collect()
}

fn json_error_line_evidence(line: &str) -> String {
    let evidence = parse_json_error_line(line)
        .and_then(|value| json_error_evidence_from_value(&value))
        .unwrap_or_else(|| line.to_string());
    bounded_excerpt(&evidence, TERMINAL_SIGNAL_EVIDENCE_MAX_LEN)
}

fn json_error_evidence_from_value(value: &Value) -> Option<String> {
    let error = json_error_event_error(value)?;
    Some(json_error_evidence(error))
}

fn json_error_evidence(error: &Value) -> String {
    let name = error_name(error).unwrap_or("unknown");
    let message = error_message(error);
    if message.is_empty() {
        return format!("provider error: opencode {name}");
    }
    format!("provider error: opencode {name}: {message}")
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
    if message_reports_storage_contention(lower_message) {
        return Some(TerminalSignalKind::ProviderStorageContention);
    }
    Some(TerminalSignalKind::Unknown)
}

/// OpenCode persists every session/project to a per-account SQLite store; under
/// concurrent load that store contends and the statement fails. These are
/// retryable on a less-loaded account, not terminal. Inputs are pre-lowercased.
fn message_reports_storage_contention(message: &str) -> bool {
    message.contains("failed to execute statement")
        || message.contains("failed query")
        || message.contains("database is locked")
        || message.contains("database is busy")
        || message.contains("sqlite_busy")
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

fn error_name(error: &Value) -> Option<&str> {
    value_at_paths(error, &["/name", "/data/name"]).and_then(Value::as_str)
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

    const INCIDENT_SQLITE_ERROR_EVENT: &[u8] = br#"{"type":"error","timestamp":1780808654364,"sessionID":"ses_15f9407ccffelCcB6CyXvpzdXK","error":{"name":"UnknownError","data":{"message":"Failed to execute statement"}}}"#;

    fn evidence(stdout: &'static [u8], stderr: &'static [u8]) -> TerminalSignalEvidence<'static> {
        evidence_with_status(stdout, stderr, TerminalStatusEvidence::Exited { code: 1 })
    }

    fn evidence_with_status(
        stdout: &'static [u8],
        stderr: &'static [u8],
        terminal_status: TerminalStatusEvidence,
    ) -> TerminalSignalEvidence<'static> {
        TerminalSignalEvidence {
            provider_name: "opencode",
            stdout,
            stderr,
            terminal_status,
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
    fn terminal_unrelated_error_uses_structured_error_evidence_before_nonzero_exit() {
        let signal = Recognizer.recognize(&evidence(
            br#"{"type":"error","error":{"data":{"message":"syntax error"}}}"#,
            b"",
        ));

        assert_eq!(signal.kind, TerminalSignalKind::Unknown);
        assert!(signal.evidence.contains("syntax error"));
    }

    #[test]
    fn terminal_sqlite_statement_error_maps_to_storage_contention_with_incident_evidence() {
        let signal = Recognizer.recognize(&evidence_with_status(
            INCIDENT_SQLITE_ERROR_EVENT,
            b"",
            TerminalStatusEvidence::Exited { code: 0 },
        ));

        assert_eq!(signal.kind, TerminalSignalKind::ProviderStorageContention);
        assert!(
            signal.evidence.contains("Failed to execute statement"),
            "signal evidence should retain the incident message: {}",
            signal.evidence
        );
    }

    #[test]
    fn terminal_plain_stderr_failed_query_maps_to_storage_contention() {
        let signal = Recognizer.recognize(&evidence_with_status(
            b"",
            br#"Failed query: insert into "project" ("id", "worktree") values (?, ?)
Error: Unexpected error, check log file at /home/u/.opencode3/opencode/log/x.log"#,
            TerminalStatusEvidence::Exited { code: 1 },
        ));

        assert_eq!(signal.kind, TerminalSignalKind::ProviderStorageContention);
    }

    #[test]
    fn recovered_session_error_followed_by_later_event_preserves_clean_exit() {
        let signal = Recognizer.recognize(&evidence_with_status(
            concat!(
                r#"{"type":"error","timestamp":1780808654364,"sessionID":"ses_15f9407ccffelCcB6CyXvpzdXK","error":{"name":"UnknownError","data":{"message":"Failed to execute statement"}}}"#,
                "\n",
                r#"{"type":"assistant","message":"continued after transient provider error"}"#
            )
            .as_bytes(),
            b"",
            TerminalStatusEvidence::Exited { code: 0 },
        ));

        assert_eq!(signal.kind, TerminalSignalKind::CleanExit);
    }

    #[test]
    fn ordinary_output_quota_and_rate_text_preserves_clean_exit() {
        let signal = Recognizer.recognize(&evidence_with_status(
            b"assistant mentioned quota exhausted and rate limit exceeded without an error event",
            b"",
            TerminalStatusEvidence::Exited { code: 0 },
        ));

        assert_eq!(signal.kind, TerminalSignalKind::CleanExit);
    }

    #[test]
    fn ordinary_output_quota_and_rate_text_preserves_nonzero_exit() {
        let signal = Recognizer.recognize(&evidence_with_status(
            b"assistant mentioned quota exhausted and rate limit exceeded without an error event",
            b"",
            TerminalStatusEvidence::Exited { code: 1 },
        ));

        assert_eq!(signal.kind, TerminalSignalKind::NonzeroExit);
    }
}
