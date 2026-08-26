//! ## Declared roles
//!
//! Roles: formatter.
//!
//! - formatter: formats canonical supervisor lifecycle, pipe, stdin, and
//!   process status error strings.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/supervision/errors.rs
//!     role: adapter
//!     Translates:
//!       - std-process-child-lifecycle-contract
//!       - std-io-pipe-drain-contract
//!       - prompt-stdin-contract
//! ```

use super::predicates::supervisor_error_is_spawn_error;

pub(super) fn supervisor_error_for_executor(err: String) -> String {
    if supervisor_error_is_spawn_error(&err) {
        err
    } else {
        wait_error_for_executor(&err)
    }
}

fn wait_error_for_executor(err: &str) -> String {
    format!("Failed to wait for process: {err}")
}

pub(super) fn missing_child_stdout_error() -> String {
    "Child stdout was not piped".to_string()
}

pub(super) fn missing_child_stderr_error() -> String {
    "Child stderr was not piped".to_string()
}

pub(super) fn missing_child_stdin_error() -> String {
    "Child stdin was not piped".to_string()
}

pub(super) fn spawn_supervised_child_error(provider_name: &str, err: &std::io::Error) -> String {
    format!("Failed to spawn '{provider_name}': {err}")
}

pub(super) fn poll_child_status_error(err: &std::io::Error) -> String {
    format!("try_wait failed: {err}")
}

pub(super) fn write_stdin_error(err: &std::io::Error) -> String {
    format!("Failed to write to stdin: {err}")
}

pub(super) fn stdin_writer_panic_error() -> String {
    "Failed to write to stdin: writer thread panicked".to_string()
}
