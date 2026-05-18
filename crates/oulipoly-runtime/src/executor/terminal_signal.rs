//! Terminal-signal DTOs and shared recognizer helpers.
//!
//! ## Schema
//!
//! AGE-139 declares the per-provider canonical quota-token vocabulary as the
//! AGE-139 contract's parsed-artifact shape for `TerminalSignalEvidence.stdout`
//! / `stderr` recognition. Both the producer side (provider CLIs whose outputs
//! AGE-139 normalizes) and the consumer side (per-provider recognizers) treat
//! this vocabulary as the stable agreement, with fixture-string-drift recorded
//! as the only sanctioned residual in
//! `planning/age-139-terminal-signal-core/risk/age-139-test-residuals.md`.
//!
//! Required-token sets:
//!
//! - Claude / Anthropic provider stdout/stderr:
//!   - `claude usage limit reached`
//!   - `usage limit reached`
//!   - `monthly limit`
//!   - `billing limit`
//!   - `rate_limit_error`
//!   - `rate limit`
//!   - `too many requests`
//!   - `resets at`
//!   - `reset_at`
//!
//! - Codex / OpenAI CLI provider stdout/stderr:
//!   - `http 429`
//!   - `status: 429`
//!   - `status 429`
//!   - `rate limit`
//!   - `rate_limit_exceeded`
//!   - `usage cap`
//!   - `billing limit`
//!   - `quota exceeded`
//!   - `reset_at`
//!   - `resets at`
//!
//! - OpenAI-compatible provider stdout/stderr (Gemini, OpenCode, ...):
//!   - `rate_limit_exceeded`
//!   - `429`
//!   - `too many requests`
//!   - `quota exhausted`
//!   - `quota exceeded`
//!   - `rate limit exceeded`
//!
//! Recognizers in `executor::providers::*` MUST match exactly this token set
//! and only this token set; any deviation is a contract violation and goes
//! through Phase 2.5 re-research before merge.

use std::time::SystemTime;

