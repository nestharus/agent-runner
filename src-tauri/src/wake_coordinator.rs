//! ## Declared roles
//!
//! `orchestration`, `accessor`, `predicate`, `mapper`

use oulipoly_state::mailbox::{
    MailboxDb, SessionRuntimeIdleUpdate, SessionRuntimeRow, WakeClaimAcquireResult,
    WakeClaimRequest,
};
use serde::Serialize;
use std::process::{Command, Stdio};
use uuid::Uuid;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

const AUTO_WAKE_ENV: &str = "OULIPOLY_AUTO_WAKE";
const AUTO_WAKE_SESSION_ID_ENV: &str = "OULIPOLY_AUTO_WAKE_SESSION_ID";
const AUTO_WAKE_TOKEN_ENV: &str = "OULIPOLY_AUTO_WAKE_TOKEN";
const AUTO_WAKE_COUNT_ENV: &str = "OULIPOLY_AUTO_WAKE_COUNT";
const AUTO_WAKE_MAX_ENV: &str = "OULIPOLY_AUTO_WAKE_MAX";
const PARENT_INVOCATION_ENV: &str = "OULIPOLY_PARENT_INVOCATION";
const DEFAULT_AUTO_WAKE_MAX: i64 = 5;
const WAKE_CLAIM_STALE_AFTER_SECONDS: i64 = 10 * 60;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct WakeDiagnostic {
    pub(crate) attempted: bool,
    pub(crate) status: String,
    pub(crate) claim_token: Option<String>,
    pub(crate) wake_pid: Option<i64>,
    pub(crate) auto_wake_count: Option<i64>,
    pub(crate) message: Option<String>,
}

impl WakeDiagnostic {
    fn status(status: &str) -> Self {
        Self {
            attempted: false,
            status: status.to_string(),
            claim_token: None,
            wake_pid: None,
            auto_wake_count: None,
            message: None,
        }
    }

    fn with_message(status: &str, message: String) -> Self {
        Self {
            attempted: false,
            status: status.to_string(),
            claim_token: None,
            wake_pid: None,
            auto_wake_count: None,
            message: Some(message),
        }
    }
}

pub(crate) fn mark_session_idle_after_turn(
    session_id: &str,
    invocation_uuid: &str,
    exit_code: Option<i32>,
) -> Result<(), String> {
    let Some(mut db) = MailboxDb::open_default_if_exists()? else {
        return Ok(());
    };
    db.mark_session_idle(SessionRuntimeIdleUpdate {
        session_id,
        invocation_uuid,
        last_exit_code: exit_code,
    })?;
    Ok(())
}

pub(crate) fn trigger_notify_wake(session_id: &str) -> WakeDiagnostic {
    start_wake_chain(StartWakeInput {
        session_id,
        reason: "notify_idle",
        auto_wake_count: 1,
        renew_token: None,
    })
}

pub(crate) fn mark_successful_turn_idle_and_recheck(
    session_id: &str,
    invocation_uuid: &str,
    exit_code: i32,
) -> Result<WakeDiagnostic, String> {
    mark_session_idle_after_turn(session_id, invocation_uuid, Some(exit_code))?;
    Ok(trigger_turn_end_recheck(session_id))
}

pub(crate) fn validate_auto_wake_child(session_id: &str) -> Result<Option<i32>, String> {
    if std::env::var_os(AUTO_WAKE_ENV).is_none() {
        return Ok(None);
    }
    let expected_session = std::env::var(AUTO_WAKE_SESSION_ID_ENV).unwrap_or_default();
    let claim_token = std::env::var(AUTO_WAKE_TOKEN_ENV).unwrap_or_default();
    if expected_session != session_id || claim_token.is_empty() {
        return Ok(Some(0));
    }
    let Some(mut db) = MailboxDb::open_default_if_exists()? else {
        return Ok(Some(0));
    };
    if db.validate_wake_claim_for_child(session_id, &claim_token)? {
        Ok(None)
    } else {
        Ok(Some(0))
    }
}

