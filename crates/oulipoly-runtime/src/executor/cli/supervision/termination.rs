//! ## Declared roles
//!
//! Roles: orchestration, predicate, mapper.
//!
//! - orchestration: sequences graceful termination, kill, process-group
//!   signaling, grace-period polling, and child reap behavior.
//! - predicate: answers whether the termination grace period elapsed.
//! - mapper: maps child ids to process-group ids for Unix signals.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/supervision/termination.rs
//!     role: adapter
//!     Translates:
//!       - std-process-child-lifecycle-contract
//!       - unix-process-group-contract
//! ```

use super::SUPERVISOR_POLL_INTERVAL;
use super::errors::kill_child_process_error;
use super::status::{
    reap_child_after_kill, try_wait_before_terminate, try_wait_during_termination_grace,
};
#[cfg(unix)]
use std::io;
use std::process::{Child, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};

const TERMINATE_GRACE_PERIOD: Duration = Duration::from_millis(250);

#[cfg(unix)]
pub(super) fn cleanup_process_group_after_child_exit(child_id: u32) -> Result<(), String> {
    let process_group = child_id as libc::pid_t;
    if !signal_exited_child_process_group(process_group, libc::SIGTERM)? {
        return Ok(());
    }
    let started = Instant::now();
    while process_group_exists(process_group)? && !terminate_grace_period_elapsed(started) {
        thread::sleep(SUPERVISOR_POLL_INTERVAL);
    }
    if process_group_exists(process_group)? {
        signal_exited_child_process_group(process_group, libc::SIGKILL)?;
    }
    Ok(())
}

#[cfg(not(unix))]
pub(super) fn cleanup_process_group_after_child_exit(_child_id: u32) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn signal_exited_child_process_group(
    process_group: libc::pid_t,
    signal: i32,
) -> Result<bool, String> {
    if unsafe { libc::killpg(process_group, signal) } == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        return Ok(false);
    }
    Err(format!(
        "failed to clean provider process group {process_group}: {error}"
    ))
}

#[cfg(unix)]
fn process_group_exists(process_group: libc::pid_t) -> Result<bool, String> {
    if unsafe { libc::killpg(process_group, 0) } == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ESRCH) => Ok(false),
        Some(libc::EPERM) => Ok(true),
        _ => Err(format!(
            "failed to inspect provider process group {process_group}: {error}"
        )),
    }
}

pub(super) fn terminate_child(child: &mut Child) -> Result<Option<ExitStatus>, String> {
    if let Some(status) = try_wait_before_terminate(child)? {
        return Ok(Some(status));
    }

    send_child_sigterm(child);
    if let Some(status) = wait_for_child_after_sigterm(child)? {
        return Ok(Some(status));
    }

    send_child_sigkill(child)?;
    reap_child_after_kill(child)
}

fn wait_for_child_after_sigterm(child: &mut Child) -> Result<Option<ExitStatus>, String> {
    wait_for_child_until_termination_grace(child, "try_wait after terminate failed")
}

pub(super) fn wait_for_child_until_termination_grace(
    child: &mut Child,
    try_wait_context: &str,
) -> Result<Option<ExitStatus>, String> {
    let started = Instant::now();
    loop {
        if let Some(status) = try_wait_during_termination_grace(child, try_wait_context)? {
            return Ok(Some(status));
        }
        if terminate_grace_period_elapsed(started) {
            return Ok(None);
        }
        thread::sleep(SUPERVISOR_POLL_INTERVAL);
    }
}

fn terminate_grace_period_elapsed(started: Instant) -> bool {
    started.elapsed() >= TERMINATE_GRACE_PERIOD
}

#[cfg(unix)]
fn send_child_sigkill(child: &mut Child) -> Result<(), String> {
    let pid = child_process_group_id(child);
    let rc = unsafe { libc::killpg(pid, libc::SIGKILL) };
    if rc == -1 {
        child.kill().map_err(|err| kill_child_process_error(&err))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn send_child_sigkill(child: &mut Child) -> Result<(), String> {
    child.kill().map_err(|err| kill_child_process_error(&err))
}

#[cfg(unix)]
fn send_child_sigterm(child: &Child) {
    use signal_hook::consts::signal::SIGTERM;
    let _ = unsafe { libc::killpg(child_process_group_id(child), SIGTERM) };
}

#[cfg(unix)]
fn child_process_group_id(child: &Child) -> libc::pid_t {
    child.id() as libc::pid_t
}

#[cfg(not(unix))]
fn send_child_sigterm(child: &mut Child) {
    let _ = child.kill();
}
