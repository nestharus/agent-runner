//! ## Declared roles
//!
//! `mapper`, `predicate`, `accessor`, `formatter`, `orchestration`
//!
//! Derive a cancel request from a monitor node and execute it safely against PID
//! reuse. Provider process groups are identity-verified before signalling.
//! Agent-bash workloads delegate cancellation to the spooler's identity-safe
//! `cancel` command. Provider-session-end remains not-yet-supported.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::observability::{CancelRef, MonitorNode, MonitorProcessIdentity};
use oulipoly_state::pid_identity::{ProcessIdentity, read_live_process_identity};
use serde::Deserialize;

/// A cancel action derived from a node's cancel reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CancelRequest {
    ProcessGroup {
        pgid: i64,
        identity: Option<RecordedIdentity>,
    },
    AgentBashHandle {
        handle: String,
        state_dir: String,
    },
    Unsupported {
        reason: &'static str,
    },
}

/// The process identity recorded when the cancel target was observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RecordedIdentity {
    os_pid: i64,
    os_boot_id: String,
    os_pid_starttime_ticks: i64,
}

/// The result of attempting a cancel, for operator feedback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CancelOutcome {
    Signalled { pgid: i64 },
    AgentBashCancelRequested { handle: String },
    AgentBashFailed { handle: String, detail: String },
    AlreadyGone,
    Unverifiable,
    IdentityMismatch,
    Unsupported { reason: &'static str },
    Failed { errno: i32 },
}

/// Classification of a recorded identity against the live process at its PID.
enum IdentityCheck {
    Verified,
    Mismatch,
    Gone,
}

/// Whether a process-group cancel should signal or just report an outcome.
enum CancelDecision {
    Signal,
    Report(CancelOutcome),
}

#[derive(Deserialize)]
struct AgentBashCancelResponse {
    handle: String,
    requested: bool,
}

/// Derive the cancel request for a node, or `None` when it exposes no cancel ref.
pub(super) fn cancel_request_for_node(node: &MonitorNode) -> Option<CancelRequest> {
    node.cancel_ref.as_ref().map(cancel_request_from_ref)
}

fn cancel_request_from_ref(cancel_ref: &CancelRef) -> CancelRequest {
    match cancel_ref {
        CancelRef::ProcessGroup { pgid, identity } => CancelRequest::ProcessGroup {
            pgid: *pgid,
            identity: identity.as_ref().map(recorded_identity),
        },
        CancelRef::AgentBashHandle { handle, state_dir } => CancelRequest::AgentBashHandle {
            handle: handle.clone(),
            state_dir: state_dir.clone(),
        },
        CancelRef::ProviderSessionEnd { .. } => CancelRequest::Unsupported {
            reason: "session end not yet supported",
        },
    }
}

fn recorded_identity(identity: &MonitorProcessIdentity) -> RecordedIdentity {
    RecordedIdentity {
        os_pid: identity.os_pid,
        os_boot_id: identity.os_boot_id.clone(),
        os_pid_starttime_ticks: identity.os_pid_starttime_ticks,
    }
}

/// Execute a cancel request. Verifies process identity before signalling so a
/// reused PID is never killed; sends `SIGTERM` to the process group.
pub(super) fn execute_cancel(request: &CancelRequest) -> CancelOutcome {
    match request {
        CancelRequest::ProcessGroup { pgid, identity } => {
            cancel_process_group(*pgid, identity.as_ref())
        }
        CancelRequest::AgentBashHandle { handle, state_dir } => {
            cancel_agent_bash_handle(handle, state_dir)
        }
        CancelRequest::Unsupported { reason } => CancelOutcome::Unsupported { reason },
    }
}

fn cancel_agent_bash_handle(handle: &str, state_dir: &str) -> CancelOutcome {
    let Some(state_home) = agent_bash_state_home(handle, state_dir) else {
        return agent_bash_failed(handle, "invalid agent-bash state directory");
    };
    execute_agent_bash_cancel(&agent_bash_binary(), handle, &state_home)
}

