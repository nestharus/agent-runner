//! ## Declared roles
//!
//! Roles: accessor.
//!
//! - accessor: reads child process status and wait outcomes while delegating
//!   canonical error formatting.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/supervision/status.rs
//!     role: adapter
//!     Translates:
//!       - std-process-child-lifecycle-contract
//! ```

use super::errors::{
    live_quota_try_wait_error, reap_child_process_error, terminate_try_wait_error,
    termination_grace_try_wait_error,
};
use std::process::{Child, ExitStatus};

pub(super) fn try_wait_before_live_quota_terminate(
    child: &mut Child,
) -> Result<Option<ExitStatus>, String> {
    child
        .try_wait()
        .map_err(|err| live_quota_try_wait_error(&err))
}

pub(super) fn try_wait_before_terminate(child: &mut Child) -> Result<Option<ExitStatus>, String> {
    child
        .try_wait()
        .map_err(|err| terminate_try_wait_error(&err))
}

pub(super) fn try_wait_during_termination_grace(
    child: &mut Child,
    try_wait_context: &str,
) -> Result<Option<ExitStatus>, String> {
    child
        .try_wait()
        .map_err(|err| termination_grace_try_wait_error(try_wait_context, &err))
}

pub(super) fn reap_child_after_kill(child: &mut Child) -> Result<Option<ExitStatus>, String> {
    child
        .wait()
        .map(Some)
        .map_err(|err| reap_child_process_error(&err))
}
