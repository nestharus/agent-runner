//! ## Declared roles
//!
//! `formatter`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/setup_flow/formatter.rs
//!     role: adapter
//!     Translates:
//!       - setup memory-open error event string contract
//!       - setup response send-error string contract
//!       - setup no-session error string contract
//! ```

use oulipoly_setup::actions::{SetupEvent, UserResponse};
use tokio::sync::mpsc;

pub fn lock_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

pub fn memory_open_error_event(error: impl std::fmt::Display) -> SetupEvent {
    SetupEvent::Error {
        message: format!("Failed to open memory store: {error}"),
        recoverable: false,
    }
}

pub fn setup_send_error(error: mpsc::error::SendError<UserResponse>) -> String {
    format!("Failed to send response: {error}")
}

pub fn no_active_setup_session_error() -> String {
    "No active setup session".to_string()
}
