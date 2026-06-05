//! Quota-script and auth-refresh process spawning, draining, and timeout
//! handling.
//!
//! ## Declared roles
//! orchestration, accessor, formatter, mapper, predicate
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/quota/process.rs
//!     role: adapter
//!     Translates:
//!       - std process execution contract (`std::process::Command`, `Child`, `ExitStatus`, `Stdio`)
//!       - std concurrent stream draining contract (`std::io::Read`, `std::thread`, `JoinHandle`)
//!       - std timeout contract (`std::time::Instant`, `std::time::Duration`)
//! ```

use std::io::Read;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread::JoinHandle;

/// Quota scripts run on the pre-dispatch path. Ninety seconds is intentionally
/// long enough for slow CLI/API startup but short enough that metrics can never
/// wedge provider selection indefinitely.
const SCRIPT_TIMEOUT_SECS: u64 = 90;
/// Auth-refresh command timeout. Should be quick (the CLI hits its own auth
/// endpoint and exits); kept tight to avoid hanging the quota path.
const REFRESH_TIMEOUT_SECS: u64 = 15;

/// Raw `auth_refresh_command` shell-out. This rotates a single-use OAuth
/// refresh token, so production callers MUST go through
/// [`super::run_auth_refresh_command_coalesced`] (the per-account single-flight
/// lock) rather than calling this directly; an unsynchronized invocation can
/// race a concurrent refresh and trigger provider reuse-detection revocation.
pub fn run_refresh_command(cmd_str: &str) -> Result<(), String> {
    let mut child = spawn_refresh_command(cmd_str)?;
    let stderr_handle = drain_child_stderr(&mut child);
    let status = wait_for_child(&mut child, REFRESH_TIMEOUT_SECS, RefreshProcessKind::Auth)?;
    let stderr_text = joined_text(stderr_handle);
    ensure_refresh_success(status, &stderr_text)
}

pub fn run_script(script: &str) -> Result<String, String> {
    run_script_with_timeout(script, SCRIPT_TIMEOUT_SECS)
}

fn run_script_with_timeout(script: &str, timeout_secs: u64) -> Result<String, String> {
    let mut child = spawn_quota_script(script)?;

    // Drain stdout/stderr concurrently to avoid pipe-full deadlocks for
    // scripts that write a lot (unlikely for quota scripts but consistent
    // with the sessions-module pattern).
    let stdout_handle = drain_child_stdout(&mut child);
    let stderr_handle = drain_child_stderr(&mut child);
    let status = wait_for_child(&mut child, timeout_secs, RefreshProcessKind::Quota)?;
    let stdout_text = joined_text(stdout_handle);
    let stderr_text = joined_text(stderr_handle);

    ensure_quota_success(status, &stderr_text)?;
    Ok(stdout_text)
}

fn spawn_refresh_command(cmd_str: &str) -> Result<Child, String> {
    let mut cmd = shell_command(cmd_str);
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::piped());
    cmd.spawn().map_err(format_refresh_spawn_error)
}

fn spawn_quota_script(script: &str) -> Result<Child, String> {
    let mut cmd = shell_command(script);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.spawn().map_err(format_quota_spawn_error)
}

fn shell_command(command: &str) -> Command {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(command);
    cmd.stdin(Stdio::null());
    configure_script_process_group(&mut cmd);
    cmd
}

#[cfg(unix)]
fn configure_script_process_group(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;

    cmd.process_group(0);
}

#[cfg(not(unix))]
fn configure_script_process_group(_cmd: &mut Command) {}

fn drain_child_stdout(child: &mut Child) -> JoinHandle<String> {
    spawn_string_drain(child.stdout.take().expect("piped"))
}

fn drain_child_stderr(child: &mut Child) -> JoinHandle<String> {
    spawn_string_drain(child.stderr.take().expect("piped"))
}

fn spawn_string_drain<R>(reader: R) -> JoinHandle<String>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || drain_to_string(reader))
}

fn drain_to_string<R: Read>(mut reader: R) -> String {
    let mut buf = String::new();
    let _ = reader.read_to_string(&mut buf);
    buf
}

fn wait_for_child(
    child: &mut Child,
    timeout_secs: u64,
    kind: RefreshProcessKind,
) -> Result<ExitStatus, String> {
    let timeout = std::time::Duration::from_secs(timeout_secs);
    let start = std::time::Instant::now();
    let mut step = wait_step(child, kind, start, timeout)?;
    while matches!(step, WaitStep::Pending) {
        std::thread::sleep(std::time::Duration::from_millis(50));
        step = wait_step(child, kind, start, timeout)?;
    }
    finish_wait_step(child, step, kind, timeout_secs)
}

