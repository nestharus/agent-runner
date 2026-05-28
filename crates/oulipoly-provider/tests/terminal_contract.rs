use oulipoly_provider::{
    TerminalSignal, TerminalSignalEvidence, TerminalSignalKind, TerminalSignalRecognizer,
    TerminalStatusEvidence,
};
use std::time::{Duration, UNIX_EPOCH};

#[derive(Debug, Clone, Copy)]
struct DummyRecognizer;

impl TerminalSignalRecognizer for DummyRecognizer {
    fn recognize(&self, evidence: &TerminalSignalEvidence<'_>) -> TerminalSignal {
        TerminalSignal {
            kind: TerminalSignalKind::CleanExit,
            provider_name: evidence.provider_name.to_string(),
            evidence: "recognized by provider contract".to_string(),
            observed_at: evidence.observed_at,
        }
    }
}

fn observed_at() -> std::time::SystemTime {
    UNIX_EPOCH + Duration::from_secs(211)
}

#[test]
fn rich_terminal_dtos_are_reachable_from_provider_root() {
    let kinds = [
        TerminalSignalKind::CleanExit,
        TerminalSignalKind::NonzeroExit,
        TerminalSignalKind::SignalExit,
        TerminalSignalKind::SpawnError,
        TerminalSignalKind::QuotaExhaustedInband,
        TerminalSignalKind::MaybeQuotaExhausted,
        TerminalSignalKind::RateLimited,
        TerminalSignalKind::ProlongedSilence,
        TerminalSignalKind::Unknown,
    ];

    for kind in kinds {
        let signal = TerminalSignal {
            kind,
            provider_name: "provider-a".to_string(),
            evidence: format!("terminal evidence for {kind:?}"),
            observed_at: observed_at(),
        };
        assert_eq!(signal.kind, kind);
        assert_eq!(signal.provider_name, "provider-a");
    }
}

#[test]
fn rich_terminal_status_evidence_and_trait_object_dispatch_are_contract_local() {
    let statuses = [
        TerminalStatusEvidence::Exited { code: 0 },
        TerminalStatusEvidence::SignalTerminated { signal: 15 },
        TerminalStatusEvidence::SpawnError {
            reason: "spawn failed".to_string(),
        },
        TerminalStatusEvidence::ProlongedSilence {
            reason: "no output observed".to_string(),
        },
        TerminalStatusEvidence::Unknown,
    ];
    let recognizer: Box<dyn TerminalSignalRecognizer> = Box::new(DummyRecognizer);

    for terminal_status in statuses {
        let evidence = TerminalSignalEvidence {
            provider_name: "provider-a",
            stdout: b"stdout tail",
            stderr: b"stderr tail",
            terminal_status,
            observed_at: observed_at(),
        };
        let signal = recognizer.recognize(&evidence);

        assert_eq!(signal.kind, TerminalSignalKind::CleanExit);
        assert_eq!(signal.provider_name, "provider-a");
        assert_eq!(signal.observed_at, observed_at());
    }
}
