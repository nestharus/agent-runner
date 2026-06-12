//! Typed terminal-outcome classification for CLI result handling.
//!
//! ## Declared roles
//!
//! `mapper`, `formatter`, `parser`, `predicate`, `orchestration`, `validator`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/terminal_outcome_adapter.rs
//!     role: adapter
//!     Translates:
//!       - ExecutionResult.terminal_signal
//!       - oulipoly_runtime::executor::ExecutionResult terminal_signal contract
//!       - oulipoly_runtime::executor::terminal_signal::TerminalSignalKind contract
//!       - oulipoly_runtime::diagnostics::ErrorCategory contract
//!       - src-tauri CLI quota retry/error-category contract
//!       - AGE-153 forced terminal-signal fixture contract
//! intrinsic_surface_declarations:
//!   - component: src-tauri/src/terminal_outcome_adapter.rs
//!     role: intrinsic-surface
//!     Domain: Tauri terminal-signal disposition facade
//!     Owns:
//!       - public terminal-outcome adapter API compatibility
//!       - split concern module re-export boundary
//! ```

mod category;
mod disposition;
mod fixture_override;
mod marker;
mod outcome;

#[allow(unused_imports)]
pub use category::TerminalOutcomeCategory;
pub use category::classify_error_category_with_fallback;
pub use disposition::{
    TerminalSignalContext, TerminalSignalDisposition, apply_terminal_signal_outcome,
    confirm_maybe_quota_exhausted,
};
pub use fixture_override::{
    apply_age153_terminal_signal_fixture_override,
    apply_age153_terminal_signal_fixture_override_to_fields,
};
#[allow(unused_imports)]
pub use marker::emit_terminal_signal_marker;
pub use outcome::{
    balanced_terminal_signal_for_outcome, resume_terminal_signal_for_outcome,
    spawn_error_terminal_signal, terminal_signal_error_category, terminal_signal_reason,
    typed_terminal_reason_fallback,
};

#[cfg(test)]
mod tests;