fn agent_bash_state_home(handle: &str, state_dir: &str) -> Option<PathBuf> {
    let state_dir = Path::new(state_dir);
    if !state_dir.is_absolute() {
        return None;
    }
    if state_dir.file_name()? != OsStr::new(handle) {
        return None;
    }
    let spool_root = state_dir.parent()?;
    if spool_root.file_name()? != OsStr::new("agent-bash") {
        return None;
    }
    spool_root.parent().map(Path::to_path_buf)
}

fn agent_bash_binary() -> OsString {
    if let Some(binary) = std::env::var_os("AGENT_BASH_BIN") {
        return binary;
    }
    let local_binary = dirs::home_dir().map(|home| home.join(".local/bin/agent-bash"));
    match local_binary {
        Some(path) if path.is_file() => path.into_os_string(),
        _ => OsString::from("agent-bash"),
    }
}

fn execute_agent_bash_cancel(binary: &OsStr, handle: &str, state_home: &Path) -> CancelOutcome {
    match Command::new(binary)
        .args(["cancel", handle])
        .env("XDG_STATE_HOME", state_home)
        .output()
    {
        Ok(output) => agent_bash_output_outcome(handle, output),
        Err(err) => agent_bash_failed(handle, format!("failed to execute agent-bash: {err}")),
    }
}

fn agent_bash_output_outcome(handle: &str, output: Output) -> CancelOutcome {
    if !output.status.success() {
        return agent_bash_failed(handle, agent_bash_failure_detail(&output));
    }
    match serde_json::from_slice::<AgentBashCancelResponse>(&output.stdout) {
        Ok(response) if response.handle == handle && response.requested => {
            CancelOutcome::AgentBashCancelRequested {
                handle: handle.to_string(),
            }
        }
        Ok(response) if response.handle == handle => CancelOutcome::AlreadyGone,
        Ok(_) => agent_bash_failed(handle, "agent-bash returned a different handle"),
        Err(err) => agent_bash_failed(handle, format!("invalid agent-bash response: {err}")),
    }
}

fn agent_bash_failure_detail(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        format!("agent-bash exited with {}", output.status)
    } else {
        stderr
    }
}

fn agent_bash_failed(handle: &str, detail: impl Into<String>) -> CancelOutcome {
    CancelOutcome::AgentBashFailed {
        handle: handle.to_string(),
        detail: detail.into(),
    }
}

fn cancel_process_group(pgid: i64, identity: Option<&RecordedIdentity>) -> CancelOutcome {
    match cancel_decision(identity) {
        CancelDecision::Signal => signal_process_group(pgid),
        CancelDecision::Report(outcome) => outcome,
    }
}

fn cancel_decision(identity: Option<&RecordedIdentity>) -> CancelDecision {
    match identity {
        None => CancelDecision::Report(CancelOutcome::Unverifiable),
        Some(identity) => decision_from_check(identity_check(identity)),
    }
}

fn decision_from_check(check: IdentityCheck) -> CancelDecision {
    match check {
        IdentityCheck::Verified => CancelDecision::Signal,
        IdentityCheck::Mismatch => CancelDecision::Report(CancelOutcome::IdentityMismatch),
        IdentityCheck::Gone => CancelDecision::Report(CancelOutcome::AlreadyGone),
    }
}

fn identity_check(identity: &RecordedIdentity) -> IdentityCheck {
    classify_identity(identity, live_identity(identity.os_pid))
}

fn live_identity(os_pid: i64) -> Option<ProcessIdentity> {
    read_live_process_identity(os_pid).ok().flatten()
}

fn classify_identity(recorded: &RecordedIdentity, live: Option<ProcessIdentity>) -> IdentityCheck {
    match live {
        Some(live) if identity_matches(recorded, &live) => IdentityCheck::Verified,
        Some(_) => IdentityCheck::Mismatch,
        None => IdentityCheck::Gone,
    }
}

fn identity_matches(recorded: &RecordedIdentity, live: &ProcessIdentity) -> bool {
    recorded.os_boot_id == live.os_boot_id
        && recorded.os_pid_starttime_ticks == live.os_pid_starttime_ticks
}