fn try_wait_child(
    child: &mut Child,
    kind: RefreshProcessKind,
) -> Result<Option<ExitStatus>, String> {
    child.try_wait().map_err(|e| format_wait_error(kind, e))
}

fn wait_step(
    child: &mut Child,
    kind: RefreshProcessKind,
    start: std::time::Instant,
    timeout: std::time::Duration,
) -> Result<WaitStep, String> {
    if let Some(status) = try_wait_child(child, kind)? {
        return Ok(WaitStep::Complete(status));
    }
    Ok(timeout_wait_step(start, timeout))
}

fn timeout_wait_step(start: std::time::Instant, timeout: std::time::Duration) -> WaitStep {
    if start.elapsed() >= timeout {
        return WaitStep::TimedOut;
    }
    WaitStep::Pending
}

fn finish_wait_step(
    child: &mut Child,
    step: WaitStep,
    kind: RefreshProcessKind,
    timeout_secs: u64,
) -> Result<ExitStatus, String> {
    match step {
        WaitStep::Complete(status) => Ok(status),
        WaitStep::TimedOut => kill_timed_out_child(child, kind, timeout_secs),
        WaitStep::Pending => unreachable!("wait_for_child exits pending loop before finishing"),
    }
}

fn kill_timed_out_child(
    child: &mut Child,
    kind: RefreshProcessKind,
    timeout_secs: u64,
) -> Result<ExitStatus, String> {
    kill_child_process_group(child);
    Err(format_timeout(kind, timeout_secs))
}

#[cfg(unix)]
fn kill_child_process_group(child: &mut Child) {
    let pgid = -(child.id() as libc::pid_t);
    // SAFETY: `pgid` targets the process group created with `process_group(0)`
    // for this child. Killing the group is required so shell grandchildren do
    // not keep pipes open or continue running after the metrics deadline.
    let _ = unsafe { libc::kill(pgid, libc::SIGKILL) };
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(not(unix))]
fn kill_child_process_group(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn joined_text(handle: JoinHandle<String>) -> String {
    handle.join().unwrap_or_default()
}

fn ensure_refresh_success(status: ExitStatus, stderr_text: &str) -> Result<(), String> {
    if !status.success() {
        return Err(format_refresh_exit(status, stderr_text));
    }
    Ok(())
}

fn ensure_quota_success(status: ExitStatus, stderr_text: &str) -> Result<(), String> {
    if !status.success() {
        return Err(format_quota_exit(status, stderr_text));
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum RefreshProcessKind {
    Auth,
    Quota,
}

enum WaitStep {
    Complete(ExitStatus),
    TimedOut,
    Pending,
}

fn format_refresh_spawn_error(error: std::io::Error) -> String {
    format!("Failed to spawn auth_refresh_command: {error}")
}

fn format_quota_spawn_error(error: std::io::Error) -> String {
    format!("Failed to spawn quota script: {error}")
}

fn format_timeout(kind: RefreshProcessKind, timeout_secs: u64) -> String {
    match kind {
        RefreshProcessKind::Auth => {
            format!("auth_refresh_command timed out after {timeout_secs}s")
        }
        RefreshProcessKind::Quota => {
            format!("script_timeout: quota script timed out after {timeout_secs}s")
        }
    }
}

fn format_wait_error(kind: RefreshProcessKind, error: std::io::Error) -> String {
    match kind {
        RefreshProcessKind::Auth => format!("auth_refresh_command wait failed: {error}"),
        RefreshProcessKind::Quota => format!("Quota script wait failed: {error}"),
    }
}

fn format_refresh_exit(status: ExitStatus, stderr_text: &str) -> String {
    format!(
        "auth_refresh_command exited {}: {}",
        status.code().unwrap_or(-1),
        stderr_text.trim()
    )
}

fn format_quota_exit(status: ExitStatus, stderr_text: &str) -> String {
    format!(
        "Quota script exited {}: {}",
        status.code().unwrap_or(-1),
        stderr_text.trim()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn quota_script_timeout_is_classified() {
        let err = run_script_with_timeout("sleep 60", 1).unwrap_err();

        assert!(err.contains("script_timeout"), "{err}");
        assert!(err.contains("quota script"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn quota_script_timeout_kills_process_group_children() {
        let dir = tempfile::tempdir().unwrap();
        let leaked_marker = dir.path().join("leaked");
        let script = format!(
            "(sleep 2; printf leaked > {}) & wait",
            leaked_marker.display()
        );

        let err = run_script_with_timeout(&script, 1).unwrap_err();
        std::thread::sleep(Duration::from_secs(3));

        assert!(err.contains("script_timeout"), "{err}");
        assert!(
            !leaked_marker.exists(),
            "timed-out quota script left a process-group child running"
        );
    }
}
