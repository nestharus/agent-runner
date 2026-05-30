//! ## Declared roles
//!
//! Roles: mapper, orchestration.
//!
//! - mapper: maps live terminal-signal recognition into status outcomes and
//!   termination handoff for the supervisor loop.
//! - orchestration: sequences live-quota termination waits and child
//!   termination handoff.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/supervision/live_quota.rs
//!     role: adapter
//!     Translates:
//!       - provider-live-terminal-signal-contract
//!       - terminal-signal-classification-contract
//!       - std-process-child-lifecycle-contract
//! ```

use super::SupervisedTerminalOutcome;
use super::status::try_wait_before_live_quota_terminate;
use super::termination::{terminate_child, wait_for_child_until_termination_grace};
use crate::executor::cli::provider_identity::ProviderRecognizer;
use crate::executor::cli::terminal_signal::{
    recognize_terminal_signal, terminal_status_from_exit_status,
};
use crate::executor::terminal_signal::{TerminalSignal, TerminalStatusEvidence};
use std::process::{Child, ExitStatus};

pub(super) fn recognize_live_terminal_signal(
    provider_name: &str,
    recognizer: ProviderRecognizer,
    stdout: &[u8],
    stderr: &[u8],
) -> TerminalSignal {
    recognize_terminal_signal(
        provider_name,
        recognizer,
        stdout,
        stderr,
        TerminalStatusEvidence::Unknown,
    )
}

pub(super) fn terminate_for_live_quota(
    child: &mut Child,
    provider_name: &str,
    recognizer: ProviderRecognizer,
    stdout: &[u8],
    stderr: &[u8],
    live_signal: TerminalSignal,
) -> Result<SupervisedTerminalOutcome, String> {
    if let Some(status) = try_wait_before_live_quota_terminate(child)? {
        Ok(live_quota_status_outcome(
            provider_name,
            recognizer,
            stdout,
            stderr,
            status,
        ))
    } else if let Some(status) = wait_for_child_after_live_quota(child)? {
        Ok(live_quota_status_outcome(
            provider_name,
            recognizer,
            stdout,
            stderr,
            status,
        ))
    } else {
        live_quota_termination_outcome(child, live_signal)
    }
}

fn live_quota_status_outcome(
    provider_name: &str,
    recognizer: ProviderRecognizer,
    stdout: &[u8],
    stderr: &[u8],
    status: ExitStatus,
) -> SupervisedTerminalOutcome {
    let terminal_status = terminal_status_from_exit_status(&status);
    let terminal_signal = recognize_terminal_signal(
        provider_name,
        recognizer,
        stdout,
        stderr,
        terminal_status.clone(),
    );
    (terminal_status, Some(terminal_signal), Some(status))
}

fn live_quota_termination_outcome(
    child: &mut Child,
    live_signal: TerminalSignal,
) -> Result<SupervisedTerminalOutcome, String> {
    Ok((
        TerminalStatusEvidence::Unknown,
        Some(live_signal),
        terminate_child(child)?,
    ))
}

fn wait_for_child_after_live_quota(child: &mut Child) -> Result<Option<ExitStatus>, String> {
    wait_for_child_until_termination_grace(child, "try_wait after live quota failed")
}