fn signal_process_group(pgid: i64) -> CancelOutcome {
    let rc = send_sigterm_to_process_group(pgid);
    if signal_succeeded(rc) {
        signalled_process_group_outcome(pgid)
    } else {
        failed_signal_outcome(last_signal_errno())
    }
}

fn send_sigterm_to_process_group(pgid: i64) -> i32 {
    unsafe { libc::killpg(pgid as libc::pid_t, libc::SIGTERM) }
}

fn signal_succeeded(rc: i32) -> bool {
    rc == 0
}

fn signalled_process_group_outcome(pgid: i64) -> CancelOutcome {
    CancelOutcome::Signalled { pgid }
}

fn last_signal_errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

fn failed_signal_outcome(errno: i32) -> CancelOutcome {
    CancelOutcome::Failed { errno }
}

/// A short operator-facing message describing a cancel outcome.
pub(super) fn cancel_outcome_message(outcome: &CancelOutcome) -> String {
    match outcome {
        CancelOutcome::Signalled { pgid } => format!("sent SIGTERM to process group {pgid}"),
        CancelOutcome::AgentBashCancelRequested { handle } => {
            format!("requested cancellation for agent-bash handle {handle}")
        }
        CancelOutcome::AgentBashFailed { handle, detail } => {
            format!("agent-bash cancellation failed for {handle}: {detail}")
        }
        CancelOutcome::AlreadyGone => "process already exited".to_string(),
        CancelOutcome::Unverifiable => "skipped: no recorded identity to verify".to_string(),
        CancelOutcome::IdentityMismatch => "skipped: PID was reused by another process".to_string(),
        CancelOutcome::Unsupported { reason } => format!("not supported: {reason}"),
        CancelOutcome::Failed { errno } => format!("signal failed (errno {errno})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observability::{
        CancelRef, LivenessStatus, MonitorNode, MonitorNodeKind, MonitorProcessIdentity,
        MonitorStatus,
    };
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn node_with_cancel_ref(cancel_ref: Option<CancelRef>) -> MonitorNode {
        MonitorNode {
            id: "n".to_string(),
            parent_id: None,
            kind: MonitorNodeKind::ProviderProcess,
            label: "p".to_string(),
            status: MonitorStatus::Running,
            pid: Some(10),
            pgid: Some(10),
            liveness: LivenessStatus::VerifiedLive,
            started_at: None,
            updated_at: None,
            completed_at: None,
            last_output_excerpt: None,
            inspect_ref: None,
            cancel_ref,
            wake: None,
            mailbox: None,
        }
    }

    fn recorded(os_pid: i64) -> RecordedIdentity {
        RecordedIdentity {
            os_pid,
            os_boot_id: "boot".to_string(),
            os_pid_starttime_ticks: 7,
        }
    }

    fn live(os_pid: i64, boot: &str, ticks: i64) -> ProcessIdentity {
        ProcessIdentity {
            os_pid,
            os_boot_id: boot.to_string(),
            os_pid_starttime_ticks: ticks,
        }
    }

    #[test]
    fn node_without_cancel_ref_has_no_request() {
        assert_eq!(cancel_request_for_node(&node_with_cancel_ref(None)), None);
    }

    #[test]
    fn process_group_ref_maps_to_process_group_request() {
        let node = node_with_cancel_ref(Some(CancelRef::ProcessGroup {
            pgid: 42,
            identity: Some(MonitorProcessIdentity {
                os_pid: 10,
                os_boot_id: "boot".to_string(),
                os_pid_starttime_ticks: 7,
            }),
        }));
        assert_eq!(
            cancel_request_for_node(&node),
            Some(CancelRequest::ProcessGroup {
                pgid: 42,
                identity: Some(recorded(10)),
            })
        );
    }

    #[test]
    fn agent_bash_ref_maps_to_handle_request() {
        let bash = node_with_cancel_ref(Some(CancelRef::AgentBashHandle {
            handle: "h".to_string(),
            state_dir: "/tmp/state/agent-bash/h".to_string(),
        }));
        assert_eq!(
            cancel_request_for_node(&bash),
            Some(CancelRequest::AgentBashHandle {
                handle: "h".to_string(),
                state_dir: "/tmp/state/agent-bash/h".to_string(),
            })
        );
    }

    #[test]
    fn session_end_is_unsupported() {
        let session = node_with_cancel_ref(Some(CancelRef::ProviderSessionEnd {
            invocation_uuid: "u".to_string(),
        }));
        assert!(matches!(
            cancel_request_for_node(&session),
            Some(CancelRequest::Unsupported { .. })
        ));
    }

    #[test]
    fn identity_classification_detects_match_mismatch_and_gone() {
        let recorded = recorded(10);
        assert!(matches!(
            classify_identity(&recorded, Some(live(10, "boot", 7))),
            IdentityCheck::Verified
        ));
        assert!(matches!(
            classify_identity(&recorded, Some(live(10, "boot", 999))),
            IdentityCheck::Mismatch
        ));
        assert!(matches!(
            classify_identity(&recorded, Some(live(10, "other-boot", 7))),
            IdentityCheck::Mismatch
        ));
        assert!(matches!(
            classify_identity(&recorded, None),
            IdentityCheck::Gone
        ));
    }

    #[test]
    fn missing_identity_refuses_to_signal() {
        assert_eq!(cancel_process_group(42, None), CancelOutcome::Unverifiable);
    }

    #[test]
    fn unsupported_request_executes_without_signalling() {
        assert_eq!(
            execute_cancel(&CancelRequest::Unsupported { reason: "x" }),
            CancelOutcome::Unsupported { reason: "x" }
        );
    }

    #[test]
    fn outcome_messages_are_human_readable() {
        assert_eq!(
            cancel_outcome_message(&CancelOutcome::Signalled { pgid: 5 }),
            "sent SIGTERM to process group 5"
        );
        assert_eq!(
            cancel_outcome_message(&CancelOutcome::IdentityMismatch),
            "skipped: PID was reused by another process"
        );
        assert_eq!(
            cancel_outcome_message(&CancelOutcome::AgentBashCancelRequested {
                handle: "h".to_string(),
            }),
            "requested cancellation for agent-bash handle h"
        );
    }

    #[test]
    fn agent_bash_state_home_requires_matching_spool_shape() {
        assert_eq!(
            agent_bash_state_home("h", "/tmp/state/agent-bash/h"),
            Some(PathBuf::from("/tmp/state"))
        );
        assert_eq!(
            agent_bash_state_home("other", "/tmp/state/agent-bash/h"),
            None
        );
        assert_eq!(
            agent_bash_state_home("h", "/tmp/state/not-agent-bash/h"),
            None
        );
        assert_eq!(agent_bash_state_home("h", "state/agent-bash/h"), None);
    }

    #[test]
    fn agent_bash_cancel_command_receives_handle_and_observed_state_home() {
        let temp = tempfile::tempdir().expect("tempdir");
        let binary = temp.path().join("agent-bash");
        let state_home = temp.path().join("state-home");
        let script = format!(
            "#!/bin/sh\n[ \"$1\" = cancel ] || exit 2\n[ \"$2\" = h ] || exit 3\n[ \"$XDG_STATE_HOME\" = \"{}\" ] || exit 4\nprintf '%s\\n' '{{\"handle\":\"h\",\"requested\":true}}'\n",
            state_home.display()
        );
        fs::write(&binary, script).expect("fake agent-bash");
        let mut permissions = fs::metadata(&binary).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&binary, permissions).expect("chmod");

        assert_eq!(
            execute_agent_bash_cancel(binary.as_os_str(), "h", &state_home),
            CancelOutcome::AgentBashCancelRequested {
                handle: "h".to_string(),
            }
        );
    }

    #[test]
    fn agent_bash_cancel_response_reports_already_gone() {
        let output = Command::new("printf")
            .args(["%s\\n", "{\"handle\":\"h\",\"requested\":false}"])
            .output()
            .expect("printf");
        assert_eq!(
            agent_bash_output_outcome("h", output),
            CancelOutcome::AlreadyGone
        );
    }
}
