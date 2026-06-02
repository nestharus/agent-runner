use std::io::Read;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread::JoinHandle;

/// Script timeout — scripts hitting the internet shouldn't hang the caller.
const SCRIPT_TIMEOUT_SECS: u64 = 30;
/// Auth-refresh command timeout. Should be quick (the CLI hits its own auth
/// endpoint and exits); kept tight to avoid hanging the quota path.
const REFRESH_TIMEOUT_SECS: u64 = 15;

pub fn run_refresh_command(cmd_str: &str) -> Result<(), String> {
    let mut child = spawn_refresh_command(cmd_str)?;
    let stderr_handle = drain_child_stderr(&mut child);
    let status = wait_for_child(&mut child, REFRESH_TIMEOUT_SECS, RefreshProcessKind::Auth)?;
    let stderr_text = joined_text(stderr_handle);
    ensure_refresh_success(status, &stderr_text)
}

pub fn run_script(script: &str) -> Result<String, String> {
    let mut child = spawn_quota_script(script)?;

    // Drain stdout/stderr concurrently to avoid pipe-full deadlocks for
    // scripts that write a lot (unlikely for quota scripts but consistent
    // with the sessions-module pattern).
    let stdout_handle = drain_child_stdout(&mut child);
    let stderr_handle = drain_child_stderr(&mut child);
    let status = wait_for_child(&mut child, SCRIPT_TIMEOUT_SECS, RefreshProcessKind::Quota)?;
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
    cmd
}

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
    let _ = child.kill();
    Err(format_timeout(kind, timeout_secs))
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
        RefreshProcessKind::Quota => format!("Quota script timed out after {timeout_secs}s"),
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