// ## Declared roles
// accessor, formatter, mapper, orchestration, validator

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalSignalKind {
    CleanExit,
    NonzeroExit,
    SignalExit,
    SpawnError,
    QuotaExhaustedInband,
    ProlongedSilence,
    Unknown,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TerminalSignal {
    pub kind: TerminalSignalKind,
    pub provider_name: String,
    pub evidence: String,
    pub observed_at: SystemTime,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalStatusEvidence {
    Exited { code: i32 },
    SignalTerminated { signal: i32 },
    SpawnError { reason: String },
    ProlongedSilence { reason: String },
    Unknown,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TerminalSignalEvidence<'a> {
    pub provider_name: &'a str,
    pub stdout: &'a [u8],
    pub stderr: &'a [u8],
    pub terminal_status: TerminalStatusEvidence,
    pub observed_at: SystemTime,
}

pub trait TerminalSignalRecognizer: Send + Sync {
    fn recognize(&self, evidence: &TerminalSignalEvidence<'_>) -> TerminalSignal;
}

pub(crate) const TERMINAL_SIGNAL_EVIDENCE_MAX_LEN: usize = 160;

pub(crate) fn bounded_excerpt(bytes: &[u8], max_len: usize) -> String {
    bounded_text(&String::from_utf8_lossy(bytes), max_len)
}

pub(crate) fn bounded_text(text: &str, max_len: usize) -> String {
    text.chars().take(max_len).collect()
}

pub(crate) fn pre_quota_terminal_signal_kind(
    status: &TerminalStatusEvidence,
) -> Option<TerminalSignalKind> {
    match status {
        TerminalStatusEvidence::SpawnError { .. } => Some(TerminalSignalKind::SpawnError),
        TerminalStatusEvidence::ProlongedSilence { .. } => {
            Some(TerminalSignalKind::ProlongedSilence)
        }
        TerminalStatusEvidence::SignalTerminated { .. } => Some(TerminalSignalKind::SignalExit),
        TerminalStatusEvidence::Exited { .. } | TerminalStatusEvidence::Unknown => None,
    }
}

pub(crate) fn post_quota_terminal_signal_kind(
    status: &TerminalStatusEvidence,
) -> Option<TerminalSignalKind> {
    match status {
        TerminalStatusEvidence::Exited { code: 0 } => Some(TerminalSignalKind::CleanExit),
        TerminalStatusEvidence::Exited { .. } => Some(TerminalSignalKind::NonzeroExit),
        TerminalStatusEvidence::SignalTerminated { .. }
        | TerminalStatusEvidence::SpawnError { .. }
        | TerminalStatusEvidence::ProlongedSilence { .. }
        | TerminalStatusEvidence::Unknown => None,
    }
}

pub(crate) fn terminal_status_evidence(status: &TerminalStatusEvidence) -> Option<String> {
    match status {
        TerminalStatusEvidence::Exited { code } => Some(format!("exit_code={code}")),
        TerminalStatusEvidence::SignalTerminated { signal } => Some(format!("signal={signal}")),
        TerminalStatusEvidence::SpawnError { reason }
        | TerminalStatusEvidence::ProlongedSilence { reason } => {
            Some(bounded_text(reason, TERMINAL_SIGNAL_EVIDENCE_MAX_LEN))
        }
        TerminalStatusEvidence::Unknown => None,
    }
}

pub(crate) fn terminal_signal(
    evidence: &TerminalSignalEvidence<'_>,
    kind: TerminalSignalKind,
    signal_evidence: String,
) -> TerminalSignal {
    TerminalSignal {
        kind,
        provider_name: evidence.provider_name.to_owned(),
        evidence: signal_evidence,
        observed_at: evidence.observed_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::providers;
    use std::time::{Duration, UNIX_EPOCH};

    fn observed_at() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(139)
    }

    fn all_kinds() -> [TerminalSignalKind; 7] {
        [
            TerminalSignalKind::CleanExit,
            TerminalSignalKind::NonzeroExit,
            TerminalSignalKind::SignalExit,
            TerminalSignalKind::SpawnError,
            TerminalSignalKind::QuotaExhaustedInband,
            TerminalSignalKind::ProlongedSilence,
            TerminalSignalKind::Unknown,
        ]
    }

    fn clean_exit_evidence(provider_name: &str) -> TerminalSignalEvidence<'_> {
        TerminalSignalEvidence {
            provider_name,
            stdout: b"",
            stderr: b"",
            terminal_status: TerminalStatusEvidence::Exited { code: 0 },
            observed_at: observed_at(),
        }
    }

    fn assert_status_evidence_is_eq<T: Eq>() {}

    fn dyn_and_static_results<R>(
        provider_name: &str,
        recognizer: R,
    ) -> (TerminalSignal, TerminalSignal)
    where
        R: TerminalSignalRecognizer + Copy + 'static,
    {
        let evidence = clean_exit_evidence(provider_name);
        let dyn_recognizer: Box<dyn TerminalSignalRecognizer> = Box::new(recognizer);

        (
            dyn_recognizer.recognize(&evidence),
            recognizer.recognize(&evidence),
        )
    }

    fn assert_dyn_matches_static<R>(provider_name: &str, recognizer: R)
    where
        R: TerminalSignalRecognizer + Copy + 'static,
    {
        let (dynamic, static_result) = dyn_and_static_results(provider_name, recognizer);
        assert_eq!(dynamic, static_result);
        assert_eq!(dynamic.provider_name, provider_name);
        assert_eq!(dynamic.observed_at, observed_at());
    }

    fn terminal_signal_for_kind(kind: TerminalSignalKind) -> TerminalSignal {
        TerminalSignal {
            kind,
            provider_name: "provider".to_string(),
            evidence: format!("evidence for {kind:?}"),
            observed_at: observed_at(),
        }
    }

    fn assert_terminal_signal_round_trip(kind: TerminalSignalKind) {
        let signal = terminal_signal_for_kind(kind);
        let cloned = signal.clone();

        assert_eq!(cloned, signal);
        assert!(format!("{cloned:?}").contains("provider"));
        assert_eq!(cloned.kind, kind);
        assert_eq!(cloned.observed_at, observed_at());
    }

    fn terminal_signal_kind_label(kind: TerminalSignalKind) -> &'static str {
        match kind {
            TerminalSignalKind::CleanExit => "clean_exit",
            TerminalSignalKind::NonzeroExit => "nonzero_exit",
            TerminalSignalKind::SignalExit => "signal_exit",
            TerminalSignalKind::SpawnError => "spawn_error",
            TerminalSignalKind::QuotaExhaustedInband => "quota_exhausted_inband",
            TerminalSignalKind::ProlongedSilence => "prolonged_silence",
            TerminalSignalKind::Unknown => "unknown",
        }
    }

    fn terminal_status_variants() -> [TerminalStatusEvidence; 5] {
        [
            TerminalStatusEvidence::Exited { code: 0 },
            TerminalStatusEvidence::SignalTerminated { signal: 15 },
            TerminalStatusEvidence::SpawnError {
                reason: "no such file".to_string(),
            },
            TerminalStatusEvidence::ProlongedSilence {
                reason: "no stdout/stderr for 600s".to_string(),
            },
            TerminalStatusEvidence::Unknown,
        ]
    }

    fn evidence_for_status(status: TerminalStatusEvidence) -> TerminalSignalEvidence<'static> {
        TerminalSignalEvidence {
            provider_name: "provider",
            stdout: b"stdout",
            stderr: b"stderr",
            terminal_status: status,
            observed_at: observed_at(),
        }
    }

    fn assert_evidence_round_trip(status: TerminalStatusEvidence) {
        let evidence = evidence_for_status(status.clone());
        let cloned = evidence.clone();

        assert_eq!(cloned, evidence);
        assert_eq!(cloned.terminal_status, status);
        assert!(format!("{cloned:?}").contains("provider"));
    }

    #[test]
    fn dto_construction_round_trip_supports_derives_for_each_kind() {
        for kind in all_kinds() {
            assert_terminal_signal_round_trip(kind);
        }
    }

    #[test]
    fn enum_vocabulary_coverage_has_all_terminal_signal_kinds() {
        let labels = all_kinds()
            .into_iter()
            .map(terminal_signal_kind_label)
            .collect::<Vec<_>>();

        assert_eq!(
            labels,
            vec![
                "clean_exit",
                "nonzero_exit",
                "signal_exit",
                "spawn_error",
                "quota_exhausted_inband",
                "prolonged_silence",
                "unknown",
            ]
        );
    }

    #[test]
    fn evidence_dto_round_trip_supports_derives_for_all_status_variants() {
        assert_status_evidence_is_eq::<TerminalStatusEvidence>();

        for status in terminal_status_variants() {
            assert_evidence_round_trip(status);
        }
    }

    #[test]
    fn trait_object_polymorphism_matches_static_call_for_claude() {
        assert_dyn_matches_static("claude", providers::claude::Recognizer);
    }

    #[test]
    fn trait_object_polymorphism_matches_static_call_for_codex() {
        assert_dyn_matches_static("codex", providers::codex::Recognizer);
    }

    #[test]
    fn trait_object_polymorphism_matches_static_call_for_openai_compat() {
        assert_dyn_matches_static("gemini", providers::openai_compat::Recognizer);
    }
}
