//! ## Declared roles
//!
//! Roles: predicate, accessor.
//!
//! - predicate: `legacy_stdout_produced_assistant_response` and its line
//!   classifiers decide whether captured provider stdout carries a real
//!   assistant answer (human-readable text on the stdout output channel) rather
//!   than only structured control/error JSON.
//! - accessor: `launch_result_produced_assistant_response` reads the single
//!   declared boolean launch marker by which an external provider explicitly
//!   reports that it produced an assistant response.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/assistant_response.rs
//!     role: adapter
//!     Translates:
//!       - provider-stdout-output-contract
//!       - assistant-response-marker-contract
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/assistant_response.rs
//!     role: intrinsic-surface
//!     Domain: host-observed assistant response productivity
//!     Owns:
//!       - plain-text assistant answer detection on the provider stdout channel
//!       - the declared launch marker name that confirms assistant output
//! ```

use oulipoly_provider::stream::LaunchResult;
use serde_json::Value;

/// Launch marker by which an external provider explicitly reports that the turn
/// produced an assistant response. Boolean-valued; a host-side declared contract
/// the provider pushes, so the host never interprets the raw stream shape.
const PRODUCED_ASSISTANT_RESPONSE_MARKER: &str = "oulipoly.produced_assistant_response";

/// Legacy path: the host observes the provider's stdout output channel directly.
/// A real assistant response appears as human-readable (non-structured-JSON)
/// text on stdout. Providers that emit only structured JSON are confirmed via
/// the session-turn delta instead, so they need no signal from this path.
pub(crate) fn legacy_stdout_produced_assistant_response(stdout: &[u8]) -> bool {
    String::from_utf8_lossy(stdout)
        .lines()
        .any(line_is_assistant_text)
}

/// External path: the provider CLI pushes an explicit boolean marker; the host
/// pulls only that declared marker, never the raw launch stream shape.
pub(crate) fn launch_result_produced_assistant_response(result: &LaunchResult) -> bool {
    result
        .retained_marker_value(PRODUCED_ASSISTANT_RESPONSE_MARKER)
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

fn line_is_assistant_text(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty() && !line_is_structured_json(trimmed)
}

fn line_is_structured_json(line: &str) -> bool {
    serde_json::from_str::<Value>(line).is_ok()
}

#[cfg(test)]
mod tests {
    use super::{
        launch_result_produced_assistant_response, legacy_stdout_produced_assistant_response,
    };
    use oulipoly_provider::stream::LaunchJsonlReader;

    fn launch_result(lines: &[&str]) -> oulipoly_provider::stream::LaunchResult {
        let text = lines.join("\n") + "\n";
        LaunchJsonlReader::new("req-1")
            .read(text.as_bytes())
            .unwrap()
    }

    fn marker_event(seq: u64, name: &str, value: &str) -> String {
        format!(
            r#"{{"contract":"oulipoly.provider/v1","request_id":"req-1","seq":{seq},"time_unix_ms":{seq},"kind":"marker","name":"{name}","value":{value}}}"#
        )
    }

    fn exit_event(seq: u64) -> String {
        format!(
            r#"{{"contract":"oulipoly.provider/v1","request_id":"req-1","seq":{seq},"time_unix_ms":{seq},"kind":"exit","status":{{"kind":"exited","code":0}},"terminal_signal":{{"kind":"clean_exit","evidence":"clean","observed_at_unix_ms":{seq}}}}}"#
        )
    }

    #[test]
    fn legacy_plain_text_stdout_counts_as_response() {
        assert!(legacy_stdout_produced_assistant_response(
            b"finished answer\n"
        ));
    }

    #[test]
    fn legacy_blank_stdout_is_not_response() {
        assert!(!legacy_stdout_produced_assistant_response(b"   \n\n"));
    }

    #[test]
    fn legacy_structured_json_only_is_not_response() {
        assert!(!legacy_stdout_produced_assistant_response(
            br#"{"type":"result","session_id":"s1"}"#
        ));
    }

    #[test]
    fn legacy_text_mixed_with_control_json_counts_as_response() {
        assert!(legacy_stdout_produced_assistant_response(
            b"{\"type\":\"system\"}\nPINGPONG\n"
        ));
    }

    #[test]
    fn launch_marker_true_counts_as_response() {
        let marker = marker_event(1, "oulipoly.produced_assistant_response", "true");
        let exit = exit_event(2);
        let result = launch_result(&[marker.as_str(), exit.as_str()]);

        assert!(launch_result_produced_assistant_response(&result));
    }

    #[test]
    fn launch_marker_false_is_not_response() {
        let marker = marker_event(1, "oulipoly.produced_assistant_response", "false");
        let exit = exit_event(2);
        let result = launch_result(&[marker.as_str(), exit.as_str()]);

        assert!(!launch_result_produced_assistant_response(&result));
    }

    #[test]
    fn launch_without_marker_is_not_response() {
        let exit = exit_event(1);
        let result = launch_result(&[exit.as_str()]);

        assert!(!launch_result_produced_assistant_response(&result));
    }
}
