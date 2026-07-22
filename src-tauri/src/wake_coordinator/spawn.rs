//! ## Declared roles
//!
//! `accessor`, `filter`, `formatter`, `mapper`, `orchestration`

use oulipoly_state::{CompositeInvocationId, mailbox::SessionRuntimeRow};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use super::constants::{
    AUTO_WAKE_COUNT_ENV, AUTO_WAKE_ENV, AUTO_WAKE_MAX_ENV, AUTO_WAKE_SESSION_ID_ENV,
    AUTO_WAKE_TOKEN_ENV, PARENT_INVOCATION_ENV,
};

pub(super) fn spawn_detached_resume(
    session_id: &str,
    runtime: Option<&SessionRuntimeRow>,
    claim_token: &str,
    auto_wake_count: i64,
    auto_wake_max: i64,
) -> Result<i64, String> {
    let exe = current_agents_exe()?;
    let cmd = detached_resume_command(
        exe,
        session_id,
        runtime,
        claim_token,
        auto_wake_count,
        auto_wake_max,
    );
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
    auto_wake_max: i64,
) -> Command {
    let mut cmd = Command::new(exe);
    configure_resume_args(&mut cmd, session_id, runtime);
    configure_wake_stdio_and_env(
        &mut cmd,
        session_id,
        runtime,
        claim_token,
        auto_wake_count,
        auto_wake_max,
    );
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
    auto_wake_max: i64,
) {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env(AUTO_WAKE_ENV, "1")
        .env(AUTO_WAKE_SESSION_ID_ENV, session_id)
        .env(AUTO_WAKE_TOKEN_ENV, claim_token)
        .env(AUTO_WAKE_COUNT_ENV, auto_wake_count.to_string())
        .env(AUTO_WAKE_MAX_ENV, auto_wake_max.to_string());
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
    let (source, invocation_uuid) = parent_invocation_fields(runtime)?;
    let invocation_uuid = validate_parent_invocation_uuid(invocation_uuid)?;
    format_parent_invocation_env(source, invocation_uuid)
}

fn parent_invocation_fields(runtime: Option<&SessionRuntimeRow>) -> Option<(&str, &str)> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime(invocation_uuid: Option<&str>, provider_name: Option<&str>) -> SessionRuntimeRow {
        SessionRuntimeRow {
            session_id: "session-a".to_string(),
            mode: "headless".to_string(),
            invocation_uuid: invocation_uuid.map(str::to_string),
            provider_name: provider_name.map(str::to_string),
            model_name: None,
            pty_control_path: None,
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            run_state: "idle".to_string(),
            running_invocation_uuid: None,
            running_os_pid: None,
            running_os_boot_id: None,
            running_os_pid_starttime_ticks: None,
            turn_started_at: None,
            turn_ended_at: None,
            turn_start_max_mailbox_seq: None,
            last_exit_code: None,
            models_dir: None,
            effective_cwd: None,
            auto_wake_count: 0,
            selected_auto_wake_max: None,
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
