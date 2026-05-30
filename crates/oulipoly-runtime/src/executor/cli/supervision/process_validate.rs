//! ## Declared roles
//!
//! Roles: validator.
//!
//! - validator: verifies Linux child parent-process invariants after process
//!   group setup.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/supervision/process_validate.rs
//!     role: adapter
//!     Translates:
//!       - unix-process-group-contract
//!       - parent-death-signal-contract
//! ```

#[cfg(target_os = "linux")]
pub(super) fn validate_child_parent_after_process_group_setup(
    parent_pid: libc::pid_t,
) -> std::io::Result<()> {
    install_parent_death_signal()?;
    validate_child_parent_pid(parent_pid)
}

#[cfg(target_os = "linux")]
fn install_parent_death_signal() -> std::io::Result<()> {
    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) } == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn validate_child_parent_pid(parent_pid: libc::pid_t) -> std::io::Result<()> {
    if unsafe { libc::getppid() } != parent_pid {
        Err(std::io::Error::from_raw_os_error(libc::ESRCH))
    } else {
        Ok(())
    }
}
