//! ## Declared roles
//!
//! Roles: predicate, filter.
//!
//! - predicate: answers return-channel blank-line and cleanup-warning
//!   decisions.
//! - filter: selects nonblank JSONL record lines for parsing.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/ipc/return_channel_predicates.rs
//!     role: adapter
//!     Translates:
//!       - returned-artifact-jsonl-contract
//!       - std-io-cleanup-warning-contract
//! ```

pub(super) fn return_channel_dir_cleanup_should_warn(err: &std::io::Error) -> bool {
    err.kind() != std::io::ErrorKind::NotFound
        && err.kind() != std::io::ErrorKind::DirectoryNotEmpty
}

pub(super) fn return_channel_record_line(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if return_channel_line_is_empty(trimmed) {
        None
    } else {
        Some(trimmed)
    }
}

fn return_channel_line_is_empty(line: &str) -> bool {
    line.is_empty()
}
