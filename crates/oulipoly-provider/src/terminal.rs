use std::time::SystemTime;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalSignalKind {
    CleanExit,
    NonzeroExit,
    SignalExit,
    SpawnError,
    QuotaExhaustedInband,
    MaybeQuotaExhausted,
    RateLimited,
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
