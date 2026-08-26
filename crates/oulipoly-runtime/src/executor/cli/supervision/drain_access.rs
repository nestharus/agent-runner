//! ## Declared roles
//!
//! Roles: accessor.
//!
//! - accessor: takes child stdout and stderr pipes and reports canonical
//!   missing-pipe errors.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/supervision/drain_access.rs
//!     role: adapter
//!     Translates:
//!       - std-io-pipe-drain-contract
//!       - std-process-child-lifecycle-contract
//! ```

use std::io::Read;
use std::process::Child;

use super::errors::{missing_child_stderr_error, missing_child_stdout_error};
use crate::executor::cli::spawn_identity::child_custody_test_fault;

pub(super) fn take_child_stdout(child: &mut Child) -> Result<impl Read + Send + 'static, String> {
    child_custody_test_fault("headless_stdout_drain")?;
    child.stdout.take().ok_or_else(missing_child_stdout_error)
}

pub(super) fn take_child_stderr(child: &mut Child) -> Result<impl Read + Send + 'static, String> {
    child_custody_test_fault("headless_stderr_drain")?;
    child.stderr.take().ok_or_else(missing_child_stderr_error)
}
