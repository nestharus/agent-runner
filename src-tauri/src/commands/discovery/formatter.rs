//! ## Declared roles
//!
//! `formatter`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/discovery/formatter.rs
//!     role: adapter
//!     Translates:
//!       - discovery blocking task join-error string contract
//! ```

pub fn discovery_join_error(error: impl std::fmt::Display) -> String {
    format!("Discovery task failed: {error}")
}
