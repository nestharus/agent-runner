//! ## Declared roles
//!
//! `accessor`, `filter`, `formatter`, `mapper`, `orchestration`

use oulipoly_state::{CompositeInvocationId, mailbox::SessionMetadataRow};
use std::process::{Child, Command, Stdio};

#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(target_os = "linux")]
use std::{fs::File, os::fd::AsRawFd};

use super::constants::{
    AUTO_WAKE_COUNT_ENV, AUTO_WAKE_ENV, AUTO_WAKE_SESSION_ID_ENV, AUTO_WAKE_TOKEN_ENV,
    PARENT_INVOCATION_ENV, WAKE_RECLAIM_HANDOFF_OWNER_ENV, WAKE_RECLAIM_HANDOFF_TOKEN_ENV,
};

pub(super) fn spawn_detached_resume(
    session_id: &str,
    runtime: Option<&SessionMetadataRow>,
    claim_token: &str,
    auto_wake_count: i64,
) -> Result<i64, String> {
    let mut launch = current_agents_command()?;
    configure_resume_command(
        &mut launch.command,
        session_id,
        runtime,
        claim_token,
        auto_wake_count,
    );
    spawn_detached_child(launch)
}

pub(super) fn spawn_detached_wake_reclaim_handoff(
    owner_token: &str,
    handoff_token: &str,
) -> Result<(), String> {
    let mut launch = current_agents_command()?;
    launch
        .command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env(WAKE_RECLAIM_HANDOFF_OWNER_ENV, owner_token)
        .env(WAKE_RECLAIM_HANDOFF_TOKEN_ENV, handoff_token)
        .env_remove(AUTO_WAKE_ENV);
    configure_handoff_detached(&mut launch.command);
    launch
        .command
        .spawn()
        .map(drop)
        .map_err(|error| format!("Failed to spawn detached wake reclaim handoff: {error}"))
}

struct CurrentAgentsCommand {
    command: Command,
    #[cfg(target_os = "linux")]
    _executable: File,
}