pub(crate) fn is_auto_wake_invocation() -> bool {
    std::env::var_os(AUTO_WAKE_ENV).is_some()
}

pub(crate) fn reset_manual_resume_wake_claim(session_id: &str) -> Result<(), String> {
    let Some(mut db) = MailboxDb::open_default_if_exists()? else {
        return Ok(());
    };
    db.release_wake_claim(session_id, None)?;
    Ok(())
}

pub(crate) fn release_current_auto_wake_claim_for_session(session_id: &str) {
    let auto_wake = current_auto_wake();
    release_current_auto_wake_claim(session_id, auto_wake.as_ref());
}

fn trigger_turn_end_recheck(session_id: &str) -> WakeDiagnostic {
    let pending_count = match pending_count(session_id) {
        Ok(count) => count,
        Err(err) => return WakeDiagnostic::with_message("storage_error", err),
    };
    let auto_wake = current_auto_wake();
    if pending_count == 0 {
        release_current_auto_wake_claim(session_id, auto_wake.as_ref());
        return WakeDiagnostic::status("no_pending");
    }
    let current_count = auto_wake.as_ref().map(|wake| wake.count).unwrap_or(0);
    let max_count = auto_wake_max();
    if current_count >= max_count {
        release_current_auto_wake_claim(session_id, auto_wake.as_ref());
        eprintln!(
            "auto_wake_cap_reached session_id={session_id} count={current_count} max={max_count}"
        );
        let mut diagnostic = WakeDiagnostic::status("auto_wake_cap_reached");
        diagnostic.auto_wake_count = Some(current_count);
        return diagnostic;
    }
    start_wake_chain(StartWakeInput {
        session_id,
        reason: "turn_end_recheck",
        auto_wake_count: current_count + 1,
        renew_token: auto_wake.as_ref().map(|wake| wake.token.as_str()),
    })
}

struct AutoWakeEnv {
    token: String,
    count: i64,
}

struct StartWakeInput<'a> {
    session_id: &'a str,
    reason: &'a str,
    auto_wake_count: i64,
    renew_token: Option<&'a str>,
}

fn start_wake_chain(input: StartWakeInput<'_>) -> WakeDiagnostic {
    let claim_token = Uuid::new_v4().to_string();
    let mut db = match MailboxDb::open_default() {
        Ok(db) => db,
        Err(err) => return WakeDiagnostic::with_message("storage_error", err),
    };
    let runtime = match db.session_runtime(input.session_id) {
        Ok(row) => row,
        Err(err) => return WakeDiagnostic::with_message("storage_error", err),
    };
    if runtime
        .as_ref()
        .is_some_and(|row| row.mode == "pty_interactive")
    {
        return WakeDiagnostic::status("busy");
    }
    let claim = match db.try_acquire_or_renew_wake_claim(
        WakeClaimRequest {
            session_id: input.session_id,
            claim_token: &claim_token,
            reason: input.reason,
            auto_wake_count: input.auto_wake_count,
            wake_invocation_uuid: None,
            stale_after_seconds: WAKE_CLAIM_STALE_AFTER_SECONDS,
        },
        input.renew_token,
    ) {
        Ok(WakeClaimAcquireResult::Acquired(claim)) => claim,
        Ok(WakeClaimAcquireResult::NoPending) => return WakeDiagnostic::status("no_pending"),
        Ok(WakeClaimAcquireResult::Busy) => return WakeDiagnostic::status("busy"),
        Ok(WakeClaimAcquireResult::AlreadyInFlight(claim)) => {
            let mut diagnostic = WakeDiagnostic::status("already_in_flight");
            diagnostic.claim_token = Some(claim.claim_token);
            diagnostic.wake_pid = claim.wake_pid;
            diagnostic.auto_wake_count = Some(claim.auto_wake_count);
            return diagnostic;
        }
        Err(err) => return WakeDiagnostic::with_message("storage_error", err),
    };
    let spawn = spawn_detached_resume(
        input.session_id,
        runtime.as_ref(),
        &claim.claim_token,
        input.auto_wake_count,
    );
    match spawn {
        Ok(wake_pid) => {
            if let Err(err) =
                db.record_wake_claim_pid(input.session_id, &claim.claim_token, wake_pid)
            {
                tracing::warn!(
                    session_id = input.session_id,
                    claim_token = %claim.claim_token,
                    "Failed to record wake PID: {err}"
                );
            }
            WakeDiagnostic {
                attempted: true,
                status: "spawned".to_string(),
                claim_token: Some(claim.claim_token),
                wake_pid: Some(wake_pid),
                auto_wake_count: Some(input.auto_wake_count),
                message: None,
            }
        }
        Err(err) => {
            let _ = db.release_wake_claim(input.session_id, Some(&claim.claim_token));
            WakeDiagnostic {
                attempted: true,
                status: "spawn_error".to_string(),
                claim_token: Some(claim.claim_token),
                wake_pid: None,
                auto_wake_count: Some(input.auto_wake_count),
                message: Some(err),
            }
        }
    }
}

