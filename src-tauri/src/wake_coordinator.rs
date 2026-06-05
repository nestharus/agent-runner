//! ## Declared roles
//!
//! `accessor`, `formatter`, `mapper`, `orchestration`, `parser`, `predicate`,
//! `validator`

use oulipoly_state::mailbox::{
    MailboxDb, SessionLiveness, SessionRuntimeIdleUpdate, SessionRuntimeRow,
    WakeClaimAcquireResult, WakeClaimRequest, WakeClaimRow,
};
use serde::Serialize;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use uuid::Uuid;

#[cfg(unix)]
use oulipoly_runtime::executor::cli::pty_broker;
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
    Ok(successful_turn_recheck(session_id))
}

fn successful_turn_recheck(session_id: &str) -> WakeDiagnostic {
    trigger_turn_end_recheck(session_id)
}

pub(crate) fn validate_auto_wake_child(session_id: &str) -> Result<Option<i32>, String> {
    if !auto_wake_marker_present() {
        return Ok(None);
    }
    let marker = auto_wake_child_marker();
    if !auto_wake_child_marker_matches(session_id, &marker) {
        return Ok(Some(0));
    }
    validate_auto_wake_child_claim(session_id, &marker.claim_token)
}

struct AutoWakeChildMarker {
    expected_session: String,
    claim_token: String,
}

fn auto_wake_marker_present() -> bool {
    std::env::var_os(AUTO_WAKE_ENV).is_some()
}

fn auto_wake_child_marker() -> AutoWakeChildMarker {
    auto_wake_child_marker_from_parts(auto_wake_expected_session(), auto_wake_child_claim_token())
}

fn auto_wake_expected_session() -> String {
    std::env::var(AUTO_WAKE_SESSION_ID_ENV).unwrap_or_default()
}

fn auto_wake_child_claim_token() -> String {
    std::env::var(AUTO_WAKE_TOKEN_ENV).unwrap_or_default()
}

fn auto_wake_child_marker_from_parts(
    expected_session: String,
    claim_token: String,
) -> AutoWakeChildMarker {
    AutoWakeChildMarker {
        expected_session,
        claim_token,
    }
}

fn auto_wake_child_marker_matches(session_id: &str, marker: &AutoWakeChildMarker) -> bool {
    marker.expected_session == session_id && !marker.claim_token.is_empty()
}

fn validate_auto_wake_child_claim(
    session_id: &str,
    claim_token: &str,
) -> Result<Option<i32>, String> {
    let Some(mut db) = open_optional_wake_mailbox()? else {
        return Ok(Some(0));
    };
    validate_auto_wake_claim_with_db(&mut db, session_id, claim_token)
}

fn open_optional_wake_mailbox() -> Result<Option<MailboxDb>, String> {
    MailboxDb::open_default_if_exists()
}

fn validate_auto_wake_claim_with_db(
    db: &mut MailboxDb,
    session_id: &str,
    claim_token: &str,
) -> Result<Option<i32>, String> {
    db.validate_wake_claim_for_child(session_id, claim_token)
        .map(auto_wake_child_validation_result)
}

fn auto_wake_child_validation_result(valid: bool) -> Option<i32> {
    if valid { None } else { Some(0) }
}

pub(crate) fn is_auto_wake_invocation() -> bool {
    auto_wake_marker_present()
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
    let pending_count = match turn_end_pending_count(session_id) {
        Ok(count) => count,
        Err(err) => return storage_error_diagnostic(err),
    };
    let auto_wake = current_auto_wake();
    if no_pending(pending_count) {
        release_current_auto_wake_claim(session_id, auto_wake.as_ref());
        return WakeDiagnostic::status("no_pending");
    }
    let current_count = current_auto_wake_count(auto_wake.as_ref());
    let max_count = auto_wake_max();
    if auto_wake_cap_reached(current_count, max_count) {
        release_current_auto_wake_claim(session_id, auto_wake.as_ref());
        emit_auto_wake_cap_reached(session_id, current_count, max_count);
        return auto_wake_cap_diagnostic(current_count);
    }
    start_wake_chain(StartWakeInput {
        session_id,
        reason: "turn_end_recheck",
        auto_wake_count: current_count + 1,
        renew_token: auto_wake.as_ref().map(|wake| wake.token.as_str()),
    })
}

fn current_auto_wake_count(auto_wake: Option<&AutoWakeEnv>) -> i64 {
    auto_wake.map(|wake| wake.count).unwrap_or(0)
}

fn turn_end_pending_count(session_id: &str) -> Result<usize, String> {
    pending_count(session_id)
}

fn no_pending(pending_count: usize) -> bool {
    pending_count == 0
}

fn auto_wake_cap_reached(current_count: i64, max_count: i64) -> bool {
    current_count >= max_count
}

