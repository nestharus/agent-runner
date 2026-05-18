use crate::executor::terminal_signal::{
    TERMINAL_SIGNAL_EVIDENCE_MAX_LEN, TerminalSignal, TerminalSignalEvidence, TerminalSignalKind,
    TerminalSignalRecognizer, bounded_excerpt, post_quota_terminal_signal_kind,
    pre_quota_terminal_signal_kind, terminal_signal, terminal_status_evidence,
};

// ## Declared roles
// accessor, filter, formatter, mapper, orchestration, parser, predicate, validator

#[derive(Clone, Copy, Debug, Default)]
pub struct Recognizer;

impl TerminalSignalRecognizer for Recognizer {
    fn recognize(&self, evidence: &TerminalSignalEvidence<'_>) -> TerminalSignal {
        if let Some(kind) = pre_quota_terminal_signal_kind(&evidence.terminal_status) {
            let signal_evidence = terminal_status_evidence(&evidence.terminal_status)
                .unwrap_or_else(|| "unknown".to_string());
            return terminal_signal(evidence, kind, signal_evidence);
        }

        let stdout_text = lowercase_lossy(evidence.stdout);
        let stderr_text = lowercase_lossy(evidence.stderr);
        let quota_stream = find_quota_matching_stream(
            evidence.stdout,
            contains_quota_token(&stdout_text),
            evidence.stderr,
            contains_quota_token(&stderr_text),
        );

        if let Some(bytes) = quota_stream {
            let signal_evidence = quota_evidence_excerpt(bytes);
            return terminal_signal(
                evidence,
                TerminalSignalKind::QuotaExhaustedInband,
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

fn find_quota_matching_stream<'a>(
    stdout: &'a [u8],
    stdout_matches: bool,
    stderr: &'a [u8],
    stderr_matches: bool,
) -> Option<&'a [u8]> {
    if stdout_matches {
        return Some(stdout);
    }

    if stderr_matches {
        return Some(stderr);
    }

    None
}

fn quota_evidence_excerpt(bytes: &[u8]) -> String {
    bounded_excerpt(bytes, TERMINAL_SIGNAL_EVIDENCE_MAX_LEN)
}

fn lowercase_lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_lowercase()
}

/// See the `## Schema` section in [`crate::executor::terminal_signal`] for the
/// canonical token vocabulary.
fn contains_quota_token(text: &str) -> bool {
    text.contains("claude usage limit reached")
        || text.contains("usage limit reached")
        || text.contains("monthly limit")
        || text.contains("billing limit")
        || text.contains("rate_limit_error")
        || text.contains("rate limit")
        || text.contains("too many requests")
        || text.contains("resets at")
        || text.contains("reset_at")
}

#[cfg(test)]
mod tests {
    use super::Recognizer;
    use crate::executor::terminal_signal::{
        TerminalSignal, TerminalSignalEvidence, TerminalSignalKind, TerminalSignalRecognizer,
        TerminalStatusEvidence,
    };
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    const PROVIDER_NAME: &str = "claude";

    fn observed_at() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(139)
    }

