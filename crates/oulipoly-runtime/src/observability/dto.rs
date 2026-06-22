//! ## Declared roles
//!
//! `mapper`
//!
//! Plain monitor snapshot DTOs. These types intentionally carry no UI or TTY
//! behavior.

use serde::Serialize;
use std::time::SystemTime;

pub type MonitorNodeId = String;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MonitorSnapshot {
    pub generated_at: SystemTime,
    pub root_invocation_uuid: Option<String>,
    pub active_session_id: Option<String>,
    pub summary: MonitorSummary,
    pub nodes: Vec<MonitorNode>,
    pub diagnostics: Vec<MonitorDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MonitorSummary {
    pub status: MonitorStatus,
    pub total_nodes: usize,
    pub invocation_nodes: usize,
    pub running_nodes: usize,
    pub pending_mailbox_count: usize,
    pub running_agent_bash_count: usize,
    pub diagnostics_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MonitorNode {
    pub id: MonitorNodeId,
    pub parent_id: Option<MonitorNodeId>,
    pub kind: MonitorNodeKind,
    pub label: String,
    pub status: MonitorStatus,
    pub pid: Option<i64>,
    pub pgid: Option<i64>,
    pub liveness: LivenessStatus,
    pub started_at: Option<String>,
    pub updated_at: Option<String>,
    pub completed_at: Option<String>,
    pub last_output_excerpt: Option<String>,
    pub inspect_ref: Option<InspectRef>,
    pub cancel_ref: Option<CancelRef>,
    pub wake: Option<WakeState>,
    pub mailbox: Option<MailboxState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MonitorNodeKind {
    Session,
    Invocation,
    ProviderProcess,
    AgentBashWorkload,
    MailboxNotification,
    WakeClaim,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MonitorStatus {
    Running,
    Idle,
    Pending,
    Delivered,
    Succeeded,
    Failed,
    Error,
    Stale,
    Unknown,
    Cancelling,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum LivenessStatus {
    VerifiedLive,
    Dead,
    PidReused,
    UnverifiedLive,
    Unknown,
    NotApplicable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum InspectRef {
    AgentBashLog { path: String, max_tail_bytes: usize },
    SessionTranscript {
        path: String,
        max_tail_bytes: usize,
        format_id: Option<String>,
        source_id: Option<String>,
    },
    MailboxPayload { seq: i64 },
    WakeClaim { session_id: String },
    InvocationStatus { uuid: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum CancelRef {
    AgentBashHandle {
        handle: String,
        state_dir: String,
    },
    ProcessGroup {
        pgid: i64,
        identity: Option<MonitorProcessIdentity>,
    },
    ProviderSessionEnd {
        invocation_uuid: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MonitorProcessIdentity {
    pub os_pid: i64,
    pub os_boot_id: String,
    pub os_pid_starttime_ticks: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WakeState {
    pub session_id: String,
    pub claim_token: String,
    pub claimed_at: String,
    pub wake_pid: Option<i64>,
    pub wake_invocation_uuid: Option<String>,
    pub reason: String,
    pub auto_wake_count: i64,
    pub min_pending_seq_at_claim: Option<i64>,
    pub max_pending_seq_at_claim: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MailboxState {
    pub seq: i64,
    pub session_id: String,
    pub kind: String,
    pub handle: String,
    pub enqueued_at: String,
    pub delivered_at: Option<String>,
    pub delivered_by_invocation_uuid: Option<String>,
    pub delivery_attempts: i64,
    pub delivery_error: Option<String>,
    pub owner_invocation_uuid: Option<String>,
    pub state_dir: String,
    pub meta_path: String,
    pub log_path: String,
    pub rc_path: String,
    pub rc: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MonitorDiagnostic {
    pub code: String,
    pub severity: MonitorDiagnosticSeverity,
    pub message: String,
    pub node_id: Option<MonitorNodeId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MonitorDiagnosticSeverity {
    Info,
    Warning,
    Error,
}