fn emit_auto_wake_cap_reached(session_id: &str, current_count: i64, max_count: i64) {
    eprintln!(
        "auto_wake_cap_reached session_id={session_id} count={current_count} max={max_count}"
    );
}

fn auto_wake_cap_diagnostic(current_count: i64) -> WakeDiagnostic {
    let mut diagnostic = WakeDiagnostic::status("auto_wake_cap_reached");
    diagnostic.auto_wake_count = Some(current_count);
    diagnostic
}

fn storage_error_diagnostic(err: String) -> WakeDiagnostic {
    WakeDiagnostic::with_message("storage_error", err)
}

struct AutoWakeEnv {
    token: String,
    count: i64,
}

#[derive(Clone, Copy)]
struct StartWakeInput<'a> {
    session_id: &'a str,
    reason: &'a str,
    auto_wake_count: i64,
    renew_token: Option<&'a str>,
}

fn start_wake_chain(input: StartWakeInput<'_>) -> WakeDiagnostic {
    let claim_token = Uuid::new_v4().to_string();
    let mut db = match open_wake_mailbox() {
        Ok(db) => db,
        Err(err) => return storage_error_diagnostic(err),
    };
    let runtime = match session_runtime_for_wake(&db, input.session_id) {
        Ok(row) => row,
        Err(err) => return storage_error_diagnostic(err),
    };
    match pty_runtime_is_busy(&mut db, input.session_id, runtime.as_ref()) {
        Ok(true) => return WakeDiagnostic::status("busy"),
        Ok(false) => {}
        Err(err) => return storage_error_diagnostic(err),
    }
    let claim_result = match acquire_wake_claim(&mut db, input, &claim_token) {
        Ok(result) => result,
        Err(err) => return storage_error_diagnostic(err),
    };
    let claim = match wake_claim_to_start(claim_result) {
        Ok(claim) => claim,
        Err(diagnostic) => return diagnostic,
    };
    let spawn = spawn_detached_resume(
        input.session_id,
        runtime.as_ref(),
        &claim.claim_token,
        input.auto_wake_count,
    );
    wake_spawn_diagnostic(&mut db, input, claim, spawn)
}

fn wake_claim_to_start(result: WakeClaimAcquireResult) -> Result<WakeClaimRow, WakeDiagnostic> {
    match result {
        WakeClaimAcquireResult::Acquired(claim) => Ok(claim),
        WakeClaimAcquireResult::NoPending => Err(WakeDiagnostic::status("no_pending")),
        WakeClaimAcquireResult::Busy => Err(WakeDiagnostic::status("busy")),
        WakeClaimAcquireResult::AlreadyInFlight(claim) => Err(already_in_flight_diagnostic(claim)),
    }
}

fn wake_spawn_diagnostic(
    db: &mut MailboxDb,
    input: StartWakeInput<'_>,
    claim: WakeClaimRow,
    spawn: Result<i64, String>,
) -> WakeDiagnostic {
    match spawn {
        Ok(wake_pid) => {
            record_wake_pid_or_warn(db, input.session_id, &claim.claim_token, wake_pid);
            spawned_wake_diagnostic(claim.claim_token, wake_pid, input.auto_wake_count)
        }
        Err(err) => {
            let _ = db.release_wake_claim(input.session_id, Some(&claim.claim_token));
            spawn_error_diagnostic(claim.claim_token, input.auto_wake_count, err)
        }
    }
}

fn open_wake_mailbox() -> Result<MailboxDb, String> {
    MailboxDb::open_default()
}

fn session_runtime_for_wake(
    db: &MailboxDb,
    session_id: &str,
) -> Result<Option<SessionRuntimeRow>, String> {
    db.session_runtime(session_id)
}

fn pty_runtime_is_busy(
    db: &mut MailboxDb,
    session_id: &str,
    runtime: Option<&SessionRuntimeRow>,
) -> Result<bool, String> {
    let Some(row) = runtime else {
        return Ok(false);
    };
    if row.mode != "pty_interactive" || row.run_state != "running" {
        return Ok(false);
    }
    let control_path = row.pty_control_path.clone();
    let liveness = db.session_liveness(session_id)?;
    if liveness == SessionLiveness::Idle {
        unlink_stale_pty_socket(control_path.as_deref());
    }
    Ok(liveness == SessionLiveness::Busy)
}

#[cfg(unix)]
fn unlink_stale_pty_socket(path: Option<&str>) {
    if let Some(path) = path {
        let _ = pty_broker::unlink_control_socket_if_owned(path);
    }
}

#[cfg(not(unix))]
fn unlink_stale_pty_socket(_path: Option<&str>) {}

fn acquire_wake_claim(
    db: &mut MailboxDb,
    input: StartWakeInput<'_>,
    claim_token: &str,
) -> Result<WakeClaimAcquireResult, String> {
    db.try_acquire_or_renew_wake_claim(wake_claim_request(input, claim_token), input.renew_token)
}