    fn evidence(
        stdout: &'static [u8],
        stderr: &'static [u8],
        terminal_status: TerminalStatusEvidence,
    ) -> TerminalSignalEvidence<'static> {
        TerminalSignalEvidence {
            provider_name: PROVIDER_NAME,
            stdout,
            stderr,
            terminal_status,
            observed_at: observed_at(),
        }
    }

    fn recognize(evidence: TerminalSignalEvidence<'static>) -> TerminalSignal {
        Recognizer.recognize(&evidence)
    }

    fn assert_kind(
        evidence: TerminalSignalEvidence<'static>,
        expected_kind: TerminalSignalKind,
    ) -> TerminalSignal {
        let signal = recognize(evidence);
        assert_eq!(signal.kind, expected_kind);
        assert_eq!(signal.provider_name, PROVIDER_NAME);
        assert_eq!(signal.observed_at, observed_at());
        signal
    }

    fn quota_text() -> &'static [u8] {
        b"Claude usage limit reached; resets at 2026-05-18T10:00:00Z"
    }

    #[test]
    fn generic_clean_exit_maps_to_clean_exit_for_claude() {
        let signal = assert_kind(
            evidence(b"done", b"", TerminalStatusEvidence::Exited { code: 0 }),
            TerminalSignalKind::CleanExit,
        );
        assert_eq!(signal.evidence, "exit_code=0");
    }

    #[test]
    fn generic_nonzero_exit_maps_to_nonzero_exit_for_claude() {
        let signal = assert_kind(
            evidence(b"", b"failed", TerminalStatusEvidence::Exited { code: 42 }),
            TerminalSignalKind::NonzeroExit,
        );
        assert_eq!(signal.evidence, "exit_code=42");
    }

    #[test]
    fn generic_signal_exit_maps_to_signal_exit_for_claude() {
        let signal = assert_kind(
            evidence(
                b"",
                b"",
                TerminalStatusEvidence::SignalTerminated { signal: 15 },
            ),
            TerminalSignalKind::SignalExit,
        );
        assert!(signal.evidence.contains("15"));
    }

    #[test]
    fn generic_spawn_error_maps_to_spawn_error_for_claude() {
        let signal = assert_kind(
            evidence(
                b"",
                b"Claude usage limit reached",
                TerminalStatusEvidence::SpawnError {
                    reason: "claude executable not found".to_string(),
                },
            ),
            TerminalSignalKind::SpawnError,
        );
        assert!(signal.evidence.contains("claude executable not found"));
    }

    #[test]
    fn generic_prolonged_silence_maps_to_prolonged_silence_for_claude() {
        let signal = assert_kind(
            evidence(
                b"Claude usage limit reached",
                b"",
                TerminalStatusEvidence::ProlongedSilence {
                    reason: "no stdout/stderr for 600s".to_string(),
                },
            ),
            TerminalSignalKind::ProlongedSilence,
        );
        assert!(signal.evidence.contains("no stdout/stderr for 600s"));
    }

    #[test]
    fn claude_quota_fixtures_map_to_quota_exhausted_inband() {
        for (name, stdout, stderr) in [
            (
                "usage-limit",
                b"Claude usage limit reached".as_slice(),
                b"".as_slice(),
            ),
            (
                "monthly-limit",
                b"".as_slice(),
                b"You've reached your monthly limit".as_slice(),
            ),
            (
                "rate-limit-code",
                b"rate_limit_error".as_slice(),
                b"".as_slice(),
            ),
            (
                "too-many-requests",
                b"".as_slice(),
                b"Too many requests".as_slice(),
            ),
            (
                "reset-window-text",
                b"resets at 10:00 UTC".as_slice(),
                b"".as_slice(),
            ),
            (
                "reset-window-key",
                b"".as_slice(),
                b"reset_at=10:00".as_slice(),
            ),
        ] {
            let signal = assert_kind(
                evidence(stdout, stderr, TerminalStatusEvidence::Unknown),
                TerminalSignalKind::QuotaExhaustedInband,
            );
            assert!(
                !signal.evidence.is_empty(),
                "fixture {name} should preserve an evidence excerpt"
            );
        }
    }

    #[test]
    fn claude_non_match_maps_to_unknown_without_panic() {
        for (name, stdout, stderr) in [
            ("empty", b"".as_slice(), b"".as_slice()),
            (
                "benign",
                b"Claude completed the requested edit.".as_slice(),
                b"".as_slice(),
            ),
        ] {
            let signal = assert_kind(
                evidence(stdout, stderr, TerminalStatusEvidence::Unknown),
                TerminalSignalKind::Unknown,
            );
            assert_eq!(signal.evidence, "unknown", "{name}");
        }
    }

    #[test]
    fn precedence_spawn_error_wins_over_quota_for_claude() {
        assert_kind(
            evidence(
                b"",
                quota_text(),
                TerminalStatusEvidence::SpawnError {
                    reason: "permission denied".to_string(),
                },
            ),
            TerminalSignalKind::SpawnError,
        );
    }

    #[test]
    fn precedence_prolonged_silence_wins_over_quota_for_claude() {
        assert_kind(
            evidence(
                quota_text(),
                b"",
                TerminalStatusEvidence::ProlongedSilence {
                    reason: "no stdout/stderr for 600s".to_string(),
                },
            ),
            TerminalSignalKind::ProlongedSilence,
        );
    }

    #[test]
    fn precedence_signal_exit_wins_over_quota_for_claude() {
        assert_kind(
            evidence(
                quota_text(),
                b"",
                TerminalStatusEvidence::SignalTerminated { signal: 15 },
            ),
            TerminalSignalKind::SignalExit,
        );
    }

    #[test]
    fn precedence_quota_wins_over_clean_exit_for_claude() {
        assert_kind(
            evidence(
                quota_text(),
                b"",
                TerminalStatusEvidence::Exited { code: 0 },
            ),
            TerminalSignalKind::QuotaExhaustedInband,
        );
    }

    #[test]
    fn precedence_quota_wins_over_nonzero_exit_for_claude() {
        assert_kind(
            evidence(
                b"",
                quota_text(),
                TerminalStatusEvidence::Exited { code: 1 },
            ),
            TerminalSignalKind::QuotaExhaustedInband,
        );
    }

    #[test]
    fn malformed_non_utf8_stdout_does_not_panic_for_claude() {
        for (name, stdout, stderr) in [
            ("stdout", b"\xff\xfe\xfa".as_slice(), b"".as_slice()),
            ("stderr", b"".as_slice(), b"\xff\xfe\xfa".as_slice()),
        ] {
            let result = std::panic::catch_unwind(|| {
                recognize(evidence(stdout, stderr, TerminalStatusEvidence::Unknown))
            });

            assert!(
                result.is_ok(),
                "recognizer must not panic on invalid UTF-8 in {name}"
            );
            assert_eq!(result.unwrap().kind, TerminalSignalKind::Unknown, "{name}");
        }
    }
}
