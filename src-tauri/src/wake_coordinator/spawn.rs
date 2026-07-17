//! ## Declared roles
//!
//! `accessor`, `filter`, `formatter`, `mapper`, `orchestration`

use oulipoly_state::{CompositeInvocationId, mailbox::SessionRuntimeRow};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use super::constants::{
    AUTO_WAKE_COUNT_ENV, AUTO_WAKE_ENV, AUTO_WAKE_SESSION_ID_ENV, AUTO_WAKE_TOKEN_ENV,
    PARENT_INVOCATION_ENV,
};

pub(super) fn spawn_detached_resume(
    session_id: &str,
    runtime: Option<&SessionRuntimeRow>,
    claim_token: &str,
    auto_wake_count: i64,
) -> Result<i64, String> {
    let exe = current_agents_exe()?;
    let cmd = detached_resume_command(exe, session_id, runtime, claim_token, auto_wake_count);
    spawn_detached_child(cmd)
}

fn current_agents_exe() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(current_agents_exe_error)
}

fn current_agents_exe_error(err: std::io::Error) -> String {
    format!("Failed to resolve current agents binary: {err}")
}

fn spawn_detached_child(mut cmd: Command) -> Result<i64, String> {
    cmd.spawn().map(child_pid).map_err(detached_spawn_error)
}

fn child_pid(child: Child) -> i64 {
    i64::from(child.id())
}

fn detached_spawn_error(err: std::io::Error) -> String {
    format!("Failed to spawn detached wake resume: {err}")
}

fn detached_resume_command(
    exe: PathBuf,
    session_id: &str,
    runtime: Option<&SessionRuntimeRow>,
    claim_token: &str,
    auto_wake_count: i64,
) -> Command {
    let mut cmd = Command::new(exe);
    configure_resume_args(&mut cmd, session_id, runtime);
    configure_wake_stdio_and_env(&mut cmd, session_id, runtime, claim_token, auto_wake_count);
    configure_detached(&mut cmd);
    cmd
}

fn configure_resume_args(cmd: &mut Command, session_id: &str, runtime: Option<&SessionRuntimeRow>) {
    cmd.arg("resume").arg("--session-id").arg(session_id);
    append_non_empty_arg(cmd, "-m", runtime.and_then(|row| row.model_name.as_deref()));
    append_non_empty_arg(
        cmd,
        "--models-dir",
        runtime.and_then(|row| row.models_dir.as_deref()),
    );
}

fn append_non_empty_arg(cmd: &mut Command, flag: &str, value: Option<&str>) {
    let Some(value) = non_empty_arg_value(value) else {
        return;
    };
    append_arg(cmd, flag, value);
}

fn non_empty_arg_value(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.is_empty())
}

fn append_arg(cmd: &mut Command, flag: &str, value: &str) {
    cmd.arg(flag).arg(value);
}

fn configure_wake_stdio_and_env(
    cmd: &mut Command,
    session_id: &str,
    runtime: Option<&SessionRuntimeRow>,
    claim_token: &str,
    auto_wake_count: i64,
) {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env(AUTO_WAKE_ENV, "1")
        .env(AUTO_WAKE_SESSION_ID_ENV, session_id)
        .env(AUTO_WAKE_TOKEN_ENV, claim_token)
        .env(AUTO_WAKE_COUNT_ENV, auto_wake_count.to_string());
    configure_parent_invocation(cmd, runtime);
}

fn configure_parent_invocation(cmd: &mut Command, runtime: Option<&SessionRuntimeRow>) {
    match parent_invocation_env(runtime) {
        Some(parent) => {
            cmd.env(PARENT_INVOCATION_ENV, parent);
        }
        None => {
            cmd.env_remove(PARENT_INVOCATION_ENV);
        }
    }
}

fn parent_invocation_env(runtime: Option<&SessionRuntimeRow>) -> Option<String> {
    let runtime = runtime?;
    let invocation_uuid = runtime.invocation_uuid.as_deref()?;
    uuid::Uuid::parse_str(invocation_uuid).ok()?;
    let source = runtime
        .provider_name
        .as_deref()
        .filter(|source| !source.is_empty())
        .unwrap_or("auto-wake");
    serde_json::to_string(&CompositeInvocationId {
        source: source.to_string(),
        id: invocation_uuid.to_string(),
    })
    .ok()
}

#[cfg(unix)]
fn configure_detached(cmd: &mut Command) {
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn configure_detached(_cmd: &mut Command) {}
