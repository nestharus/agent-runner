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
use crate::executor::cli::provider_identity::ProviderRecognizer;
use crate::executor::cli::spawn_identity::ChildGenerationCustody;
use crate::executor::cli::terminal_signal::recognize_terminal_signal;
use crate::executor::terminal_signal::{TerminalSignal, TerminalStatusEvidence};

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
    child: &mut ChildGenerationCustody<'_>,
    live_signal: TerminalSignal,
) -> Result<SupervisedTerminalOutcome, String> {
    let status = child
        .terminate_and_wait()
        .map_err(|error| format!("failed to terminate live quota generation: {error}"))?;
    Ok((
        TerminalStatusEvidence::Unknown,
        Some(live_signal),
        Some(status),
    ))
}
