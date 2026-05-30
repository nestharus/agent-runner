//! ## Declared roles
//!
//! Roles: orchestration, formatter.
//!
//! - orchestration: configures supervised commands, process-group behavior,
//!   and child spawning.
//! - formatter: materializes supervised stdio posture onto process commands.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/supervision/process.rs
//!     role: adapter
//!     Translates:
//!       - std-process-child-lifecycle-contract
//!       - unix-process-group-contract
//!       - std-io-pipe-drain-contract
//! ```

use super::SupervisorConfig;
use super::errors::spawn_supervised_child_error;
#[cfg(target_os = "linux")]
use super::process_validate::validate_child_parent_after_process_group_setup;
use super::stdin_predicates::supervised_stdin_write_needed;
use std::process::{Child, Command, Stdio};

pub(super) fn configure_supervised_command(cmd: &mut Command, config: &SupervisorConfig) {
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    if supervised_stdin_write_needed(config) {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }
}

#[cfg(target_os = "linux")]
pub(super) fn configure_supervised_process_group(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;

    let parent_pid = unsafe { libc::getpid() };
    cmd.process_group(0);
    unsafe {
        cmd.pre_exec(move || validate_child_parent_after_process_group_setup(parent_pid));
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
pub(super) fn configure_supervised_process_group(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;

    cmd.process_group(0);
}

#[cfg(not(unix))]
pub(super) fn configure_supervised_process_group(_cmd: &mut Command) {}

pub(super) fn spawn_supervised_child(
    mut cmd: Command,
    provider_name: &str,
) -> Result<Child, String> {
    cmd.spawn()
        .map_err(|err| spawn_supervised_child_error(provider_name, &err))
}
