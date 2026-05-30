//! ## Declared roles
//!
//! Roles: predicate.
//!
//! - predicate: answers supervisor classification and live terminal-signal
//!   questions without formatting or lifecycle effects.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/supervision/predicates.rs
//!     role: adapter
//!     Translates:
//!       - std-process-child-lifecycle-contract
//!       - provider-live-terminal-signal-contract
//! ```

use crate::executor::terminal_signal::{TerminalSignal, TerminalSignalKind};

pub(super) fn supervisor_error_is_spawn_error(err: &str) -> bool {
    err.starts_with("Failed to spawn")
}

pub(super) fn live_signal_is_quota_exhausted_inband(signal: &TerminalSignal) -> bool {
    signal.kind == TerminalSignalKind::QuotaExhaustedInband
}