#[cfg(target_os = "linux")]
fn current_agents_command() -> Result<CurrentAgentsCommand, String> {
    let executable = File::open("/proc/self/exe").map_err(current_agents_exe_error)?;
    let fd = executable.as_raw_fd();
    let mut command = Command::new(format!("/proc/self/fd/{fd}"));
    unsafe {
        command.pre_exec(move || {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(CurrentAgentsCommand {
        command,
        _executable: executable,
    })
}

#[cfg(not(target_os = "linux"))]
fn current_agents_command() -> Result<CurrentAgentsCommand, String> {
    let executable = std::env::current_exe().map_err(current_agents_exe_error)?;
    Ok(CurrentAgentsCommand {
        command: Command::new(executable),
    })
}

fn current_agents_exe_error(err: std::io::Error) -> String {
    format!("Failed to open the running agents executable for re-launch: {err}")
}

fn spawn_detached_child(mut launch: CurrentAgentsCommand) -> Result<i64, String> {
    launch
        .command
        .spawn()
        .map(child_pid)
        .map_err(detached_spawn_error)
}

fn child_pid(child: Child) -> i64 {
    i64::from(child.id())
}

fn detached_spawn_error(err: std::io::Error) -> String {
    format!("Failed to spawn detached wake resume: {err}")
}

fn configure_resume_command(
    cmd: &mut Command,
    session_id: &str,
    runtime: Option<&SessionMetadataRow>,
    claim_token: &str,
    auto_wake_count: i64,
) {
    configure_resume_args(cmd, session_id, runtime);
    configure_wake_stdio_and_env(cmd, session_id, runtime, claim_token, auto_wake_count);
    configure_detached(cmd);
}

fn configure_resume_args(
    cmd: &mut Command,
    session_id: &str,
    runtime: Option<&SessionMetadataRow>,
) {
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
    runtime: Option<&SessionMetadataRow>,
    claim_token: &str,
    auto_wake_count: i64,
) {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env(AUTO_WAKE_ENV, "1")
        .env(AUTO_WAKE_SESSION_ID_ENV, session_id)
        .env(AUTO_WAKE_TOKEN_ENV, claim_token)
        .env(AUTO_WAKE_COUNT_ENV, auto_wake_count.to_string())
        .env_remove(WAKE_RECLAIM_HANDOFF_OWNER_ENV)
        .env_remove(WAKE_RECLAIM_HANDOFF_TOKEN_ENV);
    configure_parent_invocation(cmd, runtime);
}

fn configure_parent_invocation(cmd: &mut Command, runtime: Option<&SessionMetadataRow>) {
    match parent_invocation_env(runtime) {
        Some(parent) => {
            cmd.env(PARENT_INVOCATION_ENV, parent);
        }
        None => {
            cmd.env_remove(PARENT_INVOCATION_ENV);
        }
    }
}

fn parent_invocation_env(runtime: Option<&SessionMetadataRow>) -> Option<String> {
    let (source, invocation_uuid) = parent_invocation_fields(runtime)?;
    let invocation_uuid = validate_parent_invocation_uuid(invocation_uuid)?;
    format_parent_invocation_env(source, invocation_uuid)
}

fn parent_invocation_fields(runtime: Option<&SessionMetadataRow>) -> Option<(&str, &str)> {
    let runtime = runtime?;
    let invocation_uuid = runtime
        .invocation_uuid
        .as_deref()
        .filter(|invocation_uuid| !invocation_uuid.is_empty())?;
    let source = runtime
        .provider_name
        .as_deref()
        .filter(|source| !source.is_empty())
        .unwrap_or("auto-wake");
    Some((source, invocation_uuid))
}

fn validate_parent_invocation_uuid(invocation_uuid: &str) -> Option<&str> {
    uuid::Uuid::parse_str(invocation_uuid)
        .ok()
        .map(|_| invocation_uuid)
}

fn format_parent_invocation_env(source: &str, invocation_uuid: &str) -> Option<String> {
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

#[cfg(unix)]
fn configure_handoff_detached(cmd: &mut Command) {
    configure_detached(cmd);
}

#[cfg(windows)]
fn configure_handoff_detached(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
}

#[cfg(not(any(unix, windows)))]
fn configure_handoff_detached(_cmd: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime(invocation_uuid: Option<&str>, provider_name: Option<&str>) -> SessionMetadataRow {
        SessionMetadataRow {
            session_id: "session-a".to_string(),
            mode: "headless".to_string(),
            invocation_uuid: invocation_uuid.map(str::to_string),
            provider_name: provider_name.map(str::to_string),
            model_name: None,
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            models_dir: None,
            effective_cwd: None,
            auto_wake_count: 0,
        }
    }

    fn command_env(cmd: &Command, key: &str) -> Option<Option<String>> {
        cmd.get_envs()
            .find(|entry| entry.0 == std::ffi::OsStr::new(key))
            .map(|(_, value)| value.map(|value| value.to_string_lossy().into_owned()))
    }

    #[test]
    fn parent_invocation_helpers_preserve_source_id_and_original_uuid_text() {
        let invocation_uuid = "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA";
        let runtime = runtime(Some(invocation_uuid), Some("provider-a"));
        assert_eq!(
            parent_invocation_fields(Some(&runtime)),
            Some(("provider-a", invocation_uuid))
        );
        assert_eq!(
            validate_parent_invocation_uuid(invocation_uuid),
            Some(invocation_uuid)
        );
        let encoded = parent_invocation_env(Some(&runtime)).unwrap();
        let decoded: CompositeInvocationId = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.source, "provider-a");
        assert_eq!(decoded.id, invocation_uuid);
    }

    #[test]
    fn parent_invocation_environment_uses_fallback_and_omits_invalid_identity() {
        let invocation_uuid = "11111111-1111-4111-8111-111111111111";
        let fallback = runtime(Some(invocation_uuid), Some(""));
        let encoded = parent_invocation_env(Some(&fallback)).unwrap();
        let decoded: CompositeInvocationId = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.source, "auto-wake");

        let invalid = runtime(Some("not-a-uuid"), Some("provider-a"));
        let empty = runtime(Some(""), Some("provider-a"));
        assert_eq!(parent_invocation_env(Some(&invalid)), None);
        assert_eq!(parent_invocation_env(Some(&empty)), None);
        assert_eq!(parent_invocation_env(None), None);

        let mut cmd = Command::new("agents");
        configure_parent_invocation(&mut cmd, Some(&fallback));
        assert!(matches!(
            command_env(&cmd, PARENT_INVOCATION_ENV),
            Some(Some(value)) if value.contains("auto-wake")
        ));
        configure_parent_invocation(&mut cmd, Some(&invalid));
        assert_eq!(command_env(&cmd, PARENT_INVOCATION_ENV), Some(None));
    }
}
