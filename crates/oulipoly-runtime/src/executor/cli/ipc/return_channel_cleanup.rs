//! ## Declared roles
//!
//! Roles: orchestration.
//!
//! - orchestration: sequences return-channel file and directory cleanup while
//!   preserving non-fatal warning behavior.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/ipc/return_channel_cleanup.rs
//!     role: adapter
//!     Translates:
//!       - return-channel-filesystem-contract
//!       - std-io-cleanup-warning-contract
//! ```

use super::return_channel_predicates::return_channel_dir_cleanup_should_warn;
use super::return_channel_warnings::{
    delete_return_channel_dir_warning, delete_return_channel_warning,
};
use std::path::Path;

pub(super) fn cleanup_return_channel(path: &Path, dir: &Path) {
    cleanup_return_channel_file(path);
    cleanup_return_channel_dir(dir);
}

fn cleanup_return_channel_file(path: &Path) {
    if let Err(err) = std::fs::remove_file(path) {
        eprintln!("{}", delete_return_channel_warning(path, &err));
    }
}

fn cleanup_return_channel_dir(dir: &Path) {
    if let Err(err) = std::fs::remove_dir(dir)
        && return_channel_dir_cleanup_should_warn(&err)
    {
        eprintln!("{}", delete_return_channel_dir_warning(dir, &err));
    }
}
