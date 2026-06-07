//! ## Declared roles
//!
//! Roles: mapper.
//!
//! - mapper: maps terminal status, optional live signals, and real child
//!   status into supervised executor output.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs
//!     role: adapter
//!     Translates:
//!       - terminal-signal-classification-contract
//!       - supervised-output-contract
//! ```

use super::{SupervisedOutput, SupervisedTerminalOutcome};
use crate::executor::cli::provider_identity::ProviderRecognizer;
use crate::executor::cli::terminal_signal::{
    exit_code_from_status, recognize_terminal_signal, synthetic_exit_code,
    terminal_reason_from_signal, terminal_status_from_exit_status,
};
use crate::executor::terminal_signal::{TerminalSignal, TerminalStatusEvidence};
use std::process::ExitStatus;

pub(super) fn terminal_outcome_from_status(status: ExitStatus) -> SupervisedTerminalOutcome {
    (
        terminal_status_from_exit_status(&status),
        None,
        Some(status),
    )
}

pub(super) fn supervised_output_from_terminal(
    provider_name: &str,
    recognizer: ProviderRecognizer,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    terminal_status: TerminalStatusEvidence,
    terminal_signal: Option<TerminalSignal>,
    real_status: Option<ExitStatus>,
) -> SupervisedOutput {
    let terminal_signal = terminal_signal.unwrap_or_else(|| {
        recognize_terminal_signal(
            provider_name,
            recognizer,
            &stdout,
            &stderr,
            terminal_status.clone(),
        )
    });
    let exit_code = real_status
        .as_ref()
        .map(exit_code_from_status)
        .unwrap_or_else(|| synthetic_exit_code(&terminal_signal));
    let terminal_reason = terminal_reason_from_signal(&terminal_signal, real_status.as_ref());

    SupervisedOutput {
        stdout,
        stderr,
        exit_code,
        terminal_reason,
        terminal_signal,
        streamed_session_id: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::cli::provider_identity::ProviderRecognizer;
    use crate::executor::terminal_signal::{TerminalSignalKind, TerminalStatusEvidence};

    const INCIDENT_SQLITE_ERROR_EVENT: &[u8] = br#"{"type":"error","timestamp":1780808654364,"sessionID":"ses_15f9407ccffelCcB6CyXvpzdXK","error":{"name":"UnknownError","data":{"message":"Failed to execute statement"}}}"#;

    #[cfg(unix)]
    #[test]
    fn opencode_terminal_structured_error_exit_zero_carries_failure_reason_evidence() {
        use std::os::unix::process::ExitStatusExt;

        let output = supervised_output_from_terminal(
            "opencode",
            ProviderRecognizer::OpenCode,
            INCIDENT_SQLITE_ERROR_EVENT.to_vec(),
            Vec::new(),
            TerminalStatusEvidence::Exited { code: 0 },
            None,
            Some(std::process::ExitStatus::from_raw(0)),
        );

        assert_eq!(output.terminal_signal.kind, TerminalSignalKind::Unknown);
        let reason = output
            .terminal_reason
            .as_deref()
            .expect("terminal structured errors must surface a terminal_reason");
        assert!(
            reason.contains("Failed to execute statement"),
            "terminal_reason should retain the incident message: {reason}"
        );
    }
}
