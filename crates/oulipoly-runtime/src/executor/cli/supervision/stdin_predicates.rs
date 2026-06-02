//! ## Declared roles
//!
//! Roles: predicate.
//!
//! - predicate: answers stdin-write control-flow questions without I/O.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/supervision/stdin_predicates.rs
//!     role: adapter
//!     Translates:
//!       - prompt-stdin-contract
//!       - terminal-signal-classification-contract
//! ```

use super::{SupervisedOutput, SupervisorConfig};
use crate::executor::terminal_signal::TerminalSignalKind;
use oulipoly_config::PromptMode;

pub(super) fn supervised_stdin_write_needed(config: &SupervisorConfig) -> bool {
    config.prompt_mode == PromptMode::Stdin && config.prompt_payload.is_some()
}

pub(super) fn stdin_write_error_is_fatal(err: Option<&str>, output: &SupervisedOutput) -> bool {
    err.is_some() && output.terminal_signal.kind == TerminalSignalKind::CleanExit
}
