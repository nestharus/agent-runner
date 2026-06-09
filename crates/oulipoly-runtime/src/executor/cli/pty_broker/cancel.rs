//! ## Declared roles
//!
//! `mapper`, `predicate`, `accessor`, `formatter`, `orchestration`
//!
//! Derive a cancel request from a monitor node and execute it safely against PID
//! reuse. Only provider process groups are cancellable today: the recorded
//! identity (boot id + start-time ticks) is re-verified against the live process
//! before any signal, so a reused PID is never killed. Agent-bash workloads and
//! provider-session-end are surfaced as not-yet-supported.

use crate::observability::{CancelRef, MonitorNode, MonitorProcessIdentity};
use oulipoly_state::pid_identity::{ProcessIdentity, read_live_process_identity};

/// A cancel action derived from a node's cancel reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CancelRequest {
    ProcessGroup {
        pgid: i64,
        identity: Option<RecordedIdentity>,
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
        CancelRef::AgentBashHandle { .. } => CancelRequest::Unsupported {
            reason: "agent-bash cancel not yet supported",
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
        CancelRequest::Unsupported { reason } => CancelOutcome::Unsupported { reason },
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
    let rc = unsafe { libc::killpg(pgid as libc::pid_t, libc::SIGTERM) };
    if rc == 0 {
        CancelOutcome::Signalled { pgid }
    } else {
        CancelOutcome::Failed {
            errno: std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
        }
    }
}

/// A short operator-facing message describing a cancel outcome.
pub(super) fn cancel_outcome_message(outcome: &CancelOutcome) -> String {
    match outcome {
        CancelOutcome::Signalled { pgid } => format!("sent SIGTERM to process group {pgid}"),
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
    fn agent_bash_and_session_end_are_unsupported() {
        let bash = node_with_cancel_ref(Some(CancelRef::AgentBashHandle {
            handle: "h".to_string(),
            state_dir: "d".to_string(),
        }));
        assert!(matches!(
            cancel_request_for_node(&bash),
            Some(CancelRequest::Unsupported { .. })
        ));
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
    }
}