fn wake_claim_request<'a>(input: StartWakeInput<'a>, claim_token: &'a str) -> WakeClaimRequest<'a> {
    WakeClaimRequest {
        session_id: input.session_id,
        claim_token,
        reason: input.reason,
        auto_wake_count: input.auto_wake_count,
        wake_invocation_uuid: None,
        stale_after_seconds: WAKE_CLAIM_STALE_AFTER_SECONDS,
    }
}

fn already_in_flight_diagnostic(claim: WakeClaimRow) -> WakeDiagnostic {
    let mut diagnostic = WakeDiagnostic::status("already_in_flight");
    diagnostic.claim_token = Some(claim.claim_token);
    diagnostic.wake_pid = claim.wake_pid;
    diagnostic.auto_wake_count = Some(claim.auto_wake_count);
    diagnostic
}

fn record_wake_pid_or_warn(db: &mut MailboxDb, session_id: &str, claim_token: &str, wake_pid: i64) {
    if let Err(err) = db.record_wake_claim_pid(session_id, claim_token, wake_pid) {
        tracing::warn!(session_id, claim_token, "Failed to record wake PID: {err}");
    }
}

fn spawned_wake_diagnostic(
    claim_token: String,
    wake_pid: i64,
    auto_wake_count: i64,
) -> WakeDiagnostic {
    WakeDiagnostic {
        attempted: true,
        status: "spawned".to_string(),
        claim_token: Some(claim_token),
        wake_pid: Some(wake_pid),
        auto_wake_count: Some(auto_wake_count),
        message: None,
    }
}

fn spawn_error_diagnostic(
    claim_token: String,
    auto_wake_count: i64,
    err: String,
) -> WakeDiagnostic {
    WakeDiagnostic {
        attempted: true,
        status: "spawn_error".to_string(),
        claim_token: Some(claim_token),
        wake_pid: None,
        auto_wake_count: Some(auto_wake_count),
        message: Some(err),
    }
}

fn spawn_detached_resume(
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
    configure_wake_stdio_and_env(&mut cmd, session_id, claim_token, auto_wake_count);
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
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return;
    };
    cmd.arg(flag).arg(value);
}

fn configure_wake_stdio_and_env(
    cmd: &mut Command,
    session_id: &str,
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
        .env_remove(PARENT_INVOCATION_ENV);
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
    auto_wake_marker_present()
        .then(current_auto_wake_env)
        .flatten()
}

fn auto_wake_max() -> i64 {
    validated_auto_wake_max(parsed_auto_wake_max())
}

fn current_auto_wake_env() -> Option<AutoWakeEnv> {
    Some(auto_wake_env(auto_wake_token()?, auto_wake_count()))
}

fn auto_wake_count() -> i64 {
    parse_auto_wake_count(auto_wake_count_value())
}

fn parsed_auto_wake_max() -> Option<i64> {
    parse_auto_wake_max(auto_wake_max_value())
}

fn auto_wake_token() -> Option<String> {
    std::env::var(AUTO_WAKE_TOKEN_ENV).ok()
}

fn auto_wake_count_value() -> Option<String> {
    std::env::var(AUTO_WAKE_COUNT_ENV).ok()
}

fn auto_wake_max_value() -> Option<String> {
    std::env::var(AUTO_WAKE_MAX_ENV).ok()
}

fn parse_auto_wake_count(value: Option<String>) -> i64 {
    value.and_then(|value| value.parse().ok()).unwrap_or(1)
}

fn parse_auto_wake_max(value: Option<String>) -> Option<i64> {
    value.and_then(|value| value.parse::<i64>().ok())
}

fn validated_auto_wake_max(value: Option<i64>) -> i64 {
    value
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_AUTO_WAKE_MAX)
}

fn auto_wake_env(token: String, count: i64) -> AutoWakeEnv {
    AutoWakeEnv { token, count }
}

fn release_current_auto_wake_claim(session_id: &str, auto_wake: Option<&AutoWakeEnv>) {
    let Some(auto_wake) = auto_wake else {
        return;
    };
    match MailboxDb::open_default_if_exists() {
        Ok(Some(mut db)) => release_wake_claim_or_warn(&mut db, session_id, &auto_wake.token),
        Ok(None) => {}
        Err(err) => warn_open_sidecar_for_release_failed(session_id, err),
    }
}

fn release_wake_claim_or_warn(db: &mut MailboxDb, session_id: &str, token: &str) {
    if let Err(err) = db.release_wake_claim(session_id, Some(token)) {
        warn_release_wake_claim_failed(session_id, err);
    }
}

fn warn_release_wake_claim_failed(session_id: &str, err: String) {
    tracing::warn!(session_id, "Failed to release wake claim: {err}");
}

fn warn_open_sidecar_for_release_failed(session_id: &str, err: String) {
    tracing::warn!(
        session_id,
        "Failed to open sidecar to release wake claim: {err}"
    );
}