fn spawn_detached_resume(
    session_id: &str,
    runtime: Option<&SessionRuntimeRow>,
    claim_token: &str,
    auto_wake_count: i64,
) -> Result<i64, String> {
    let exe = std::env::current_exe()
        .map_err(|err| format!("Failed to resolve current agents binary: {err}"))?;
    let mut cmd = Command::new(exe);
    cmd.arg("resume").arg("--session-id").arg(session_id);
    if let Some(model_name) = runtime.and_then(|row| row.model_name.as_deref())
        && !model_name.is_empty()
    {
        cmd.arg("-m").arg(model_name);
    }
    if let Some(models_dir) = runtime.and_then(|row| row.models_dir.as_deref())
        && !models_dir.is_empty()
    {
        cmd.arg("--models-dir").arg(models_dir);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env(AUTO_WAKE_ENV, "1")
        .env(AUTO_WAKE_SESSION_ID_ENV, session_id)
        .env(AUTO_WAKE_TOKEN_ENV, claim_token)
        .env(AUTO_WAKE_COUNT_ENV, auto_wake_count.to_string())
        .env_remove(PARENT_INVOCATION_ENV);
    configure_detached(&mut cmd);
    let child = cmd
        .spawn()
        .map_err(|err| format!("Failed to spawn detached wake resume: {err}"))?;
    Ok(i64::from(child.id()))
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

fn pending_count(session_id: &str) -> Result<usize, String> {
    let Some(db) = MailboxDb::open_default_if_exists()? else {
        return Ok(0);
    };
    db.list_pending(session_id).map(|rows| rows.len())
}

fn current_auto_wake() -> Option<AutoWakeEnv> {
    std::env::var_os(AUTO_WAKE_ENV)?;
    let token = std::env::var(AUTO_WAKE_TOKEN_ENV).ok()?;
    let count = std::env::var(AUTO_WAKE_COUNT_ENV)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1);
    Some(AutoWakeEnv { token, count })
}

fn auto_wake_max() -> i64 {
    std::env::var(AUTO_WAKE_MAX_ENV)
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_AUTO_WAKE_MAX)
}

fn release_current_auto_wake_claim(session_id: &str, auto_wake: Option<&AutoWakeEnv>) {
    let Some(auto_wake) = auto_wake else {
        return;
    };
    match MailboxDb::open_default_if_exists() {
        Ok(Some(mut db)) => {
            if let Err(err) = db.release_wake_claim(session_id, Some(&auto_wake.token)) {
                tracing::warn!(session_id, "Failed to release wake claim: {err}");
            }
        }
        Ok(None) => {}
        Err(err) => tracing::warn!(
            session_id,
            "Failed to open sidecar to release wake claim: {err}"
        ),
    }
}
