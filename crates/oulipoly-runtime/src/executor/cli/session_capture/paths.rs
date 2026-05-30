//! ## Declared roles
//!
//! Roles: accessor, formatter.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/session_capture/paths.rs
//!     role: adapter
//!     Translates:
//!       - runtime-last-message-sidecar-contract
//! ```

use std::path::PathBuf;

pub(super) fn last_message_capture_path() -> PathBuf {
    temp_path_for_filename(last_message_capture_filename())
}

fn last_message_capture_filename() -> String {
    format!("oulipoly-last-message-{}", uuid::Uuid::new_v4())
}

fn temp_path_for_filename(filename: String) -> PathBuf {
    std::env::temp_dir().join(filename)
}
