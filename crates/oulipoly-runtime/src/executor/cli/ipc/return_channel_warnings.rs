//! ## Declared roles
//!
//! Roles: formatter.
//!
//! - formatter: owns canonical return-channel error and warning strings.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/ipc/return_channel_warnings.rs
//!     role: adapter
//!     Translates:
//!       - return-channel-filesystem-contract
//!       - returned-artifact-jsonl-contract
//!       - std-io-cleanup-warning-contract
//! ```

use std::fmt::Display;
use std::path::Path;

pub(super) fn return_channel_parent_invocation_parse_error(err: &dyn Display) -> String {
    format!("Failed to parse parent invocation for return channel: {err}")
}

pub(super) fn create_return_channel_dir_error(dir: &Path, err: &std::io::Error) -> String {
    format!(
        "Failed to create return channel directory {}: {err}",
        dir.display()
    )
}

pub(super) fn create_return_channel_file_error(path: &Path, err: &std::io::Error) -> String {
    format!("Failed to create return channel {}: {err}", path.display())
}

pub(super) fn delete_return_channel_warning(path: &Path, err: &std::io::Error) -> String {
    format!(
        "Warning: failed to delete return channel {}: {err}",
        path.display()
    )
}

pub(super) fn delete_return_channel_dir_warning(dir: &Path, err: &std::io::Error) -> String {
    format!(
        "Warning: failed to delete return channel directory {}: {err}",
        dir.display()
    )
}

pub(super) fn read_return_channel_warning(path: &Path, err: &std::io::Error) -> String {
    format!(
        "Warning: failed to read return channel {}: {err}",
        path.display()
    )
}

pub(super) fn parse_return_channel_line_warning(
    line_number: usize,
    path: &Path,
    err: &serde_json::Error,
) -> String {
    format!(
        "Warning: failed to parse return channel line {} in {}: {err}",
        line_number,
        path.display()
    )
}
