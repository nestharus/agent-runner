//! ## Declared roles
//!
//! Roles: accessor.
//!
//! - accessor: takes supervised stdin payloads and child stdin handles.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/supervision/stdin_access.rs
//!     role: adapter
//!     Translates:
//!       - std-io-pipe-drain-contract
//!       - prompt-stdin-contract
//! ```

use super::SupervisorConfig;
use super::errors::missing_child_stdin_error;
use std::io::Write;
use std::process::Child;

pub(super) fn take_supervised_stdin_payload(config: &mut SupervisorConfig) -> Option<Vec<u8>> {
    config.prompt_payload.take()
}

pub(super) fn take_child_stdin(child: &mut Child) -> Result<impl Write + Send + 'static, String> {
    child.stdin.take().ok_or_else(missing_child_stdin_error)
}
