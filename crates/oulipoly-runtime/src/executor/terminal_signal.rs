use std::time::SystemTime;

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

pub(crate) fn pre_quota_terminal_status(
    status: &TerminalStatusEvidence,
) -> Option<(TerminalSignalKind, String)> {
    match status {
        TerminalStatusEvidence::SpawnError { reason } => Some((
            TerminalSignalKind::SpawnError,
            bounded_text(reason, TERMINAL_SIGNAL_EVIDENCE_MAX_LEN),
        )),
        TerminalStatusEvidence::ProlongedSilence { reason } => Some((
            TerminalSignalKind::ProlongedSilence,
            bounded_text(reason, TERMINAL_SIGNAL_EVIDENCE_MAX_LEN),
        )),
        TerminalStatusEvidence::SignalTerminated { signal } => {
            Some((TerminalSignalKind::SignalExit, format!("signal={signal}")))
        }
        TerminalStatusEvidence::Exited { .. } | TerminalStatusEvidence::Unknown => None,
    }
}

pub(crate) fn post_quota_terminal_status(
    status: &TerminalStatusEvidence,
) -> Option<(TerminalSignalKind, String)> {
    match status {
        TerminalStatusEvidence::Exited { code: 0 } => {
            Some((TerminalSignalKind::CleanExit, "exit_code=0".to_string()))
        }
        TerminalStatusEvidence::Exited { code } => {
            Some((TerminalSignalKind::NonzeroExit, format!("exit_code={code}")))
        }
        TerminalStatusEvidence::SignalTerminated { .. }
        | TerminalStatusEvidence::SpawnError { .. }
        | TerminalStatusEvidence::ProlongedSilence { .. }
        | TerminalStatusEvidence::Unknown => None,
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

    fn assert_dyn_matches_static<R>(provider_name: &str, recognizer: R)
    where
        R: TerminalSignalRecognizer + Copy + 'static,
    {
        let evidence = clean_exit_evidence(provider_name);
        let dyn_recognizer: Box<dyn TerminalSignalRecognizer> = Box::new(recognizer);

        let dynamic = dyn_recognizer.recognize(&evidence);
        let static_result = recognizer.recognize(&evidence);

        assert_eq!(dynamic, static_result);
        assert_eq!(dynamic.provider_name, provider_name);
        assert_eq!(dynamic.observed_at, observed_at());
    }

    #[test]
    fn dto_construction_round_trip_supports_derives_for_each_kind() {
        for kind in all_kinds() {
            let signal = TerminalSignal {
                kind,
                provider_name: "provider".to_string(),
                evidence: format!("evidence for {kind:?}"),
                observed_at: observed_at(),
            };

            let cloned = signal.clone();

            assert_eq!(cloned, signal);
            assert!(format!("{cloned:?}").contains("provider"));
            assert_eq!(cloned.kind, kind);
            assert_eq!(cloned.observed_at, observed_at());
        }
    }

    #[test]
    fn enum_vocabulary_coverage_has_all_terminal_signal_kinds() {
        let labels = all_kinds()
            .into_iter()
            .map(|kind| match kind {
                TerminalSignalKind::CleanExit => "clean_exit",
                TerminalSignalKind::NonzeroExit => "nonzero_exit",
                TerminalSignalKind::SignalExit => "signal_exit",
                TerminalSignalKind::SpawnError => "spawn_error",
                TerminalSignalKind::QuotaExhaustedInband => "quota_exhausted_inband",
                TerminalSignalKind::ProlongedSilence => "prolonged_silence",
                TerminalSignalKind::Unknown => "unknown",
            })
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

        let statuses = [
            TerminalStatusEvidence::Exited { code: 0 },
            TerminalStatusEvidence::SignalTerminated { signal: 15 },
            TerminalStatusEvidence::SpawnError {
                reason: "no such file".to_string(),
            },
            TerminalStatusEvidence::ProlongedSilence {
                reason: "no stdout/stderr for 600s".to_string(),
            },
            TerminalStatusEvidence::Unknown,
        ];

        for status in statuses {
            let evidence = TerminalSignalEvidence {
                provider_name: "provider",
                stdout: b"stdout",
                stderr: b"stderr",
                terminal_status: status.clone(),
                observed_at: observed_at(),
            };

            let cloned = evidence.clone();

            assert_eq!(cloned, evidence);
            assert_eq!(cloned.terminal_status, status);
            assert!(format!("{cloned:?}").contains("provider"));
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
