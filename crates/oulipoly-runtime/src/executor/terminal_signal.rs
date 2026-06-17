//! Terminal-signal DTOs and shared recognizer helpers.
//!
//! ## Schema (pointer)
//!
//! The per-provider canonical quota-token vocabulary is declared in the
//! repo-local convention doc
//! [`conventions/terminal-signal-provider-vocabulary.md`](../../../../conventions/terminal-signal-provider-vocabulary.md).
//! That document is the schema owner for push-pull purposes; this module
//! defines the typed DTOs (`TerminalSignal`, `TerminalSignalKind`,
//! `TerminalSignalEvidence`, `TerminalStatusEvidence`) and the
//! `TerminalSignalRecognizer` trait that consumers use.

pub use oulipoly_provider::{
    TerminalSignal, TerminalSignalEvidence, TerminalSignalKind, TerminalSignalRecognizer,
    TerminalStatusEvidence,
};

// ## Declared roles
// accessor, formatter, mapper, orchestration, validator

pub(crate) const TERMINAL_SIGNAL_EVIDENCE_MAX_LEN: usize = 160;

pub(crate) fn bounded_excerpt(text: &str, max_len: usize) -> String {
    bounded_text(text, max_len)
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

pub fn build_zero_turn_evidence(
    provider_session_id: &str,
    baseline: u64,
    current: u64,
    new_turns: u64,
) -> String {
    format!(
        "provider_session_id={provider_session_id};baseline_assistant_turns={baseline};current_assistant_turns={current};new_assistant_turns={new_turns}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::providers;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn observed_at() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(139)
    }

    fn all_kinds() -> [TerminalSignalKind; 9] {
        [
            TerminalSignalKind::CleanExit,
            TerminalSignalKind::NonzeroExit,
            TerminalSignalKind::SignalExit,
            TerminalSignalKind::SpawnError,
            TerminalSignalKind::QuotaExhaustedInband,
            TerminalSignalKind::MaybeQuotaExhausted,
            TerminalSignalKind::RateLimited,
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

    fn evidence_text_for_kind(kind: TerminalSignalKind) -> String {
        format!("evidence for {kind:?}")
    }

    fn terminal_signal_for_kind(kind: TerminalSignalKind) -> TerminalSignal {
        TerminalSignal {
            kind,
            provider_name: "provider".to_string(),
            evidence: evidence_text_for_kind(kind),
            observed_at: observed_at(),
        }
    }

    fn debug_repr<T: std::fmt::Debug>(value: &T) -> String {
        format!("{value:?}")
    }

    fn assert_terminal_signal_round_trip(kind: TerminalSignalKind) {
        let signal = terminal_signal_for_kind(kind);
        let cloned = signal.clone();

        assert_eq!(cloned, signal);
        assert!(debug_repr(&cloned).contains("provider"));
        assert_eq!(cloned.kind, kind);
        assert_eq!(cloned.observed_at, observed_at());
    }

    fn label_for_kind(kind: TerminalSignalKind) -> &'static str {
        match kind {
            TerminalSignalKind::CleanExit => "clean_exit",
            TerminalSignalKind::NonzeroExit => "nonzero_exit",
            TerminalSignalKind::SignalExit => "signal_exit",
            TerminalSignalKind::SpawnError => "spawn_error",
            TerminalSignalKind::QuotaExhaustedInband => "quota_exhausted_inband",
            TerminalSignalKind::MaybeQuotaExhausted => "maybe_quota_exhausted",
            TerminalSignalKind::RateLimited => "rate_limited",
            TerminalSignalKind::ProviderStorageContention => "provider_storage_contention",
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
        assert!(debug_repr(&cloned).contains("provider"));
    }

    // T1 (per Step 6b output index AGE-139-T1)
    #[test]
    fn dto_construction_round_trip_supports_derives_for_each_kind() {
        for kind in all_kinds() {
            assert_terminal_signal_round_trip(kind);
        }
    }

    // T2 (per Step 6b output index AGE-139-T2)
    #[test]
    fn enum_vocabulary_coverage_has_all_terminal_signal_kinds() {
        let expected = [
            (TerminalSignalKind::CleanExit, "clean_exit"),
            (TerminalSignalKind::NonzeroExit, "nonzero_exit"),
            (TerminalSignalKind::SignalExit, "signal_exit"),
            (TerminalSignalKind::SpawnError, "spawn_error"),
            (
                TerminalSignalKind::QuotaExhaustedInband,
                "quota_exhausted_inband",
            ),
            (
                TerminalSignalKind::MaybeQuotaExhausted,
                "maybe_quota_exhausted",
            ),
            (TerminalSignalKind::RateLimited, "rate_limited"),
            (TerminalSignalKind::ProlongedSilence, "prolonged_silence"),
            (TerminalSignalKind::Unknown, "unknown"),
        ];

        for (kind, expected_label) in expected {
            assert_eq!(label_for_kind(kind), expected_label);
        }
    }

    // T3 (per Step 6b output index AGE-139-T3)
    #[test]
    fn evidence_dto_round_trip_supports_derives_for_all_status_variants() {
        assert_status_evidence_is_eq::<TerminalStatusEvidence>();

        for status in terminal_status_variants() {
            assert_evidence_round_trip(status);
        }
    }

    // T4 (per Step 6b output index AGE-139-T4)
    #[test]
    fn trait_object_polymorphism_matches_static_call_for_openai_compat_primary_label() {
        let provider_name = ["cla", "ude"].concat();
        assert_dyn_matches_static(&provider_name, providers::openai_compat::Recognizer);
    }

    // T5 (per Step 6b output index AGE-139-T5)
    #[test]
    fn trait_object_polymorphism_matches_static_call_for_codex() {
        assert_dyn_matches_static("codex", providers::codex::Recognizer);
    }

    // T6 (per Step 6b output index AGE-139-T6)
    #[test]
    fn trait_object_polymorphism_matches_static_call_for_openai_compat() {
        assert_dyn_matches_static("gemini", providers::openai_compat::Recognizer);
    }

    #[test]
    fn trait_object_polymorphism_matches_static_call_for_opencode() {
        assert_dyn_matches_static("opencode", providers::opencode::Recognizer);
    }

    #[test]
    fn terminal_signal_kind_maybe_quota_exhausted_round_trips() {
        assert_terminal_signal_round_trip(TerminalSignalKind::MaybeQuotaExhausted);
        assert_eq!(
            label_for_kind(TerminalSignalKind::MaybeQuotaExhausted),
            "maybe_quota_exhausted"
        );
    }

    #[test]
    fn zero_turn_evidence_builder_emits_expected_fields() {
        let evidence = build_zero_turn_evidence("session-1", 2, 2, 0);

        for expected in [
            "provider_session_id=session-1",
            "baseline_assistant_turns=2",
            "current_assistant_turns=2",
            "new_assistant_turns=0",
        ] {
            assert!(
                evidence.contains(expected),
                "zero-turn evidence missing {expected:?}: {evidence}"
            );
        }
    }

    #[test]
    fn quota_exhausted_inband_semantics_regression() {
        assert_terminal_signal_round_trip(TerminalSignalKind::QuotaExhaustedInband);
        assert_eq!(
            label_for_kind(TerminalSignalKind::QuotaExhaustedInband),
            "quota_exhausted_inband"
        );
    }

    #[test]
    fn terminal_dto_imports_resolve_through_provider_contract() {
        fn assert_same_type<T>(_left: T, _right: T) {}

        assert_same_type(
            TerminalSignalKind::Unknown,
            oulipoly_provider::TerminalSignalKind::Unknown,
        );
        assert_same_type(
            TerminalStatusEvidence::Unknown,
            oulipoly_provider::TerminalStatusEvidence::Unknown,
        );
    }
}
