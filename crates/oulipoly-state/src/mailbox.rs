//! ## Declared roles
//!
//! `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `parser`,
//! `predicate`, `validator`
//!
//! PID-sidecar facade for mailbox delivery and cross-entity transactions.
//! Runtime lifecycle, wake/session state, payload retention, completion authority,
//! namespace authority, and schema evolution expose separate capability owners
//! while retaining one physical SQLite transaction boundary.

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration as StdDuration, Instant};
use uuid::Uuid;

use crate::pid_identity::{self, ProcessIdentity};

#[path = "mailbox/schema.rs"]
mod schema;

pub const AGENT_BASH_COMPLETE_KIND: &str = "agent_bash_complete";
pub const MAILBOX_DELIVERY_UNCONFIRMED_ERROR: &str = "mailbox_delivery_unconfirmed";
pub const MAILBOX_INGRESS_EXPIRED_ERROR: &str = "mailbox_ingress_expired";
pub const MAILBOX_PAYLOAD_VERIFICATION_FAILED_ERROR: &str = "mailbox_payload_verification_failed";
pub const SUBMITTED_INPUT_KIND: &str = "input";
pub const WAKE_SWEEP_ABANDONED_ERROR: &str = "wake_sweep_abandoned";
pub const MAILBOX_PAYLOAD_RETENTION_POLICY: &str = "until_terminal_disposition";
pub const TERMINAL_HISTORY_KEEP_ROWS: usize = 1_024;
const TERMINAL_HISTORY_MAINTENANCE_BATCH: usize = 256;
const TERMINAL_HISTORY_MAINTENANCE_PROGRESS_OPS: i32 = 1_000;
const TERMINAL_HISTORY_MAINTENANCE_TIMEOUT: StdDuration = StdDuration::from_millis(100);
const TERMINAL_HISTORY_MAINTENANCE_BUSY_TIMEOUT: StdDuration = StdDuration::from_millis(50);
const COMPACTED_PAYLOAD_SCHEMA_VERSION: u8 = 1;
// Agent-bash registration must not inherit test-support's shortened generic writer wait.
const COMPLETION_AUTHORITY_SQLITE_TIMEOUT: StdDuration = StdDuration::from_secs(5);
const MAILBOX_ROW_COLUMNS: &str = "seq, session_id, kind, handle, payload_json, enqueued_at,
    delivered_at, delivered_by_invocation_uuid, delivery_attempts,
    delivery_error, owner_invocation_uuid, matched_os_pid,
    matched_os_boot_id, matched_os_pid_starttime_ticks,
    matched_chain_index, state_dir, meta_path, log_path, rc_path, rc,
    payload_file_path, payload_sha256, payload_byte_len,
    payload_retention_policy, payload_compacted_at,
    submission_token, target_kind, target_id";
// Reserves ?1 for the session ID and ?2 for the optional chain ID;
// embedding queries must number additional parameters from ?3.
const PENDING_MAILBOX_TARGET_PREDICATE: &str = "(
    (target_kind IS NULL AND session_id = ?1)
    OR (target_kind = 'session' AND target_id = ?1)
    OR (?2 IS NOT NULL AND target_kind = 'chain' AND target_id = ?2)
)";

fn bounded_pending_mailbox_query() -> String {
    format!(
        "SELECT {MAILBOX_ROW_COLUMNS}
         FROM mailbox
         WHERE delivered_at IS NULL
           AND seq > ?3
           AND (delivery_error IS NULL OR delivery_error != ?5)
           AND (delivery_error IS NULL OR delivery_error != ?6)
           AND (delivery_error IS NULL OR delivery_error != ?7)
           AND {PENDING_MAILBOX_TARGET_PREDICATE}
         ORDER BY seq ASC
         LIMIT ?4"
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct RuntimeGenerationId(Uuid);

impl RuntimeGenerationId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn parse(value: &str) -> Result<Self, GenerationStorageError> {
        Uuid::parse_str(value).map(Self).map_err(|err| {
            GenerationStorageError::new(format!("Invalid runtime generation UUID: {err}"))
        })
    }
}

impl Default for RuntimeGenerationId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for RuntimeGenerationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct DeliveryClaimId(Uuid);

impl DeliveryClaimId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn parse(value: &str) -> Result<Self, GenerationStorageError> {
        Uuid::parse_str(value).map(Self).map_err(|err| {
            GenerationStorageError::new(format!("Invalid delivery claim UUID: {err}"))
        })
    }
}

impl Default for DeliveryClaimId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for DeliveryClaimId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct DrainRequestId(Uuid);

impl DrainRequestId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn parse(value: &str) -> Result<Self, GenerationStorageError> {
        Uuid::parse_str(value).map(Self).map_err(|err| {
            GenerationStorageError::new(format!("Invalid drain request UUID: {err}"))
        })
    }
}

impl Default for DrainRequestId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for DrainRequestId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeLifecycleState {
    Starting,
    Running,
    Draining,
    Exited,
}

impl RuntimeLifecycleState {
    fn parse(value: &str) -> Result<Self, GenerationStorageError> {
        match value {
            "starting" => Ok(Self::Starting),
            "running" => Ok(Self::Running),
            "draining" => Ok(Self::Draining),
            "exited" => Ok(Self::Exited),
            other => Err(GenerationStorageError::new(format!(
                "Invalid runtime generation lifecycle state: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeTerminalReason {
    StartupFailed,
    OrderlyCompletion,
    AbnormalTermination,
    Cancelled,
    RecoveredDead,
}

impl RuntimeTerminalReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::StartupFailed => "startup_failed",
            Self::OrderlyCompletion => "orderly_completion",
            Self::AbnormalTermination => "abnormal_termination",
            Self::Cancelled => "cancelled",
            Self::RecoveredDead => "recovered_dead",
        }
    }

    fn parse(value: &str) -> Result<Self, GenerationStorageError> {
        match value {
            "startup_failed" => Ok(Self::StartupFailed),
            "orderly_completion" => Ok(Self::OrderlyCompletion),
            "abnormal_termination" => Ok(Self::AbnormalTermination),
            "cancelled" => Ok(Self::Cancelled),
            "recovered_dead" => Ok(Self::RecoveredDead),
            other => Err(GenerationStorageError::new(format!(
                "Invalid runtime generation terminal reason: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum ExactProcessEvidence {
    Recorded(ProcessIdentity),
    NotRecorded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuntimeGenerationRow {
    pub generation_id: RuntimeGenerationId,
    pub lifecycle_state: RuntimeLifecycleState,
    pub spawn_invocation_uuid: String,
    pub session_id: Option<String>,
    pub runtime_mode: String,
    pub provider_name: String,
    pub model_name: Option<String>,
    pub pty_control_path: Option<String>,
    pub models_dir: Option<String>,
    pub effective_cwd: Option<String>,
    pub spawned_os_pid: Option<i64>,
    pub exact_process_evidence: ExactProcessEvidence,
    pub creator_process_evidence: ExactProcessEvidence,
    pub created_at: String,
    pub running_at: Option<String>,
    pub draining_at: Option<String>,
    pub exited_at: Option<String>,
    pub terminal_reason: Option<RuntimeTerminalReason>,
    pub exit_code: Option<i32>,
    pub drain_request_id: Option<DrainRequestId>,
    pub drain_requested_at: Option<String>,
    pub drain_requested_by_invocation_uuid: Option<String>,
    pub active_delivery_claim_id: Option<DeliveryClaimId>,
    pub active_delivery_claimed_at: Option<String>,
    pub active_delivery_seqs: Vec<i64>,
}

#[derive(Debug, Clone, Copy)]
pub struct RuntimeGenerationFence<'a> {
    pub generation_id: &'a RuntimeGenerationId,
    pub spawn_invocation_uuid: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub struct CreateRuntimeGeneration<'a> {
    pub generation_id: &'a RuntimeGenerationId,
    pub spawn_invocation_uuid: &'a str,
    pub session_id: Option<&'a str>,
    pub runtime_mode: &'a str,
    pub provider_name: &'a str,
    pub model_name: Option<&'a str>,
    pub pty_control_path: Option<&'a str>,
    pub models_dir: Option<&'a str>,
    pub effective_cwd: Option<&'a str>,
}

#[derive(Debug, Clone, Copy)]
pub struct BindRuntimeGenerationRunning<'a> {
    pub fence: RuntimeGenerationFence<'a>,
    pub spawned_os_pid: i64,
    pub exact_process_identity: &'a ProcessIdentity,
    pub os_pgid: Option<i64>,
}

#[derive(Debug, Clone, Copy)]
pub struct AttachRuntimeGenerationSession<'a> {
    pub fence: RuntimeGenerationFence<'a>,
    pub session_id: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub enum RuntimeGenerationSelector<'a> {
    Exact(RuntimeGenerationFence<'a>),
    ProcessIdentity(&'a ProcessIdentity),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeGenerationResolution {
    NotFound,
    Found(Box<RuntimeGenerationRow>),
    Ambiguous(Vec<RuntimeGenerationRow>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionGenerationProjection {
    None,
    One(Box<RuntimeGenerationRow>),
    Multiple(Vec<RuntimeGenerationRow>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalCompatibilityReconciliation {
    Reconciled,
    NoGeneration,
}

#[derive(Debug, Clone, Copy)]
pub struct ExitRuntimeGenerationNonOrderly<'a> {
    pub fence: RuntimeGenerationFence<'a>,
    pub reason: RuntimeTerminalReason,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenerationMutation<T> {
    Applied(T),
    AlreadyApplied(T),
    Rejected(GenerationRejection),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenerationRejection {
    NotFound,
    FenceMismatch,
    SessionConflict,
    ProcessIdentityConflict,
    DrainRequestConflict,
    IllegalPredecessor {
        expected: RuntimeLifecycleState,
        actual: RuntimeLifecycleState,
    },
    InvariantViolation,
}

#[derive(Debug, Clone, Copy)]
pub struct AcquireRuntimeGenerationDelivery<'a> {
    pub fence: RuntimeGenerationFence<'a>,
    pub claim_id: &'a DeliveryClaimId,
    pub seqs: &'a [i64],
    pub stale_after_seconds: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryClaimAcquireResult {
    Acquired(RuntimeGenerationRow),
    Recovered(RuntimeGenerationRow),
    AlreadyInFlight(RuntimeGenerationRow),
    Rejected(GenerationRejection),
}

#[derive(Debug, Clone, Copy)]
pub struct ConfirmRuntimeGenerationDelivery<'a> {
    pub fence: RuntimeGenerationFence<'a>,
    pub claim_id: &'a DeliveryClaimId,
    pub seqs: &'a [i64],
    pub delivered_by_invocation_uuid: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub struct FailRuntimeGenerationDelivery<'a> {
    pub fence: RuntimeGenerationFence<'a>,
    pub claim_id: &'a DeliveryClaimId,
    pub seqs: &'a [i64],
    pub delivery_error: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationStorageError {
    message: String,
}

impl GenerationStorageError {
    fn new(message: String) -> Self {
        Self { message }
    }
}

impl fmt::Display for GenerationStorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for GenerationStorageError {}

#[derive(Debug, Clone, Copy)]
pub struct RequestRuntimeGenerationDrain<'a> {
    pub fence: RuntimeGenerationFence<'a>,
    pub drain_request_id: &'a DrainRequestId,
    pub requested_by_invocation_uuid: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrainHandoff {
    Ready,
    ClaimOutstanding {
        generation_id: RuntimeGenerationId,
        claim_id: DeliveryClaimId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrainRequestResult {
    Installed(RuntimeGenerationRow, DrainHandoff),
    AlreadyInstalled(RuntimeGenerationRow, DrainHandoff),
    Rejected(GenerationRejection),
}

#[derive(Debug, Clone, Copy)]
pub struct AdvanceRuntimeGenerationDrain<'a> {
    pub fence: RuntimeGenerationFence<'a>,
    pub drain_request_id: &'a DrainRequestId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrainAdvanceResult {
    Advanced(RuntimeGenerationRow),
    AlreadyDraining(RuntimeGenerationRow),
    AlreadyExited(RuntimeGenerationRow),
    WaitingOnClaim(DeliveryClaimId),
    Rejected(GenerationRejection),
}

#[derive(Debug, Clone, Copy)]
pub struct FinishRuntimeGenerationDrain<'a> {
    pub fence: RuntimeGenerationFence<'a>,
    pub drain_request_id: &'a DrainRequestId,
    pub exit_code: Option<i32>,
    pub compatibility_exit_code: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrainFinishResult {
    Finished(RuntimeGenerationRow),
    AlreadyExited(RuntimeGenerationRow),
    NotDraining(RuntimeLifecycleState),
    Rejected(GenerationRejection),
}

const MAILBOX_PAYLOAD_DIRECTORY: &str = "inbox-payloads";
const MAILBOX_PAYLOAD_ADDRESS_VERSION: &str = "v1";
const MAILBOX_PAYLOAD_ALGORITHM: &str = "sha256";
const INPUT_IDENTITY_DOMAIN: &str = "oulipoly.inbox.input";
const INPUT_IDENTITY_VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxTargetKind {
    Session,
    Chain,
}

impl InboxTargetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Chain => "chain",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InboxTarget<'a> {
    pub kind: InboxTargetKind,
    pub id: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub struct SubmittedInputEnqueue<'a> {
    pub submission_token: &'a str,
    pub target: InboxTarget<'a>,
    pub input: &'a [u8],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublishedMailboxPayload {
    pub address: String,
    pub file_path: PathBuf,
    pub sha256: String,
    pub byte_len: u64,
    pub retention_policy: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct DeliveredPayloadCompactionStats {
    pub eligible_rows: usize,
    pub inline_bytes: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct DeliveredPayloadCompactionReport {
    pub scanned_rows: usize,
    pub compacted_rows: usize,
    pub retained_payload_bytes: u64,
    pub inline_bytes_reclaimed: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct TerminalHistoryRetentionStats {
    pub terminal_mailbox_rows: usize,
    pub prunable_mailbox_rows: usize,
    pub resolved_delivery_attempts: usize,
    pub prunable_delivery_attempts: usize,
    pub reclaimable_payload_files: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct TerminalHistoryPruneReport {
    pub mailbox_rows_deleted: usize,
    pub listeners_detached: usize,
    pub delivery_attempts_deleted: usize,
    pub delivery_attempt_items_deleted: usize,
    pub payload_files_deleted: usize,
    pub payload_bytes_reclaimed: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailboxRow {
    pub seq: i64,
    pub session_id: String,
    pub kind: String,
    pub handle: String,
    pub payload_json: String,
    pub enqueued_at: String,
    pub delivered_at: Option<String>,
    pub delivered_by_invocation_uuid: Option<String>,
    pub delivery_attempts: i64,
    pub delivery_error: Option<String>,
    pub owner_invocation_uuid: Option<String>,
    pub matched_os_pid: Option<i64>,
    pub matched_os_boot_id: Option<String>,
    pub matched_os_pid_starttime_ticks: Option<i64>,
    pub matched_chain_index: Option<i64>,
    pub state_dir: String,
    pub meta_path: String,
    pub log_path: String,
    pub rc_path: String,
    pub rc: i32,
    pub payload_file_path: Option<String>,
    pub payload_sha256: Option<String>,
    pub payload_byte_len: Option<i64>,
    pub payload_retention_policy: Option<String>,
    pub payload_compacted_at: Option<String>,
    pub submission_token: Option<String>,
    pub target_kind: Option<String>,
    pub target_id: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct AgentBashCompleteEnqueue<'a> {
    pub session_id: &'a str,
    pub handle: &'a str,
    pub payload_json: &'a str,
    pub owner_invocation_uuid: Option<&'a str>,
    pub matched_os_pid: Option<i64>,
    pub matched_os_boot_id: Option<&'a str>,
    pub matched_os_pid_starttime_ticks: Option<i64>,
    pub matched_chain_index: Option<i64>,
    pub state_dir: &'a str,
    pub meta_path: &'a str,
    pub log_path: &'a str,
    pub rc_path: &'a str,
    pub rc: i32,
}

#[derive(Debug, Clone, Copy)]
pub struct CompletionEventRegistrationInput<'a> {
    pub event_id: &'a str,
    pub delivery_mode: &'a str,
    pub owner_session_id: Option<&'a str>,
    pub owner_invocation_uuid: Option<&'a str>,
    pub state_dir: &'a str,
    pub meta_path: &'a str,
    pub log_path: &'a str,
    pub rc_path: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub struct CompletionEventTriggerInput<'a> {
    pub event_id: &'a str,
    pub payload_json: &'a str,
    pub state_dir: &'a str,
    pub meta_path: &'a str,
    pub log_path: &'a str,
    pub rc_path: &'a str,
    pub rc: i32,
    pub consumed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompletionEventRow {
    pub event_id: String,
    pub kind: String,
    pub state: String,
    pub delivery_mode: String,
    pub state_dir: String,
    pub meta_path: String,
    pub log_path: String,
    pub rc_path: String,
    pub rc: Option<i32>,
    pub payload_json: Option<String>,
    pub payload_file_path: Option<String>,
    pub payload_sha256: Option<String>,
    pub payload_byte_len: Option<i64>,
    pub payload_retention_policy: Option<String>,
    pub created_at: String,
    pub triggered_at: Option<String>,
    pub payload_reclaimed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompletionEventListenerRow {
    pub event_id: String,
    pub listener_id: String,
    pub session_id: String,
    pub owner_invocation_uuid: String,
    pub active: bool,
    pub mailbox_seq: Option<i64>,
    pub acknowledged_at: Option<String>,
    pub acknowledgement_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompletionEventRegistrationResult {
    pub inserted: bool,
    pub event: CompletionEventRow,
    pub listeners: Vec<CompletionEventListenerRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompletionEventTriggerResult {
    pub triggered: bool,
    pub event: CompletionEventRow,
    pub listeners: Vec<CompletionEventListenerRow>,
    pub mailbox_rows: Vec<MailboxRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnqueueResult {
    Inserted(MailboxRow),
    AlreadyEnqueued(MailboxRow),
    Conflict { existing: MailboxRow },
}

#[derive(Debug, Clone, Copy)]
pub struct SessionMetadataUpsert<'a> {
    pub session_id: &'a str,
    pub mode: &'a str,
    pub invocation_uuid: Option<&'a str>,
    pub provider_name: Option<&'a str>,
    pub model_name: Option<&'a str>,
    pub models_dir: Option<&'a str>,
    pub effective_cwd: Option<&'a str>,
}

#[derive(Debug, Clone, Copy)]
pub struct LegacyRuntimeProjection<'a> {
    pub session_id: &'a str,
    pub mode: &'a str,
    pub invocation_uuid: &'a str,
    pub provider_name: Option<&'a str>,
    pub model_name: Option<&'a str>,
    pub identity: &'a ProcessIdentity,
    pub pty_control_path: Option<&'a str>,
    pub turn_start_max_mailbox_seq: Option<i64>,
    pub models_dir: Option<&'a str>,
    pub effective_cwd: Option<&'a str>,
}

#[derive(Debug, Clone, Copy)]
pub struct LegacyRuntimeProjectionSettlement<'a> {
    pub session_id: &'a str,
    pub invocation_uuid: &'a str,
    pub last_exit_code: Option<i32>,
}

/// Resume and wake-policy metadata. This row intentionally exposes no live
/// process or lifecycle state; `runtime_generation` is the sole live authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMetadataRow {
    pub session_id: String,
    pub mode: String,
    pub invocation_uuid: Option<String>,
    pub provider_name: Option<String>,
    pub model_name: Option<String>,
    pub updated_at: String,
    pub models_dir: Option<String>,
    pub effective_cwd: Option<String>,
    pub auto_wake_count: i64,
}

/// Compatibility columns retained in the installed `session_runtime` table.
/// Generation lifecycle transactions maintain this projection atomically; no
/// production liveness or wake decision treats it as runtime authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyRuntimeProjectionRow {
    pub session_id: String,
    pub pty_control_path: Option<String>,
    pub updated_at: String,
    pub run_state: String,
    pub running_invocation_uuid: Option<String>,
    pub running_os_pid: Option<i64>,
    pub running_os_boot_id: Option<String>,
    pub running_os_pid_starttime_ticks: Option<i64>,
    pub turn_started_at: Option<String>,
    pub turn_ended_at: Option<String>,
    pub turn_start_max_mailbox_seq: Option<i64>,
    pub last_exit_code: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionLiveness {
    Busy,
    Idle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeGenerationReadOnlyLiveness {
    Busy,
    Idle,
    StaleMissingInvocation,
    StaleMissingIdentity,
    StaleDead,
    StalePidReused,
}

#[derive(Debug, Clone, Copy)]
pub struct WakeClaimRequest<'a> {
    pub session_id: &'a str,
    pub claim_token: &'a str,
    pub reason: &'a str,
    pub auto_wake_count: i64,
    pub wake_invocation_uuid: Option<&'a str>,
    pub stale_after_seconds: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeClaimAcquireResult {
    Acquired(WakeClaimRow),
    NoPending,
    Busy,
    AlreadyInFlight(WakeClaimRow),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeClaimRow {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeSweepCandidate {
    pub session_id: String,
    pub auto_wake_count: i64,
    pub min_pending_seq: i64,
    pub max_pending_seq: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionAdmissionRow {
    pub queue_sequence: i64,
    pub admission_id: String,
    pub registration_identity: String,
    pub session_id: Option<String>,
    pub state: String,
    pub queue_reason: String,
    pub claim_token: Option<String>,
    pub claimed_at_unix_ms: Option<i64>,
    pub runtime_generation_uuid: Option<String>,
    pub launcher_os_pid: i64,
    pub launcher_os_boot_id: String,
    pub launcher_os_pid_starttime_ticks: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionAdmissionAttempt {
    Admitted(Box<SessionAdmissionRow>),
    LaunchMaterializing,
    Waiting,
    Empty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailboxDeliveryAttemptDisposition {
    Pending,
    Resolved,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxDeliveryWindow {
    pub attempt_id: String,
    pub session_id: String,
    pub delivery_invocation_uuid: String,
    pub acknowledged_at: Option<String>,
    pub submission_started_at: Option<String>,
    pub resolved_at: Option<String>,
    pub rows: Vec<MailboxRow>,
    pub remaining_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxDeliveryObservationAnchor {
    pub provider_name: String,
    pub provider_instance_id: String,
    pub settings_id: String,
    pub provider_session_id: String,
    pub resume_token: String,
    pub expected_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingMailboxDeliveryObservation {
    pub attempt_id: String,
    pub anchor: MailboxDeliveryObservationAnchor,
}

fn validate_delivery_observation_anchor(
    session_id: &str,
    anchor: &MailboxDeliveryObservationAnchor,
) -> Result<(), String> {
    for (name, value) in [
        ("provider name", anchor.provider_name.as_str()),
        ("provider instance id", anchor.provider_instance_id.as_str()),
        ("settings id", anchor.settings_id.as_str()),
        ("provider session id", anchor.provider_session_id.as_str()),
    ] {
        if value.is_empty() || value.len() > 1024 {
            return Err(format!("invalid mailbox delivery observation {name}"));
        }
    }
    if anchor.provider_session_id != session_id {
        return Err("mailbox delivery observation session identity mismatch".to_string());
    }
    if anchor.resume_token.is_empty() || anchor.resume_token.len() > 4096 {
        return Err("invalid mailbox delivery observation anchor token".to_string());
    }
    if anchor.expected_sha256.len() != 64
        || !anchor
            .expected_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err("invalid mailbox delivery observation expected digest".to_string());
    }
    Ok(())
}

fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxDeliveryEvidenceObligation {
    pub attempt_id: String,
    pub session_id: String,
    pub turn_generation_id: String,
    pub observed_at: i64,
    pub legacy: bool,
}

struct WakeSweepSessionState {
    session_id: String,
    min_pending_seq: i64,
    max_pending_seq: i64,
    claim: Option<WakeClaimRow>,
}

/// Typed mailbox operations intentionally retain the writable connection and
/// do not expose that connection through this API. This Rust capability
/// boundary does not claim protection from a process with arbitrary local
/// filesystem/SQLite write authority; such a process is trusted terminal
/// storage authority.
///
/// ```compile_fail
/// use oulipoly_state::mailbox::MailboxDb;
///
/// fn raw_write_capability(mailbox: &MailboxDb) {
///     let _ = mailbox.connection();
/// }
/// ```
pub struct MailboxDb {
    conn: Connection,
    path: PathBuf,
    _read_only_snapshot: Option<crate::read_only_snapshot::ReadOnlySnapshot>,
    _namespace_authority: Option<MailboxAuthorityFence>,
}

/// Exclusive writer for durable runtime-generation lifecycle transitions.
pub struct RuntimeLifecycleRepository<'a> {
    conn: &'a mut Connection,
}

/// Read-only runtime-generation projection and liveness surface.
pub struct RuntimeLifecycleReader<'a> {
    conn: &'a Connection,
}

/// Content-addressed payload publication, verification, hydration, and compaction.
pub struct PayloadRetentionRepository<'a> {
    conn: &'a Connection,
    path: &'a Path,
}

/// Session metadata plus atomic wake-claim admission and lifecycle authority.
pub struct WakeSessionRepository<'a> {
    conn: &'a mut Connection,
}

/// Durable FIFO admission and in-flight launch reservation authority.
pub struct SessionAdmissionRepository<'a> {
    conn: &'a mut Connection,
}

/// Read-only session metadata, compatibility projection, and wake-claim surface.
pub struct WakeSessionReader<'a> {
    conn: &'a Connection,
}

/// Serializes cooperating canonical-path creation and replacement independently
/// of SQLite's inode-scoped writer locks.
pub(crate) struct MailboxAuthorityFence {
    file: std::fs::File,
    sidecar_path: PathBuf,
    target_identity: Option<MailboxFileIdentity>,
}

/// Exclusive sidecar namespace and SQLite-writer authority for the supported
/// State-plus-sidecar destructive rebuild protocol.
///
/// The sidecar capability cannot outlive the exact State rebuild authority
/// from which it was derived.
///
/// ```compile_fail
/// use oulipoly_state::StateDb;
/// use oulipoly_state::mailbox::MailboxDb;
///
/// let directory = tempfile::tempdir().unwrap();
/// let state_path = directory.path().join("state.db");
/// drop(StateDb::open(&state_path).unwrap());
/// let state_authority = StateDb::acquire_rebuild_authority(&state_path).unwrap();
/// let mut sidecar_authority = MailboxDb::acquire_rebuild_authority(&state_authority).unwrap();
/// drop(state_authority);
/// let _writable_state = StateDb::open(&state_path).unwrap();
/// sidecar_authority.reset().unwrap();
/// ```
pub struct MailboxDbRebuildAuthority<'state> {
    namespace: MailboxAuthorityFence,
    writer: Option<Connection>,
    _state_authority: &'state crate::StateDbRebuildAuthority,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MailboxFileIdentity {
    volume: u64,
    file: u64,
}

#[derive(Debug)]
pub(crate) enum MailboxAuthorityFenceError {
    Open {
        path: PathBuf,
        source: std::io::Error,
    },
    Lock(fs4::TryLockError),
    Timeout {
        path: PathBuf,
        timeout: StdDuration,
    },
}

impl fmt::Display for MailboxAuthorityFenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open { path, source } => write!(
                formatter,
                "Failed to open PID mailbox sidecar authority fence {}: {source}",
                path.display()
            ),
            Self::Lock(error) => {
                write!(
                    formatter,
                    "Failed to lock PID mailbox sidecar authority: {error}"
                )
            }
            Self::Timeout { path, timeout } => write!(
                formatter,
                "Timed out after {}ms acquiring PID mailbox sidecar authority fence {}",
                timeout.as_millis(),
                path.display()
            ),
        }
    }
}

impl std::error::Error for MailboxAuthorityFenceError {}

pub(crate) struct CompletionAuthorityFence<'a> {
    tx: Transaction<'a>,
}

pub(crate) const COMPLETION_CONTINUITY_GENESIS_DIGEST: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompletionContinuityHead {
    pub authority_ordinal: i64,
    pub admission_id: String,
    pub sidecar_generation: String,
    pub invocation_uuid: String,
    pub event_id: String,
    pub owner_invocation_uuid: String,
    pub owner_session_id: String,
    pub previous_continuity_digest: String,
    pub continuity_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompletionMaterializationSummary {
    pub materialized_count: i64,
    pub authority_ordinal: i64,
    pub sidecar_generation: String,
    pub continuity_digest: String,
}

impl CompletionAuthorityFence<'_> {
    pub(crate) fn sidecar_generation(&self) -> Result<String, String> {
        sidecar_generation_on(&self.tx)
    }

    pub(crate) fn completion_continuity_head(
        &self,
    ) -> Result<Option<CompletionContinuityHead>, String> {
        completion_continuity_head_on(&self.tx)
    }

    pub(crate) fn completion_materialization_summary(
        &self,
        invocation_uuid: &str,
    ) -> Result<Option<CompletionMaterializationSummary>, String> {
        completion_materialization_summary_on(&self.tx, invocation_uuid)
    }

    pub(crate) fn preflight_completion_event_registration(
        &self,
        input: &CompletionEventRegistrationInput<'_>,
    ) -> Result<(), String> {
        preflight_completion_event_registration_on(&self.tx, input)
    }

    pub(crate) fn register_completion_event(
        self,
        input: CompletionEventRegistrationInput<'_>,
        continuity: &CompletionContinuityHead,
    ) -> Result<CompletionEventRegistrationResult, String> {
        let inserted = register_completion_event_on(&self.tx, &input, &now_rfc3339())?;
        append_completion_continuity_on(&self.tx, continuity)?;
        let result = completion_event_registration_on(&self.tx, input.event_id, inserted)?;
        self.tx
            .commit()
            .map_err(|err| format!("Failed to commit completion event registration: {err}"))?;
        Ok(result)
    }
}

impl MailboxAuthorityFence {
    pub(crate) fn acquire(path: &Path) -> Result<Self, MailboxAuthorityFenceError> {
        Self::acquire_with_mode(path, false)
    }

    pub(crate) fn acquire_exclusive(path: &Path) -> Result<Self, MailboxAuthorityFenceError> {
        Self::acquire_with_mode(path, true)
    }

    fn acquire_with_mode(path: &Path, exclusive: bool) -> Result<Self, MailboxAuthorityFenceError> {
        const RETRY_INTERVAL: StdDuration = StdDuration::from_millis(10);
        #[cfg(not(test))]
        const ACQUISITION_TIMEOUT: StdDuration = StdDuration::from_secs(5);
        #[cfg(test)]
        const ACQUISITION_TIMEOUT: StdDuration = StdDuration::from_millis(500);

        ensure_parent_dir(path).map_err(|error| MailboxAuthorityFenceError::Open {
            path: path.to_path_buf(),
            source: std::io::Error::other(error),
        })?;
        let sidecar_path =
            normalized_mailbox_path(path).map_err(|source| MailboxAuthorityFenceError::Open {
                path: path.to_path_buf(),
                source,
            })?;
        validate_mailbox_storage_path(&sidecar_path).map_err(|source| {
            MailboxAuthorityFenceError::Open {
                path: sidecar_path.clone(),
                source,
            }
        })?;
        let target_identity = inspect_mailbox_storage_file(&sidecar_path).map_err(|source| {
            MailboxAuthorityFenceError::Open {
                path: sidecar_path.clone(),
                source,
            }
        })?;
        validate_mailbox_sqlite_artifacts(&sidecar_path).map_err(|source| {
            MailboxAuthorityFenceError::Open {
                path: sidecar_path.clone(),
                source,
            }
        })?;
        let authority_path = mailbox_authority_path(&sidecar_path);
        let initial_authority_identity =
            inspect_mailbox_storage_file(&authority_path).map_err(|source| {
                MailboxAuthorityFenceError::Open {
                    path: authority_path.clone(),
                    source,
                }
            })?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&authority_path)
            .map_err(|source| MailboxAuthorityFenceError::Open {
                path: authority_path.clone(),
                source,
            })?;
        let opened_authority_identity = opened_mailbox_file_identity(&file, &authority_path)
            .map_err(|source| MailboxAuthorityFenceError::Open {
                path: authority_path.clone(),
                source,
            })?;
        if initial_authority_identity.is_some_and(|identity| identity != opened_authority_identity)
        {
            return Err(MailboxAuthorityFenceError::Open {
                path: authority_path,
                source: std::io::Error::new(
                    ErrorKind::InvalidInput,
                    "PID mailbox authority fence changed during open",
                ),
            });
        }
        let deadline = Instant::now() + ACQUISITION_TIMEOUT;
        loop {
            let lock_result = if exclusive {
                <std::fs::File as fs4::FileExt>::try_lock(&file)
            } else {
                <std::fs::File as fs4::FileExt>::try_lock_shared(&file)
            };
            match lock_result {
                Ok(()) => {
                    let retained_authority_identity = inspect_mailbox_storage_file(&authority_path)
                        .map_err(|source| MailboxAuthorityFenceError::Open {
                            path: authority_path.clone(),
                            source,
                        })?
                        .ok_or_else(|| MailboxAuthorityFenceError::Open {
                            path: authority_path.clone(),
                            source: std::io::Error::new(
                                ErrorKind::NotFound,
                                "PID mailbox authority fence is missing after lock",
                            ),
                        })?;
                    if retained_authority_identity != opened_authority_identity {
                        return Err(MailboxAuthorityFenceError::Open {
                            path: authority_path,
                            source: std::io::Error::new(
                                ErrorKind::InvalidInput,
                                "PID mailbox authority fence changed during lock acquisition",
                            ),
                        });
                    }
                    let retained_target_identity =
                        validate_mailbox_source(path, &sidecar_path, target_identity).map_err(
                            |source| MailboxAuthorityFenceError::Open {
                                path: sidecar_path.clone(),
                                source,
                            },
                        )?;
                    return Ok(Self {
                        file,
                        sidecar_path,
                        target_identity: retained_target_identity,
                    });
                }
                Err(fs4::TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(RETRY_INTERVAL);
                }
                Err(fs4::TryLockError::WouldBlock) => {
                    return Err(MailboxAuthorityFenceError::Timeout {
                        path: authority_path,
                        timeout: ACQUISITION_TIMEOUT,
                    });
                }
                Err(error) => return Err(MailboxAuthorityFenceError::Lock(error)),
            }
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.sidecar_path
    }

    pub(crate) fn validate_opened_target(&self) -> Result<(), String> {
        let observed = inspect_mailbox_storage_file(&self.sidecar_path)
            .map_err(|error| format!("Failed to validate PID mailbox sidecar target: {error}"))?
            .ok_or_else(|| "PID mailbox sidecar target is missing after open".to_string())?;
        if self
            .target_identity
            .is_some_and(|expected| expected != observed)
        {
            return Err("PID mailbox sidecar target changed during open".to_string());
        }
        validate_mailbox_sqlite_artifacts(&self.sidecar_path)
            .map_err(|error| format!("Failed to validate PID mailbox SQLite artifacts: {error}"))?;
        Ok(())
    }
}

impl Drop for MailboxAuthorityFence {
    fn drop(&mut self) {
        let _ = <std::fs::File as fs4::FileExt>::unlock(&self.file);
    }
}

enum BoundedMailboxRowsError {
    Prepare(rusqlite::Error),
    Query(rusqlite::Error),
    Row(rusqlite::Error),
}

impl MailboxDb {
    pub fn path_for_state_db(state_db_path: &Path) -> PathBuf {
        if state_db_path.file_name() == Some(std::ffi::OsStr::new("state.db")) {
            return state_db_path.with_file_name("pid-identity.db");
        }
        let mut sidecar_name = state_db_path
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("state"))
            .to_os_string();
        sidecar_name.push(".pid-identity.db");
        state_db_path.with_file_name(sidecar_name)
    }

    pub fn acquire_rebuild_authority<'state>(
        state_authority: &'state crate::StateDbRebuildAuthority,
    ) -> Result<MailboxDbRebuildAuthority<'state>, String> {
        let sidecar_path = Self::path_for_state_db(state_authority.path());
        let namespace = match MailboxAuthorityFence::acquire_exclusive(&sidecar_path) {
            Ok(namespace) => namespace,
            Err(error @ MailboxAuthorityFenceError::Timeout { .. }) => {
                return Err(format!(
                    "process_integrity: completion_authority_contention: failed to acquire PID mailbox rebuild authority: {error}"
                ));
            }
            Err(error) => {
                return Err(format!(
                    "Failed to acquire PID mailbox rebuild authority: {error}"
                ));
            }
        };
        let writer = acquire_mailbox_rebuild_writer(&namespace)?;
        Ok(MailboxDbRebuildAuthority {
            namespace,
            writer,
            _state_authority: state_authority,
        })
    }

    pub fn default_path() -> Result<PathBuf, String> {
        pid_identity::default_path()
    }

    pub fn open_default() -> Result<Self, String> {
        let path = Self::default_path()?;
        Self::open(&path)
    }

    pub fn open_default_if_exists() -> Result<Option<Self>, String> {
        let path = Self::default_path()?;
        crate::rebuild_recovery::ensure_writable_open_allowed(&path)?;
        if !path.exists() {
            return Ok(None);
        }
        let authority = MailboxAuthorityFence::acquire(&path).map_err(|error| error.to_string())?;
        if !path.exists() {
            return Ok(None);
        }
        crate::rebuild_recovery::ensure_writable_open_allowed(authority.path())?;
        Self::open_with_owned_authority(authority).map(Some)
    }

    pub fn open(path: &Path) -> Result<Self, String> {
        let authority = MailboxAuthorityFence::acquire(path).map_err(|error| error.to_string())?;
        crate::rebuild_recovery::ensure_writable_open_allowed(authority.path())?;
        Self::open_with_owned_authority(authority)
    }

    fn open_with_owned_authority(authority: MailboxAuthorityFence) -> Result<Self, String> {
        let mut mailbox = Self::open_with_authority(&authority)?;
        mailbox._namespace_authority = Some(authority);
        Ok(mailbox)
    }

    pub(crate) fn open_with_authority(authority: &MailboxAuthorityFence) -> Result<Self, String> {
        let path = authority.path();
        let mut conn = Connection::open(path)
            .map_err(|err| format!("Failed to open PID mailbox sidecar: {err}"))?;
        authority.validate_opened_target()?;
        configure_writable_sidecar_connection(&conn)?;
        set_wal_mode(&conn)?;
        ensure_shared_sidecar_schema(&mut conn)?;
        Ok(Self {
            conn,
            path: path.to_path_buf(),
            _read_only_snapshot: None,
            _namespace_authority: None,
        })
    }

    pub fn open_read_only(path: &Path) -> Result<Self, String> {
        Self::open_read_only_with_cancel(path, &|| false)
    }

    pub fn open_read_only_with_cancel(
        path: &Path,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<Self, String> {
        let snapshot =
            crate::read_only_snapshot::ReadOnlySnapshot::create_with_cancel(path, is_cancelled)
                .map_err(|err| format!("Failed to open PID mailbox sidecar read-only: {err}"))?;
        Self::open_snapshot(path, snapshot)
    }

    pub fn open_read_only_with_pid_identity_and_work_timeout(
        path: &Path,
        retry_timeout: StdDuration,
        work_timeout: StdDuration,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<(crate::pid_identity::PidIdentityDb, Self), String> {
        let snapshot =
            crate::read_only_snapshot::ReadOnlySnapshot::create_with_retry_and_work_timeout(
                path,
                retry_timeout,
                work_timeout,
                is_cancelled,
            )
            .map_err(|err| format!("Failed to open PID mailbox sidecar read-only: {err}"))?;
        let pid = crate::pid_identity::PidIdentityDb::open_snapshot(path, snapshot.clone())?;
        let mailbox = Self::open_snapshot(path, snapshot)?;
        Ok((pid, mailbox))
    }

    fn open_snapshot(
        path: &Path,
        snapshot: crate::read_only_snapshot::ReadOnlySnapshot,
    ) -> Result<Self, String> {
        let conn = Connection::open_with_flags(snapshot.path(), OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|err| format!("Failed to open PID mailbox sidecar read-only: {err}"))?;
        Ok(Self {
            conn,
            path: path.to_path_buf(),
            _read_only_snapshot: Some(snapshot),
            _namespace_authority: None,
        })
    }

    pub(crate) fn open_existing_for_completion_authority(
        authority: &MailboxAuthorityFence,
    ) -> Result<Self, String> {
        let path = authority.path();
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
            .map_err(|err| format!("Failed to open PID mailbox sidecar authority: {err}"))?;
        authority.validate_opened_target()?;
        configure_writable_sidecar_connection(&conn)?;
        conn.busy_timeout(COMPLETION_AUTHORITY_SQLITE_TIMEOUT)
            .map_err(|err| format!("Failed to configure PID mailbox sidecar authority: {err}"))?;
        Ok(Self {
            conn,
            path: path.to_path_buf(),
            _read_only_snapshot: None,
            _namespace_authority: None,
        })
    }

    pub(crate) fn begin_completion_authority_fence(
        &mut self,
    ) -> Result<CompletionAuthorityFence<'_>, String> {
        // A write transaction excludes sidecar writers between authority validation and the
        // state commit. This deliberately accepts writer contention to close that TOCTOU window.
        self.conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map(|tx| CompletionAuthorityFence { tx })
            .map_err(|err| {
                if sqlite_error_is_contention(&err) {
                    format!(
                        "completion_authority_contention: timed out acquiring PID mailbox SQLite writer: {err}"
                    )
                } else {
                    format!("Failed to fence PID mailbox sidecar authority: {err}")
                }
            })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    #[cfg(test)]
    pub(crate) fn connection(&self) -> &Connection {
        &self.conn
    }

    pub fn sidecar_generation(&self) -> Result<String, String> {
        sidecar_generation_on(&self.conn)
    }

    pub fn contains_completion_obligation(
        &self,
        event_id: &str,
        owner_invocation_uuid: &str,
        owner_session_id: &str,
    ) -> Result<bool, String> {
        contains_completion_obligation_on(
            &self.conn,
            event_id,
            owner_invocation_uuid,
            owner_session_id,
        )
    }

    pub fn runtime_lifecycle(&mut self) -> RuntimeLifecycleRepository<'_> {
        RuntimeLifecycleRepository {
            conn: &mut self.conn,
        }
    }

    pub fn runtime_lifecycle_reader(&self) -> RuntimeLifecycleReader<'_> {
        RuntimeLifecycleReader { conn: &self.conn }
    }

    pub fn payloads(&self) -> PayloadRetentionRepository<'_> {
        PayloadRetentionRepository {
            conn: &self.conn,
            path: &self.path,
        }
    }

    pub fn wake_sessions(&mut self) -> WakeSessionRepository<'_> {
        WakeSessionRepository {
            conn: &mut self.conn,
        }
    }

    pub fn session_admissions(&mut self) -> SessionAdmissionRepository<'_> {
        SessionAdmissionRepository {
            conn: &mut self.conn,
        }
    }

    pub fn wake_session_reader(&self) -> WakeSessionReader<'_> {
        WakeSessionReader { conn: &self.conn }
    }
}

impl RuntimeLifecycleRepository<'_> {
    pub fn create_runtime_generation(
        &mut self,
        request: CreateRuntimeGeneration<'_>,
    ) -> Result<GenerationMutation<RuntimeGenerationRow>, GenerationStorageError> {
        validate_runtime_generation_create(&request)?;
        let creator_process_identity = current_runtime_creator_identity()?;
        let now = now_rfc3339();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(generation_storage_error(
                "start generation creation transaction",
            ))?;
        let changed = tx
            .execute(
                "INSERT OR IGNORE INTO runtime_generation (
                    generation_uuid, lifecycle_state, spawn_invocation_uuid, session_id,
                    runtime_mode, provider_name, model_name, pty_control_path, models_dir,
                    effective_cwd, created_at, creator_identity_os_pid,
                    creator_identity_os_boot_id, creator_identity_os_pid_starttime_ticks
                 ) VALUES (?1, 'starting', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    request.generation_id.to_string(),
                    request.spawn_invocation_uuid,
                    request.session_id,
                    request.runtime_mode,
                    request.provider_name,
                    request.model_name,
                    request.pty_control_path,
                    request.models_dir,
                    request.effective_cwd,
                    &now,
                    creator_process_identity.os_pid,
                    &creator_process_identity.os_boot_id,
                    creator_process_identity.os_pid_starttime_ticks,
                ],
            )
            .map_err(generation_storage_error(
                "insert starting runtime generation",
            ))?;
        let row = runtime_generation_by_id_on(&tx, request.generation_id)?.ok_or_else(|| {
            GenerationStorageError::new("Runtime generation missing after create".to_string())
        })?;
        let result =
            map_runtime_generation_create(changed, row, &request, &creator_process_identity);
        bind_runtime_generation_admission_on(
            &tx,
            request.spawn_invocation_uuid,
            request.generation_id,
            &creator_process_identity,
        )?;
        tx.commit().map_err(generation_storage_error(
            "commit generation creation transaction",
        ))?;
        Ok(result)
    }

    pub fn bind_runtime_generation_running(
        &mut self,
        request: BindRuntimeGenerationRunning<'_>,
    ) -> Result<GenerationMutation<RuntimeGenerationRow>, GenerationStorageError> {
        validate_runtime_generation_binding(&request)?;
        let now = now_rfc3339();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(generation_storage_error(
                "start generation binding transaction",
            ))?;
        let Some(before) = runtime_generation_by_id_on(&tx, request.fence.generation_id)? else {
            tx.commit().map_err(generation_storage_error(
                "commit missing generation binding transaction",
            ))?;
            return Ok(GenerationMutation::Rejected(GenerationRejection::NotFound));
        };
        if let Err(rejection) = validate_generation_binding_fence(&before, &request) {
            tx.commit().map_err(generation_storage_error(
                "commit rejected generation binding transaction",
            ))?;
            return Ok(GenerationMutation::Rejected(rejection));
        }
        if before.lifecycle_state == RuntimeLifecycleState::Running {
            let result = map_running_generation_binding_replay(before, &request);
            tx.commit().map_err(generation_storage_error(
                "commit replayed generation binding transaction",
            ))?;
            return Ok(result);
        }
        if let Err(rejection) = validate_generation_binding_predecessor(&before) {
            tx.commit().map_err(generation_storage_error(
                "commit illegal generation binding transaction",
            ))?;
            return Ok(GenerationMutation::Rejected(rejection));
        }
        if !bind_generation_process_identity(&tx, &before, &request, &now)? {
            tx.commit().map_err(generation_storage_error(
                "commit conflicting process identity binding transaction",
            ))?;
            return Ok(GenerationMutation::Rejected(
                GenerationRejection::ProcessIdentityConflict,
            ));
        }
        let identity = request.exact_process_identity;
        let changed = tx
            .execute(
                "UPDATE runtime_generation
                 SET lifecycle_state = 'running',
                     spawned_os_pid = ?3,
                     identity_os_pid = ?4,
                     identity_os_boot_id = ?5,
                     identity_os_pid_starttime_ticks = ?6,
                     running_at = ?7
                 WHERE generation_uuid = ?1
                   AND spawn_invocation_uuid = ?2
                   AND lifecycle_state = 'starting'",
                params![
                    request.fence.generation_id.to_string(),
                    request.fence.spawn_invocation_uuid,
                    request.spawned_os_pid,
                    identity.os_pid,
                    &identity.os_boot_id,
                    identity.os_pid_starttime_ticks,
                    &now,
                ],
            )
            .map_err(generation_storage_error("bind runtime generation running"))?;
        if changed != 1 {
            return Err(GenerationStorageError::new(
                "Runtime generation predecessor changed during immediate binding transaction"
                    .to_string(),
            ));
        }
        let row =
            runtime_generation_by_id_on(&tx, request.fence.generation_id)?.ok_or_else(|| {
                GenerationStorageError::new("Runtime generation missing after binding".to_string())
            })?;
        project_running_generation_on(&tx, &row)?;
        tx.commit().map_err(generation_storage_error(
            "commit generation binding transaction",
        ))?;
        Ok(GenerationMutation::Applied(row))
    }

    pub fn attach_runtime_generation_session(
        &mut self,
        request: AttachRuntimeGenerationSession<'_>,
    ) -> Result<GenerationMutation<RuntimeGenerationRow>, GenerationStorageError> {
        validate_generation_attachment_session_id(&request)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(generation_storage_error(
                "start generation session attachment transaction",
            ))?;
        let Some(before) = runtime_generation_by_id_on(&tx, request.fence.generation_id)? else {
            tx.commit().map_err(generation_storage_error(
                "commit missing session attachment transaction",
            ))?;
            return Ok(GenerationMutation::Rejected(GenerationRejection::NotFound));
        };
        if let Err(rejection) = validate_generation_attachment_fence(&before, &request) {
            tx.commit().map_err(generation_storage_error(
                "commit rejected session attachment transaction",
            ))?;
            return Ok(GenerationMutation::Rejected(rejection));
        }
        if before.session_id.is_some() {
            let result = map_generation_attachment_replay(before, &request);
            tx.commit().map_err(generation_storage_error(
                "commit replayed session attachment transaction",
            ))?;
            return Ok(result);
        }
        tx.execute(
            "UPDATE runtime_generation
             SET session_id = ?3
             WHERE generation_uuid = ?1
               AND spawn_invocation_uuid = ?2
               AND session_id IS NULL",
            params![
                request.fence.generation_id.to_string(),
                request.fence.spawn_invocation_uuid,
                request.session_id,
            ],
        )
        .map_err(generation_storage_error(
            "attach runtime generation session",
        ))?;
        bind_runtime_generation_admission_session_on(
            &tx,
            request.fence.generation_id,
            request.session_id,
        )?;
        if let ExactProcessEvidence::Recorded(identity) = &before.exact_process_evidence
            && !pid_identity::attach_identity_session_on(&tx, identity, request.session_id)
                .map_err(GenerationStorageError::new)?
        {
            tx.rollback().map_err(generation_storage_error(
                "roll back inconsistent generation session attachment",
            ))?;
            return Ok(GenerationMutation::Rejected(
                GenerationRejection::InvariantViolation,
            ));
        }
        let row =
            runtime_generation_by_id_on(&tx, request.fence.generation_id)?.ok_or_else(|| {
                GenerationStorageError::new(
                    "Runtime generation missing after session attachment".to_string(),
                )
            })?;
        project_running_generation_on(&tx, &row)?;
        tx.commit().map_err(generation_storage_error(
            "commit generation session attachment transaction",
        ))?;
        Ok(GenerationMutation::Applied(row))
    }
}

impl RuntimeLifecycleReader<'_> {
    pub fn resolve_runtime_generation(
        &self,
        selector: RuntimeGenerationSelector<'_>,
    ) -> Result<RuntimeGenerationResolution, GenerationStorageError> {
        let rows = match selector {
            RuntimeGenerationSelector::Exact(fence) => {
                runtime_generation_by_id_on(self.conn, fence.generation_id)?
                    .filter(|row| row.spawn_invocation_uuid == fence.spawn_invocation_uuid)
                    .into_iter()
                    .collect()
            }
            RuntimeGenerationSelector::ProcessIdentity(identity) => {
                runtime_generations_by_process_identity_on(self.conn, identity)?
            }
        };
        Ok(match rows.len() {
            0 => RuntimeGenerationResolution::NotFound,
            1 => RuntimeGenerationResolution::Found(Box::new(
                rows.into_iter().next().expect("length checked"),
            )),
            _ => RuntimeGenerationResolution::Ambiguous(rows),
        })
    }

    pub fn runtime_generation(
        &self,
        generation_id: &RuntimeGenerationId,
    ) -> Result<Option<RuntimeGenerationRow>, GenerationStorageError> {
        runtime_generation_by_id_on(self.conn, generation_id)
    }

    pub fn session_generation_projection(
        &self,
        session_id: &str,
    ) -> Result<SessionGenerationProjection, GenerationStorageError> {
        let sql = format_runtime_generations_for_session_sql(true);
        let rows = runtime_generations_for_session_on(self.conn, session_id, &sql)?;
        Ok(match rows.len() {
            0 => SessionGenerationProjection::None,
            1 => SessionGenerationProjection::One(Box::new(
                rows.into_iter().next().expect("length checked"),
            )),
            _ => SessionGenerationProjection::Multiple(rows),
        })
    }

    pub fn runtime_generation_history(
        &self,
        session_id: &str,
    ) -> Result<Vec<RuntimeGenerationRow>, GenerationStorageError> {
        let sql = format_runtime_generations_for_session_sql(false);
        runtime_generations_for_session_on(self.conn, session_id, &sql)
    }

    pub fn classify_session_liveness(
        &self,
        session_id: &str,
    ) -> Result<RuntimeGenerationReadOnlyLiveness, String> {
        let sql = format_runtime_generations_for_session_sql(true);
        let generations = runtime_generations_for_session_on(self.conn, session_id, &sql)
            .map_err(|error| error.to_string())?;
        Ok(classify_generation_liveness_read_only(&generations))
    }
}

impl RuntimeLifecycleRepository<'_> {
    pub fn reconcile_session_liveness(
        &mut self,
        session_id: &str,
    ) -> Result<SessionLiveness, String> {
        let sql = format_runtime_generations_for_session_sql(true);
        let generations = runtime_generations_for_session_on(self.conn, session_id, &sql)
            .map_err(|error| error.to_string())?;
        let mut observed_busy = false;
        for generation in generations {
            match generation_liveness_observation(&generation) {
                GenerationLivenessObservation::Busy => observed_busy = true,
                GenerationLivenessObservation::Stale => {
                    let result = self
                        .exit_runtime_generation_non_orderly(ExitRuntimeGenerationNonOrderly {
                            fence: RuntimeGenerationFence {
                                generation_id: &generation.generation_id,
                                spawn_invocation_uuid: &generation.spawn_invocation_uuid,
                            },
                            reason: RuntimeTerminalReason::RecoveredDead,
                            exit_code: None,
                        })
                        .map_err(|error| error.to_string())?;
                    if matches!(result, GenerationMutation::Rejected(_)) {
                        observed_busy = true;
                    }
                }
            }
        }
        Ok(if observed_busy {
            SessionLiveness::Busy
        } else {
            SessionLiveness::Idle
        })
    }

    /// Updates only the non-authoritative terminal projection after proving
    /// that one exact runtime generation owns the completed invocation.
    pub fn reconcile_terminal_compatibility_projection(
        &mut self,
        session_id: &str,
        spawn_invocation_uuid: &str,
        compatibility_exit_code: Option<i32>,
    ) -> Result<TerminalCompatibilityReconciliation, GenerationStorageError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(generation_storage_error(
                "start terminal compatibility reconciliation transaction",
            ))?;
        let (generation_count, exited_count) = tx
            .query_row(
                "SELECT COUNT(*),
                        COALESCE(SUM(CASE WHEN lifecycle_state = 'exited' THEN 1 ELSE 0 END), 0)
                 FROM runtime_generation
                 WHERE session_id = ?1 AND spawn_invocation_uuid = ?2",
                params![session_id, spawn_invocation_uuid],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .map_err(generation_storage_error(
                "read terminal compatibility generation authority",
            ))?;
        if generation_count == 0 {
            tx.commit().map_err(generation_storage_error(
                "commit absent terminal compatibility reconciliation",
            ))?;
            return Ok(TerminalCompatibilityReconciliation::NoGeneration);
        }
        if generation_count != 1 || exited_count != 1 {
            return Err(GenerationStorageError::new(format!(
                "Runtime generation authority for session {session_id} invocation {spawn_invocation_uuid} is not one exact exited generation"
            )));
        }
        tx.execute(
            "UPDATE session_runtime
             SET last_exit_code = ?3
             WHERE session_id = ?1
               AND run_state = 'idle'
               AND running_invocation_uuid IS NULL
               AND EXISTS (
                   SELECT 1
                   FROM runtime_generation
                   WHERE session_id = ?1
                     AND spawn_invocation_uuid = ?2
                     AND lifecycle_state = 'exited'
               )",
            params![session_id, spawn_invocation_uuid, compatibility_exit_code],
        )
        .map_err(generation_storage_error(
            "reconcile terminal compatibility projection",
        ))?;
        tx.commit().map_err(generation_storage_error(
            "commit terminal compatibility reconciliation",
        ))?;
        Ok(TerminalCompatibilityReconciliation::Reconciled)
    }

    pub fn acquire_runtime_generation_delivery(
        &mut self,
        request: AcquireRuntimeGenerationDelivery<'_>,
    ) -> Result<DeliveryClaimAcquireResult, GenerationStorageError> {
        validate_delivery_claim_seqs(request.seqs)?;
        let now = now_rfc3339();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(generation_storage_error(
                "start runtime generation delivery claim transaction",
            ))?;
        let Some(before) = runtime_generation_by_id_on(&tx, request.fence.generation_id)? else {
            tx.commit().map_err(generation_storage_error(
                "commit missing delivery claim transaction",
            ))?;
            return Ok(DeliveryClaimAcquireResult::Rejected(
                GenerationRejection::NotFound,
            ));
        };
        if let Err(rejection) = validate_generation_fence(&before, request.fence) {
            tx.commit().map_err(generation_storage_error(
                "commit rejected delivery claim transaction",
            ))?;
            return Ok(DeliveryClaimAcquireResult::Rejected(rejection));
        }
        let generation_session_id = match runtime_generation_session_id(&before) {
            Ok(session_id) => session_id.to_string(),
            Err(rejection) => {
                tx.commit().map_err(generation_storage_error(
                    "commit sessionless delivery claim transaction",
                ))?;
                return Ok(DeliveryClaimAcquireResult::Rejected(rejection));
            }
        };
        let has_existing_claim = before.active_delivery_claim_id.is_some();
        if has_existing_claim && validate_existing_delivery_claim(&before).is_err() {
            tx.commit().map_err(generation_storage_error(
                "commit invalid existing delivery claim transaction",
            ))?;
            return Ok(DeliveryClaimAcquireResult::Rejected(
                GenerationRejection::InvariantViolation,
            ));
        }
        if has_existing_claim {
            let claimed_states = mailbox_delivery_states_on(
                &tx,
                &generation_session_id,
                &before.active_delivery_seqs,
            )?;
            if !all_mailbox_seqs_owned(&claimed_states) {
                tx.commit().map_err(generation_storage_error(
                    "commit cross-session delivery claim transaction",
                ))?;
                return Ok(DeliveryClaimAcquireResult::Rejected(
                    GenerationRejection::SessionConflict,
                ));
            }
        }
        let existing_claim_stale =
            runtime_delivery_claim_is_stale(&before, request.stale_after_seconds);
        if has_existing_claim
            && map_existing_delivery_claim(&before, existing_claim_stale)
                == ExistingDeliveryClaimDisposition::AlreadyInFlight
        {
            tx.commit().map_err(generation_storage_error(
                "commit in-flight delivery claim transaction",
            ))?;
            return Ok(DeliveryClaimAcquireResult::AlreadyInFlight(before));
        }
        if has_existing_claim {
            tx.execute(
                "UPDATE runtime_generation
                 SET active_delivery_claimed_at = ?3
                 WHERE generation_uuid = ?1
                   AND spawn_invocation_uuid = ?2
                   AND lifecycle_state = 'running'
                   AND active_delivery_claim_uuid IS NOT NULL",
                params![
                    request.fence.generation_id.to_string(),
                    request.fence.spawn_invocation_uuid,
                    &now,
                ],
            )
            .map_err(generation_storage_error(
                "renew stale runtime generation delivery claim",
            ))?;
            let row = runtime_generation_by_id_on(&tx, request.fence.generation_id)?.ok_or_else(
                || {
                    GenerationStorageError::new(
                        "Runtime generation missing after delivery claim recovery".to_string(),
                    )
                },
            )?;
            tx.commit().map_err(generation_storage_error(
                "commit recovered delivery claim transaction",
            ))?;
            return Ok(DeliveryClaimAcquireResult::Recovered(row));
        }
        match validate_new_delivery_claim(&before, true, false) {
            Ok(()) => {}
            Err(rejection @ GenerationRejection::IllegalPredecessor { .. }) => {
                tx.commit().map_err(generation_storage_error(
                    "commit illegal delivery claim transaction",
                ))?;
                return Ok(DeliveryClaimAcquireResult::Rejected(rejection));
            }
            Err(rejection @ GenerationRejection::DrainRequestConflict) => {
                tx.commit().map_err(generation_storage_error(
                    "commit drain-blocked delivery claim transaction",
                ))?;
                return Ok(DeliveryClaimAcquireResult::Rejected(rejection));
            }
            Err(rejection) => {
                tx.commit().map_err(generation_storage_error(
                    "commit invalid delivery claim batch transaction",
                ))?;
                return Ok(DeliveryClaimAcquireResult::Rejected(rejection));
            }
        }
        let delivery_states =
            mailbox_delivery_states_on(&tx, &generation_session_id, request.seqs)?;
        if !all_mailbox_seqs_owned(&delivery_states) {
            tx.commit().map_err(generation_storage_error(
                "commit cross-session delivery claim batch transaction",
            ))?;
            return Ok(DeliveryClaimAcquireResult::Rejected(
                GenerationRejection::SessionConflict,
            ));
        }
        let rows_pending = all_mailbox_seqs_pending(&delivery_states);
        let claim_encodings = active_delivery_claim_encodings_on(&tx, request.fence.generation_id)?;
        let claim_batches = parse_active_delivery_claim_batches(claim_encodings)?;
        let overlaps = delivery_claim_batches_overlap(&claim_batches, request.seqs);
        if let Err(rejection) = validate_new_delivery_claim(&before, rows_pending, overlaps) {
            tx.commit().map_err(generation_storage_error(
                "commit invalid delivery claim batch transaction",
            ))?;
            return Ok(DeliveryClaimAcquireResult::Rejected(rejection));
        }
        let seqs_json = serialize_delivery_seqs(request.seqs)?;
        let changed = tx
            .execute(
                "UPDATE runtime_generation
                 SET active_delivery_claim_uuid = ?3,
                     active_delivery_claimed_at = ?4,
                     active_delivery_seqs_json = ?5
                 WHERE generation_uuid = ?1
                   AND spawn_invocation_uuid = ?2
                   AND lifecycle_state = 'running'
                   AND drain_request_uuid IS NULL
                   AND active_delivery_claim_uuid IS NULL",
                params![
                    request.fence.generation_id.to_string(),
                    request.fence.spawn_invocation_uuid,
                    request.claim_id.to_string(),
                    &now,
                    &seqs_json,
                ],
            )
            .map_err(generation_storage_error(
                "acquire runtime generation delivery claim",
            ))?;
        if changed != 1 {
            tx.commit().map_err(generation_storage_error(
                "commit lost delivery claim transaction",
            ))?;
            return Ok(DeliveryClaimAcquireResult::Rejected(
                GenerationRejection::InvariantViolation,
            ));
        }
        let row =
            runtime_generation_by_id_on(&tx, request.fence.generation_id)?.ok_or_else(|| {
                GenerationStorageError::new(
                    "Runtime generation missing after delivery claim acquisition".to_string(),
                )
            })?;
        tx.commit().map_err(generation_storage_error(
            "commit acquired delivery claim transaction",
        ))?;
        Ok(map_acquired_delivery_claim(row))
    }

    pub fn confirm_runtime_generation_delivery(
        &mut self,
        request: ConfirmRuntimeGenerationDelivery<'_>,
    ) -> Result<GenerationMutation<RuntimeGenerationRow>, GenerationStorageError> {
        validate_delivery_claim_seqs(request.seqs)?;
        let now = now_rfc3339();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(generation_storage_error(
                "start runtime generation delivery confirmation transaction",
            ))?;
        let before = runtime_generation_by_id_on(&tx, request.fence.generation_id)?;
        if let Err(rejection) = validate_active_delivery_claim(
            before.as_ref(),
            request.fence,
            request.claim_id,
            request.seqs,
        ) {
            tx.commit().map_err(generation_storage_error(
                "commit rejected delivery confirmation transaction",
            ))?;
            return Ok(GenerationMutation::Rejected(rejection));
        }
        let before = before.expect("validated active claim requires a generation row");
        let session_id = before
            .session_id
            .as_deref()
            .expect("validated active claim requires a session");
        if let Err(rejection) = validate_running_delivery_confirmation(&before) {
            tx.commit().map_err(generation_storage_error(
                "commit non-running delivery confirmation transaction",
            ))?;
            return Ok(GenerationMutation::Rejected(rejection));
        }
        for seq in request.seqs {
            let changed = tx
                .execute(
                    "UPDATE mailbox
                     SET delivered_at = ?3,
                         delivered_by_invocation_uuid = ?4,
                         delivery_attempts = delivery_attempts + 1,
                         delivery_error = NULL
                     WHERE seq = ?1
                       AND session_id = ?2
                       AND delivered_at IS NULL",
                    params![seq, session_id, &now, request.delivered_by_invocation_uuid],
                )
                .map_err(generation_storage_error(
                    "confirm claimed mailbox row delivery",
                ))?;
            validate_claimed_mailbox_row_change(*seq, changed, "confirmation")?;
        }
        clear_runtime_delivery_claim_on(&tx, request.fence, request.claim_id)?;
        let row =
            runtime_generation_by_id_on(&tx, request.fence.generation_id)?.ok_or_else(|| {
                GenerationStorageError::new(
                    "Runtime generation missing after delivery confirmation".to_string(),
                )
            })?;
        tx.commit().map_err(generation_storage_error(
            "commit delivery confirmation transaction",
        ))?;
        Ok(map_applied_generation(row))
    }

    pub fn fail_runtime_generation_delivery(
        &mut self,
        request: FailRuntimeGenerationDelivery<'_>,
    ) -> Result<GenerationMutation<RuntimeGenerationRow>, GenerationStorageError> {
        reject_unauthorized_terminal_wake_abandonment(request.delivery_error)
            .map_err(GenerationStorageError::new)?;
        validate_delivery_claim_seqs(request.seqs)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(generation_storage_error(
                "start runtime generation delivery failure transaction",
            ))?;
        let before = runtime_generation_by_id_on(&tx, request.fence.generation_id)?;
        if let Err(rejection) = validate_active_delivery_claim(
            before.as_ref(),
            request.fence,
            request.claim_id,
            request.seqs,
        ) {
            tx.commit().map_err(generation_storage_error(
                "commit rejected delivery failure transaction",
            ))?;
            return Ok(GenerationMutation::Rejected(rejection));
        }
        let before = before.expect("validated active claim requires a generation row");
        let session_id = before
            .session_id
            .as_deref()
            .expect("validated active claim requires a session");
        for seq in request.seqs {
            let changed = tx
                .execute(
                    "UPDATE mailbox
                     SET delivery_attempts = delivery_attempts + 1,
                         delivery_error = ?3
                     WHERE seq = ?1
                       AND session_id = ?2
                       AND delivered_at IS NULL",
                    params![seq, session_id, request.delivery_error],
                )
                .map_err(generation_storage_error(
                    "record claimed mailbox row delivery failure",
                ))?;
            validate_claimed_mailbox_row_change(*seq, changed, "failure")?;
        }
        clear_runtime_delivery_claim_on(&tx, request.fence, request.claim_id)?;
        let row =
            runtime_generation_by_id_on(&tx, request.fence.generation_id)?.ok_or_else(|| {
                GenerationStorageError::new(
                    "Runtime generation missing after delivery failure".to_string(),
                )
            })?;
        tx.commit().map_err(generation_storage_error(
            "commit delivery failure transaction",
        ))?;
        Ok(map_failed_delivery_generation(row))
    }

    pub fn exit_runtime_generation_non_orderly(
        &mut self,
        request: ExitRuntimeGenerationNonOrderly<'_>,
    ) -> Result<GenerationMutation<RuntimeGenerationRow>, GenerationStorageError> {
        validate_non_orderly_reason(&request)?;
        let now = now_rfc3339();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(generation_storage_error(
                "start non-orderly generation exit transaction",
            ))?;
        let Some(before) = runtime_generation_by_id_on(&tx, request.fence.generation_id)? else {
            tx.commit().map_err(generation_storage_error(
                "commit missing generation exit transaction",
            ))?;
            return Ok(GenerationMutation::Rejected(GenerationRejection::NotFound));
        };
        if let Err(rejection) = validate_generation_fence(&before, request.fence) {
            tx.commit().map_err(generation_storage_error(
                "commit rejected generation exit transaction",
            ))?;
            return Ok(GenerationMutation::Rejected(rejection));
        }
        if let Some(result) = map_non_orderly_exit_replay(&before, &request) {
            tx.commit().map_err(generation_storage_error(
                "commit replayed generation exit transaction",
            ))?;
            return Ok(result);
        }
        if let Err(rejection) = validate_recovered_dead_process(&before, &request) {
            tx.commit().map_err(generation_storage_error(
                "commit live generation recovery rejection",
            ))?;
            return Ok(GenerationMutation::Rejected(rejection));
        }
        if let Err(rejection) = validate_non_orderly_predecessor(&before, &request) {
            tx.commit().map_err(generation_storage_error(
                "commit illegal generation exit transaction",
            ))?;
            return Ok(GenerationMutation::Rejected(rejection));
        }
        tx.execute(
            "UPDATE runtime_generation
             SET lifecycle_state = 'exited', exited_at = ?3, terminal_reason = ?4, exit_code = ?5,
                 active_delivery_claim_uuid = NULL,
                 active_delivery_claimed_at = NULL,
                 active_delivery_seqs_json = NULL
              WHERE generation_uuid = ?1
                AND spawn_invocation_uuid = ?2
                AND (
                    lifecycle_state IN ('starting', 'running')
                    OR (lifecycle_state = 'draining' AND ?4 = 'recovered_dead')
                )",
            params![
                request.fence.generation_id.to_string(),
                request.fence.spawn_invocation_uuid,
                &now,
                request.reason.as_str(),
                request.exit_code
            ],
        )
        .map_err(generation_storage_error(
            "exit runtime generation non-orderly",
        ))?;
        settle_runtime_generation_admission_on(&tx, request.fence.generation_id)?;
        let row =
            runtime_generation_by_id_on(&tx, request.fence.generation_id)?.ok_or_else(|| {
                GenerationStorageError::new("Runtime generation missing after exit".to_string())
            })?;
        project_exited_generation_on(&tx, &row, &now, row.exit_code)?;
        tx.commit().map_err(generation_storage_error(
            "commit non-orderly generation exit transaction",
        ))?;
        Ok(map_applied_generation(row))
    }

    pub fn request_runtime_generation_drain(
        &mut self,
        request: RequestRuntimeGenerationDrain<'_>,
    ) -> Result<DrainRequestResult, GenerationStorageError> {
        let now = now_rfc3339();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(generation_storage_error(
                "start generation drain request transaction",
            ))?;
        let Some(before) = runtime_generation_by_id_on(&tx, request.fence.generation_id)? else {
            tx.commit().map_err(generation_storage_error(
                "commit missing drain request transaction",
            ))?;
            return Ok(DrainRequestResult::Rejected(GenerationRejection::NotFound));
        };
        if let Err(rejection) = validate_drain_request_fence(&before, &request) {
            tx.commit().map_err(generation_storage_error(
                "commit rejected drain request transaction",
            ))?;
            return Ok(DrainRequestResult::Rejected(rejection));
        }
        if let Some(result) = map_drain_request_replay(&before, &request) {
            tx.commit().map_err(generation_storage_error(
                "commit replayed drain request transaction",
            ))?;
            return Ok(result);
        }
        if let Err(rejection) = validate_drain_request_predecessor(&before) {
            tx.commit().map_err(generation_storage_error(
                "commit illegal drain request transaction",
            ))?;
            return Ok(DrainRequestResult::Rejected(rejection));
        }
        tx.execute(
            "UPDATE runtime_generation
             SET drain_request_uuid = ?3, drain_requested_at = ?4,
                 drain_requested_by_invocation_uuid = ?5
             WHERE generation_uuid = ?1
               AND spawn_invocation_uuid = ?2
               AND lifecycle_state = 'running'
               AND drain_request_uuid IS NULL",
            params![
                request.fence.generation_id.to_string(),
                request.fence.spawn_invocation_uuid,
                request.drain_request_id.to_string(),
                &now,
                request.requested_by_invocation_uuid
            ],
        )
        .map_err(generation_storage_error(
            "install runtime generation drain request",
        ))?;
        let row =
            runtime_generation_by_id_on(&tx, request.fence.generation_id)?.ok_or_else(|| {
                GenerationStorageError::new(
                    "Runtime generation missing after drain request".to_string(),
                )
            })?;
        tx.commit().map_err(generation_storage_error(
            "commit generation drain request transaction",
        ))?;
        Ok(map_installed_drain_request(row))
    }

    pub fn advance_runtime_generation_drain(
        &mut self,
        request: AdvanceRuntimeGenerationDrain<'_>,
    ) -> Result<DrainAdvanceResult, GenerationStorageError> {
        let now = now_rfc3339();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(generation_storage_error(
                "start generation drain advance transaction",
            ))?;
        let Some(before) = runtime_generation_by_id_on(&tx, request.fence.generation_id)? else {
            tx.commit().map_err(generation_storage_error(
                "commit missing drain advance transaction",
            ))?;
            return Ok(DrainAdvanceResult::Rejected(GenerationRejection::NotFound));
        };
        match validate_drain_advance_identity(&before, &request) {
            Ok(()) => {}
            Err(GenerationRejection::FenceMismatch) => {
                tx.commit().map_err(generation_storage_error(
                    "commit rejected drain advance transaction",
                ))?;
                return Ok(DrainAdvanceResult::Rejected(
                    GenerationRejection::FenceMismatch,
                ));
            }
            Err(rejection) => {
                tx.commit().map_err(generation_storage_error(
                    "commit mismatched drain advance transaction",
                ))?;
                return Ok(DrainAdvanceResult::Rejected(rejection));
            }
        }
        match map_drain_advance_blocker(&before) {
            None => {}
            Some(DrainAdvanceResult::WaitingOnClaim(claim_id)) => {
                tx.commit().map_err(generation_storage_error(
                    "commit waiting drain advance transaction",
                ))?;
                return Ok(DrainAdvanceResult::WaitingOnClaim(claim_id));
            }
            Some(DrainAdvanceResult::AlreadyDraining(row)) => {
                tx.commit().map_err(generation_storage_error(
                    "commit replayed drain advance transaction",
                ))?;
                return Ok(DrainAdvanceResult::AlreadyDraining(row));
            }
            Some(DrainAdvanceResult::AlreadyExited(row)) => {
                tx.commit().map_err(generation_storage_error(
                    "commit exited drain advance transaction",
                ))?;
                return Ok(DrainAdvanceResult::AlreadyExited(row));
            }
            Some(DrainAdvanceResult::Rejected(rejection)) => {
                tx.commit().map_err(generation_storage_error(
                    "commit illegal drain advance transaction",
                ))?;
                return Ok(DrainAdvanceResult::Rejected(rejection));
            }
            Some(DrainAdvanceResult::Advanced(_)) => unreachable!("advanced rows are not blockers"),
        }
        tx.execute(
            "UPDATE runtime_generation
             SET lifecycle_state = 'draining', draining_at = ?4
             WHERE generation_uuid = ?1
               AND spawn_invocation_uuid = ?2
               AND lifecycle_state = 'running'
               AND drain_request_uuid = ?3
               AND active_delivery_claim_uuid IS NULL",
            params![
                request.fence.generation_id.to_string(),
                request.fence.spawn_invocation_uuid,
                request.drain_request_id.to_string(),
                &now
            ],
        )
        .map_err(generation_storage_error("advance runtime generation drain"))?;
        let row =
            runtime_generation_by_id_on(&tx, request.fence.generation_id)?.ok_or_else(|| {
                GenerationStorageError::new(
                    "Runtime generation missing after drain advance".to_string(),
                )
            })?;
        tx.commit().map_err(generation_storage_error(
            "commit generation drain advance transaction",
        ))?;
        Ok(map_advanced_drain(row))
    }

    pub fn finish_runtime_generation_drain(
        &mut self,
        request: FinishRuntimeGenerationDrain<'_>,
    ) -> Result<DrainFinishResult, GenerationStorageError> {
        let now = now_rfc3339();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(generation_storage_error(
                "start generation drain finish transaction",
            ))?;
        let Some(before) = runtime_generation_by_id_on(&tx, request.fence.generation_id)? else {
            tx.commit().map_err(generation_storage_error(
                "commit missing drain finish transaction",
            ))?;
            return Ok(DrainFinishResult::Rejected(GenerationRejection::NotFound));
        };
        match validate_drain_finish_identity(&before, &request) {
            Ok(()) => {}
            Err(GenerationRejection::FenceMismatch) => {
                tx.commit().map_err(generation_storage_error(
                    "commit rejected drain finish transaction",
                ))?;
                return Ok(DrainFinishResult::Rejected(
                    GenerationRejection::FenceMismatch,
                ));
            }
            Err(rejection) => {
                tx.commit().map_err(generation_storage_error(
                    "commit mismatched drain finish transaction",
                ))?;
                return Ok(DrainFinishResult::Rejected(rejection));
            }
        }
        match map_drain_finish_predecessor(&before) {
            None => {}
            Some(DrainFinishResult::AlreadyExited(row)) => {
                tx.commit().map_err(generation_storage_error(
                    "commit replayed drain finish transaction",
                ))?;
                return Ok(DrainFinishResult::AlreadyExited(row));
            }
            Some(DrainFinishResult::NotDraining(actual)) => {
                tx.commit().map_err(generation_storage_error(
                    "commit premature drain finish transaction",
                ))?;
                return Ok(DrainFinishResult::NotDraining(actual));
            }
            Some(_) => unreachable!("only predecessor dispositions are mapped"),
        }
        if let Err(rejection) = validate_drain_finish_claim(&before) {
            tx.commit().map_err(generation_storage_error(
                "commit withheld drain finish transaction",
            ))?;
            return Ok(DrainFinishResult::Rejected(rejection));
        }
        tx.execute(
            "UPDATE runtime_generation
             SET lifecycle_state = 'exited', exited_at = ?4,
                 terminal_reason = 'orderly_completion', exit_code = ?5
             WHERE generation_uuid = ?1
               AND spawn_invocation_uuid = ?2
               AND lifecycle_state = 'draining'
               AND drain_request_uuid = ?3
               AND active_delivery_claim_uuid IS NULL",
            params![
                request.fence.generation_id.to_string(),
                request.fence.spawn_invocation_uuid,
                request.drain_request_id.to_string(),
                &now,
                request.exit_code
            ],
        )
        .map_err(generation_storage_error("finish runtime generation drain"))?;
        settle_runtime_generation_admission_on(&tx, request.fence.generation_id)?;
        let row =
            runtime_generation_by_id_on(&tx, request.fence.generation_id)?.ok_or_else(|| {
                GenerationStorageError::new(
                    "Runtime generation missing after drain finish".to_string(),
                )
            })?;
        project_exited_generation_on(&tx, &row, &now, request.compatibility_exit_code)?;
        tx.commit().map_err(generation_storage_error(
            "commit generation drain finish transaction",
        ))?;
        Ok(map_finished_drain(row))
    }
}

impl MailboxDb {
    #[cfg(test)]
    pub(crate) fn register_completion_event(
        &mut self,
        input: CompletionEventRegistrationInput<'_>,
    ) -> Result<CompletionEventRegistrationResult, String> {
        validate_completion_event_registration(&input)?;
        let now = now_rfc3339();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|err| {
                format!("Failed to start completion event registration transaction: {err}")
            })?;
        let inserted = register_completion_event_on(&tx, &input, &now)?;
        let result = completion_event_registration_on(&tx, input.event_id, inserted)?;
        tx.commit()
            .map_err(|err| format!("Failed to commit completion event registration: {err}"))?;
        Ok(result)
    }

    pub fn activate_completion_event_listeners(
        &mut self,
        event_id: &str,
    ) -> Result<CompletionEventTriggerResult, String> {
        let now = now_rfc3339();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|err| {
                format!("Failed to start completion listener activation transaction: {err}")
            })?;
        let event = completion_event_by_id_on(&tx, event_id)?
            .ok_or_else(|| format!("Completion event {event_id} is not registered"))?;
        tx.execute(
            "UPDATE completion_event_listener
             SET active = 1
             WHERE event_id = ?1 AND acknowledged_at IS NULL",
            params![event_id],
        )
        .map_err(|err| format!("Failed to activate completion event listeners: {err}"))?;
        if event.state == "triggered" {
            materialize_completion_event_listeners(&tx, &event, &now)?;
        }
        tx.commit()
            .map_err(|err| format!("Failed to commit completion listener activation: {err}"))?;
        self.completion_event_trigger_result(event_id, false)
    }

    pub fn trigger_completion_event(
        &mut self,
        input: CompletionEventTriggerInput<'_>,
    ) -> Result<CompletionEventTriggerResult, String> {
        validate_completion_event_trigger(&input)?;
        let published = self
            .payloads()
            .publish_immutable_payload(input.payload_json.as_bytes())?;
        let payload_json = compacted_payload_json(AGENT_BASH_COMPLETE_KIND, &published)?;
        let now = now_rfc3339();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|err| {
                format!("Failed to start completion event trigger transaction: {err}")
            })?;
        verify_published_payload(&published)?;
        let event = completion_event_by_id_on(&tx, input.event_id)?
            .ok_or_else(|| format!("Completion event {} is not registered", input.event_id))?;
        validate_completion_event_trigger_source(&event, &input)?;
        let triggered = if event.state == "pending" {
            trigger_completion_event_row(&tx, &input, &payload_json, &published, &now)?;
            true
        } else {
            validate_completion_event_trigger_replay(&event, &input, &published)?;
            tx.execute(
                "UPDATE completion_event
                 SET payload_reclaimed_at = NULL
                 WHERE event_id = ?1 AND payload_reclaimed_at IS NOT NULL",
                params![input.event_id],
            )
            .map_err(|err| format!("Failed to refresh replayed completion payload: {err}"))?;
            false
        };
        if input.consumed {
            acknowledge_consumed_completion_event_listeners(&tx, input.event_id, &now)?;
        }
        let event = completion_event_by_id_on(&tx, input.event_id)?
            .ok_or_else(|| format!("Completion event {} disappeared", input.event_id))?;
        materialize_completion_event_listeners(&tx, &event, &now)?;
        tx.commit()
            .map_err(|err| format!("Failed to commit completion event trigger: {err}"))?;
        self.maintain_terminal_history();
        self.completion_event_trigger_result(input.event_id, triggered)
    }

    pub fn completion_event(&self, event_id: &str) -> Result<Option<CompletionEventRow>, String> {
        completion_event_by_id_on(&self.conn, event_id)
    }

    pub fn completion_event_listeners(
        &self,
        event_id: &str,
    ) -> Result<Vec<CompletionEventListenerRow>, String> {
        completion_event_listeners_on(&self.conn, event_id)
    }

    pub fn acknowledge_consumed_completion_event_for_mailbox_seq(
        &mut self,
        mailbox_seq: i64,
        consumer_session_id: &str,
        consumer_invocation_uuid: &str,
    ) -> Result<Option<String>, String> {
        let now = now_rfc3339();
        let tx = begin_consumed_completion_acknowledgement(&mut self.conn)?;
        let Some(binding) = consumed_completion_binding(&tx, mailbox_seq)? else {
            commit_consumed_completion_acknowledgement(tx)?;
            return Ok(None);
        };
        if !binding.owner_matches(consumer_session_id, consumer_invocation_uuid) {
            commit_consumed_completion_acknowledgement(tx)?;
            return Ok(None);
        }
        if binding.is_settled() || completion_consumption_claimed(&tx, &binding.event_id)? {
            commit_consumed_completion_acknowledgement(tx)?;
            return Ok(None);
        }
        let mailbox_changed =
            consume_completion_mailbox_row(&tx, mailbox_seq, &now, &binding.owner_invocation_uuid)?;
        validate_consumed_completion_change(mailbox_changed, "mailbox row", mailbox_seq)?;
        let listener_changed = acknowledge_consumed_completion_listener(&tx, mailbox_seq, &now)?;
        validate_consumed_completion_change(listener_changed, "listener", mailbox_seq)?;
        resolve_completed_delivery_attempts_for_mailbox_seq(&tx, mailbox_seq, &now)?;
        commit_consumed_completion_acknowledgement(tx)?;
        self.maintain_terminal_history();
        Ok(Some(binding.event_id))
    }

    fn completion_event_trigger_result(
        &self,
        event_id: &str,
        triggered: bool,
    ) -> Result<CompletionEventTriggerResult, String> {
        let event = self
            .completion_event(event_id)?
            .ok_or_else(|| format!("Completion event {event_id} disappeared"))?;
        let listeners = self.completion_event_listeners(event_id)?;
        let mailbox_rows = completion_event_mailbox_rows_on(&self.conn, event_id)?;
        Ok(CompletionEventTriggerResult {
            triggered,
            event,
            listeners,
            mailbox_rows,
        })
    }

    pub fn enqueue_agent_bash_complete(
        &mut self,
        input: &AgentBashCompleteEnqueue<'_>,
    ) -> Result<EnqueueResult, String> {
        let published = self
            .payloads()
            .publish_immutable_payload(input.payload_json.as_bytes())?;
        let payload_json = compacted_payload_json(AGENT_BASH_COMPLETE_KIND, &published)?;
        let now = now_rfc3339();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|err| format!("Failed to start mailbox enqueue transaction: {err}"))?;
        verify_published_payload(&published)?;
        let result =
            enqueue_agent_bash_complete_in_tx(&tx, input, &payload_json, &published, &now)?;
        tx.commit()
            .map_err(|err| format!("Failed to commit mailbox enqueue transaction: {err}"))?;
        Ok(result)
    }

    pub fn enqueue_submitted_input(
        &mut self,
        input: &SubmittedInputEnqueue<'_>,
    ) -> Result<EnqueueResult, String> {
        validate_submitted_input(input)?;
        let published = self.payloads().publish_immutable_payload(input.input)?;
        let handle = submitted_input_handle(input.submission_token, input.target)?;
        let payload_json = submitted_input_payload_json(input, &published)?;
        let now = now_rfc3339();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|err| format!("Failed to start input enqueue transaction: {err}"))?;
        verify_published_payload(&published)?;
        let result =
            enqueue_submitted_input_in_tx(&tx, input, &handle, &payload_json, &published, &now)?;
        tx.commit()
            .map_err(|err| format!("Failed to commit input enqueue transaction: {err}"))?;
        Ok(result)
    }
}

impl PayloadRetentionRepository<'_> {
    fn payload_reference(&self, bytes: &[u8]) -> Result<PublishedMailboxPayload, String> {
        let byte_len = u64::try_from(bytes.len())
            .map_err(|_| "Mailbox payload length does not fit u64".to_string())?;
        let sha256 = sha256_hex(bytes);
        Ok(PublishedMailboxPayload {
            address: payload_address(&sha256),
            file_path: self.payload_path_for_sha256(&sha256)?,
            sha256,
            byte_len,
            retention_policy: MAILBOX_PAYLOAD_RETENTION_POLICY.to_string(),
        })
    }

    fn payload_path_for_sha256(&self, sha256: &str) -> Result<PathBuf, String> {
        validate_sha256_hex(sha256)?;
        let root = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        Ok(root
            .join(MAILBOX_PAYLOAD_DIRECTORY)
            .join(MAILBOX_PAYLOAD_ADDRESS_VERSION)
            .join(MAILBOX_PAYLOAD_ALGORITHM)
            .join(&sha256[..2])
            .join(sha256))
    }

    /// Publishes bytes before their acceptance metadata is committed. A caller
    /// must treat a later metadata failure as non-accepting; the immutable,
    /// unreferenced file is retained for safe retry or governed cleanup.
    pub fn publish_immutable_payload(
        &self,
        bytes: &[u8],
    ) -> Result<PublishedMailboxPayload, String> {
        let payload = self.payload_reference(bytes)?;
        publish_payload_file(&payload.file_path, bytes)?;
        verify_published_payload(&payload)?;
        Ok(payload)
    }

    pub fn verify_published_payload(
        &self,
        payload: &PublishedMailboxPayload,
    ) -> Result<(), String> {
        let expected_path = self.payload_path_for_sha256(&payload.sha256)?;
        if payload.file_path != expected_path {
            return Err(format!(
                "Mailbox payload address does not resolve inside the runner-owned store: {}",
                payload.file_path.display()
            ));
        }
        verify_published_payload(payload)
    }

    pub fn verify_mailbox_row_payload(&self, row: &MailboxRow) -> Result<(), String> {
        let Some(payload) = published_payload_from_row(row)? else {
            return Ok(());
        };
        self.verify_published_payload(&payload)
    }

    pub fn hydrate_agent_bash_payload_json(&self, row: &MailboxRow) -> Result<String, String> {
        if row.kind != AGENT_BASH_COMPLETE_KIND || row.payload_compacted_at.is_none() {
            return Ok(row.payload_json.clone());
        }
        self.verify_mailbox_row_payload(row)?;
        let path = row
            .payload_file_path
            .as_deref()
            .ok_or_else(|| format!("Mailbox row {} has no retained payload path", row.seq))?;
        fs::read_to_string(path).map_err(|err| {
            format!(
                "Failed to read mailbox row {} retained payload: {err}",
                row.seq
            )
        })
    }

    pub fn delivered_payload_compaction_stats(
        &self,
    ) -> Result<DeliveredPayloadCompactionStats, String> {
        let (eligible_rows, inline_bytes) = self
            .conn
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(length(CAST(payload_json AS BLOB))), 0)
                 FROM mailbox
                 WHERE kind = ?1
                   AND delivered_at IS NOT NULL
                   AND payload_compacted_at IS NULL",
                params![AGENT_BASH_COMPLETE_KIND],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .map_err(|err| format!("Failed to read delivered payload compaction stats: {err}"))?;
        Ok(DeliveredPayloadCompactionStats {
            eligible_rows: usize::try_from(eligible_rows)
                .map_err(|_| "Delivered payload row count does not fit usize".to_string())?,
            inline_bytes: u64::try_from(inline_bytes)
                .map_err(|_| "Delivered payload byte count must not be negative".to_string())?,
        })
    }

    pub fn compact_delivered_payloads(
        &self,
        limit: usize,
    ) -> Result<DeliveredPayloadCompactionReport, String> {
        if limit == 0 {
            return Ok(DeliveredPayloadCompactionReport::default());
        }
        let candidates = delivered_payload_compaction_candidates(self.conn, limit)?;
        let mut report = DeliveredPayloadCompactionReport {
            scanned_rows: candidates.len(),
            ..DeliveredPayloadCompactionReport::default()
        };
        for candidate in candidates {
            let original_len = candidate.payload_json.len() as u64;
            let published = self.retained_payload_for_compaction(&candidate)?;
            let compacted_json = compacted_payload_json(&candidate.kind, &published)?;
            let tx = Transaction::new_unchecked(self.conn, TransactionBehavior::Immediate)
                .map_err(|err| {
                    format!("Failed to start delivered payload compaction transaction: {err}")
                })?;
            verify_published_payload(&published)?;
            let changed = mark_payload_compacted(&tx, &candidate, &published, &compacted_json)?;
            tx.commit()
                .map_err(|err| format!("Failed to commit delivered payload compaction: {err}"))?;
            let delta = map_compaction_report_delta(
                original_len,
                &published,
                compacted_json.len(),
                changed,
            );
            merge_compaction_report(&mut report, delta);
        }
        Ok(report)
    }

    fn retained_payload_for_compaction(
        &self,
        candidate: &DeliveredPayloadCompactionCandidate,
    ) -> Result<PublishedMailboxPayload, String> {
        match candidate.published_payload()? {
            Some(payload) => {
                self.verify_published_payload(&payload)?;
                Ok(payload)
            }
            None => self.publish_immutable_payload(candidate.payload_json.as_bytes()),
        }
    }
}

impl MailboxDb {
    pub fn terminal_history_retention_stats(
        &self,
    ) -> Result<TerminalHistoryRetentionStats, String> {
        terminal_history_retention_stats_on(&self.conn, TERMINAL_HISTORY_KEEP_ROWS)
    }

    pub fn prune_terminal_history(
        &mut self,
        limit: usize,
    ) -> Result<TerminalHistoryPruneReport, String> {
        self.prune_terminal_history_with_keep(limit, TERMINAL_HISTORY_KEEP_ROWS)
    }

    fn prune_terminal_history_with_keep(
        &mut self,
        limit: usize,
        keep: usize,
    ) -> Result<TerminalHistoryPruneReport, String> {
        if limit == 0 {
            return Ok(TerminalHistoryPruneReport::default());
        }
        let limit = i64::try_from(limit)
            .map_err(|_| "Terminal history prune limit does not fit SQLite INTEGER".to_string())?;
        let keep = i64::try_from(keep)
            .map_err(|_| "Terminal history keep count does not fit SQLite INTEGER".to_string())?;
        // Candidate discovery can scan retained history. Keep it outside the write
        // transaction so maintenance cannot block unrelated mailbox writers.
        let attempt_ids = prunable_delivery_attempt_ids(&self.conn, keep, limit)?;
        let mailbox_rows = prunable_terminal_mailbox_rows(&self.conn, keep, limit)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|err| format!("Failed to start terminal history prune transaction: {err}"))?;
        let mut report = TerminalHistoryPruneReport::default();

        for attempt_id in &attempt_ids {
            let still_prunable: bool = tx
                .query_row(
                    "SELECT EXISTS (
                         SELECT 1 FROM mailbox_delivery_attempts
                         WHERE attempt_id = ?1
                           AND resolved_at IS NOT NULL
                           AND (
                               evidence_disposition IS NULL
                               OR evidence_disposition NOT IN ('pending', 'legacy_pending')
                               OR evidence_reconciled_at IS NOT NULL
                           )
                     )",
                    params![attempt_id],
                    |row| row.get(0),
                )
                .map_err(|err| format!("Failed to revalidate resolved delivery attempt: {err}"))?;
            if !still_prunable {
                continue;
            }
            report.delivery_attempt_items_deleted += tx
                .execute(
                    "DELETE FROM mailbox_delivery_attempt_items WHERE attempt_id = ?1",
                    params![attempt_id],
                )
                .map_err(|err| format!("Failed to prune delivery attempt items: {err}"))?;
            report.delivery_attempts_deleted += tx
                .execute(
                    "DELETE FROM mailbox_delivery_attempts
                     WHERE attempt_id = ?1 AND resolved_at IS NOT NULL",
                    params![attempt_id],
                )
                .map_err(|err| format!("Failed to prune resolved delivery attempt: {err}"))?;
        }

        for row in &mailbox_rows {
            let still_prunable: bool = tx
                .query_row(
                    "SELECT EXISTS (
                         SELECT 1
                         FROM mailbox AS candidate
                         WHERE candidate.seq = ?1
                           AND candidate.delivered_at IS NOT NULL
                           AND candidate.kind = ?2
                           AND NOT EXISTS (
                               SELECT 1
                               FROM completion_event_listener AS listener
                               WHERE listener.mailbox_seq = candidate.seq
                                 AND listener.acknowledged_at IS NULL
                           )
                           AND NOT EXISTS (
                               SELECT 1
                               FROM mailbox_delivery_attempt_items AS item
                               JOIN mailbox_delivery_attempts AS attempt
                                 ON attempt.attempt_id = item.attempt_id
                               WHERE item.mailbox_seq = candidate.seq
                                 AND attempt.resolved_at IS NULL
                           )
                     )",
                    params![row.seq, AGENT_BASH_COMPLETE_KIND],
                    |query_row| query_row.get(0),
                )
                .map_err(|err| format!("Failed to revalidate terminal mailbox row: {err}"))?;
            if !still_prunable {
                continue;
            }
            report.listeners_detached += tx
                .execute(
                    "UPDATE completion_event_listener
                     SET mailbox_seq = NULL
                     WHERE mailbox_seq = ?1 AND acknowledged_at IS NOT NULL",
                    params![row.seq],
                )
                .map_err(|err| format!("Failed to detach terminal completion listener: {err}"))?;
            report.delivery_attempt_items_deleted += tx
                .execute(
                    "DELETE FROM mailbox_delivery_attempt_items
                     WHERE mailbox_seq = ?1",
                    params![row.seq],
                )
                .map_err(|err| format!("Failed to detach terminal delivery history: {err}"))?;
            report.mailbox_rows_deleted += tx
                .execute(
                    "DELETE FROM mailbox WHERE seq = ?1 AND delivered_at IS NOT NULL",
                    params![row.seq],
                )
                .map_err(|err| format!("Failed to prune terminal mailbox row: {err}"))?;
        }
        tx.commit()
            .map_err(|err| format!("Failed to commit terminal history prune: {err}"))?;

        for row in mailbox_rows {
            if let Some(payload) = row.payload {
                merge_payload_reclaim_result(
                    &mut report,
                    self.reclaim_payload_if_terminal(&payload)?,
                );
            }
        }
        let completion_payloads = reclaimable_completion_payloads(&self.conn, limit)?;
        for payload in completion_payloads {
            merge_payload_reclaim_result(&mut report, self.reclaim_payload_if_terminal(&payload)?);
        }
        Ok(report)
    }

    pub fn vacuum_terminal_history(&mut self) -> Result<(), String> {
        truncate_terminal_history_wal(&self.conn, "before VACUUM")?;
        self.conn
            .execute_batch("VACUUM;")
            .map_err(|err| format!("Failed to reclaim PID mailbox sidecar pages: {err}"))?;
        truncate_terminal_history_wal(&self.conn, "after VACUUM")
    }

    fn reclaim_payload_if_terminal(
        &mut self,
        payload: &RetiredPayload,
    ) -> Result<PayloadReclaimResult, String> {
        let expected_path = self.payloads().payload_path_for_sha256(&payload.sha256)?;
        if payload.file_path != expected_path {
            return Err(format!(
                "Refusing to reclaim mailbox payload outside the content-addressed store: {}",
                payload.file_path.display()
            ));
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|err| format!("Failed to start mailbox payload reclaim transaction: {err}"))?;
        let live: bool = tx
            .query_row(
                "SELECT EXISTS (
                     SELECT 1 FROM mailbox WHERE payload_sha256 = ?1
                 ) OR EXISTS (
                     SELECT 1
                     FROM completion_event AS event
                     JOIN completion_event_listener AS listener
                       ON listener.event_id = event.event_id
                     WHERE event.payload_sha256 = ?1
                       AND listener.acknowledged_at IS NULL
                 )",
                params![&payload.sha256],
                |row| row.get(0),
            )
            .map_err(|err| format!("Failed to inspect mailbox payload references: {err}"))?;
        if live {
            tx.commit()
                .map_err(|err| format!("Failed to finish mailbox payload inspection: {err}"))?;
            return Ok(PayloadReclaimResult::default());
        }

        let reclaimed_bytes = match fs::metadata(&payload.file_path) {
            Ok(metadata) => {
                fs::remove_file(&payload.file_path).map_err(|err| {
                    format!(
                        "Failed to remove terminal mailbox payload {}: {err}",
                        payload.file_path.display()
                    )
                })?;
                metadata.len()
            }
            Err(error) if error.kind() == ErrorKind::NotFound => 0,
            Err(error) => {
                return Err(format!(
                    "Failed to inspect terminal mailbox payload {}: {error}",
                    payload.file_path.display()
                ));
            }
        };
        tx.execute(
            "UPDATE completion_event
             SET payload_reclaimed_at = COALESCE(payload_reclaimed_at, ?2)
             WHERE payload_sha256 = ?1
               AND state = 'triggered'
               AND NOT EXISTS (
                   SELECT 1 FROM completion_event_listener AS listener
                   WHERE listener.event_id = completion_event.event_id
                     AND listener.acknowledged_at IS NULL
               )",
            params![&payload.sha256, now_rfc3339()],
        )
        .map_err(|err| format!("Failed to record terminal payload reclamation: {err}"))?;
        tx.commit()
            .map_err(|err| format!("Failed to commit mailbox payload reclamation: {err}"))?;
        Ok(PayloadReclaimResult {
            files_deleted: usize::from(reclaimed_bytes > 0),
            bytes_reclaimed: reclaimed_bytes,
        })
    }

    fn maintain_terminal_history(&mut self) {
        let deadline = Instant::now() + TERMINAL_HISTORY_MAINTENANCE_TIMEOUT;
        if let Err(error) =
            self.conn.progress_handler(
                TERMINAL_HISTORY_MAINTENANCE_PROGRESS_OPS,
                Some(move || {
                    #[cfg(test)]
                    COUNT_COMPLETION_FINALIZATION_VM_STEPS.with(|enabled| {
                        if enabled.get() {
                            COMPLETION_FINALIZATION_VM_STEPS.with(|count| {
                                count.set(count.get().saturating_add(
                                    TERMINAL_HISTORY_MAINTENANCE_PROGRESS_OPS as usize,
                                ))
                            });
                        }
                    });
                    Instant::now() >= deadline
                }),
            )
        {
            tracing::warn!(error = %error, "failed to bound terminal mailbox maintenance");
            return;
        }
        if let Err(error) = self
            .conn
            .busy_timeout(TERMINAL_HISTORY_MAINTENANCE_BUSY_TIMEOUT)
        {
            let _ = self.conn.progress_handler(0, None::<fn() -> bool>);
            #[cfg(test)]
            install_completion_finalization_vm_counter(&self.conn);
            tracing::warn!(error = %error, "failed to bound terminal mailbox writer wait");
            return;
        }

        let result = self.prune_terminal_history(TERMINAL_HISTORY_MAINTENANCE_BATCH);
        let progress_reset = self.conn.progress_handler(0, None::<fn() -> bool>);
        let timeout_reset = self.conn.busy_timeout(mailbox_writer_sqlite_timeout());
        #[cfg(test)]
        install_completion_finalization_vm_counter(&self.conn);

        if let Err(error) = result {
            tracing::warn!(error = %error, "bounded terminal mailbox maintenance failed");
        }
        if let Err(error) = progress_reset {
            tracing::warn!(error = %error, "failed to clear terminal mailbox maintenance budget");
        }
        if let Err(error) = timeout_reset {
            tracing::warn!(error = %error, "failed to restore terminal mailbox writer wait");
        }
    }
}

fn truncate_terminal_history_wal(conn: &Connection, phase: &str) -> Result<(), String> {
    let (busy, log_frames, checkpointed_frames) = conn
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(|err| format!("Failed to checkpoint PID mailbox sidecar {phase}: {err}"))?;
    if busy == 0 {
        return Ok(());
    }
    Err(format!(
        "PID mailbox sidecar checkpoint remained busy {phase}: checkpointed {checkpointed_frames} of {log_frames} WAL frames"
    ))
}

impl MailboxDb {
    pub fn list_pending(&self, session_id: &str) -> Result<Vec<MailboxRow>, String> {
        self.list_pending_for_delivery(session_id, None)
    }

    pub fn list_pending_for_delivery(
        &self,
        session_id: &str,
        chain_id: Option<&str>,
    ) -> Result<Vec<MailboxRow>, String> {
        let query = format!(
            "SELECT {MAILBOX_ROW_COLUMNS}
             FROM mailbox
             WHERE delivered_at IS NULL
               AND {PENDING_MAILBOX_TARGET_PREDICATE}
             ORDER BY seq ASC"
        );
        let mut stmt = self
            .conn
            .prepare(&query)
            .map_err(|err| format!("Failed to prepare pending mailbox query: {err}"))?;
        let rows = stmt
            .query_map(params![session_id, chain_id], map_mailbox_row)
            .map_err(|err| format!("Failed to query pending mailbox rows: {err}"))?;
        collect_rows(rows)
    }

    pub fn list_pending_for_delivery_after(
        &self,
        session_id: &str,
        chain_id: Option<&str>,
        after_seq: i64,
        limit: usize,
    ) -> Result<Vec<MailboxRow>, String> {
        if session_id.is_empty() {
            return Err("Mailbox session id cannot be empty".to_string());
        }
        if after_seq < 0 {
            return Err("Mailbox cursor cannot be negative".to_string());
        }
        let limit = i64::try_from(limit)
            .ok()
            .filter(|limit| *limit > 0)
            .ok_or_else(|| "Mailbox batch limit must be positive".to_string())?;
        let query = bounded_pending_mailbox_query();
        let mut stmt = self
            .conn
            .prepare(&query)
            .map_err(|err| format!("Failed to prepare bounded pending mailbox query: {err}"))?;
        let rows = stmt
            .query_map(
                params![
                    session_id,
                    chain_id,
                    after_seq,
                    limit,
                    WAKE_SWEEP_ABANDONED_ERROR,
                    MAILBOX_PAYLOAD_VERIFICATION_FAILED_ERROR,
                    MAILBOX_INGRESS_EXPIRED_ERROR,
                ],
                map_mailbox_row,
            )
            .map_err(|err| format!("Failed to query bounded pending mailbox rows: {err}"))?;
        collect_rows(rows)
    }

    pub fn list_mailbox(&self, session_id: &str, all: bool) -> Result<Vec<MailboxRow>, String> {
        if all {
            self.list_mailbox_all(session_id)
        } else {
            self.list_pending(session_id)
        }
    }

    pub fn notifications_paused(&self, session_id: &str) -> Result<bool, String> {
        self.conn
            .query_row(
                "SELECT paused FROM mailbox_notification_control WHERE session_id = ?1",
                params![session_id],
                |row| row.get::<_, bool>(0),
            )
            .optional()
            .map(|paused| paused.unwrap_or(false))
            .map_err(|err| format!("Failed to query mailbox notification control: {err}"))
    }

    pub fn set_notifications_paused(
        &mut self,
        session_id: &str,
        paused: bool,
    ) -> Result<(), String> {
        let now = now_rfc3339();
        self.conn
            .execute(
                "INSERT INTO mailbox_notification_control (session_id, paused, updated_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(session_id) DO UPDATE SET
                    paused = excluded.paused,
                    updated_at = excluded.updated_at",
                params![session_id, paused, &now],
            )
            .map(|_| ())
            .map_err(|err| format!("Failed to update mailbox notification control: {err}"))
    }

    pub fn acknowledge_range(
        &mut self,
        session_id: &str,
        from_seq: i64,
        to_seq: i64,
        delivered_by: &str,
    ) -> Result<usize, String> {
        if from_seq > to_seq {
            return Err(format!(
                "Mailbox acknowledgement range is reversed: {from_seq} > {to_seq}"
            ));
        }
        let now = now_rfc3339();
        let tx = self.conn.transaction().map_err(|err| {
            format!("Failed to start mailbox range acknowledgement transaction: {err}")
        })?;
        let changed = tx
            .execute(
                "UPDATE mailbox
                 SET delivered_at = ?4,
                     delivered_by_invocation_uuid = ?5,
                     delivery_attempts = delivery_attempts + 1,
                     delivery_error = NULL
                 WHERE session_id = ?1
                   AND seq >= ?2
                   AND seq <= ?3
                   AND delivered_at IS NULL",
                params![session_id, from_seq, to_seq, &now, delivered_by],
            )
            .map_err(|err| format!("Failed to acknowledge mailbox range: {err}"))?;
        tx.execute(
            "UPDATE completion_event_listener
             SET acknowledged_at = COALESCE(acknowledged_at, ?4),
                 acknowledgement_reason = COALESCE(acknowledgement_reason, 'manual_ack')
             WHERE mailbox_seq IN (
                 SELECT seq FROM mailbox
                 WHERE session_id = ?1 AND seq >= ?2 AND seq <= ?3 AND delivered_at IS NOT NULL
             )",
            params![session_id, from_seq, to_seq, &now],
        )
        .map_err(|err| format!("Failed to acknowledge completion listeners in range: {err}"))?;
        resolve_completed_delivery_attempts(&tx, session_id, &now, None)?;
        tx.commit().map_err(|err| {
            format!("Failed to commit mailbox range acknowledgement transaction: {err}")
        })?;
        self.maintain_terminal_history();
        Ok(changed)
    }

    pub fn list_mailbox_bounded(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<MailboxRow>, String> {
        if bounded_mailbox_limit_is_zero(limit) {
            return Ok(Vec::new());
        }
        self.bounded_mailbox_rows(session_id, bounded_mailbox_sql_limit(limit))
            .map_err(format_bounded_mailbox_rows_error)
    }

    pub fn list_delivery_invocation_children(
        &self,
        owner_invocation_uuid: &str,
        limit: usize,
    ) -> Result<Vec<String>, String> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut stmt = self
            .conn
            .prepare(
                "SELECT delivered_by_invocation_uuid
                 FROM mailbox
                 WHERE owner_invocation_uuid = ?1
                   AND delivered_by_invocation_uuid IS NOT NULL
                   AND delivered_by_invocation_uuid != owner_invocation_uuid
                 GROUP BY delivered_by_invocation_uuid
                 ORDER BY MIN(seq), delivered_by_invocation_uuid
                 LIMIT ?2",
            )
            .map_err(|err| format!("Failed to prepare mailbox delivery-child query: {err}"))?;
        let rows = stmt
            .query_map(
                params![owner_invocation_uuid, bounded_mailbox_sql_limit(limit)],
                |row| row.get(0),
            )
            .map_err(|err| format!("Failed to query mailbox delivery children: {err}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|err| format!("Failed to map mailbox delivery children: {err}"))
    }

    fn bounded_mailbox_rows(
        &self,
        session_id: &str,
        limit: i64,
    ) -> Result<Vec<MailboxRow>, BoundedMailboxRowsError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT seq, session_id, kind, handle, payload_json, enqueued_at,
                        delivered_at, delivered_by_invocation_uuid, delivery_attempts,
                        delivery_error, owner_invocation_uuid, matched_os_pid,
                        matched_os_boot_id, matched_os_pid_starttime_ticks,
                        matched_chain_index, state_dir, meta_path, log_path, rc_path, rc,
                        payload_file_path, payload_sha256, payload_byte_len,
                        payload_retention_policy, payload_compacted_at,
                        submission_token, target_kind, target_id
                 FROM mailbox
                 WHERE session_id = ?1
                 ORDER BY CASE WHEN delivered_at IS NULL THEN 0 ELSE 1 END, seq DESC
                  LIMIT ?2",
            )
            .map_err(BoundedMailboxRowsError::Prepare)?;
        let rows = stmt
            .query_map(params![session_id, limit], map_mailbox_row)
            .map_err(BoundedMailboxRowsError::Query)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(BoundedMailboxRowsError::Row)
    }

    pub fn mark_delivered(
        &mut self,
        session_id: &str,
        chain_id: Option<&str>,
        seqs: &[i64],
        delivered_by_invocation_uuid: &str,
    ) -> Result<(), String> {
        if seqs.is_empty() {
            return Ok(());
        }
        let now = now_rfc3339();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|err| format!("Failed to start mailbox delivery transaction: {err}"))?;
        let delivery_states = mailbox_delivery_target_states_on(&tx, session_id, chain_id, seqs)
            .map_err(|err| format!("Failed to validate mailbox delivery batch: {err}"))?;
        if !all_mailbox_seqs_owned(&delivery_states) {
            return Err(
                "Mailbox delivery batch contains a missing or foreign-session row".to_string(),
            );
        }
        let update_sql = format!(
            "UPDATE mailbox
             SET delivered_at = ?4,
                 delivered_by_invocation_uuid = ?5,
                 delivery_attempts = delivery_attempts + 1,
                 delivery_error = NULL
             WHERE seq = ?3
               AND delivered_at IS NULL
               AND {PENDING_MAILBOX_TARGET_PREDICATE}"
        );
        for seq in seqs {
            tx.execute(
                &update_sql,
                params![
                    session_id,
                    chain_id,
                    seq,
                    &now,
                    delivered_by_invocation_uuid
                ],
            )
            .map_err(|err| format!("Failed to mark mailbox row delivered: {err}"))?;
        }
        acknowledge_completion_event_listeners_for_seqs(&tx, session_id, chain_id, seqs, &now)?;
        resolve_completed_delivery_attempts(&tx, session_id, &now, None)?;
        tx.commit()
            .map_err(|err| format!("Failed to commit mailbox delivery transaction: {err}"))?;
        self.maintain_terminal_history();
        Ok(())
    }

    pub fn register_delivery_attempt(
        &mut self,
        attempt_id: &str,
        session_id: &str,
        delivery_invocation_uuid: &str,
        seqs: &[i64],
        remaining_count: usize,
    ) -> Result<(), String> {
        self.register_delivery_attempt_for_target(
            attempt_id,
            session_id,
            None,
            delivery_invocation_uuid,
            seqs,
            remaining_count,
        )
    }

    pub fn register_headless_delivery_attempt(
        &mut self,
        attempt_id: &str,
        session_id: &str,
        chain_id: Option<&str>,
        delivery_invocation_uuid: &str,
        seqs: &[i64],
        remaining_count: usize,
    ) -> Result<(), String> {
        self.register_delivery_attempt_for_target(
            attempt_id,
            session_id,
            chain_id,
            delivery_invocation_uuid,
            seqs,
            remaining_count,
        )
    }

    fn register_delivery_attempt_for_target(
        &mut self,
        attempt_id: &str,
        session_id: &str,
        chain_id: Option<&str>,
        delivery_invocation_uuid: &str,
        seqs: &[i64],
        remaining_count: usize,
    ) -> Result<(), String> {
        if seqs.is_empty() {
            return Err("Cannot register an empty mailbox delivery attempt".to_string());
        }
        let now = now_rfc3339();
        let tx = self.conn.transaction().map_err(|err| {
            format!("Failed to start mailbox delivery attempt transaction: {err}")
        })?;
        tx.execute(
            "UPDATE mailbox_delivery_attempts
             SET resolved_at = ?3
             WHERE session_id = ?1
                AND delivery_invocation_uuid != ?2
                AND acknowledged_at IS NULL
                AND submission_started_at IS NULL
                AND resolved_at IS NULL",
            params![session_id, delivery_invocation_uuid, &now],
        )
        .map_err(|err| {
            format!("Failed to resolve prior unacknowledged mailbox deliveries: {err}")
        })?;
        tx.execute(
            "INSERT INTO mailbox_delivery_attempts (
                attempt_id, session_id, delivery_invocation_uuid, created_at,
                prepared_remaining_count
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                attempt_id,
                session_id,
                delivery_invocation_uuid,
                &now,
                remaining_count as i64
            ],
        )
        .map_err(|err| format!("Failed to insert mailbox delivery attempt: {err}"))?;
        let target_states = if chain_id.is_some() {
            mailbox_delivery_target_states_on(&tx, session_id, chain_id, seqs)
                .map_err(|err| format!("Failed to validate mailbox delivery item: {err}"))?
        } else {
            mailbox_delivery_states_on(&tx, session_id, seqs)
                .map_err(|err| format!("Failed to validate mailbox delivery item: {err}"))?
        };
        for (seq, target_state) in seqs.iter().zip(target_states) {
            if target_state.is_none() {
                return Err(format!(
                    "Mailbox delivery item {seq} does not belong to session {session_id}"
                ));
            }
            tx.execute(
                "INSERT INTO mailbox_delivery_attempt_items (attempt_id, mailbox_seq)
                 VALUES (?1, ?2)",
                params![attempt_id, seq],
            )
            .map_err(|err| format!("Failed to insert mailbox delivery attempt item: {err}"))?;
        }
        resolve_completed_delivery_attempts(&tx, session_id, &now, None)?;
        tx.commit()
            .map_err(|err| format!("Failed to commit mailbox delivery attempt: {err}"))?;
        self.maintain_terminal_history();
        Ok(())
    }

    pub fn bind_delivery_attempt_invocation(
        &mut self,
        attempt_id: &str,
        session_id: &str,
        delivery_invocation_uuid: &str,
    ) -> Result<(), String> {
        let changed = self
            .conn
            .execute(
                "UPDATE mailbox_delivery_attempts
                 SET delivery_invocation_uuid = ?3
                 WHERE attempt_id = ?1
                   AND session_id = ?2
                   AND resolved_at IS NULL",
                params![attempt_id, session_id, delivery_invocation_uuid],
            )
            .map_err(|err| format!("Failed to bind mailbox delivery invocation: {err}"))?;
        if changed == 0 {
            return Err(format!(
                "Mailbox delivery attempt {attempt_id} is missing, resolved, or belongs to another session"
            ));
        }
        Ok(())
    }

    pub fn record_delivery_observation_anchor(
        &self,
        attempt_id: &str,
        session_id: &str,
        anchor: &MailboxDeliveryObservationAnchor,
    ) -> Result<(), String> {
        validate_delivery_observation_anchor(session_id, anchor)?;
        let changed = self
            .conn
            .execute(
                "UPDATE mailbox_delivery_attempts
                 SET observation_provider_name = ?3,
                     observation_provider_instance_id = ?4,
                     observation_settings_id = ?5,
                     observation_session_id = ?6,
                     observation_anchor_token = ?7,
                     observation_expected_sha256 = ?8,
                     observation_error = NULL,
                     observation_confirmed_turn_id = NULL,
                     observation_confirmed_at = NULL
                  WHERE attempt_id = ?1 AND session_id = ?2
                    AND submission_started_at IS NULL AND resolved_at IS NULL
                    AND observation_anchor_token IS NULL",
                params![
                    attempt_id,
                    session_id,
                    anchor.provider_name,
                    anchor.provider_instance_id,
                    anchor.settings_id,
                    anchor.provider_session_id,
                    anchor.resume_token,
                    anchor.expected_sha256,
                ],
            )
            .map_err(|err| {
                format!("Failed to record mailbox delivery observation anchor: {err}")
            })?;
        if changed == 0 && self.delivery_observation_anchor(attempt_id)?.is_some() {
            return Ok(());
        }
        if changed != 1 {
            return Err(format!(
                "Mailbox delivery attempt {attempt_id} is unavailable for observation anchoring"
            ));
        }
        Ok(())
    }

    pub fn record_delivery_observation_anchor_failure(
        &self,
        attempt_id: &str,
        session_id: &str,
        error: &str,
    ) -> Result<(), String> {
        let error = truncate_utf8(error, 1024);
        let changed = self
            .conn
            .execute(
                "UPDATE mailbox_delivery_attempts
                 SET observation_provider_name = NULL,
                     observation_provider_instance_id = NULL,
                     observation_settings_id = NULL,
                     observation_session_id = NULL,
                     observation_anchor_token = NULL,
                     observation_expected_sha256 = NULL,
                     observation_error = ?3,
                     observation_confirmed_turn_id = NULL,
                     observation_confirmed_at = NULL
                  WHERE attempt_id = ?1 AND session_id = ?2
                    AND submission_started_at IS NULL AND resolved_at IS NULL
                    AND observation_anchor_token IS NULL",
                params![attempt_id, session_id, error],
            )
            .map_err(|err| {
                format!("Failed to record mailbox delivery observation anchor failure: {err}")
            })?;
        if changed == 0 && self.delivery_observation_anchor(attempt_id)?.is_some() {
            return Ok(());
        }
        if changed != 1 {
            return Err(format!(
                "Mailbox delivery attempt {attempt_id} is unavailable for observation anchoring"
            ));
        }
        Ok(())
    }

    pub fn delivery_observation_anchor(
        &self,
        attempt_id: &str,
    ) -> Result<Option<MailboxDeliveryObservationAnchor>, String> {
        self.conn
            .query_row(
                "SELECT observation_provider_name, observation_provider_instance_id,
                        observation_settings_id, observation_session_id,
                        observation_anchor_token, observation_expected_sha256
                 FROM mailbox_delivery_attempts
                 WHERE attempt_id = ?1 AND resolved_at IS NULL
                   AND observation_anchor_token IS NOT NULL",
                params![attempt_id],
                |row| {
                    Ok(MailboxDeliveryObservationAnchor {
                        provider_name: row.get(0)?,
                        provider_instance_id: row.get(1)?,
                        settings_id: row.get(2)?,
                        provider_session_id: row.get(3)?,
                        resume_token: row.get(4)?,
                        expected_sha256: row.get(5)?,
                    })
                },
            )
            .optional()
            .map_err(|err| format!("Failed to read mailbox delivery observation anchor: {err}"))
    }

    pub fn pending_delivery_observations(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<PendingMailboxDeliveryObservation>, String> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut stmt = self
            .conn
            .prepare(
                "SELECT attempts.attempt_id, attempts.observation_provider_name,
                        attempts.observation_provider_instance_id,
                        attempts.observation_settings_id, attempts.observation_session_id,
                        attempts.observation_anchor_token,
                        attempts.observation_expected_sha256
                 FROM mailbox_delivery_attempts AS attempts
                 WHERE attempts.session_id = ?1
                   AND attempts.resolved_at IS NULL
                   AND attempts.observation_anchor_token IS NOT NULL
                   AND attempts.observation_confirmed_at IS NULL
                   AND EXISTS (
                       SELECT 1
                       FROM mailbox_delivery_attempt_items AS items
                       JOIN mailbox ON mailbox.seq = items.mailbox_seq
                       WHERE items.attempt_id = attempts.attempt_id
                         AND mailbox.delivered_at IS NULL
                   )
                 ORDER BY attempts.created_at, attempts.attempt_id
                 LIMIT ?2",
            )
            .map_err(|err| {
                format!("Failed to prepare pending delivery observation query: {err}")
            })?;
        let rows = stmt
            .query_map(
                params![session_id, bounded_mailbox_sql_limit(limit)],
                |row| {
                    Ok(PendingMailboxDeliveryObservation {
                        attempt_id: row.get(0)?,
                        anchor: MailboxDeliveryObservationAnchor {
                            provider_name: row.get(1)?,
                            provider_instance_id: row.get(2)?,
                            settings_id: row.get(3)?,
                            provider_session_id: row.get(4)?,
                            resume_token: row.get(5)?,
                            expected_sha256: row.get(6)?,
                        },
                    })
                },
            )
            .map_err(|err| format!("Failed to query pending delivery observations: {err}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|err| format!("Failed to read pending delivery observation: {err}"))
    }

    pub fn record_delivery_observation_confirmation(
        &self,
        attempt_id: &str,
        turn_id: &str,
    ) -> Result<(), String> {
        if turn_id.is_empty() || turn_id.len() > 1024 {
            return Err("invalid mailbox delivery observation turn id".to_string());
        }
        let changed = self
            .conn
            .execute(
                "UPDATE mailbox_delivery_attempts
                 SET observation_confirmed_turn_id = ?2, observation_confirmed_at = ?3
                 WHERE attempt_id = ?1 AND resolved_at IS NULL
                   AND observation_anchor_token IS NOT NULL",
                params![attempt_id, turn_id, now_rfc3339()],
            )
            .map_err(|err| {
                format!("Failed to record mailbox delivery observation confirmation: {err}")
            })?;
        if changed != 1 {
            return Err(format!(
                "Mailbox delivery attempt {attempt_id} has no active observation anchor"
            ));
        }
        Ok(())
    }

    pub fn delivery_observation_confirmation(
        &self,
        attempt_id: &str,
    ) -> Result<Option<String>, String> {
        self.conn
            .query_row(
                "SELECT observation_confirmed_turn_id
                 FROM mailbox_delivery_attempts
                 WHERE attempt_id = ?1 AND observation_confirmed_at IS NOT NULL",
                params![attempt_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|err| {
                format!("Failed to read mailbox delivery observation confirmation: {err}")
            })
    }

    pub fn register_or_reuse_delivery_attempt(
        &mut self,
        attempt_id: &str,
        session_id: &str,
        delivery_invocation_uuid: &str,
        turn_generation_id: &str,
        seqs: &[i64],
        remaining_count: usize,
    ) -> Result<String, String> {
        if seqs.is_empty() {
            return Err("Cannot register an empty mailbox delivery attempt".to_string());
        }
        let now = now_rfc3339();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|err| format!("Failed to start mailbox delivery claim transaction: {err}"))?;
        resolve_completed_delivery_attempts(&tx, session_id, &now, None)?;
        let existing = tx
            .query_row(
                "SELECT attempt_id, evidence_turn_generation_id
                 FROM mailbox_delivery_attempts
                 WHERE session_id = ?1
                   AND delivery_invocation_uuid = ?2
                   AND resolved_at IS NULL
                 ORDER BY created_at, attempt_id
                 LIMIT 1",
                params![session_id, delivery_invocation_uuid],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .optional()
            .map_err(|err| format!("Failed to query reusable mailbox delivery attempt: {err}"))?;
        if let Some((existing, recorded_generation_id)) = existing {
            if let Some(recorded_generation_id) = recorded_generation_id
                && recorded_generation_id != turn_generation_id
            {
                return Err(format!(
                    "Mailbox delivery attempt {existing} belongs to State generation {recorded_generation_id}, not {turn_generation_id}"
                ));
            }
            tx.execute(
                "UPDATE mailbox_delivery_attempts
                 SET evidence_turn_generation_id = COALESCE(evidence_turn_generation_id, ?2)
                 WHERE attempt_id = ?1",
                params![&existing, turn_generation_id],
            )
            .map_err(|err| format!("Failed to bind reused mailbox delivery evidence: {err}"))?;
            tx.commit().map_err(|err| {
                format!("Failed to commit reused mailbox delivery attempt: {err}")
            })?;
            return Ok(existing);
        }
        let retained_submission = tx
            .query_row(
                "SELECT attempt_id
                 FROM mailbox_delivery_attempts
                 WHERE session_id = ?1
                   AND submission_started_at IS NOT NULL
                   AND resolved_at IS NULL
                 ORDER BY submission_started_at, created_at, attempt_id
                 LIMIT 1",
                params![session_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|err| format!("Failed to query retained mailbox submission: {err}"))?;
        if let Some(retained_submission) = retained_submission {
            return Err(format!(
                "mailbox_delivery_submission_uncertain:{retained_submission}"
            ));
        }
        tx.execute(
            "UPDATE mailbox_delivery_attempts
             SET resolved_at = ?3
             WHERE session_id = ?1
                AND delivery_invocation_uuid != ?2
                AND acknowledged_at IS NULL
                AND submission_started_at IS NULL
                AND resolved_at IS NULL",
            params![session_id, delivery_invocation_uuid, &now],
        )
        .map_err(|err| {
            format!("Failed to resolve prior unacknowledged mailbox deliveries: {err}")
        })?;
        tx.execute(
            "INSERT INTO mailbox_delivery_attempts (
                attempt_id, session_id, delivery_invocation_uuid, created_at,
                prepared_remaining_count, evidence_turn_generation_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                attempt_id,
                session_id,
                delivery_invocation_uuid,
                &now,
                remaining_count as i64,
                turn_generation_id,
            ],
        )
        .map_err(|err| format!("Failed to insert mailbox delivery attempt: {err}"))?;
        for seq in seqs {
            let belongs_to_session = tx
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM mailbox
                        WHERE seq = ?1 AND session_id = ?2
                     )",
                    params![seq, session_id],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(|err| format!("Failed to validate mailbox delivery item: {err}"))?;
            if !belongs_to_session {
                return Err(format!(
                    "Mailbox delivery item {seq} does not belong to session {session_id}"
                ));
            }
            tx.execute(
                "INSERT INTO mailbox_delivery_attempt_items (attempt_id, mailbox_seq)
                 VALUES (?1, ?2)",
                params![attempt_id, seq],
            )
            .map_err(|err| format!("Failed to insert mailbox delivery attempt item: {err}"))?;
        }
        tx.commit()
            .map_err(|err| format!("Failed to commit mailbox delivery claim: {err}"))?;
        Ok(attempt_id.to_string())
    }

    pub fn delivery_attempt_disposition(
        &self,
        attempt_id: &str,
    ) -> Result<Option<MailboxDeliveryAttemptDisposition>, String> {
        let Some((total, pending)) = self
            .conn
            .query_row(
                "SELECT COUNT(*),
                        SUM(CASE WHEN mailbox.delivered_at IS NULL THEN 1 ELSE 0 END)
                 FROM mailbox_delivery_attempts AS attempts
                 JOIN mailbox_delivery_attempt_items AS items
                   ON items.attempt_id = attempts.attempt_id
                 JOIN mailbox ON mailbox.seq = items.mailbox_seq
                 WHERE attempts.attempt_id = ?1
                 GROUP BY attempts.attempt_id",
                params![attempt_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()
            .map_err(|err| format!("Failed to query mailbox delivery attempt: {err}"))?
        else {
            return Ok(None);
        };
        let disposition = if pending == 0 {
            MailboxDeliveryAttemptDisposition::Resolved
        } else if pending == total {
            MailboxDeliveryAttemptDisposition::Pending
        } else {
            MailboxDeliveryAttemptDisposition::Stale
        };
        Ok(Some(disposition))
    }

    pub fn delivery_attempt_window(
        &self,
        attempt_id: &str,
    ) -> Result<Option<MailboxDeliveryWindow>, String> {
        let Some((
            session_id,
            delivery_invocation_uuid,
            acknowledged_at,
            submission_started_at,
            resolved_at,
        )) = self
            .conn
            .query_row(
                "SELECT session_id, delivery_invocation_uuid, acknowledged_at,
                        submission_started_at, resolved_at
                 FROM mailbox_delivery_attempts WHERE attempt_id = ?1",
                params![attempt_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(|err| format!("Failed to query mailbox delivery window owner: {err}"))?
        else {
            return Ok(None);
        };
        let mut stmt = self
            .conn
            .prepare(
                "SELECT mailbox.seq, mailbox.session_id, mailbox.kind, mailbox.handle,
                        mailbox.payload_json, mailbox.enqueued_at, mailbox.delivered_at,
                        mailbox.delivered_by_invocation_uuid, mailbox.delivery_attempts,
                        mailbox.delivery_error, mailbox.owner_invocation_uuid,
                        mailbox.matched_os_pid, mailbox.matched_os_boot_id,
                         mailbox.matched_os_pid_starttime_ticks, mailbox.matched_chain_index,
                         mailbox.state_dir, mailbox.meta_path, mailbox.log_path,
                         mailbox.rc_path, mailbox.rc, mailbox.payload_file_path,
                         mailbox.payload_sha256, mailbox.payload_byte_len,
                         mailbox.payload_retention_policy, mailbox.payload_compacted_at,
                         mailbox.submission_token, mailbox.target_kind, mailbox.target_id
                 FROM mailbox_delivery_attempt_items AS items
                 JOIN mailbox ON mailbox.seq = items.mailbox_seq
                 WHERE items.attempt_id = ?1 AND mailbox.delivered_at IS NULL
                 ORDER BY mailbox.seq",
            )
            .map_err(|err| format!("Failed to prepare mailbox delivery window query: {err}"))?;
        let rows = stmt
            .query_map(params![attempt_id], map_mailbox_row)
            .map_err(|err| format!("Failed to query mailbox delivery window: {err}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| format!("Failed to read mailbox delivery window: {err}"))?
            .into_iter()
            .filter(mailbox_row_is_deliverable_pending)
            .collect::<Vec<_>>();
        let pending_count = self
            .list_pending(&session_id)?
            .into_iter()
            .filter(mailbox_row_is_deliverable_pending)
            .count();
        Ok(Some(MailboxDeliveryWindow {
            attempt_id: attempt_id.to_string(),
            session_id,
            delivery_invocation_uuid,
            acknowledged_at,
            submission_started_at,
            resolved_at,
            remaining_count: pending_count.saturating_sub(rows.len()),
            rows,
        }))
    }

    pub fn unresolved_delivery_attempt_windows(
        &self,
        session_id: &str,
    ) -> Result<Vec<MailboxDeliveryWindow>, String> {
        let attempt_ids = {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT attempts.attempt_id
                     FROM mailbox_delivery_attempts AS attempts
                     WHERE attempts.session_id = ?1
                       AND attempts.resolved_at IS NULL
                       AND EXISTS (
                           SELECT 1
                           FROM mailbox_delivery_attempt_items AS items
                           JOIN mailbox ON mailbox.seq = items.mailbox_seq
                           WHERE items.attempt_id = attempts.attempt_id
                             AND mailbox.delivered_at IS NULL
                       )
                     ORDER BY attempts.created_at, attempts.attempt_id",
                )
                .map_err(|err| {
                    format!("Failed to prepare unresolved mailbox delivery query: {err}")
                })?;
            stmt.query_map(params![session_id], |row| row.get::<_, String>(0))
                .map_err(|err| format!("Failed to query unresolved mailbox deliveries: {err}"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|err| format!("Failed to read unresolved mailbox deliveries: {err}"))?
        };
        attempt_ids
            .into_iter()
            .map(|attempt_id| {
                self.delivery_attempt_window(&attempt_id)?
                    .ok_or_else(|| format!("Mailbox delivery attempt {attempt_id} disappeared"))
            })
            .collect()
    }

    pub fn delivery_attempt_item_seqs(&self, attempt_id: &str) -> Result<Vec<i64>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT mailbox_seq FROM mailbox_delivery_attempt_items
                 WHERE attempt_id = ?1 ORDER BY mailbox_seq",
            )
            .map_err(|err| format!("Failed to prepare mailbox delivery item query: {err}"))?;
        stmt.query_map(params![attempt_id], |row| row.get(0))
            .map_err(|err| format!("Failed to query mailbox delivery items: {err}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| format!("Failed to read mailbox delivery item: {err}"))
    }

    pub fn accepted_delivery_attempt_windows(
        &self,
        session_id: &str,
    ) -> Result<Vec<MailboxDeliveryWindow>, String> {
        let oldest_deliverable_seq = self
            .list_pending(session_id)?
            .into_iter()
            .find(mailbox_row_is_deliverable_pending)
            .map(|row| row.seq);
        let Some(oldest_deliverable_seq) = oldest_deliverable_seq else {
            return Ok(Vec::new());
        };
        let mut stmt = self
            .conn
            .prepare(
                "SELECT attempts.attempt_id
                 FROM mailbox_delivery_attempts AS attempts
                 WHERE attempts.session_id = ?1
                   AND attempts.acknowledged_at IS NOT NULL
                   AND attempts.resolved_at IS NULL
                    AND EXISTS (
                        SELECT 1
                        FROM mailbox_delivery_attempt_items AS prefix_items
                        WHERE prefix_items.attempt_id = attempts.attempt_id
                          AND prefix_items.mailbox_seq = ?2
                    )
                  ORDER BY attempts.acknowledged_at, attempts.created_at, attempts.attempt_id",
            )
            .map_err(|err| format!("Failed to prepare accepted delivery attempt query: {err}"))?;
        let attempt_ids = stmt
            .query_map(params![session_id, oldest_deliverable_seq], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|err| format!("Failed to query accepted delivery attempts: {err}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| format!("Failed to read accepted delivery attempts: {err}"))?;
        drop(stmt);
        attempt_ids
            .into_iter()
            .map(|attempt_id| {
                self.delivery_attempt_window(&attempt_id)?.ok_or_else(|| {
                    format!("Accepted mailbox delivery attempt {attempt_id} disappeared")
                })
            })
            .collect()
    }

    pub fn record_delivery_attempt_transport_ack(
        &mut self,
        attempt_id: &str,
    ) -> Result<bool, String> {
        let now = now_rfc3339();
        self.conn
            .execute(
                "UPDATE mailbox_delivery_attempts
                 SET submission_started_at = COALESCE(submission_started_at, ?2),
                     acknowledged_at = COALESCE(acknowledged_at, ?2)
                 WHERE attempt_id = ?1
                   AND resolved_at IS NULL",
                params![attempt_id, &now],
            )
            .map(|changed| changed > 0)
            .map_err(|err| format!("Failed to record mailbox delivery transport ACK: {err}"))
    }

    pub fn begin_delivery_attempt_submission(&mut self, attempt_id: &str) -> Result<bool, String> {
        let now = now_rfc3339();
        self.conn
            .execute(
                "UPDATE mailbox_delivery_attempts
                 SET submission_started_at = COALESCE(submission_started_at, ?2)
                 WHERE attempt_id = ?1
                   AND resolved_at IS NULL",
                params![attempt_id, &now],
            )
            .map(|changed| changed > 0)
            .map_err(|err| format!("Failed to start mailbox delivery submission: {err}"))
    }

    pub fn delivery_attempt_submission_started(&self, attempt_id: &str) -> Result<bool, String> {
        self.conn
            .query_row(
                "SELECT submission_started_at IS NOT NULL
                 FROM mailbox_delivery_attempts WHERE attempt_id = ?1",
                params![attempt_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|err| format!("Failed to inspect mailbox delivery submission: {err}"))?
            .ok_or_else(|| format!("Mailbox delivery attempt {attempt_id} is missing"))
    }

    pub fn resolve_unacknowledged_delivery_attempt(
        &mut self,
        attempt_id: &str,
    ) -> Result<bool, String> {
        let now = now_rfc3339();
        self.conn
            .execute(
                "UPDATE mailbox_delivery_attempts
                 SET resolved_at = ?2
                 WHERE attempt_id = ?1
                   AND acknowledged_at IS NULL
                   AND submission_started_at IS NULL
                   AND resolved_at IS NULL",
                params![attempt_id, &now],
            )
            .map(|changed| changed > 0)
            .map_err(|err| format!("Failed to resolve unacknowledged mailbox delivery: {err}"))
    }

    pub fn confirm_delivery_attempt(&mut self, attempt_id: &str) -> Result<bool, String> {
        let now = now_rfc3339();
        let observed_at = now_unix_millis()?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|err| {
                format!("Failed to start mailbox delivery confirmation transaction: {err}")
            })?;
        let Some((session_id, delivery_invocation_uuid)) = tx
            .query_row(
                "SELECT session_id, delivery_invocation_uuid
                 FROM mailbox_delivery_attempts
                 WHERE attempt_id = ?1
                   AND submission_started_at IS NOT NULL
                   AND (
                       evidence_disposition IS NULL
                       OR evidence_disposition IN (
                           'pending', 'legacy_pending', 'reconciled', 'legacy_reconciled'
                       )
                   )",
                params![attempt_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(|err| format!("Failed to query confirmed delivery attempt owner: {err}"))?
        else {
            return Ok(false);
        };
        tx.execute(
            "UPDATE mailbox_delivery_attempts
             SET acknowledged_at = COALESCE(acknowledged_at, ?2),
                 evidence_observed_at = COALESCE(evidence_observed_at, ?3),
                 evidence_disposition = COALESCE(evidence_disposition, 'pending')
             WHERE attempt_id = ?1",
            params![attempt_id, &now, observed_at],
        )
        .map_err(|err| format!("Failed to record mailbox delivery transport ACK: {err}"))?;
        tx.execute(
            "UPDATE mailbox
             SET delivered_at = ?3,
                 delivered_by_invocation_uuid = ?4,
                 delivery_attempts = delivery_attempts + 1,
                 delivery_error = NULL
             WHERE session_id = ?1
               AND delivered_at IS NULL
               AND seq IN (
                   SELECT mailbox_seq
                   FROM mailbox_delivery_attempt_items
                   WHERE attempt_id = ?2
               )",
            params![&session_id, attempt_id, &now, &delivery_invocation_uuid],
        )
        .map_err(|err| format!("Failed to confirm mailbox delivery items: {err}"))?;
        tx.execute(
            "UPDATE completion_event_listener
             SET acknowledged_at = COALESCE(acknowledged_at, ?2),
                 acknowledgement_reason = COALESCE(acknowledgement_reason, 'injected')
             WHERE mailbox_seq IN (
                 SELECT mailbox_seq FROM mailbox_delivery_attempt_items WHERE attempt_id = ?1
             )",
            params![attempt_id, &now],
        )
        .map_err(|err| format!("Failed to acknowledge injected completion listeners: {err}"))?;
        resolve_completed_delivery_attempts(&tx, &session_id, &now, Some(attempt_id))?;
        tx.commit()
            .map_err(|err| format!("Failed to commit mailbox delivery confirmation: {err}"))?;
        self.maintain_terminal_history();
        Ok(true)
    }

    pub fn pending_delivery_evidence_obligations(
        &self,
        session_id: &str,
    ) -> Result<Vec<MailboxDeliveryEvidenceObligation>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT attempt_id, session_id, evidence_turn_generation_id,
                        evidence_observed_at,
                        evidence_disposition = 'legacy_pending'
                 FROM mailbox_delivery_attempts
                 WHERE session_id = ?1
                   AND acknowledged_at IS NOT NULL
                   AND evidence_turn_generation_id IS NOT NULL
                   AND evidence_observed_at IS NOT NULL
                   AND evidence_disposition IN ('pending', 'legacy_pending')
                   AND evidence_reconciled_at IS NULL
                 ORDER BY acknowledged_at, created_at, attempt_id",
            )
            .map_err(|err| format!("Failed to prepare mailbox evidence obligations: {err}"))?;
        stmt.query_map(params![session_id], |row| {
            Ok(MailboxDeliveryEvidenceObligation {
                attempt_id: row.get(0)?,
                session_id: row.get(1)?,
                turn_generation_id: row.get(2)?,
                observed_at: row.get(3)?,
                legacy: row.get(4)?,
            })
        })
        .map_err(|err| format!("Failed to query mailbox evidence obligations: {err}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("Failed to read mailbox evidence obligation: {err}"))
    }

    pub fn pending_delivery_evidence_obligation_session_ids(
        &self,
        limit: usize,
    ) -> Result<Vec<String>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT DISTINCT session_id
                 FROM mailbox_delivery_attempts
                 WHERE acknowledged_at IS NOT NULL
                   AND evidence_turn_generation_id IS NOT NULL
                   AND evidence_observed_at IS NOT NULL
                   AND evidence_disposition IN ('pending', 'legacy_pending')
                   AND evidence_reconciled_at IS NULL
                 ORDER BY session_id
                 LIMIT ?1",
            )
            .map_err(|err| format!("Failed to prepare mailbox evidence session query: {err}"))?;
        stmt.query_map(params![limit as i64], |row| row.get(0))
            .map_err(|err| format!("Failed to query mailbox evidence sessions: {err}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| format!("Failed to read mailbox evidence session: {err}"))
    }

    pub fn delivery_evidence_obligation(
        &self,
        attempt_id: &str,
    ) -> Result<Option<MailboxDeliveryEvidenceObligation>, String> {
        self.conn
            .query_row(
                "SELECT attempt_id, session_id, evidence_turn_generation_id,
                        evidence_observed_at,
                        evidence_disposition IN ('legacy_pending', 'legacy_reconciled')
                 FROM mailbox_delivery_attempts
                 WHERE attempt_id = ?1
                   AND acknowledged_at IS NOT NULL
                   AND evidence_turn_generation_id IS NOT NULL
                   AND evidence_observed_at IS NOT NULL",
                params![attempt_id],
                |row| {
                    Ok(MailboxDeliveryEvidenceObligation {
                        attempt_id: row.get(0)?,
                        session_id: row.get(1)?,
                        turn_generation_id: row.get(2)?,
                        observed_at: row.get(3)?,
                        legacy: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(|err| format!("Failed to read mailbox evidence obligation: {err}"))
    }

    pub fn mark_delivery_evidence_reconciled(&mut self, attempt_id: &str) -> Result<bool, String> {
        let now = now_rfc3339();
        self.conn
            .execute(
                "UPDATE mailbox_delivery_attempts
                 SET evidence_reconciled_at = COALESCE(evidence_reconciled_at, ?2),
                     evidence_disposition = CASE evidence_disposition
                         WHEN 'legacy_pending' THEN 'legacy_reconciled'
                         ELSE 'reconciled'
                       END
                 WHERE attempt_id = ?1
                   AND acknowledged_at IS NOT NULL
                   AND evidence_observed_at IS NOT NULL
                   AND evidence_reconciled_at IS NULL",
                params![attempt_id, &now],
            )
            .map(|changed| changed > 0)
            .map_err(|err| format!("Failed to clear mailbox evidence obligation: {err}"))
    }

    pub fn adopt_legacy_delivery_evidence_observed_at(
        &mut self,
        attempt_id: &str,
        observed_at: i64,
    ) -> Result<bool, String> {
        self.conn
            .execute(
                "UPDATE mailbox_delivery_attempts
                 SET evidence_observed_at = ?2
                 WHERE attempt_id = ?1
                   AND evidence_disposition = 'legacy_pending'
                   AND evidence_reconciled_at IS NULL",
                params![attempt_id, observed_at],
            )
            .map(|changed| changed > 0)
            .map_err(|err| format!("Failed to adopt legacy mailbox evidence timestamp: {err}"))
    }

    pub fn fail_unobserved_delivery_attempt(
        &mut self,
        attempt_id: &str,
        delivery_error: &str,
    ) -> Result<bool, String> {
        reject_unauthorized_terminal_wake_abandonment(delivery_error)?;
        let now = now_rfc3339();
        let tx = self.conn.transaction().map_err(|err| {
            format!("Failed to start unobserved mailbox delivery transaction: {err}")
        })?;
        let session_id = tx
            .query_row(
                "SELECT session_id
                 FROM mailbox_delivery_attempts
                 WHERE attempt_id = ?1
                   AND acknowledged_at IS NOT NULL
                   AND resolved_at IS NULL",
                params![attempt_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|err| format!("Failed to query unobserved mailbox delivery: {err}"))?;
        let Some(session_id) = session_id else {
            return Ok(false);
        };
        tx.execute(
            "UPDATE mailbox
             SET delivery_attempts = delivery_attempts + 1,
                 delivery_error = ?3
             WHERE session_id = ?1
               AND delivered_at IS NULL
               AND seq IN (
                   SELECT mailbox_seq
                   FROM mailbox_delivery_attempt_items
                   WHERE attempt_id = ?2
               )",
            params![&session_id, attempt_id, delivery_error],
        )
        .map_err(|err| format!("Failed to mark unobserved mailbox delivery rows: {err}"))?;
        tx.execute(
            "UPDATE mailbox_delivery_attempts
             SET resolved_at = ?2
             WHERE attempt_id = ?1 AND resolved_at IS NULL",
            params![attempt_id, &now],
        )
        .map_err(|err| format!("Failed to resolve unobserved mailbox delivery: {err}"))?;
        tx.commit()
            .map_err(|err| format!("Failed to commit unobserved mailbox delivery: {err}"))?;
        Ok(true)
    }

    pub fn mark_delivery_failed(
        &mut self,
        session_id: &str,
        chain_id: Option<&str>,
        seqs: &[i64],
        delivery_error: &str,
    ) -> Result<(), String> {
        reject_unauthorized_terminal_wake_abandonment(delivery_error)?;
        if seqs.is_empty() {
            return Ok(());
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|err| {
                format!("Failed to start mailbox delivery failure transaction: {err}")
            })?;
        let delivery_states = mailbox_delivery_target_states_on(&tx, session_id, chain_id, seqs)
            .map_err(|err| format!("Failed to validate mailbox delivery failure batch: {err}"))?;
        if !all_mailbox_seqs_pending(&delivery_states) {
            return Err(
                "Mailbox delivery failure batch contains a missing, settled, or foreign-target row"
                    .to_string(),
            );
        }
        let update_sql = format!(
            "UPDATE mailbox
             SET delivery_attempts = delivery_attempts + 1,
                 delivery_error = ?4
             WHERE seq = ?3
               AND delivered_at IS NULL
               AND {PENDING_MAILBOX_TARGET_PREDICATE}"
        );
        for seq in seqs {
            tx.execute(
                &update_sql,
                params![session_id, chain_id, seq, delivery_error],
            )
            .map_err(|err| format!("Failed to mark mailbox row delivery failed: {err}"))?;
        }
        tx.commit()
            .map_err(|err| format!("Failed to commit mailbox delivery failure transaction: {err}"))
    }

    #[cfg(test)]
    fn force_pending_abandoned_for_test(
        &mut self,
        session_id: &str,
        limit: usize,
    ) -> Result<usize, String> {
        if limit == 0 {
            return Ok(0);
        }
        let tx = self
            .conn
            .transaction()
            .map_err(|err| format!("Failed to start mailbox abandonment transaction: {err}"))?;
        let changed = tx
            .execute(
                "UPDATE mailbox
                 SET delivery_error = ?2
                 WHERE seq IN (
                    SELECT seq
                    FROM mailbox
                     WHERE session_id = ?1
                       AND delivered_at IS NULL
                       AND (delivery_error IS NULL OR delivery_error != ?2)
                    ORDER BY seq ASC
                    LIMIT ?3
                 )",
                params![session_id, WAKE_SWEEP_ABANDONED_ERROR, limit as i64],
            )
            .map_err(|err| format!("Failed to mark mailbox rows abandoned: {err}"))?;
        if changed > 0 {
            tx.execute(
                "DELETE FROM session_wake_claim WHERE session_id = ?1",
                params![session_id],
            )
            .map_err(|err| format!("Failed to release abandoned wake claim: {err}"))?;
        }
        tx.commit()
            .map_err(|err| format!("Failed to commit mailbox abandonment transaction: {err}"))?;
        Ok(changed)
    }
}

impl WakeSessionRepository<'_> {
    pub fn upsert_session_metadata(
        &mut self,
        input: SessionMetadataUpsert<'_>,
    ) -> Result<(), String> {
        let now = now_rfc3339();
        self.conn
            .execute(
                "INSERT INTO session_runtime (
                    session_id,
                    mode,
                    invocation_uuid,
                    provider_name,
                    model_name,
                    updated_at,
                    models_dir,
                    effective_cwd
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(session_id)
                 DO UPDATE SET
                    mode = excluded.mode,
                    invocation_uuid = COALESCE(excluded.invocation_uuid, session_runtime.invocation_uuid),
                    provider_name = excluded.provider_name,
                    model_name = excluded.model_name,
                    updated_at = excluded.updated_at,
                    models_dir = COALESCE(excluded.models_dir, session_runtime.models_dir),
                    effective_cwd = COALESCE(excluded.effective_cwd, session_runtime.effective_cwd)",
                params![
                    input.session_id,
                    input.mode,
                    input.invocation_uuid,
                    input.provider_name,
                    input.model_name,
                    &now,
                    input.models_dir,
                    input.effective_cwd,
                ],
            )
            .map_err(|err| format!("Failed to upsert session runtime row: {err}"))?;
        Ok(())
    }

    /// Installs a compatibility projection for a pre-generation runtime.
    /// New runtime producers must use `RuntimeLifecycleRepository` instead.
    pub fn project_legacy_runtime_running(
        &mut self,
        input: LegacyRuntimeProjection<'_>,
    ) -> Result<(), String> {
        validate_running_run_state()?;
        let now = now_rfc3339();
        let turn_start_max_mailbox_seq = self.running_turn_start_max_mailbox_seq(&input)?;
        project_runtime_compatibility_row(self.conn, input, &now, turn_start_max_mailbox_seq)
    }

    /// Settles a retained compatibility projection. Generation-owned callers
    /// settle it inside the same lifecycle transaction.
    pub fn settle_legacy_runtime_projection(
        &mut self,
        input: LegacyRuntimeProjectionSettlement<'_>,
    ) -> Result<bool, String> {
        validate_idle_run_state()?;
        let now = now_rfc3339();
        settle_runtime_compatibility_row(self.conn, input, &now)
    }
}

impl SessionAdmissionRepository<'_> {
    pub fn enqueue(
        &mut self,
        admission_id: &str,
        registration_identity: &str,
        session_id: Option<&str>,
        launcher: &ProcessIdentity,
        now_unix_ms: i64,
    ) -> Result<SessionAdmissionRow, String> {
        validate_session_admission_identity(admission_id, "admission_id")?;
        validate_session_admission_identity(registration_identity, "registration_identity")?;
        validate_optional_session_admission_identity(session_id, "session_id")?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|err| format!("Failed to start session admission enqueue: {err}"))?;
        tx.execute(
            "INSERT OR IGNORE INTO session_admission_queue (
                admission_id, registration_identity, session_id, state, queue_reason,
                launcher_os_pid, launcher_os_boot_id,
                launcher_os_pid_starttime_ticks,
                created_at_unix_ms, updated_at_unix_ms
             ) VALUES (?1, ?2, ?3, 'queued', 'fifo_wait', ?4, ?5, ?6, ?7, ?7)",
            params![
                admission_id,
                registration_identity,
                session_id,
                launcher.os_pid,
                &launcher.os_boot_id,
                launcher.os_pid_starttime_ticks,
                now_unix_ms,
            ],
        )
        .map_err(|err| format!("Failed to enqueue session admission: {err}"))?;
        let existing = session_admission_by_registration_on(&tx, registration_identity)?
            .ok_or_else(|| "Session admission row is missing after enqueue".to_string())?;
        if let (Some(existing_session), Some(requested_session)) =
            (existing.session_id.as_deref(), session_id)
            && existing_session != requested_session
        {
            return Err(format!(
                "Session admission {} is already bound to session {existing_session}, not {requested_session}",
                existing.admission_id
            ));
        }
        if existing.session_id.is_none() && session_id.is_some() {
            tx.execute(
                "UPDATE session_admission_queue
                 SET session_id = ?2, updated_at_unix_ms = ?3
                 WHERE registration_identity = ?1 AND session_id IS NULL",
                params![registration_identity, session_id, now_unix_ms],
            )
            .map_err(|err| format!("Failed to bind queued session admission: {err}"))?;
        }
        let row = session_admission_by_registration_on(&tx, registration_identity)?
            .ok_or_else(|| "Session admission row disappeared during enqueue".to_string())?;
        tx.commit()
            .map_err(|err| format!("Failed to commit session admission enqueue: {err}"))?;
        Ok(row)
    }

    pub fn row(&self, registration_identity: &str) -> Result<Option<SessionAdmissionRow>, String> {
        session_admission_by_registration_on(self.conn, registration_identity)
    }

    pub fn update_queued_reason(
        &mut self,
        registration_identity: &str,
        reason: &str,
        now_unix_ms: i64,
    ) -> Result<bool, String> {
        validate_session_admission_identity(reason, "queue_reason")?;
        self.conn
            .execute(
                "UPDATE session_admission_queue
                 SET queue_reason = ?2, updated_at_unix_ms = ?3
                 WHERE registration_identity = ?1 AND state = 'queued'",
                params![registration_identity, reason, now_unix_ms],
            )
            .map(|changed| changed == 1)
            .map_err(|err| format!("Failed to update session admission queue reason: {err}"))
    }

    pub fn cancel_queued(
        &mut self,
        registration_identity: &str,
        admission_id: &str,
        reason: &str,
        now_unix_ms: i64,
    ) -> Result<bool, String> {
        validate_session_admission_identity(registration_identity, "registration_identity")?;
        validate_session_admission_identity(admission_id, "admission_id")?;
        validate_session_admission_identity(reason, "queue_reason")?;
        self.conn
            .execute(
                "UPDATE session_admission_queue
                 SET state = 'cancelled', queue_reason = ?3, updated_at_unix_ms = ?4
                 WHERE registration_identity = ?1
                   AND admission_id = ?2
                   AND state = 'queued'",
                params![registration_identity, admission_id, reason, now_unix_ms],
            )
            .map(|changed| changed == 1)
            .map_err(|err| format!("Failed to cancel exact queued session admission: {err}"))
    }

    pub fn try_admit_next(
        &mut self,
        claim_token: &str,
        now_unix_ms: i64,
        stale_before_unix_ms: i64,
    ) -> Result<SessionAdmissionAttempt, String> {
        self.try_admit(claim_token, now_unix_ms, stale_before_unix_ms, None)
    }

    pub fn try_admit_registration(
        &mut self,
        registration_identity: &str,
        claim_token: &str,
        now_unix_ms: i64,
        stale_before_unix_ms: i64,
    ) -> Result<SessionAdmissionAttempt, String> {
        validate_session_admission_identity(registration_identity, "registration_identity")?;
        self.try_admit(
            claim_token,
            now_unix_ms,
            stale_before_unix_ms,
            Some(registration_identity),
        )
    }

    fn try_admit(
        &mut self,
        claim_token: &str,
        now_unix_ms: i64,
        stale_before_unix_ms: i64,
        requested_registration_identity: Option<&str>,
    ) -> Result<SessionAdmissionAttempt, String> {
        validate_session_admission_identity(claim_token, "claim_token")?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|err| format!("Failed to start session admission drain: {err}"))?;
        if cancel_dead_session_admission_head_on(&tx, now_unix_ms)? {
            tx.commit()
                .map_err(|err| format!("Failed to commit dead admission cancellation: {err}"))?;
            return Ok(SessionAdmissionAttempt::Waiting);
        }
        reconcile_dead_starting_generations_on(&tx, now_unix_ms)?;
        recover_stale_session_admissions_on(&tx, stale_before_unix_ms, now_unix_ms)?;
        if unmaterialized_session_admission_exists_on(&tx)? {
            tx.commit()
                .map_err(|err| format!("Failed to commit materializing admission drain: {err}"))?;
            return Ok(SessionAdmissionAttempt::LaunchMaterializing);
        }
        let next = next_session_admission_on(&tx)?;
        let Some(registration_identity) = next else {
            tx.commit()
                .map_err(|err| format!("Failed to commit empty admission drain: {err}"))?;
            return Ok(SessionAdmissionAttempt::Empty);
        };
        if requested_registration_identity
            .is_some_and(|requested| requested != registration_identity)
        {
            tx.commit()
                .map_err(|err| format!("Failed to commit waiting admission drain: {err}"))?;
            return Ok(SessionAdmissionAttempt::Waiting);
        };
        let changed = tx
            .execute(
                "UPDATE session_admission_queue
                 SET state = 'admitted', queue_reason = 'admission_claimed', claim_token = ?2,
                     claimed_at_unix_ms = ?3, updated_at_unix_ms = ?3
                 WHERE registration_identity = ?1 AND state = 'queued'",
                params![&registration_identity, claim_token, now_unix_ms],
            )
            .map_err(|err| format!("Failed to reserve session admission: {err}"))?;
        if changed != 1 {
            return Err("Session admission changed during serialized drain".to_string());
        }
        let row = session_admission_by_registration_on(&tx, &registration_identity)?
            .ok_or_else(|| "Admitted session row disappeared".to_string())?;
        tx.commit()
            .map_err(|err| format!("Failed to commit session admission: {err}"))?;
        Ok(SessionAdmissionAttempt::Admitted(Box::new(row)))
    }

    pub fn begin_launch(
        &mut self,
        registration_identity: &str,
        claim_token: &str,
        now_unix_ms: i64,
    ) -> Result<bool, String> {
        self.conn
            .execute(
                "UPDATE session_admission_queue
                 SET state = 'launching', queue_reason = 'launching', updated_at_unix_ms = ?3
                 WHERE registration_identity = ?1
                   AND claim_token = ?2
                   AND state = 'admitted'",
                params![registration_identity, claim_token, now_unix_ms],
            )
            .map(|changed| changed == 1)
            .map_err(|err| format!("Failed to begin exact session admission launch: {err}"))
    }

    pub fn bind_session(
        &mut self,
        registration_identity: &str,
        claim_token: &str,
        session_id: &str,
        now_unix_ms: i64,
    ) -> Result<bool, String> {
        validate_session_admission_identity(session_id, "session_id")?;
        self.conn
            .execute(
                "UPDATE session_admission_queue
                 SET session_id = ?3, updated_at_unix_ms = ?4
                 WHERE registration_identity = ?1
                   AND claim_token = ?2
                   AND (session_id IS NULL OR session_id = ?3)",
                params![registration_identity, claim_token, session_id, now_unix_ms],
            )
            .map(|changed| changed == 1)
            .map_err(|err| format!("Failed to bind session admission identity: {err}"))
    }

    pub fn settle(
        &mut self,
        registration_identity: &str,
        claim_token: &str,
    ) -> Result<bool, String> {
        let now_unix_ms = now_unix_millis()?;
        self.conn
            .execute(
                "UPDATE session_admission_queue
                 SET state = 'settled', queue_reason = 'settled', updated_at_unix_ms = ?3
                 WHERE registration_identity = ?1
                   AND claim_token = ?2
                   AND state IN ('admitted', 'launching')",
                params![registration_identity, claim_token, now_unix_ms],
            )
            .map(|changed| changed == 1)
            .map_err(|err| format!("Failed to settle exact session admission: {err}"))
    }
}

impl WakeSessionReader<'_> {
    pub fn session_metadata(&self, session_id: &str) -> Result<Option<SessionMetadataRow>, String> {
        session_metadata_row(self.conn, session_id)
    }

    pub fn legacy_runtime_projection(
        &self,
        session_id: &str,
    ) -> Result<Option<LegacyRuntimeProjectionRow>, String> {
        let row = legacy_runtime_projection_row(self.conn, session_id)?;
        validate_legacy_runtime_projection(row.as_ref())?;
        Ok(row)
    }

    pub fn wake_claim(&self, session_id: &str) -> Result<Option<WakeClaimRow>, String> {
        wake_claim(self.conn, session_id)
    }
}

impl WakeSessionRepository<'_> {
    pub fn try_acquire_wake_claim(
        &mut self,
        input: WakeClaimRequest<'_>,
    ) -> Result<WakeClaimAcquireResult, String> {
        self.try_acquire_or_renew_wake_claim(input, None)
    }

    pub fn try_acquire_or_renew_wake_claim(
        &mut self,
        input: WakeClaimRequest<'_>,
        renew_token: Option<&str>,
    ) -> Result<WakeClaimAcquireResult, String> {
        self.try_acquire_startable_wake_claim(input, renew_token)
    }

    pub fn try_acquire_startable_wake_claim(
        &mut self,
        input: WakeClaimRequest<'_>,
        renew_token: Option<&str>,
    ) -> Result<WakeClaimAcquireResult, String> {
        let now = now_rfc3339();
        let tx = begin_wake_claim_transaction(self.conn)?;
        if wake_claim_runtime_is_busy_tx(&tx, input.session_id)? {
            commit_empty_wake_claim_transaction(tx)?;
            return Ok(WakeClaimAcquireResult::Busy);
        }
        if wake_claim_notifications_paused_tx(&tx, input.session_id)? {
            commit_empty_wake_claim_transaction(tx)?;
            return Ok(WakeClaimAcquireResult::NoPending);
        }
        let pending_bounds = pending_seq_bounds_for_claim_tx(&tx, input.session_id)?;
        let Some((min_seq, max_seq)) = pending_bounds else {
            commit_empty_wake_claim_transaction(tx)?;
            return Ok(WakeClaimAcquireResult::NoPending);
        };
        if let Some(existing) = fresh_in_flight_wake_claim_for_input(&tx, input, renew_token)? {
            commit_existing_wake_claim_transaction(tx)?;
            return Ok(WakeClaimAcquireResult::AlreadyInFlight(existing));
        }
        let claim = acquire_wake_claim_tx(&tx, input, &now, min_seq, max_seq)?;
        commit_wake_claim_transaction(tx)?;
        Ok(WakeClaimAcquireResult::Acquired(claim))
    }
}

impl WakeSessionRepository<'_> {
    pub fn release_wake_claim(
        &mut self,
        session_id: &str,
        claim_token: &str,
    ) -> Result<bool, String> {
        let changed = self
            .conn
            .execute(
                "DELETE FROM session_wake_claim
                 WHERE session_id = ?1
                   AND claim_token = ?2
                   AND wake_invocation_uuid IS NULL",
                params![session_id, claim_token],
            )
            .map_err(|err| format!("Failed to release wake claim: {err}"))?;
        Ok(changed > 0)
    }

    pub fn release_wake_claim_for_manual_resume(
        &mut self,
        session_id: &str,
        claim_token: &str,
    ) -> Result<bool, String> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|err| format!("Failed to start manual wake-claim release: {err}"))?;
        let Some(claim) = wake_claim_tx(&tx, session_id)? else {
            tx.commit()
                .map_err(|err| format!("Failed to commit missing manual wake claim: {err}"))?;
            return Ok(false);
        };
        if claim.claim_token != claim_token
            || !wake_claim_is_releasable_for_manual_resume(&tx, &claim)?
        {
            tx.commit()
                .map_err(|err| format!("Failed to commit retained manual wake claim: {err}"))?;
            return Ok(false);
        }
        let changed = tx
            .execute(
                "DELETE FROM session_wake_claim
                 WHERE session_id = ?1
                   AND claim_token = ?2",
                params![session_id, claim_token],
            )
            .map_err(|err| format!("Failed to release manual wake claim: {err}"))?;
        if changed != 1 {
            return Err("Manual wake claim changed during exact-token release".to_string());
        }
        tx.commit()
            .map_err(|err| format!("Failed to commit manual wake-claim release: {err}"))?;
        Ok(true)
    }

    pub fn release_admitted_wake_claim(
        &mut self,
        session_id: &str,
        claim_token: &str,
    ) -> Result<bool, String> {
        let changed = self
            .conn
            .execute(
                "DELETE FROM session_wake_claim
                 WHERE session_id = ?1
                   AND claim_token = ?2
                   AND wake_invocation_uuid IS NOT NULL",
                params![session_id, claim_token],
            )
            .map_err(|err| format!("Failed to release admitted wake claim: {err}"))?;
        Ok(changed > 0)
    }

    pub fn record_wake_claim_pid(
        &mut self,
        session_id: &str,
        claim_token: &str,
        wake_pid: i64,
    ) -> Result<bool, String> {
        let changed = self
            .conn
            .execute(
                "UPDATE session_wake_claim
                 SET wake_pid = ?3,
                     wake_os_boot_id = NULL,
                     wake_os_pid_starttime_ticks = NULL
                 WHERE session_id = ?1
                   AND claim_token = ?2",
                params![session_id, claim_token, wake_pid],
            )
            .map_err(|err| format!("Failed to record wake claim PID: {err}"))?;
        Ok(changed > 0)
    }

    pub fn record_wake_claim_pid_identity(
        &mut self,
        session_id: &str,
        claim_token: &str,
        wake_pid: i64,
    ) -> Result<bool, String> {
        let identity = pid_identity::read_live_process_identity(wake_pid)?;
        let changed = match identity {
            Some(identity) => self.conn.execute(
                "UPDATE session_wake_claim
                 SET wake_pid = ?3,
                     wake_os_boot_id = ?4,
                     wake_os_pid_starttime_ticks = ?5
                 WHERE session_id = ?1
                   AND claim_token = ?2",
                params![
                    session_id,
                    claim_token,
                    wake_pid,
                    &identity.os_boot_id,
                    identity.os_pid_starttime_ticks,
                ],
            ),
            None => self.conn.execute(
                "UPDATE session_wake_claim
                 SET wake_pid = ?3
                 WHERE session_id = ?1
                   AND claim_token = ?2
                   AND wake_pid IS NULL",
                params![session_id, claim_token, wake_pid],
            ),
        }
        .map_err(|err| format!("Failed to record wake claim process identity: {err}"))?;
        Ok(changed > 0)
    }

    pub fn wake_sweep_candidates(
        &mut self,
        stale_after_seconds: i64,
        limit: usize,
    ) -> Result<Vec<WakeSweepCandidate>, String> {
        let session_ids = self.pending_wake_session_ids_for_sweep(limit)?;
        self.wake_sweep_candidates_for_sessions(stale_after_seconds, limit, session_ids)
    }

    pub fn pending_delivery_session_ids(&self, limit: usize) -> Result<Vec<String>, String> {
        self.pending_wake_session_ids(limit)
    }

    fn wake_sweep_candidates_for_sessions(
        &mut self,
        stale_after_seconds: i64,
        limit: usize,
        session_ids: Vec<String>,
    ) -> Result<Vec<WakeSweepCandidate>, String> {
        let mut candidates = Vec::new();
        for session_id in session_ids {
            if wake_sweep_candidates_at_limit(&candidates, limit) {
                break;
            }
            self.push_wake_sweep_candidate_for_session(
                &mut candidates,
                session_id,
                stale_after_seconds,
            )?;
        }
        Ok(candidates)
    }

    fn push_wake_sweep_candidate_for_session(
        &mut self,
        candidates: &mut Vec<WakeSweepCandidate>,
        session_id: String,
        stale_after_seconds: i64,
    ) -> Result<(), String> {
        if let Some(candidate) =
            self.wake_sweep_candidate_for_session(session_id, stale_after_seconds)?
        {
            candidates.push(candidate);
        }
        Ok(())
    }

    fn wake_sweep_candidate_for_session(
        &mut self,
        session_id: String,
        stale_after_seconds: i64,
    ) -> Result<Option<WakeSweepCandidate>, String> {
        let Some(state) = self.wake_sweep_session_state(session_id)? else {
            return Ok(None);
        };
        self.wake_sweep_candidate_from_state(state, stale_after_seconds)
    }

    fn wake_sweep_session_state(
        &mut self,
        session_id: String,
    ) -> Result<Option<WakeSweepSessionState>, String> {
        if self.session_is_busy(&session_id)? {
            return Ok(None);
        }
        let Some((min_pending_seq, max_pending_seq)) = self.pending_seq_bounds(&session_id)? else {
            return Ok(None);
        };
        let claim = wake_claim(self.conn, &session_id)?;
        Ok(Some(WakeSweepSessionState {
            session_id,
            min_pending_seq,
            max_pending_seq,
            claim,
        }))
    }

    fn wake_sweep_candidate_from_state(
        &self,
        state: WakeSweepSessionState,
        stale_after_seconds: i64,
    ) -> Result<Option<WakeSweepCandidate>, String> {
        if !self.wake_sweep_state_is_candidate(&state, stale_after_seconds)? {
            return Ok(None);
        }
        Ok(Some(self.wake_sweep_candidate_from_eligible_state(state)?))
    }

    fn wake_sweep_state_is_candidate(
        &self,
        state: &WakeSweepSessionState,
        stale_after_seconds: i64,
    ) -> Result<bool, String> {
        match state.claim.as_ref() {
            Some(claim) => wake_claim_is_reclaimable(self.conn, claim, stale_after_seconds),
            None => Ok(true),
        }
    }

    fn wake_sweep_candidate_from_eligible_state(
        &self,
        state: WakeSweepSessionState,
    ) -> Result<WakeSweepCandidate, String> {
        Ok(wake_sweep_candidate(
            state.session_id.clone(),
            self.next_auto_wake_count_for_session(&state.session_id, state.claim.as_ref())?,
            state.min_pending_seq,
            state.max_pending_seq,
        ))
    }

    fn next_auto_wake_count_for_session(
        &self,
        session_id: &str,
        claim: Option<&WakeClaimRow>,
    ) -> Result<i64, String> {
        let persisted = self.persisted_auto_wake_count(session_id)?;
        Ok(next_auto_wake_count(
            persisted,
            claim_auto_wake_count(claim),
        ))
    }

    fn persisted_auto_wake_count(&self, session_id: &str) -> Result<i64, String> {
        Ok(session_metadata_row(self.conn, session_id)?
            .map(|runtime| runtime.auto_wake_count)
            .unwrap_or(0))
    }

    pub fn validate_wake_claim_for_child(
        &mut self,
        session_id: &str,
        claim_token: &str,
        child_identity: &ProcessIdentity,
    ) -> Result<bool, String> {
        let observed_busy = self.session_is_busy(session_id)?;
        let admission_id = Uuid::new_v4().to_string();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|err| format!("Failed to start wake-child admission transaction: {err}"))?;
        let claim = wake_claim(&tx, session_id)?;
        let Some(claim) = claim.filter(|claim| wake_claim_matches_child(claim, claim_token)) else {
            tx.commit()
                .map_err(|err| format!("Failed to commit rejected wake-child admission: {err}"))?;
            return Ok(false);
        };
        if claim.wake_invocation_uuid.is_some() {
            let replay_matches =
                wake_claim_has_matching_live_process_identity(&tx, &claim, child_identity)?;
            tx.commit()
                .map_err(|err| format!("Failed to commit replayed wake-child admission: {err}"))?;
            return Ok(replay_matches);
        }
        if observed_busy || wake_claim_runtime_is_busy_tx(&tx, session_id)? {
            tx.execute(
                "DELETE FROM session_wake_claim
                 WHERE session_id = ?1
                   AND claim_token = ?2
                   AND wake_invocation_uuid IS NULL",
                params![session_id, claim_token],
            )
            .map_err(|err| format!("Failed to release busy wake-child claim: {err}"))?;
            tx.commit()
                .map_err(|err| format!("Failed to commit busy wake-child rejection: {err}"))?;
            return Ok(false);
        }
        let changed = tx
            .execute(
                "UPDATE session_wake_claim
                 SET wake_invocation_uuid = ?3,
                     wake_pid = ?4,
                     wake_os_boot_id = ?5,
                     wake_os_pid_starttime_ticks = ?6
                 WHERE session_id = ?1
                   AND claim_token = ?2
                   AND wake_invocation_uuid IS NULL",
                params![
                    session_id,
                    claim_token,
                    &admission_id,
                    child_identity.os_pid,
                    &child_identity.os_boot_id,
                    child_identity.os_pid_starttime_ticks,
                ],
            )
            .map_err(|err| format!("Failed to admit wake child: {err}"))?;
        if changed != 1 {
            return Err("Wake claim changed during child admission".to_string());
        }
        tx.commit()
            .map_err(|err| format!("Failed to commit wake-child admission: {err}"))?;
        Ok(true)
    }

    #[cfg(test)]
    fn force_wake_claim_age_for_test(
        &mut self,
        session_id: &str,
        seconds_old: i64,
    ) -> Result<(), String> {
        let claimed_at = aged_wake_claim_timestamp(seconds_old);
        self.conn
            .execute(
                "UPDATE session_wake_claim SET claimed_at = ?2 WHERE session_id = ?1",
                params![session_id, &claimed_at],
            )
            .map_err(|err| format!("Failed to age wake claim for test: {err}"))?;
        Ok(())
    }
}

impl MailboxDb {
    fn list_mailbox_all(&self, session_id: &str) -> Result<Vec<MailboxRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT seq, session_id, kind, handle, payload_json, enqueued_at,
                        delivered_at, delivered_by_invocation_uuid, delivery_attempts,
                        delivery_error, owner_invocation_uuid, matched_os_pid,
                        matched_os_boot_id, matched_os_pid_starttime_ticks,
                        matched_chain_index, state_dir, meta_path, log_path, rc_path, rc,
                        payload_file_path, payload_sha256, payload_byte_len,
                        payload_retention_policy, payload_compacted_at,
                        submission_token, target_kind, target_id
                 FROM mailbox
                 WHERE session_id = ?1
                 ORDER BY seq ASC",
            )
            .map_err(|err| format!("Failed to prepare mailbox query: {err}"))?;
        let rows = stmt
            .query_map(params![session_id], map_mailbox_row)
            .map_err(|err| format!("Failed to query mailbox rows: {err}"))?;
        collect_rows(rows)
    }
}

impl WakeSessionRepository<'_> {
    fn pending_wake_session_ids_for_sweep(&self, limit: usize) -> Result<Vec<String>, String> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let rotating_limit = limit.saturating_sub(limit / 2);
        let newest_limit = limit.saturating_sub(rotating_limit);
        let cursor = self.wake_sweep_cursor()?;
        let mut rotating = self.pending_wake_sessions_in_seq_range(cursor, None, rotating_limit)?;
        if cursor.is_some() && rotating.len() < rotating_limit {
            let remaining = rotating_limit.saturating_sub(rotating.len());
            rotating.extend(self.pending_wake_sessions_in_seq_range(None, cursor, remaining)?);
        }
        if let Some((_, next_cursor)) = rotating.last() {
            self.set_wake_sweep_cursor(*next_cursor)?;
        }
        let rotating = rotating
            .into_iter()
            .map(|(session_id, _)| session_id)
            .collect();
        let newest = self.newest_pending_wake_session_ids(newest_limit)?;
        Ok(merge_pending_wake_session_ids(limit, rotating, newest))
    }

    fn pending_wake_session_ids(&self, limit: usize) -> Result<Vec<String>, String> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let oldest_limit = limit.saturating_sub(limit / 2);
        let newest_limit = limit.saturating_sub(oldest_limit);
        let oldest = self.oldest_pending_wake_session_ids(oldest_limit)?;
        let newest = self.newest_pending_wake_session_ids(newest_limit)?;
        Ok(merge_pending_wake_session_ids(limit, oldest, newest))
    }

    fn oldest_pending_wake_session_ids(&self, limit: usize) -> Result<Vec<String>, String> {
        self.pending_wake_session_ids_by_oldest_seq(limit, "ASC")
    }

    fn newest_pending_wake_session_ids(&self, limit: usize) -> Result<Vec<String>, String> {
        self.pending_wake_session_ids_by_oldest_seq(limit, "DESC")
    }

    fn pending_wake_session_ids_by_oldest_seq(
        &self,
        limit: usize,
        direction: &str,
    ) -> Result<Vec<String>, String> {
        let query = pending_wake_session_ids_by_oldest_seq_query(direction);
        let mut stmt = self
            .conn
            .prepare(&query)
            .map_err(|err| format!("Failed to prepare pending wake session query: {err}"))?;
        let rows = stmt
            .query_map(params![limit as i64, WAKE_SWEEP_ABANDONED_ERROR], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|err| format!("Failed to query pending wake sessions: {err}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|err| format!("Failed to read pending wake session row: {err}"))
    }

    fn pending_wake_sessions_in_seq_range(
        &self,
        after_seq: Option<i64>,
        through_seq: Option<i64>,
        limit: usize,
    ) -> Result<Vec<(String, i64)>, String> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut stmt = self
            .conn
            .prepare(pending_wake_sessions_in_seq_range_query())
            .map_err(|err| format!("Failed to prepare rotating wake session query: {err}"))?;
        let rows = stmt
            .query_map(
                params![
                    limit as i64,
                    WAKE_SWEEP_ABANDONED_ERROR,
                    after_seq,
                    through_seq
                ],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .map_err(|err| format!("Failed to query rotating wake sessions: {err}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|err| format!("Failed to read rotating wake session row: {err}"))
    }

    fn wake_sweep_cursor(&self) -> Result<Option<i64>, String> {
        self.conn
            .query_row(
                "SELECT after_pending_seq FROM wake_sweep_progress WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|err| format!("Failed to read wake sweep cursor: {err}"))
    }

    fn set_wake_sweep_cursor(&self, after_pending_seq: i64) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO wake_sweep_progress (singleton, after_pending_seq)
                 VALUES (1, ?1)
                 ON CONFLICT(singleton) DO UPDATE SET
                    after_pending_seq = excluded.after_pending_seq",
                params![after_pending_seq],
            )
            .map(|_| ())
            .map_err(|err| format!("Failed to advance wake sweep cursor: {err}"))
    }

    fn pending_seq_bounds(&self, session_id: &str) -> Result<Option<(i64, i64)>, String> {
        pending_seq_bounds_on(self.conn, session_id)
    }
}

impl MailboxDb {
    #[cfg(test)]
    fn enqueue_agent_bash_complete_then_rollback(
        &mut self,
        input: &AgentBashCompleteEnqueue<'_>,
    ) -> Result<(), String> {
        let published = self
            .payloads()
            .publish_immutable_payload(input.payload_json.as_bytes())?;
        let payload_json = compacted_payload_json(AGENT_BASH_COMPLETE_KIND, &published)?;
        let now = now_rfc3339();
        let tx = self
            .conn
            .transaction()
            .map_err(|err| format!("Failed to start mailbox rollback test transaction: {err}"))?;
        let _ = enqueue_agent_bash_complete_in_tx(&tx, input, &payload_json, &published, &now)?;
        Err("forced rollback before commit".to_string())
    }
}

impl WakeSessionRepository<'_> {
    fn max_mailbox_seq(&self, session_id: &str) -> Result<Option<i64>, String> {
        max_mailbox_seq_on(self.conn, session_id)
    }

    fn running_turn_start_max_mailbox_seq(
        &self,
        input: &LegacyRuntimeProjection<'_>,
    ) -> Result<Option<i64>, String> {
        let Some(seq) = input.turn_start_max_mailbox_seq else {
            return self.max_mailbox_seq(input.session_id);
        };
        Ok(Some(seq))
    }

    fn session_is_busy(&mut self, session_id: &str) -> Result<bool, String> {
        RuntimeLifecycleRepository { conn: self.conn }
            .reconcile_session_liveness(session_id)
            .map(|liveness| liveness == SessionLiveness::Busy)
    }
}

impl MailboxDbRebuildAuthority<'_> {
    pub fn sqlite_member_paths(&self) -> Vec<PathBuf> {
        let mut paths = vec![self.namespace.path().to_path_buf()];
        paths.extend(mailbox_sqlite_artifact_paths(self.namespace.path()));
        paths
    }

    pub fn reset(&mut self) -> Result<(), String> {
        #[cfg(windows)]
        self.release_writer();

        for path in self.sqlite_member_paths().into_iter().rev() {
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(format!(
                        "Failed to reset PID mailbox rebuild member {}: {error}",
                        path.display()
                    ));
                }
            }
        }
        self.release_writer();
        self.namespace.target_identity = None;
        Ok(())
    }

    pub fn initialize_after_rebuild(&mut self) -> Result<(), String> {
        if self.writer.is_some() {
            return Err(
                "PID mailbox rebuild writer remains active during initialization".to_string(),
            );
        }
        let mailbox = MailboxDb::open_with_authority(&self.namespace)?;
        drop(mailbox);
        self.namespace.target_identity = inspect_mailbox_storage_file(self.namespace.path())
            .map_err(|error| format!("Failed to bind rebuilt PID mailbox identity: {error}"))?;
        Ok(())
    }

    fn release_writer(&mut self) {
        drop(self.writer.take());
    }
}

fn acquire_mailbox_rebuild_writer(
    namespace: &MailboxAuthorityFence,
) -> Result<Option<Connection>, String> {
    if !namespace.path().exists() {
        return Ok(None);
    }
    let connection =
        Connection::open_with_flags(namespace.path(), OpenFlags::SQLITE_OPEN_READ_WRITE).map_err(
            |error| {
                format!(
                    "process_integrity: completion_authority_rebuild_writer_unavailable: failed to open existing PID mailbox rebuild writer; retry after reconciling sidecar access: {error}"
                )
            },
        )?;
    connection
        .busy_timeout(mailbox_writer_sqlite_timeout())
        .map_err(|error| format!("Failed to configure PID mailbox rebuild writer: {error}"))?;
    match connection.execute_batch("BEGIN IMMEDIATE") {
        Ok(()) => Ok(Some(connection)),
        Err(error)
            if matches!(
                error.sqlite_error_code(),
                Some(rusqlite::ffi::ErrorCode::DatabaseBusy)
                    | Some(rusqlite::ffi::ErrorCode::DatabaseLocked)
            ) =>
        {
            Err(format!(
                "process_integrity: completion_authority_contention: timed out acquiring PID mailbox rebuild writer: {error}"
            ))
        }
        Err(error) if sqlite_error_is_corrupt(&error) => Ok(None),
        Err(error) => Err(format!(
            "process_integrity: completion_authority_rebuild_writer_unavailable: failed to prove the existing PID mailbox writer cut; retry after reconciling sidecar access: {error}"
        )),
    }
}

fn sqlite_error_is_corrupt(error: &rusqlite::Error) -> bool {
    matches!(
        error.sqlite_error_code(),
        Some(rusqlite::ffi::ErrorCode::DatabaseCorrupt)
            | Some(rusqlite::ffi::ErrorCode::NotADatabase)
    )
}

fn sqlite_error_is_contention(error: &rusqlite::Error) -> bool {
    matches!(
        error.sqlite_error_code(),
        Some(rusqlite::ffi::ErrorCode::DatabaseBusy)
            | Some(rusqlite::ffi::ErrorCode::DatabaseLocked)
    )
}

#[cfg(not(any(test, feature = "test-support")))]
fn mailbox_writer_sqlite_timeout() -> StdDuration {
    StdDuration::from_secs(5)
}

#[cfg(any(test, feature = "test-support"))]
fn mailbox_writer_sqlite_timeout() -> StdDuration {
    StdDuration::from_millis(500)
}

pub fn mailbox_row_is_deliverable_pending(row: &MailboxRow) -> bool {
    row.delivered_at.is_none() && row.delivery_error.as_deref() != Some(WAKE_SWEEP_ABANDONED_ERROR)
}

fn resolve_completed_delivery_attempts(
    tx: &Transaction<'_>,
    session_id: &str,
    resolved_at: &str,
    resolved_by_attempt_id: Option<&str>,
) -> Result<(), String> {
    tx.execute(
        "UPDATE mailbox_delivery_attempts AS attempt
         SET resolved_at = COALESCE(resolved_at, ?2),
             resolved_by_attempt_id = COALESCE(resolved_by_attempt_id, ?3)
         WHERE session_id = ?1
           AND resolved_at IS NULL
           AND NOT EXISTS (
                 SELECT 1
                 FROM mailbox_delivery_attempt_items AS unresolved
                 JOIN mailbox ON mailbox.seq = unresolved.mailbox_seq
                 WHERE unresolved.attempt_id = attempt.attempt_id
                   AND mailbox.delivered_at IS NULL
             )",
        params![session_id, resolved_at, resolved_by_attempt_id],
    )
    .map(|_| ())
    .map_err(|err| format!("Failed to resolve completed mailbox delivery attempts: {err}"))
}

struct ConsumedCompletionBinding {
    event_id: String,
    owner_session_id: String,
    owner_invocation_uuid: String,
    acknowledged_at: Option<String>,
    delivered_at: Option<String>,
}

impl ConsumedCompletionBinding {
    fn owner_matches(&self, session_id: &str, invocation_uuid: &str) -> bool {
        self.owner_session_id == session_id && self.owner_invocation_uuid == invocation_uuid
    }

    fn is_settled(&self) -> bool {
        self.acknowledged_at.is_some() || self.delivered_at.is_some()
    }
}

fn begin_consumed_completion_acknowledgement(
    conn: &mut Connection,
) -> Result<Transaction<'_>, String> {
    conn.transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(format_consumed_completion_transaction_start_error)
}

fn format_consumed_completion_transaction_start_error(err: rusqlite::Error) -> String {
    format!("Failed to start consumed completion acknowledgement transaction: {err}")
}

fn consumed_completion_binding(
    tx: &Transaction<'_>,
    mailbox_seq: i64,
) -> Result<Option<ConsumedCompletionBinding>, String> {
    tx.query_row(
        "SELECT listener.event_id, listener.session_id,
                listener.owner_invocation_uuid, listener.acknowledged_at,
                mailbox.delivered_at
         FROM completion_event_listener AS listener
         JOIN mailbox ON mailbox.seq = listener.mailbox_seq
         WHERE listener.mailbox_seq = ?1",
        params![mailbox_seq],
        map_consumed_completion_binding,
    )
    .optional()
    .map_err(format_consumed_completion_binding_error)
}

fn map_consumed_completion_binding(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<ConsumedCompletionBinding> {
    Ok(ConsumedCompletionBinding {
        event_id: row.get(0)?,
        owner_session_id: row.get(1)?,
        owner_invocation_uuid: row.get(2)?,
        acknowledged_at: row.get(3)?,
        delivered_at: row.get(4)?,
    })
}

fn format_consumed_completion_binding_error(err: rusqlite::Error) -> String {
    format!("Failed to resolve mailbox completion event: {err}")
}

fn completion_consumption_claimed(tx: &Transaction<'_>, event_id: &str) -> Result<bool, String> {
    tx.query_row(
        "SELECT EXISTS(
            SELECT 1
            FROM completion_event_listener
            WHERE event_id = ?1
              AND acknowledgement_reason = 'consumed_in_call'
         )",
        params![event_id],
        |row| row.get(0),
    )
    .map_err(format_completion_consumption_claim_error)
}

fn format_completion_consumption_claim_error(err: rusqlite::Error) -> String {
    format!("Failed to inspect completion consumption claim: {err}")
}

fn consume_completion_mailbox_row(
    tx: &Transaction<'_>,
    mailbox_seq: i64,
    acknowledged_at: &str,
    owner_invocation_uuid: &str,
) -> Result<usize, String> {
    tx.execute(
        "UPDATE mailbox
         SET delivered_at = ?2,
             delivered_by_invocation_uuid = ?3,
             delivery_attempts = delivery_attempts + 1,
             delivery_error = NULL
         WHERE seq = ?1 AND delivered_at IS NULL",
        params![mailbox_seq, acknowledged_at, owner_invocation_uuid],
    )
    .map_err(format_consumed_completion_mailbox_error)
}

fn format_consumed_completion_mailbox_error(err: rusqlite::Error) -> String {
    format!("Failed to consume completion event mailbox row: {err}")
}

fn acknowledge_consumed_completion_listener(
    tx: &Transaction<'_>,
    mailbox_seq: i64,
    acknowledged_at: &str,
) -> Result<usize, String> {
    tx.execute(
        "UPDATE completion_event_listener
         SET active = 0,
             acknowledged_at = ?2,
             acknowledgement_reason = 'consumed_in_call'
         WHERE mailbox_seq = ?1 AND acknowledged_at IS NULL",
        params![mailbox_seq, acknowledged_at],
    )
    .map_err(format_consumed_completion_listener_error)
}

fn format_consumed_completion_listener_error(err: rusqlite::Error) -> String {
    format!("Failed to acknowledge consumed completion listener: {err}")
}

fn validate_consumed_completion_change(
    changed: usize,
    target: &str,
    mailbox_seq: i64,
) -> Result<(), String> {
    if changed == 1 {
        return Ok(());
    }
    Err(format_consumed_completion_change_error(target, mailbox_seq))
}

fn format_consumed_completion_change_error(target: &str, mailbox_seq: i64) -> String {
    format!(
        "Completion event {target} for mailbox row {mailbox_seq} changed while it was being consumed"
    )
}

fn commit_consumed_completion_acknowledgement(tx: Transaction<'_>) -> Result<(), String> {
    tx.commit()
        .map_err(format_consumed_completion_transaction_commit_error)
}

fn format_consumed_completion_transaction_commit_error(err: rusqlite::Error) -> String {
    format!("Failed to commit consumed completion acknowledgement: {err}")
}

fn resolve_completed_delivery_attempts_for_mailbox_seq(
    tx: &Transaction<'_>,
    mailbox_seq: i64,
    resolved_at: &str,
) -> Result<(), String> {
    tx.execute(
        "UPDATE mailbox_delivery_attempts AS attempt
         SET resolved_at = COALESCE(resolved_at, ?2)
         WHERE resolved_at IS NULL
           AND EXISTS (
                 SELECT 1
                 FROM mailbox_delivery_attempt_items AS consumed
                 WHERE consumed.attempt_id = attempt.attempt_id
                   AND consumed.mailbox_seq = ?1
             )
           AND NOT EXISTS (
                 SELECT 1
                 FROM mailbox_delivery_attempt_items AS unresolved
                 JOIN mailbox ON mailbox.seq = unresolved.mailbox_seq
                 WHERE unresolved.attempt_id = attempt.attempt_id
                   AND mailbox.delivered_at IS NULL
             )",
        params![mailbox_seq, resolved_at],
    )
    .map(|_| ())
    .map_err(format_exact_row_delivery_attempt_resolution_error)
}

fn format_exact_row_delivery_attempt_resolution_error(err: rusqlite::Error) -> String {
    format!("Failed to resolve exact-row mailbox delivery attempts: {err}")
}

fn payload_address(sha256: &str) -> String {
    format!("{MAILBOX_PAYLOAD_ADDRESS_VERSION}:{MAILBOX_PAYLOAD_ALGORITHM}:{sha256}")
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn validate_sha256_hex(value: &str) -> Result<(), String> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Ok(());
    }
    Err("Mailbox payload SHA-256 must be 64 hexadecimal characters".to_string())
}

fn map_compaction_report_delta(
    original_len: u64,
    published: &PublishedMailboxPayload,
    compacted_len: usize,
    changed: bool,
) -> DeliveredPayloadCompactionReport {
    if !changed {
        return DeliveredPayloadCompactionReport::default();
    }
    DeliveredPayloadCompactionReport {
        scanned_rows: 0,
        compacted_rows: 1,
        retained_payload_bytes: published.byte_len,
        inline_bytes_reclaimed: original_len.saturating_sub(compacted_len as u64),
    }
}

fn merge_compaction_report(
    report: &mut DeliveredPayloadCompactionReport,
    delta: DeliveredPayloadCompactionReport,
) {
    report.compacted_rows += delta.compacted_rows;
    report.retained_payload_bytes = report
        .retained_payload_bytes
        .saturating_add(delta.retained_payload_bytes);
    report.inline_bytes_reclaimed = report
        .inline_bytes_reclaimed
        .saturating_add(delta.inline_bytes_reclaimed);
}

fn validate_payload_publication(path: &Path) -> Result<Option<&Path>, String> {
    let directory = path
        .parent()
        .ok_or_else(|| "Mailbox payload path has no parent directory".to_string())?;
    if path.exists() {
        Ok(None)
    } else {
        Ok(Some(directory))
    }
}

fn publish_payload_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let Some(directory) = validate_payload_publication(path)? else {
        return Ok(());
    };
    ensure_durable_directory(directory)?;

    let temp_path = directory.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("payload"),
        Uuid::new_v4()
    ));
    write_payload_temp_file(&temp_path, bytes)?;
    publish_payload_temp_file(&temp_path, path)?;
    sync_directory(directory)?;
    Ok(())
}

#[cfg(not(windows))]
fn publish_payload_temp_file(temp_path: &Path, path: &Path) -> Result<(), String> {
    match fs::hard_link(temp_path, path) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::AlreadyExists => {}
        Err(err) => {
            let _ = fs::remove_file(temp_path);
            return Err(format!(
                "Failed to publish immutable mailbox payload: {err}"
            ));
        }
    }
    fs::remove_file(temp_path)
        .map_err(|err| format!("Failed to remove mailbox payload temporary file: {err}"))
}

#[cfg(windows)]
fn publish_payload_temp_file(temp_path: &Path, path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};

    let source = temp_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    } != 0
    {
        return Ok(());
    }
    let err = std::io::Error::last_os_error();
    let _ = fs::remove_file(temp_path);
    if err.kind() == ErrorKind::AlreadyExists {
        Ok(())
    } else {
        Err(format!(
            "Failed to publish immutable mailbox payload: {err}"
        ))
    }
}

fn write_payload_temp_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|err| format!("Failed to create mailbox payload temporary file: {err}"))?;
    file.write_all(bytes)
        .map_err(|err| format!("Failed to write mailbox payload temporary file: {err}"))?;
    let mut permissions = file
        .metadata()
        .map_err(|err| format!("Failed to inspect mailbox payload temporary file: {err}"))?
        .permissions();
    permissions.set_readonly(true);
    file.set_permissions(permissions)
        .map_err(|err| format!("Failed to make mailbox payload immutable: {err}"))?;
    file.sync_all()
        .map_err(|err| format!("Failed to sync mailbox payload file: {err}"))
}

fn verify_published_payload(payload: &PublishedMailboxPayload) -> Result<(), String> {
    if payload.address != payload_address(&payload.sha256) {
        return Err("Mailbox payload address does not match its SHA-256".to_string());
    }
    if payload.retention_policy != MAILBOX_PAYLOAD_RETENTION_POLICY {
        return Err(format!(
            "Unsupported mailbox payload retention policy: {}",
            payload.retention_policy
        ));
    }
    let metadata = fs::symlink_metadata(&payload.file_path).map_err(|err| {
        format!(
            "Failed to access immutable mailbox payload {}: {err}",
            payload.file_path.display()
        )
    })?;
    if !metadata.is_file() || metadata.len() != payload.byte_len {
        return Err(format!(
            "Mailbox payload length mismatch for {}: expected {}, found {}",
            payload.file_path.display(),
            payload.byte_len,
            metadata.len()
        ));
    }
    if !metadata.permissions().readonly() {
        return Err(format!(
            "Mailbox payload is not immutable: {}",
            payload.file_path.display()
        ));
    }
    let actual_sha256 = format_sha256_digest(&sha256_file(&payload.file_path)?);
    if actual_sha256 != payload.sha256 {
        return Err(format!(
            "Mailbox payload integrity mismatch for {}",
            payload.file_path.display()
        ));
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<[u8; 32], String> {
    let mut file = File::open(path)
        .map_err(|err| format!("Failed to open mailbox payload for verification: {err}"))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|err| format!("Failed to read mailbox payload for verification: {err}"))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(digest.finalize().into())
}

fn format_sha256_digest(digest: &[u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn published_payload_from_row(row: &MailboxRow) -> Result<Option<PublishedMailboxPayload>, String> {
    match (
        row.payload_file_path.as_deref(),
        row.payload_sha256.as_deref(),
        row.payload_byte_len,
        row.payload_retention_policy.as_deref(),
    ) {
        (None, None, None, None) => Ok(None),
        (Some(file_path), Some(sha256), Some(byte_len), Some(retention_policy)) => {
            let byte_len = u64::try_from(byte_len)
                .map_err(|_| "Mailbox payload byte length must not be negative".to_string())?;
            Ok(Some(PublishedMailboxPayload {
                address: payload_address(sha256),
                file_path: PathBuf::from(file_path),
                sha256: sha256.to_string(),
                byte_len,
                retention_policy: retention_policy.to_string(),
            }))
        }
        _ => Err(format!(
            "Mailbox row {} has incomplete immutable payload metadata",
            row.seq
        )),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DurableDirectoryState {
    Existing,
    Missing,
}

fn validate_durable_directory_state(path: &Path) -> Result<DurableDirectoryState, String> {
    if path.is_dir() {
        return Ok(DurableDirectoryState::Existing);
    }
    if path.exists() {
        Err(format!("Path is not a directory: {}", path.display()))
    } else {
        Ok(DurableDirectoryState::Missing)
    }
}

fn ensure_durable_directory(path: &Path) -> Result<(), String> {
    if validate_durable_directory_state(path)? == DurableDirectoryState::Existing {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        ensure_durable_directory(parent)?;
    }
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::AlreadyExists && path.is_dir() => return Ok(()),
        Err(err) => {
            return Err(format!(
                "Failed to create directory {}: {err}",
                path.display()
            ));
        }
    }
    sync_directory(path)?;
    if let Some(parent) = path.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|err| format!("Failed to sync directory {}: {err}", path.display()))
}

#[cfg(windows)]
fn sync_directory(_path: &Path) -> Result<(), String> {
    // Windows cannot FlushFileBuffers on directory handles. Final payload
    // publication uses MoveFileExW with MOVEFILE_WRITE_THROUGH instead.
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn sync_directory(path: &Path) -> Result<(), String> {
    Err(format!(
        "Durable mailbox payload directory publication is unsupported on this platform: {}",
        path.display()
    ))
}

#[cfg(test)]
fn aged_wake_claim_timestamp(seconds_old: i64) -> String {
    (Utc::now() - Duration::seconds(seconds_old)).to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn merge_pending_wake_session_ids(
    limit: usize,
    mut oldest: Vec<String>,
    newest: Vec<String>,
) -> Vec<String> {
    for session_id in newest {
        if oldest.len() >= limit {
            break;
        }
        if !oldest.iter().any(|existing| existing == &session_id) {
            oldest.push(session_id);
        }
    }
    oldest
}

fn wake_sweep_candidates_at_limit(candidates: &[WakeSweepCandidate], limit: usize) -> bool {
    candidates.len() >= limit
}

fn pending_wake_session_ids_by_oldest_seq_query(direction: &str) -> String {
    format!(
        "SELECT session_id
                  FROM mailbox
                  WHERE delivered_at IS NULL
                    AND (delivery_error IS NULL OR delivery_error != ?2)
                  GROUP BY session_id
                  ORDER BY MIN(seq) {direction}
                  LIMIT ?1",
    )
}

fn pending_wake_sessions_in_seq_range_query() -> &'static str {
    "SELECT session_id, MIN(seq) AS oldest_seq
     FROM mailbox
     WHERE delivered_at IS NULL
       AND (delivery_error IS NULL OR delivery_error != ?2)
     GROUP BY session_id
     HAVING (?3 IS NULL OR oldest_seq > ?3)
        AND (?4 IS NULL OR oldest_seq <= ?4)
     ORDER BY oldest_seq ASC
     LIMIT ?1"
}

fn wake_sweep_candidate(
    session_id: String,
    auto_wake_count: i64,
    min_pending_seq: i64,
    max_pending_seq: i64,
) -> WakeSweepCandidate {
    WakeSweepCandidate {
        session_id,
        auto_wake_count,
        min_pending_seq,
        max_pending_seq,
    }
}

fn begin_wake_claim_transaction(
    conn: &mut Connection,
) -> Result<rusqlite::Transaction<'_>, String> {
    conn.transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(format_start_wake_claim_tx_error)
}

fn format_start_wake_claim_tx_error(err: rusqlite::Error) -> String {
    format!("Failed to start wake claim transaction: {err}")
}

fn pending_seq_bounds_for_claim_tx(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
) -> Result<Option<(i64, i64)>, String> {
    let query = format!(
        "SELECT MIN(seq), MAX(seq)
         FROM mailbox
         WHERE delivered_at IS NULL
           AND {PENDING_MAILBOX_TARGET_PREDICATE}
           AND (delivery_error IS NULL OR delivery_error != ?3)"
    );
    tx.query_row(
        &query,
        params![session_id, Option::<&str>::None, WAKE_SWEEP_ABANDONED_ERROR,],
        |row| {
            let min_seq: Option<i64> = row.get(0)?;
            let max_seq: Option<i64> = row.get(1)?;
            Ok(min_seq.zip(max_seq))
        },
    )
    .map_err(|err| format!("Failed to read deliverable wake-claim bounds: {err}"))
}

fn wake_claim_runtime_is_busy_tx(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
) -> Result<bool, String> {
    tx.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM runtime_generation
             WHERE session_id = ?1 AND lifecycle_state != 'exited'
         )",
        params![session_id],
        |row| row.get::<_, bool>(0),
    )
    .map_err(|err| format!("Failed to validate wake-claim runtime authority: {err}"))
}

fn wake_claim_notifications_paused_tx(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
) -> Result<bool, String> {
    tx.query_row(
        "SELECT COALESCE((
             SELECT paused FROM mailbox_notification_control WHERE session_id = ?1
         ), 0)",
        params![session_id],
        |row| row.get::<_, bool>(0),
    )
    .map_err(|err| format!("Failed to validate wake-claim notification state: {err}"))
}

fn fresh_in_flight_wake_claim_for_input(
    tx: &rusqlite::Transaction<'_>,
    input: WakeClaimRequest<'_>,
    renew_token: Option<&str>,
) -> Result<Option<WakeClaimRow>, String> {
    fresh_in_flight_wake_claim(
        tx,
        wake_claim_tx(tx, input.session_id)?,
        input.stale_after_seconds,
        renew_token,
    )
}

fn commit_empty_wake_claim_transaction(tx: rusqlite::Transaction<'_>) -> Result<(), String> {
    tx.commit().map_err(format_empty_wake_claim_commit_error)
}

fn format_empty_wake_claim_commit_error(err: rusqlite::Error) -> String {
    format!("Failed to commit empty wake claim transaction: {err}")
}

fn commit_existing_wake_claim_transaction(tx: rusqlite::Transaction<'_>) -> Result<(), String> {
    tx.commit().map_err(format_existing_wake_claim_commit_error)
}

fn format_existing_wake_claim_commit_error(err: rusqlite::Error) -> String {
    format!("Failed to commit existing wake claim transaction: {err}")
}

fn commit_wake_claim_transaction(tx: rusqlite::Transaction<'_>) -> Result<(), String> {
    tx.commit().map_err(format_wake_claim_commit_error)
}

fn format_wake_claim_commit_error(err: rusqlite::Error) -> String {
    format!("Failed to commit wake claim transaction: {err}")
}

fn claim_auto_wake_count(claim: Option<&WakeClaimRow>) -> Option<i64> {
    claim.map(|claim| claim.auto_wake_count)
}

fn next_auto_wake_count(persisted: i64, claim: Option<i64>) -> i64 {
    claim
        .unwrap_or(persisted)
        .max(persisted)
        .saturating_add(1)
        .max(1)
}

fn bounded_mailbox_limit_is_zero(limit: usize) -> bool {
    limit == 0
}

fn bounded_mailbox_sql_limit(limit: usize) -> i64 {
    limit as i64
}

fn format_bounded_mailbox_rows_error(err: BoundedMailboxRowsError) -> String {
    match err {
        BoundedMailboxRowsError::Prepare(err) => {
            format!("Failed to prepare bounded mailbox query: {err}")
        }
        BoundedMailboxRowsError::Query(err) => {
            format!("Failed to query bounded mailbox rows: {err}")
        }
        BoundedMailboxRowsError::Row(err) => format!("Failed to read mailbox row: {err}"),
    }
}

#[derive(Serialize)]
struct CanonicalSubmittedInputIdentity<'a> {
    domain: &'static str,
    v: u8,
    submission_id: &'a str,
    target: CanonicalInboxTarget<'a>,
}

#[derive(Serialize)]
struct CanonicalInboxTarget<'a> {
    kind: &'static str,
    id: &'a str,
}

#[derive(Serialize)]
struct SubmittedInputPayloadMetadata<'a> {
    schema_version: u8,
    kind: &'static str,
    submission_token: &'a str,
    target: CanonicalInboxTarget<'a>,
    payload: SubmittedInputPayloadReference<'a>,
}

#[derive(Serialize)]
struct SubmittedInputPayloadReference<'a> {
    address: &'a str,
    file_path: &'a Path,
    sha256: &'a str,
    byte_len: u64,
    retention_policy: &'a str,
}

#[derive(Serialize)]
struct CompactedMailboxPayloadMetadata<'a> {
    schema_version: u8,
    kind: &'a str,
    payload: SubmittedInputPayloadReference<'a>,
}

struct DeliveredPayloadCompactionCandidate {
    seq: i64,
    kind: String,
    payload_json: String,
    payload_file_path: Option<String>,
    payload_sha256: Option<String>,
    payload_byte_len: Option<i64>,
    payload_retention_policy: Option<String>,
}

#[derive(Debug)]
struct PrunableTerminalMailboxRow {
    seq: i64,
    payload: Option<RetiredPayload>,
}

#[derive(Debug)]
struct RetiredPayload {
    file_path: PathBuf,
    sha256: String,
}

#[derive(Debug, Default)]
struct PayloadReclaimResult {
    files_deleted: usize,
    bytes_reclaimed: u64,
}

impl DeliveredPayloadCompactionCandidate {
    fn published_payload(&self) -> Result<Option<PublishedMailboxPayload>, String> {
        match (
            self.payload_file_path.as_deref(),
            self.payload_sha256.as_deref(),
            self.payload_byte_len,
            self.payload_retention_policy.as_deref(),
        ) {
            (None, None, None, None) => Ok(None),
            (Some(file_path), Some(sha256), Some(byte_len), Some(retention_policy)) => {
                let byte_len = u64::try_from(byte_len).map_err(|_| {
                    format!(
                        "Mailbox row {} has a negative payload byte length",
                        self.seq
                    )
                })?;
                Ok(Some(PublishedMailboxPayload {
                    address: payload_address(sha256),
                    file_path: PathBuf::from(file_path),
                    sha256: sha256.to_string(),
                    byte_len,
                    retention_policy: retention_policy.to_string(),
                }))
            }
            _ => Err(format!(
                "Mailbox row {} has incomplete retained payload metadata",
                self.seq
            )),
        }
    }
}

fn terminal_history_retention_stats_on(
    conn: &Connection,
    keep: usize,
) -> Result<TerminalHistoryRetentionStats, String> {
    let keep = i64::try_from(keep)
        .map_err(|_| "Terminal history keep count does not fit SQLite INTEGER".to_string())?;
    let terminal_mailbox_rows = count_rows(
        conn,
        "SELECT COUNT(*) FROM mailbox WHERE delivered_at IS NOT NULL",
        [],
        "terminal mailbox rows",
    )?;
    let prunable_mailbox_rows = count_rows(
        conn,
        "SELECT COUNT(*)
         FROM mailbox AS candidate
         WHERE candidate.delivered_at IS NOT NULL
           AND candidate.kind = ?2
           AND candidate.seq NOT IN (
                SELECT seq FROM mailbox
                WHERE delivered_at IS NOT NULL AND kind = ?2
                ORDER BY seq DESC
                LIMIT ?1
           )
           AND NOT EXISTS (
               SELECT 1 FROM completion_event_listener AS listener
               WHERE listener.mailbox_seq = candidate.seq
                 AND listener.acknowledged_at IS NULL
           )
           AND NOT EXISTS (
               SELECT 1
               FROM mailbox_delivery_attempt_items AS item
               JOIN mailbox_delivery_attempts AS attempt
                 ON attempt.attempt_id = item.attempt_id
               WHERE item.mailbox_seq = candidate.seq
                 AND attempt.resolved_at IS NULL
           )",
        params![keep, AGENT_BASH_COMPLETE_KIND],
        "prunable terminal mailbox rows",
    )?;
    let resolved_delivery_attempts = count_rows(
        conn,
        "SELECT COUNT(*) FROM mailbox_delivery_attempts WHERE resolved_at IS NOT NULL",
        [],
        "resolved mailbox delivery attempts",
    )?;
    let prunable_delivery_attempts = count_rows(
        conn,
        "SELECT COUNT(*)
         FROM mailbox_delivery_attempts AS candidate
         WHERE candidate.resolved_at IS NOT NULL
           AND (
               candidate.evidence_disposition IS NULL
               OR candidate.evidence_disposition NOT IN ('pending', 'legacy_pending')
               OR candidate.evidence_reconciled_at IS NOT NULL
           )
           AND candidate.attempt_id NOT IN (
                SELECT attempt_id FROM mailbox_delivery_attempts
                WHERE resolved_at IS NOT NULL
                  AND (
                      evidence_disposition IS NULL
                      OR evidence_disposition NOT IN ('pending', 'legacy_pending')
                      OR evidence_reconciled_at IS NOT NULL
                  )
                ORDER BY created_at DESC, attempt_id DESC
                LIMIT ?1
           )",
        params![keep],
        "prunable mailbox delivery attempts",
    )?;
    let reclaimable_payload_files = count_rows(
        conn,
        "SELECT COUNT(*) FROM (
             SELECT DISTINCT event.payload_sha256
             FROM completion_event AS event
             WHERE event.state = 'triggered'
               AND event.payload_reclaimed_at IS NULL
               AND event.payload_file_path IS NOT NULL
               AND event.payload_sha256 IS NOT NULL
               AND NOT EXISTS (
                   SELECT 1 FROM mailbox
                   WHERE mailbox.payload_sha256 = event.payload_sha256
               )
               AND NOT EXISTS (
                   SELECT 1
                   FROM completion_event AS shared_event
                   JOIN completion_event_listener AS listener
                     ON listener.event_id = shared_event.event_id
                   WHERE shared_event.payload_sha256 = event.payload_sha256
                     AND listener.acknowledged_at IS NULL
               )
         )",
        [],
        "reclaimable terminal payload files",
    )?;
    Ok(TerminalHistoryRetentionStats {
        terminal_mailbox_rows,
        prunable_mailbox_rows,
        resolved_delivery_attempts,
        prunable_delivery_attempts,
        reclaimable_payload_files,
    })
}

fn count_rows<P: rusqlite::Params>(
    conn: &Connection,
    sql: &str,
    params: P,
    target: &str,
) -> Result<usize, String> {
    let count = conn
        .query_row(sql, params, |row| row.get::<_, i64>(0))
        .map_err(|err| format!("Failed to count {target}: {err}"))?;
    usize::try_from(count).map_err(|_| format!("{target} count does not fit usize"))
}

fn prunable_delivery_attempt_ids(
    conn: &Connection,
    keep: i64,
    limit: i64,
) -> Result<Vec<String>, String> {
    let mut statement = conn
        .prepare(
            "SELECT attempt_id
             FROM mailbox_delivery_attempts AS candidate
             WHERE candidate.resolved_at IS NOT NULL
               AND (
                   candidate.evidence_disposition IS NULL
                   OR candidate.evidence_disposition NOT IN ('pending', 'legacy_pending')
                   OR candidate.evidence_reconciled_at IS NOT NULL
               )
               AND candidate.attempt_id NOT IN (
                    SELECT attempt_id FROM mailbox_delivery_attempts
                    WHERE resolved_at IS NOT NULL
                      AND (
                          evidence_disposition IS NULL
                          OR evidence_disposition NOT IN ('pending', 'legacy_pending')
                          OR evidence_reconciled_at IS NOT NULL
                      )
                    ORDER BY created_at DESC, attempt_id DESC
                    LIMIT ?1
               )
             ORDER BY created_at ASC, attempt_id ASC
             LIMIT ?2",
        )
        .map_err(|err| format!("Failed to prepare resolved delivery attempt pruning: {err}"))?;
    let rows = statement
        .query_map(params![keep, limit], |row| row.get(0))
        .map_err(|err| format!("Failed to query resolved delivery attempts: {err}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("Failed to read resolved delivery attempt: {err}"))
}

fn prunable_terminal_mailbox_rows(
    conn: &Connection,
    keep: i64,
    limit: i64,
) -> Result<Vec<PrunableTerminalMailboxRow>, String> {
    let mut statement = conn
        .prepare(
            "SELECT candidate.seq, candidate.payload_file_path, candidate.payload_sha256
             FROM mailbox AS candidate
             WHERE candidate.delivered_at IS NOT NULL
               AND candidate.kind = ?2
               AND candidate.seq NOT IN (
                   SELECT seq FROM mailbox
                   WHERE delivered_at IS NOT NULL AND kind = ?2
                   ORDER BY seq DESC
                   LIMIT ?1
               )
               AND NOT EXISTS (
                   SELECT 1 FROM completion_event_listener AS listener
                   WHERE listener.mailbox_seq = candidate.seq
                     AND listener.acknowledged_at IS NULL
               )
               AND NOT EXISTS (
                   SELECT 1
                   FROM mailbox_delivery_attempt_items AS item
                   JOIN mailbox_delivery_attempts AS attempt
                     ON attempt.attempt_id = item.attempt_id
                   WHERE item.mailbox_seq = candidate.seq
                     AND attempt.resolved_at IS NULL
               )
             ORDER BY candidate.seq ASC
             LIMIT ?3",
        )
        .map_err(|err| format!("Failed to prepare terminal mailbox pruning: {err}"))?;
    let rows = statement
        .query_map(params![keep, AGENT_BASH_COMPLETE_KIND, limit], |row| {
            let file_path = row.get::<_, Option<String>>(1)?;
            let sha256 = row.get::<_, Option<String>>(2)?;
            Ok(PrunableTerminalMailboxRow {
                seq: row.get(0)?,
                payload: file_path
                    .zip(sha256)
                    .map(|(file_path, sha256)| RetiredPayload {
                        file_path: PathBuf::from(file_path),
                        sha256,
                    }),
            })
        })
        .map_err(|err| format!("Failed to query terminal mailbox rows: {err}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("Failed to read terminal mailbox row: {err}"))
}

fn reclaimable_completion_payloads(
    conn: &Connection,
    limit: i64,
) -> Result<Vec<RetiredPayload>, String> {
    let mut statement = conn
        .prepare(
            "SELECT event.payload_file_path, event.payload_sha256
             FROM completion_event AS event
             WHERE event.state = 'triggered'
               AND event.payload_reclaimed_at IS NULL
               AND event.payload_file_path IS NOT NULL
               AND event.payload_sha256 IS NOT NULL
               AND NOT EXISTS (
                   SELECT 1 FROM mailbox
                   WHERE mailbox.payload_sha256 = event.payload_sha256
               )
               AND NOT EXISTS (
                   SELECT 1
                   FROM completion_event AS shared_event
                   JOIN completion_event_listener AS listener
                     ON listener.event_id = shared_event.event_id
                   WHERE shared_event.payload_sha256 = event.payload_sha256
                     AND listener.acknowledged_at IS NULL
               )
             GROUP BY event.payload_sha256
             ORDER BY MIN(event.triggered_at), event.payload_sha256
             LIMIT ?1",
        )
        .map_err(|err| format!("Failed to prepare terminal payload reclamation: {err}"))?;
    let rows = statement
        .query_map(params![limit], |row| {
            Ok(RetiredPayload {
                file_path: PathBuf::from(row.get::<_, String>(0)?),
                sha256: row.get(1)?,
            })
        })
        .map_err(|err| format!("Failed to query terminal payloads: {err}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("Failed to read terminal payload: {err}"))
}

fn merge_payload_reclaim_result(
    report: &mut TerminalHistoryPruneReport,
    reclaimed: PayloadReclaimResult,
) {
    report.payload_files_deleted += reclaimed.files_deleted;
    report.payload_bytes_reclaimed += reclaimed.bytes_reclaimed;
}

pub fn submitted_input_handle(
    submission_token: &str,
    target: InboxTarget<'_>,
) -> Result<String, String> {
    validate_submission_token(submission_token)?;
    validate_inbox_target(target)?;
    format_submitted_input_handle(submission_token, target)
}

fn format_submitted_input_handle(
    submission_token: &str,
    target: InboxTarget<'_>,
) -> Result<String, String> {
    let canonical = CanonicalSubmittedInputIdentity {
        domain: INPUT_IDENTITY_DOMAIN,
        v: INPUT_IDENTITY_VERSION,
        submission_id: submission_token,
        target: canonical_inbox_target(target),
    };
    let bytes = serde_json::to_vec(&canonical)
        .map_err(|err| format!("Failed to encode canonical input identity: {err}"))?;
    Ok(sha256_hex(&bytes))
}

fn validate_submitted_input(input: &SubmittedInputEnqueue<'_>) -> Result<(), String> {
    validate_submission_token(input.submission_token)?;
    validate_inbox_target(input.target)
}

fn validate_submission_token(submission_token: &str) -> Result<(), String> {
    if submission_token.is_empty() {
        return Err("Submission token must not be empty".to_string());
    }
    Ok(())
}

fn validate_inbox_target(target: InboxTarget<'_>) -> Result<(), String> {
    if target.id.is_empty() {
        return Err("Inbox target id must not be empty".to_string());
    }
    Ok(())
}

fn canonical_inbox_target(target: InboxTarget<'_>) -> CanonicalInboxTarget<'_> {
    CanonicalInboxTarget {
        kind: target.kind.as_str(),
        id: target.id,
    }
}

fn submitted_input_payload_json(
    input: &SubmittedInputEnqueue<'_>,
    published: &PublishedMailboxPayload,
) -> Result<String, String> {
    serde_json::to_string(&SubmittedInputPayloadMetadata {
        schema_version: 1,
        kind: SUBMITTED_INPUT_KIND,
        submission_token: input.submission_token,
        target: canonical_inbox_target(input.target),
        payload: SubmittedInputPayloadReference {
            address: &published.address,
            file_path: &published.file_path,
            sha256: &published.sha256,
            byte_len: published.byte_len,
            retention_policy: &published.retention_policy,
        },
    })
    .map_err(|err| format!("Failed to encode submitted input metadata: {err}"))
}

fn compacted_payload_json(
    kind: &str,
    published: &PublishedMailboxPayload,
) -> Result<String, String> {
    serde_json::to_string(&CompactedMailboxPayloadMetadata {
        schema_version: COMPACTED_PAYLOAD_SCHEMA_VERSION,
        kind,
        payload: SubmittedInputPayloadReference {
            address: &published.address,
            file_path: &published.file_path,
            sha256: &published.sha256,
            byte_len: published.byte_len,
            retention_policy: &published.retention_policy,
        },
    })
    .map_err(|err| format!("Failed to encode compacted mailbox payload metadata: {err}"))
}

fn delivered_payload_compaction_candidates(
    conn: &Connection,
    limit: usize,
) -> Result<Vec<DeliveredPayloadCompactionCandidate>, String> {
    let limit = i64::try_from(limit).map_err(|_| {
        "Delivered payload compaction limit does not fit SQLite INTEGER".to_string()
    })?;
    let mut statement = conn
        .prepare(
            "SELECT seq, kind, payload_json, payload_file_path, payload_sha256,
                    payload_byte_len, payload_retention_policy
             FROM mailbox
             WHERE kind = ?1
               AND delivered_at IS NOT NULL
               AND payload_compacted_at IS NULL
             ORDER BY seq ASC
             LIMIT ?2",
        )
        .map_err(|err| format!("Failed to prepare delivered payload compaction query: {err}"))?;
    let rows = statement
        .query_map(params![AGENT_BASH_COMPLETE_KIND, limit], |row| {
            Ok(DeliveredPayloadCompactionCandidate {
                seq: row.get(0)?,
                kind: row.get(1)?,
                payload_json: row.get(2)?,
                payload_file_path: row.get(3)?,
                payload_sha256: row.get(4)?,
                payload_byte_len: row.get(5)?,
                payload_retention_policy: row.get(6)?,
            })
        })
        .map_err(|err| format!("Failed to query delivered payload compaction rows: {err}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("Failed to read delivered payload compaction row: {err}"))
}

fn mark_payload_compacted(
    conn: &Connection,
    candidate: &DeliveredPayloadCompactionCandidate,
    published: &PublishedMailboxPayload,
    compacted_json: &str,
) -> Result<bool, String> {
    let compacted_at = now_rfc3339();
    let byte_len = i64::try_from(published.byte_len)
        .map_err(|_| "Compacted payload length does not fit SQLite INTEGER".to_string())?;
    conn.execute(
        "UPDATE mailbox
         SET payload_json = ?2,
             payload_file_path = ?3,
             payload_sha256 = ?4,
             payload_byte_len = ?5,
             payload_retention_policy = ?6,
             payload_compacted_at = ?7
         WHERE seq = ?1
           AND kind = ?8
           AND delivered_at IS NOT NULL
           AND payload_compacted_at IS NULL
           AND payload_json = ?9",
        params![
            candidate.seq,
            compacted_json,
            published.file_path.to_string_lossy().as_ref(),
            &published.sha256,
            byte_len,
            &published.retention_policy,
            compacted_at,
            AGENT_BASH_COMPLETE_KIND,
            &candidate.payload_json,
        ],
    )
    .map(|changed| changed == 1)
    .map_err(|err| {
        format!(
            "Failed to compact delivered mailbox row {}: {err}",
            candidate.seq
        )
    })
}

fn enqueue_submitted_input_in_tx(
    tx: &rusqlite::Transaction<'_>,
    input: &SubmittedInputEnqueue<'_>,
    handle: &str,
    payload_json: &str,
    published: &PublishedMailboxPayload,
    now: &str,
) -> Result<EnqueueResult, String> {
    let changed = insert_submitted_input_tx(tx, input, handle, payload_json, published, now)?;
    let row = query_mailbox_by_kind_handle_tx(tx, SUBMITTED_INPUT_KIND, handle)?
        .ok_or_else(|| "Input row missing after enqueue conflict check".to_string())?;
    Ok(submitted_input_enqueue_result(
        changed, row, input, published,
    ))
}

fn insert_submitted_input_tx(
    tx: &rusqlite::Transaction<'_>,
    input: &SubmittedInputEnqueue<'_>,
    handle: &str,
    payload_json: &str,
    published: &PublishedMailboxPayload,
    now: &str,
) -> Result<usize, String> {
    let payload_path = published.file_path.to_string_lossy();
    let payload_dir = published
        .file_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_string_lossy();
    // The predecessor table requires notification artifact columns. For input
    // rows they are non-authoritative compatibility carriers: kind-aware readers
    // use target_kind/target_id and payload_file_path, and must not interpret
    // session_id, rc, or notification paths as input facts.
    tx.execute(
        "INSERT OR IGNORE INTO mailbox (
            session_id, kind, handle, payload_json, enqueued_at,
            state_dir, meta_path, log_path, rc_path, rc,
            payload_file_path, payload_sha256, payload_byte_len,
            payload_retention_policy, submission_token, target_kind, target_id
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?7, 0, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            input.target.id,
            SUBMITTED_INPUT_KIND,
            handle,
            payload_json,
            now,
            payload_dir.as_ref(),
            payload_path.as_ref(),
            published.sha256,
            i64::try_from(published.byte_len)
                .map_err(|_| "Input payload length does not fit SQLite INTEGER".to_string())?,
            published.retention_policy,
            input.submission_token,
            input.target.kind.as_str(),
            input.target.id,
        ],
    )
    .map_err(|err| format!("Failed to insert submitted input row: {err}"))
}

fn submitted_input_enqueue_result(
    changed: usize,
    row: MailboxRow,
    input: &SubmittedInputEnqueue<'_>,
    published: &PublishedMailboxPayload,
) -> EnqueueResult {
    if changed > 0 {
        return EnqueueResult::Inserted(row);
    }
    if submitted_input_row_matches(&row, input, published) {
        EnqueueResult::AlreadyEnqueued(row)
    } else {
        EnqueueResult::Conflict { existing: row }
    }
}

fn submitted_input_row_matches(
    row: &MailboxRow,
    input: &SubmittedInputEnqueue<'_>,
    published: &PublishedMailboxPayload,
) -> bool {
    row.submission_token.as_deref() == Some(input.submission_token)
        && row.target_kind.as_deref() == Some(input.target.kind.as_str())
        && row.target_id.as_deref() == Some(input.target.id)
        && row.payload_sha256.as_deref() == Some(published.sha256.as_str())
        && row.payload_byte_len == i64::try_from(published.byte_len).ok()
}

fn enqueue_agent_bash_complete_in_tx(
    tx: &rusqlite::Transaction<'_>,
    input: &AgentBashCompleteEnqueue<'_>,
    payload_json: &str,
    published: &PublishedMailboxPayload,
    now: &str,
) -> Result<EnqueueResult, String> {
    let changed = insert_agent_bash_complete_tx(tx, input, payload_json, published, now)?;
    let row = query_mailbox_by_kind_handle_tx(tx, AGENT_BASH_COMPLETE_KIND, input.handle)?
        .ok_or_else(|| "Mailbox row missing after enqueue conflict check".to_string())?;
    Ok(agent_bash_enqueue_result(changed, row, input, published))
}

fn insert_agent_bash_complete_tx(
    tx: &rusqlite::Transaction<'_>,
    input: &AgentBashCompleteEnqueue<'_>,
    payload_json: &str,
    published: &PublishedMailboxPayload,
    now: &str,
) -> Result<usize, String> {
    tx.execute(
        "INSERT OR IGNORE INTO mailbox (
            session_id,
            kind,
            handle,
            payload_json,
            enqueued_at,
            owner_invocation_uuid,
            matched_os_pid,
            matched_os_boot_id,
            matched_os_pid_starttime_ticks,
            matched_chain_index,
            state_dir,
            meta_path,
            log_path,
            rc_path,
            rc,
            payload_file_path,
            payload_sha256,
            payload_byte_len,
            payload_retention_policy,
            payload_compacted_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?5)",
        params![
            input.session_id,
            AGENT_BASH_COMPLETE_KIND,
            input.handle,
            payload_json,
            now,
            input.owner_invocation_uuid,
            input.matched_os_pid,
            input.matched_os_boot_id,
            input.matched_os_pid_starttime_ticks,
            input.matched_chain_index,
            input.state_dir,
            input.meta_path,
            input.log_path,
            input.rc_path,
            input.rc,
            published.file_path.to_string_lossy().as_ref(),
            published.sha256,
            i64::try_from(published.byte_len)
                .map_err(|_| "Mailbox payload length does not fit SQLite INTEGER".to_string())?,
            published.retention_policy,
        ],
    )
    .map_err(|err| format!("Failed to insert mailbox row: {err}"))
}

fn agent_bash_enqueue_result(
    changed: usize,
    row: MailboxRow,
    input: &AgentBashCompleteEnqueue<'_>,
    published: &PublishedMailboxPayload,
) -> EnqueueResult {
    if changed > 0 {
        return EnqueueResult::Inserted(row);
    }
    if row.session_id == input.session_id && row_payload_matches(&row, input, published) {
        EnqueueResult::AlreadyEnqueued(row)
    } else {
        EnqueueResult::Conflict { existing: row }
    }
}

fn row_payload_matches(
    row: &MailboxRow,
    input: &AgentBashCompleteEnqueue<'_>,
    published: &PublishedMailboxPayload,
) -> bool {
    match (&row.payload_sha256, row.payload_byte_len) {
        (Some(sha256), Some(byte_len)) => {
            sha256 == &published.sha256 && byte_len == published.byte_len as i64
        }
        (None, None) => row.payload_json == input.payload_json,
        _ => false,
    }
}

fn validate_runtime_generation_create(
    request: &CreateRuntimeGeneration<'_>,
) -> Result<(), GenerationStorageError> {
    if request.spawn_invocation_uuid.is_empty() {
        return Err(GenerationStorageError::new(
            "Runtime generation spawn invocation UUID must not be empty".to_string(),
        ));
    }
    if request.provider_name.is_empty() {
        return Err(GenerationStorageError::new(
            "Runtime generation provider name must not be empty".to_string(),
        ));
    }
    validate_runtime_mode(request.runtime_mode)
}

fn current_runtime_creator_identity() -> Result<ProcessIdentity, GenerationStorageError> {
    let os_pid = i64::from(std::process::id());
    pid_identity::read_live_process_identity(os_pid)
        .map_err(GenerationStorageError::new)?
        .ok_or_else(|| {
            GenerationStorageError::new(format!(
                "Runtime generation creator process {os_pid} is not live"
            ))
        })
}

fn validate_runtime_mode(mode: &str) -> Result<(), GenerationStorageError> {
    match mode {
        "headless" | "pty_interactive" => Ok(()),
        other => Err(GenerationStorageError::new(format!(
            "Invalid runtime generation mode: {other}"
        ))),
    }
}

fn validate_runtime_generation_binding(
    request: &BindRuntimeGenerationRunning<'_>,
) -> Result<(), GenerationStorageError> {
    if request.spawned_os_pid <= 0 {
        return Err(GenerationStorageError::new(
            "Runtime generation spawned OS PID must be positive".to_string(),
        ));
    }
    if request.exact_process_identity.os_pid != request.spawned_os_pid {
        return Err(GenerationStorageError::new(
            "Exact process identity PID does not match spawned OS PID".to_string(),
        ));
    }
    Ok(())
}

fn runtime_generation_create_matches(
    row: &RuntimeGenerationRow,
    request: &CreateRuntimeGeneration<'_>,
    creator_process_identity: &ProcessIdentity,
) -> bool {
    row.lifecycle_state == RuntimeLifecycleState::Starting
        && row.spawn_invocation_uuid == request.spawn_invocation_uuid
        && row.session_id.as_deref() == request.session_id
        && row.runtime_mode == request.runtime_mode
        && row.provider_name == request.provider_name
        && row.model_name.as_deref() == request.model_name
        && row.pty_control_path.as_deref() == request.pty_control_path
        && row.models_dir.as_deref() == request.models_dir
        && row.effective_cwd.as_deref() == request.effective_cwd
        && row.creator_process_evidence
            == ExactProcessEvidence::Recorded(creator_process_identity.clone())
}

fn runtime_generation_binding_matches(
    row: &RuntimeGenerationRow,
    request: &BindRuntimeGenerationRunning<'_>,
) -> bool {
    row.spawned_os_pid == Some(request.spawned_os_pid)
        && row.exact_process_evidence
            == ExactProcessEvidence::Recorded(request.exact_process_identity.clone())
}

fn map_runtime_generation_create(
    changed: usize,
    row: RuntimeGenerationRow,
    request: &CreateRuntimeGeneration<'_>,
    creator_process_identity: &ProcessIdentity,
) -> GenerationMutation<RuntimeGenerationRow> {
    if changed == 1 {
        return GenerationMutation::Applied(row);
    }
    if runtime_generation_create_matches(&row, request, creator_process_identity) {
        GenerationMutation::AlreadyApplied(row)
    } else {
        GenerationMutation::Rejected(GenerationRejection::FenceMismatch)
    }
}

fn validate_generation_binding_fence(
    before: &RuntimeGenerationRow,
    request: &BindRuntimeGenerationRunning<'_>,
) -> Result<(), GenerationRejection> {
    validate_generation_fence(before, request.fence)
}

fn validate_generation_binding_predecessor(
    before: &RuntimeGenerationRow,
) -> Result<(), GenerationRejection> {
    if before.lifecycle_state == RuntimeLifecycleState::Starting {
        return Ok(());
    }
    Err(GenerationRejection::IllegalPredecessor {
        expected: RuntimeLifecycleState::Starting,
        actual: before.lifecycle_state,
    })
}

fn map_running_generation_binding_replay(
    before: RuntimeGenerationRow,
    request: &BindRuntimeGenerationRunning<'_>,
) -> GenerationMutation<RuntimeGenerationRow> {
    if runtime_generation_binding_matches(&before, request) {
        GenerationMutation::AlreadyApplied(before)
    } else {
        GenerationMutation::Rejected(GenerationRejection::ProcessIdentityConflict)
    }
}

fn bind_generation_process_identity(
    conn: &Connection,
    before: &RuntimeGenerationRow,
    request: &BindRuntimeGenerationRunning<'_>,
    recorded_at: &str,
) -> Result<bool, GenerationStorageError> {
    let identity = request.exact_process_identity;
    pid_identity::bind_identity_on(
        conn,
        pid_identity::PidIdentityRecord {
            identity,
            os_pgid: request.os_pgid,
            invocation_uuid: request.fence.spawn_invocation_uuid,
            session_id: before.session_id.as_deref(),
            provider_name: Some(&before.provider_name),
            model_name: before.model_name.as_deref(),
            recorded_at,
        },
    )
    .map_err(GenerationStorageError::new)
}

fn validate_generation_attachment_session_id(
    request: &AttachRuntimeGenerationSession<'_>,
) -> Result<(), GenerationStorageError> {
    if !request.session_id.is_empty() {
        return Ok(());
    }
    Err(GenerationStorageError::new(
        "Runtime generation session_id must not be empty".to_string(),
    ))
}

fn validate_generation_attachment_fence(
    before: &RuntimeGenerationRow,
    request: &AttachRuntimeGenerationSession<'_>,
) -> Result<(), GenerationRejection> {
    validate_generation_fence(before, request.fence)
}

fn map_generation_attachment_replay(
    before: RuntimeGenerationRow,
    request: &AttachRuntimeGenerationSession<'_>,
) -> GenerationMutation<RuntimeGenerationRow> {
    if before.session_id.as_deref() == Some(request.session_id) {
        GenerationMutation::AlreadyApplied(before)
    } else {
        GenerationMutation::Rejected(GenerationRejection::SessionConflict)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExistingDeliveryClaimDisposition {
    AlreadyInFlight,
    Recover,
}

fn validate_existing_delivery_claim(
    before: &RuntimeGenerationRow,
) -> Result<(), GenerationRejection> {
    if !before.active_delivery_seqs.is_empty() && before.active_delivery_claimed_at.is_some() {
        return Ok(());
    }
    Err(GenerationRejection::InvariantViolation)
}

fn map_existing_delivery_claim(
    _before: &RuntimeGenerationRow,
    stale: bool,
) -> ExistingDeliveryClaimDisposition {
    if stale {
        ExistingDeliveryClaimDisposition::Recover
    } else {
        ExistingDeliveryClaimDisposition::AlreadyInFlight
    }
}

fn validate_new_delivery_claim(
    before: &RuntimeGenerationRow,
    rows_pending: bool,
    overlaps: bool,
) -> Result<(), GenerationRejection> {
    if before.lifecycle_state != RuntimeLifecycleState::Running {
        return Err(GenerationRejection::IllegalPredecessor {
            expected: RuntimeLifecycleState::Running,
            actual: before.lifecycle_state,
        });
    }
    if before.drain_request_id.is_some() {
        return Err(GenerationRejection::DrainRequestConflict);
    }
    if !rows_pending || overlaps {
        return Err(GenerationRejection::InvariantViolation);
    }
    Ok(())
}

fn map_acquired_delivery_claim(row: RuntimeGenerationRow) -> DeliveryClaimAcquireResult {
    DeliveryClaimAcquireResult::Acquired(row)
}

fn validate_active_delivery_claim(
    row: Option<&RuntimeGenerationRow>,
    fence: RuntimeGenerationFence<'_>,
    claim_id: &DeliveryClaimId,
    seqs: &[i64],
) -> Result<(), GenerationRejection> {
    let Some(row) = row else {
        return Err(GenerationRejection::NotFound);
    };
    validate_generation_fence(row, fence)?;
    runtime_generation_session_id(row)?;
    if row.active_delivery_claim_id.as_ref() != Some(claim_id)
        || row.active_delivery_seqs != seqs
        || row.active_delivery_claimed_at.is_none()
    {
        return Err(GenerationRejection::InvariantViolation);
    }
    Ok(())
}

fn validate_running_delivery_confirmation(
    row: &RuntimeGenerationRow,
) -> Result<(), GenerationRejection> {
    if row.lifecycle_state == RuntimeLifecycleState::Running {
        return Ok(());
    }
    Err(GenerationRejection::IllegalPredecessor {
        expected: RuntimeLifecycleState::Running,
        actual: row.lifecycle_state,
    })
}

fn runtime_generation_session_id(row: &RuntimeGenerationRow) -> Result<&str, GenerationRejection> {
    row.session_id
        .as_deref()
        .filter(|session_id| !session_id.is_empty())
        .ok_or(GenerationRejection::SessionConflict)
}

fn validate_claimed_mailbox_row_change(
    seq: i64,
    changed: usize,
    operation: &str,
) -> Result<(), GenerationStorageError> {
    if changed == 1 {
        return Ok(());
    }
    Err(GenerationStorageError::new(format!(
        "Claimed mailbox row {seq} was not owned and pending at {operation}"
    )))
}

fn map_applied_generation(row: RuntimeGenerationRow) -> GenerationMutation<RuntimeGenerationRow> {
    GenerationMutation::Applied(row)
}

fn map_failed_delivery_generation(
    row: RuntimeGenerationRow,
) -> GenerationMutation<RuntimeGenerationRow> {
    GenerationMutation::Applied(row)
}

fn validate_non_orderly_reason(
    request: &ExitRuntimeGenerationNonOrderly<'_>,
) -> Result<(), GenerationStorageError> {
    if request.reason != RuntimeTerminalReason::OrderlyCompletion {
        return Ok(());
    }
    Err(GenerationStorageError::new(
        "Orderly completion must pass through draining".to_string(),
    ))
}

fn validate_generation_fence(
    before: &RuntimeGenerationRow,
    fence: RuntimeGenerationFence<'_>,
) -> Result<(), GenerationRejection> {
    if before.spawn_invocation_uuid == fence.spawn_invocation_uuid {
        return Ok(());
    }
    Err(GenerationRejection::FenceMismatch)
}

fn validate_recovered_dead_process(
    before: &RuntimeGenerationRow,
    request: &ExitRuntimeGenerationNonOrderly<'_>,
) -> Result<(), GenerationRejection> {
    if request.reason != RuntimeTerminalReason::RecoveredDead {
        return Ok(());
    }
    let ExactProcessEvidence::Recorded(recorded) = generation_liveness_process(before) else {
        return Err(GenerationRejection::InvariantViolation);
    };
    match pid_identity::observe_live_process_identity(recorded.os_pid) {
        pid_identity::ProcessIdentityObservation::ExactLive(live) if live == *recorded => {
            Err(GenerationRejection::ProcessIdentityConflict)
        }
        pid_identity::ProcessIdentityObservation::ExactLive(_)
        | pid_identity::ProcessIdentityObservation::Dead => Ok(()),
        pid_identity::ProcessIdentityObservation::Unsupported
        | pid_identity::ProcessIdentityObservation::ReadError(_) => {
            Err(GenerationRejection::InvariantViolation)
        }
    }
}

fn validate_non_orderly_predecessor(
    before: &RuntimeGenerationRow,
    request: &ExitRuntimeGenerationNonOrderly<'_>,
) -> Result<(), GenerationRejection> {
    if matches!(
        before.lifecycle_state,
        RuntimeLifecycleState::Starting | RuntimeLifecycleState::Running
    ) || (before.lifecycle_state == RuntimeLifecycleState::Draining
        && request.reason == RuntimeTerminalReason::RecoveredDead)
    {
        return Ok(());
    }
    Err(GenerationRejection::IllegalPredecessor {
        expected: RuntimeLifecycleState::Running,
        actual: before.lifecycle_state,
    })
}

fn map_non_orderly_exit_replay(
    before: &RuntimeGenerationRow,
    request: &ExitRuntimeGenerationNonOrderly<'_>,
) -> Option<GenerationMutation<RuntimeGenerationRow>> {
    if before.lifecycle_state != RuntimeLifecycleState::Exited {
        return None;
    }
    if before.terminal_reason == Some(request.reason) && before.exit_code == request.exit_code {
        Some(GenerationMutation::AlreadyApplied(before.clone()))
    } else {
        Some(GenerationMutation::Rejected(
            GenerationRejection::IllegalPredecessor {
                expected: RuntimeLifecycleState::Running,
                actual: RuntimeLifecycleState::Exited,
            },
        ))
    }
}

fn validate_drain_request_fence(
    before: &RuntimeGenerationRow,
    request: &RequestRuntimeGenerationDrain<'_>,
) -> Result<(), GenerationRejection> {
    validate_generation_fence(before, request.fence)
}

fn validate_drain_request_predecessor(
    before: &RuntimeGenerationRow,
) -> Result<(), GenerationRejection> {
    if before.lifecycle_state == RuntimeLifecycleState::Running {
        return Ok(());
    }
    Err(GenerationRejection::IllegalPredecessor {
        expected: RuntimeLifecycleState::Running,
        actual: before.lifecycle_state,
    })
}

fn map_drain_request_replay(
    before: &RuntimeGenerationRow,
    request: &RequestRuntimeGenerationDrain<'_>,
) -> Option<DrainRequestResult> {
    let existing = before.drain_request_id.as_ref()?;
    if existing == request.drain_request_id {
        Some(DrainRequestResult::AlreadyInstalled(
            before.clone(),
            drain_handoff(before),
        ))
    } else {
        Some(DrainRequestResult::Rejected(
            GenerationRejection::DrainRequestConflict,
        ))
    }
}

fn map_installed_drain_request(row: RuntimeGenerationRow) -> DrainRequestResult {
    let handoff = drain_handoff(&row);
    DrainRequestResult::Installed(row, handoff)
}

fn validate_drain_advance_identity(
    before: &RuntimeGenerationRow,
    request: &AdvanceRuntimeGenerationDrain<'_>,
) -> Result<(), GenerationRejection> {
    validate_generation_fence(before, request.fence)?;
    if before.drain_request_id.as_ref() == Some(request.drain_request_id) {
        return Ok(());
    }
    Err(GenerationRejection::DrainRequestConflict)
}

fn map_drain_advance_blocker(before: &RuntimeGenerationRow) -> Option<DrainAdvanceResult> {
    if let Some(claim_id) = before.active_delivery_claim_id.clone() {
        return Some(DrainAdvanceResult::WaitingOnClaim(claim_id));
    }
    match before.lifecycle_state {
        RuntimeLifecycleState::Running => None,
        RuntimeLifecycleState::Draining => {
            Some(DrainAdvanceResult::AlreadyDraining(before.clone()))
        }
        RuntimeLifecycleState::Exited => Some(DrainAdvanceResult::AlreadyExited(before.clone())),
        actual => Some(DrainAdvanceResult::Rejected(
            GenerationRejection::IllegalPredecessor {
                expected: RuntimeLifecycleState::Running,
                actual,
            },
        )),
    }
}

fn map_advanced_drain(row: RuntimeGenerationRow) -> DrainAdvanceResult {
    DrainAdvanceResult::Advanced(row)
}

fn validate_drain_finish_identity(
    before: &RuntimeGenerationRow,
    request: &FinishRuntimeGenerationDrain<'_>,
) -> Result<(), GenerationRejection> {
    validate_generation_fence(before, request.fence)?;
    if before.drain_request_id.as_ref() == Some(request.drain_request_id) {
        return Ok(());
    }
    Err(GenerationRejection::DrainRequestConflict)
}

fn validate_drain_finish_claim(before: &RuntimeGenerationRow) -> Result<(), GenerationRejection> {
    if before.active_delivery_claim_id.is_none() {
        return Ok(());
    }
    Err(GenerationRejection::InvariantViolation)
}

fn map_drain_finish_predecessor(before: &RuntimeGenerationRow) -> Option<DrainFinishResult> {
    match before.lifecycle_state {
        RuntimeLifecycleState::Draining => None,
        RuntimeLifecycleState::Exited => Some(DrainFinishResult::AlreadyExited(before.clone())),
        actual => Some(DrainFinishResult::NotDraining(actual)),
    }
}

fn map_finished_drain(row: RuntimeGenerationRow) -> DrainFinishResult {
    DrainFinishResult::Finished(row)
}

fn generation_storage_error(
    action: &'static str,
) -> impl FnOnce(rusqlite::Error) -> GenerationStorageError {
    move |err| GenerationStorageError::new(format!("Failed to {action}: {err}"))
}

fn runtime_generation_by_id_on(
    conn: &Connection,
    generation_id: &RuntimeGenerationId,
) -> Result<Option<RuntimeGenerationRow>, GenerationStorageError> {
    conn.query_row(
        &runtime_generation_select_sql("generation_uuid = ?1"),
        params![generation_id.to_string()],
        map_runtime_generation_row,
    )
    .optional()
    .map_err(generation_storage_error("read runtime generation by UUID"))
}

fn runtime_generations_by_process_identity_on(
    conn: &Connection,
    identity: &ProcessIdentity,
) -> Result<Vec<RuntimeGenerationRow>, GenerationStorageError> {
    let mut statement = conn
        .prepare(&runtime_generation_select_sql(
            "identity_os_pid = ?1 AND identity_os_boot_id = ?2 AND identity_os_pid_starttime_ticks = ?3",
        ))
        .map_err(generation_storage_error("prepare exact runtime generation lookup"))?;
    let rows = statement
        .query_map(
            params![
                identity.os_pid,
                &identity.os_boot_id,
                identity.os_pid_starttime_ticks,
            ],
            map_runtime_generation_row,
        )
        .map_err(generation_storage_error(
            "query exact runtime generation lookup",
        ))?;
    collect_runtime_generation_rows(rows)
}

fn format_runtime_generations_for_session_sql(nonterminal_only: bool) -> String {
    let predicate = if nonterminal_only {
        "session_id = ?1 AND lifecycle_state != 'exited'"
    } else {
        "session_id = ?1"
    };
    format!(
        "{} ORDER BY created_at ASC, generation_uuid ASC",
        runtime_generation_select_sql(predicate)
    )
}

fn runtime_generations_for_session_on(
    conn: &Connection,
    session_id: &str,
    sql: &str,
) -> Result<Vec<RuntimeGenerationRow>, GenerationStorageError> {
    let mut statement = conn.prepare(sql).map_err(generation_storage_error(
        "prepare session runtime generation lookup",
    ))?;
    let rows = statement
        .query_map(params![session_id], map_runtime_generation_row)
        .map_err(generation_storage_error(
            "query session runtime generations",
        ))?;
    collect_runtime_generation_rows(rows)
}

fn runtime_generation_select_sql(predicate: &str) -> String {
    format!(
        "SELECT generation_uuid, lifecycle_state, spawn_invocation_uuid, session_id,
                runtime_mode, provider_name, model_name, pty_control_path, models_dir,
                effective_cwd, spawned_os_pid, identity_os_pid, identity_os_boot_id,
                identity_os_pid_starttime_ticks, created_at, running_at, draining_at,
                exited_at, terminal_reason, exit_code, drain_request_uuid,
                drain_requested_at, drain_requested_by_invocation_uuid,
                active_delivery_claim_uuid, active_delivery_claimed_at,
                active_delivery_seqs_json, creator_identity_os_pid,
                creator_identity_os_boot_id, creator_identity_os_pid_starttime_ticks
         FROM runtime_generation
         WHERE {predicate}"
    )
}

fn collect_runtime_generation_rows<F>(
    rows: rusqlite::MappedRows<'_, F>,
) -> Result<Vec<RuntimeGenerationRow>, GenerationStorageError>
where
    F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<RuntimeGenerationRow>,
{
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(generation_storage_error("read runtime generation row"))
}

struct RawRuntimeGenerationFields {
    generation_uuid: String,
    lifecycle_state: String,
    identity_os_pid: Option<i64>,
    identity_os_boot_id: Option<String>,
    identity_os_pid_starttime_ticks: Option<i64>,
    creator_identity_os_pid: Option<i64>,
    creator_identity_os_boot_id: Option<String>,
    creator_identity_os_pid_starttime_ticks: Option<i64>,
    terminal_reason: Option<String>,
    drain_request_uuid: Option<String>,
    active_delivery_claim_uuid: Option<String>,
    active_delivery_seqs_json: Option<String>,
}

#[derive(Debug)]
struct ParsedRuntimeGenerationFields {
    generation_id: RuntimeGenerationId,
    lifecycle_state: RuntimeLifecycleState,
    exact_process_evidence: ExactProcessEvidence,
    creator_process_evidence: ExactProcessEvidence,
    terminal_reason: Option<RuntimeTerminalReason>,
    drain_request_id: Option<DrainRequestId>,
    active_delivery_claim_id: Option<DeliveryClaimId>,
    active_delivery_seqs: Vec<i64>,
}

fn parse_runtime_generation_fields(
    raw: RawRuntimeGenerationFields,
) -> rusqlite::Result<ParsedRuntimeGenerationFields> {
    let generation_id =
        RuntimeGenerationId::parse(&raw.generation_uuid).map_err(to_sql_conversion_error)?;
    let lifecycle_state =
        RuntimeLifecycleState::parse(&raw.lifecycle_state).map_err(to_sql_conversion_error)?;
    let exact_process_evidence = exact_process_evidence_from_columns(
        raw.identity_os_pid,
        raw.identity_os_boot_id,
        raw.identity_os_pid_starttime_ticks,
        11,
    )?;
    let creator_process_evidence = exact_process_evidence_from_columns(
        raw.creator_identity_os_pid,
        raw.creator_identity_os_boot_id,
        raw.creator_identity_os_pid_starttime_ticks,
        26,
    )?;
    let terminal_reason = raw
        .terminal_reason
        .as_deref()
        .map(RuntimeTerminalReason::parse)
        .transpose()
        .map_err(to_sql_conversion_error)?;
    let drain_request_id = raw
        .drain_request_uuid
        .as_deref()
        .map(DrainRequestId::parse)
        .transpose()
        .map_err(to_sql_conversion_error)?;
    let active_delivery_claim_id = raw
        .active_delivery_claim_uuid
        .as_deref()
        .map(DeliveryClaimId::parse)
        .transpose()
        .map_err(to_sql_conversion_error)?;
    let has_active_delivery_seqs = raw.active_delivery_seqs_json.is_some();
    let active_delivery_seqs = raw
        .active_delivery_seqs_json
        .as_deref()
        .map(parse_delivery_seqs)
        .transpose()
        .map_err(to_sql_conversion_error)?
        .unwrap_or_default();
    if has_active_delivery_seqs {
        validate_delivery_claim_seqs(&active_delivery_seqs).map_err(to_sql_conversion_error)?;
    }
    Ok(ParsedRuntimeGenerationFields {
        generation_id,
        lifecycle_state,
        exact_process_evidence,
        creator_process_evidence,
        terminal_reason,
        drain_request_id,
        active_delivery_claim_id,
        active_delivery_seqs,
    })
}

fn map_runtime_generation_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RuntimeGenerationRow> {
    let parsed = parse_runtime_generation_fields(RawRuntimeGenerationFields {
        generation_uuid: row.get(0)?,
        lifecycle_state: row.get(1)?,
        identity_os_pid: row.get(11)?,
        identity_os_boot_id: row.get(12)?,
        identity_os_pid_starttime_ticks: row.get(13)?,
        creator_identity_os_pid: row.get(26)?,
        creator_identity_os_boot_id: row.get(27)?,
        creator_identity_os_pid_starttime_ticks: row.get(28)?,
        terminal_reason: row.get(18)?,
        drain_request_uuid: row.get(20)?,
        active_delivery_claim_uuid: row.get(23)?,
        active_delivery_seqs_json: row.get(25)?,
    })?;
    Ok(RuntimeGenerationRow {
        generation_id: parsed.generation_id,
        lifecycle_state: parsed.lifecycle_state,
        spawn_invocation_uuid: row.get(2)?,
        session_id: row.get(3)?,
        runtime_mode: row.get(4)?,
        provider_name: row.get(5)?,
        model_name: row.get(6)?,
        pty_control_path: row.get(7)?,
        models_dir: row.get(8)?,
        effective_cwd: row.get(9)?,
        spawned_os_pid: row.get(10)?,
        exact_process_evidence: parsed.exact_process_evidence,
        creator_process_evidence: parsed.creator_process_evidence,
        created_at: row.get(14)?,
        running_at: row.get(15)?,
        draining_at: row.get(16)?,
        exited_at: row.get(17)?,
        terminal_reason: parsed.terminal_reason,
        exit_code: row.get(19)?,
        drain_request_id: parsed.drain_request_id,
        drain_requested_at: row.get(21)?,
        drain_requested_by_invocation_uuid: row.get(22)?,
        active_delivery_claim_id: parsed.active_delivery_claim_id,
        active_delivery_claimed_at: row.get(24)?,
        active_delivery_seqs: parsed.active_delivery_seqs,
    })
}

fn exact_process_evidence_from_columns(
    os_pid: Option<i64>,
    os_boot_id: Option<String>,
    os_pid_starttime_ticks: Option<i64>,
    column_index: usize,
) -> rusqlite::Result<ExactProcessEvidence> {
    match (os_pid, os_boot_id, os_pid_starttime_ticks) {
        (None, None, None) => Ok(ExactProcessEvidence::NotRecorded),
        (Some(os_pid), Some(os_boot_id), Some(os_pid_starttime_ticks)) => {
            Ok(ExactProcessEvidence::Recorded(ProcessIdentity {
                os_pid,
                os_boot_id,
                os_pid_starttime_ticks,
            }))
        }
        _ => Err(rusqlite::Error::FromSqlConversionFailure(
            column_index,
            rusqlite::types::Type::Null,
            Box::new(GenerationStorageError::new(
                "Runtime generation has partial exact process identity evidence".to_string(),
            )),
        )),
    }
}

fn to_sql_conversion_error(error: GenerationStorageError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}

fn drain_handoff(row: &RuntimeGenerationRow) -> DrainHandoff {
    match row.active_delivery_claim_id.clone() {
        Some(claim_id) => DrainHandoff::ClaimOutstanding {
            generation_id: row.generation_id.clone(),
            claim_id,
        },
        None => DrainHandoff::Ready,
    }
}

fn validate_delivery_claim_seqs(seqs: &[i64]) -> Result<(), GenerationStorageError> {
    if seqs.is_empty() || seqs.iter().any(|seq| *seq <= 0) {
        return Err(GenerationStorageError::new(
            "Delivery claim requires positive mailbox sequence numbers".to_string(),
        ));
    }
    if seqs.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(GenerationStorageError::new(
            "Delivery claim mailbox sequence numbers must be strictly increasing".to_string(),
        ));
    }
    Ok(())
}

fn reject_unauthorized_terminal_wake_abandonment(delivery_error: &str) -> Result<(), String> {
    if delivery_error == WAKE_SWEEP_ABANDONED_ERROR {
        return Err(
            "Terminal wake abandonment requires a dedicated authority-bearing disposition"
                .to_string(),
        );
    }
    Ok(())
}

fn serialize_delivery_seqs(seqs: &[i64]) -> Result<String, GenerationStorageError> {
    serde_json::to_string(seqs).map_err(|err| {
        GenerationStorageError::new(format!(
            "Failed to serialize runtime generation delivery batch: {err}"
        ))
    })
}

fn parse_delivery_seqs(value: &str) -> Result<Vec<i64>, GenerationStorageError> {
    serde_json::from_str::<Vec<i64>>(value).map_err(|err| {
        GenerationStorageError::new(format!(
            "Failed to parse runtime generation delivery batch: {err}"
        ))
    })
}

fn runtime_delivery_claim_is_stale(row: &RuntimeGenerationRow, stale_after_seconds: i64) -> bool {
    let Some(claimed_at) = row
        .active_delivery_claimed_at
        .as_deref()
        .and_then(parse_claimed_at)
    else {
        return true;
    };
    claim_age_exceeds_stale_after(claimed_at, stale_after_seconds)
}

fn mailbox_delivery_states_on(
    conn: &Connection,
    session_id: &str,
    seqs: &[i64],
) -> Result<Vec<Option<Option<String>>>, GenerationStorageError> {
    let mut states = Vec::with_capacity(seqs.len());
    for seq in seqs {
        let state = conn
            .query_row(
                "SELECT delivered_at FROM mailbox WHERE session_id = ?1 AND seq = ?2",
                params![session_id, seq],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(generation_storage_error(
                "verify pending mailbox delivery claim row",
            ))?;
        states.push(state);
    }
    Ok(states)
}

fn mailbox_delivery_target_states_on(
    conn: &Connection,
    session_id: &str,
    chain_id: Option<&str>,
    seqs: &[i64],
) -> Result<Vec<Option<Option<String>>>, GenerationStorageError> {
    let sql = format!(
        "SELECT delivered_at FROM mailbox
         WHERE seq = ?3 AND {PENDING_MAILBOX_TARGET_PREDICATE}"
    );
    let mut states = Vec::with_capacity(seqs.len());
    for seq in seqs {
        let state = conn
            .query_row(&sql, params![session_id, chain_id, seq], |row| {
                row.get::<_, Option<String>>(0)
            })
            .optional()
            .map_err(generation_storage_error(
                "verify mailbox delivery target authority",
            ))?;
        states.push(state);
    }
    Ok(states)
}

fn all_mailbox_seqs_owned(states: &[Option<Option<String>>]) -> bool {
    states.iter().all(Option::is_some)
}

fn all_mailbox_seqs_pending(states: &[Option<Option<String>>]) -> bool {
    states.iter().all(|state| matches!(state, Some(None)))
}

fn active_delivery_claim_encodings_on(
    conn: &Connection,
    generation_id: &RuntimeGenerationId,
) -> Result<Vec<Option<String>>, GenerationStorageError> {
    let mut statement = conn
        .prepare(
            "SELECT active_delivery_seqs_json
             FROM runtime_generation
             WHERE generation_uuid != ?1
               AND active_delivery_claim_uuid IS NOT NULL",
        )
        .map_err(generation_storage_error(
            "prepare active delivery claim overlap query",
        ))?;
    let rows = statement
        .query_map(params![generation_id.to_string()], |row| {
            row.get::<_, Option<String>>(0)
        })
        .map_err(generation_storage_error(
            "query active delivery claim overlap",
        ))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(generation_storage_error(
            "read active delivery claim overlap row",
        ))
}

fn parse_active_delivery_claim_batch(value: String) -> Result<Vec<i64>, GenerationStorageError> {
    let seqs = parse_delivery_seqs(&value)?;
    validate_delivery_claim_seqs(&seqs)?;
    Ok(seqs)
}

fn parse_active_delivery_claim_batches(
    values: Vec<Option<String>>,
) -> Result<Vec<Vec<i64>>, GenerationStorageError> {
    if values.iter().any(Option::is_none) {
        return Ok(vec![Vec::new()]);
    }
    values
        .into_iter()
        .map(|value| parse_active_delivery_claim_batch(value.expect("missing values returned")))
        .collect()
}

fn delivery_claim_batches_overlap(batches: &[Vec<i64>], requested_seqs: &[i64]) -> bool {
    batches
        .iter()
        .any(|claimed_seqs| delivery_claim_batch_overlaps(claimed_seqs, requested_seqs))
}

fn delivery_claim_batch_overlaps(claimed_seqs: &[i64], requested_seqs: &[i64]) -> bool {
    claimed_seqs.is_empty()
        || claimed_seqs
            .iter()
            .any(|seq| requested_seqs.binary_search(seq).is_ok())
}

fn clear_runtime_delivery_claim_on(
    conn: &Connection,
    fence: RuntimeGenerationFence<'_>,
    claim_id: &DeliveryClaimId,
) -> Result<(), GenerationStorageError> {
    let changed = conn
        .execute(
            "UPDATE runtime_generation
             SET active_delivery_claim_uuid = NULL,
                 active_delivery_claimed_at = NULL,
                 active_delivery_seqs_json = NULL
             WHERE generation_uuid = ?1
               AND spawn_invocation_uuid = ?2
               AND active_delivery_claim_uuid = ?3",
            params![
                fence.generation_id.to_string(),
                fence.spawn_invocation_uuid,
                claim_id.to_string(),
            ],
        )
        .map_err(generation_storage_error(
            "clear runtime generation delivery claim",
        ))?;
    if changed != 1 {
        return Err(GenerationStorageError::new(
            "Runtime generation delivery claim changed before settlement".to_string(),
        ));
    }
    Ok(())
}

fn project_running_generation_on(
    conn: &Connection,
    generation: &RuntimeGenerationRow,
) -> Result<(), GenerationStorageError> {
    if generation.lifecycle_state != RuntimeLifecycleState::Running {
        return Ok(());
    }
    let Some(session_id) = generation.session_id.as_deref() else {
        return Ok(());
    };
    let ExactProcessEvidence::Recorded(identity) = &generation.exact_process_evidence else {
        return Ok(());
    };
    let projected_at = generation.running_at.as_deref().ok_or_else(|| {
        GenerationStorageError::new(
            "Running runtime generation has no running timestamp for compatibility projection"
                .to_string(),
        )
    })?;
    let turn_start_max_mailbox_seq =
        max_mailbox_seq_on(conn, session_id).map_err(GenerationStorageError::new)?;
    project_runtime_compatibility_row(
        conn,
        LegacyRuntimeProjection {
            session_id,
            mode: &generation.runtime_mode,
            invocation_uuid: &generation.spawn_invocation_uuid,
            provider_name: Some(&generation.provider_name),
            model_name: generation.model_name.as_deref(),
            identity,
            pty_control_path: generation.pty_control_path.as_deref(),
            turn_start_max_mailbox_seq,
            models_dir: generation.models_dir.as_deref(),
            effective_cwd: generation.effective_cwd.as_deref(),
        },
        projected_at,
        turn_start_max_mailbox_seq,
    )
    .map_err(GenerationStorageError::new)
}

fn project_exited_generation_on(
    conn: &Connection,
    generation: &RuntimeGenerationRow,
    projected_at: &str,
    compatibility_exit_code: Option<i32>,
) -> Result<(), GenerationStorageError> {
    let Some(session_id) = generation.session_id.as_deref() else {
        return Ok(());
    };
    settle_runtime_compatibility_row(
        conn,
        LegacyRuntimeProjectionSettlement {
            session_id,
            invocation_uuid: &generation.spawn_invocation_uuid,
            last_exit_code: compatibility_exit_code,
        },
        projected_at,
    )
    .map(|_| ())
    .map_err(GenerationStorageError::new)
}

fn project_runtime_compatibility_row(
    conn: &Connection,
    input: LegacyRuntimeProjection<'_>,
    now: &str,
    turn_start_max_mailbox_seq: Option<i64>,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO session_runtime (
            session_id,
            mode,
            invocation_uuid,
            provider_name,
            model_name,
            pty_control_path,
            updated_at,
            run_state,
            running_invocation_uuid,
            running_os_pid,
            running_os_boot_id,
            running_os_pid_starttime_ticks,
            turn_started_at,
            turn_ended_at,
            turn_start_max_mailbox_seq,
            last_exit_code,
            models_dir,
            effective_cwd
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'running', ?3, ?8, ?9, ?10, ?7, NULL, ?11, NULL, ?12, ?13)
         ON CONFLICT(session_id)
         DO UPDATE SET
            mode = excluded.mode,
            invocation_uuid = CASE
                WHEN session_runtime.invocation_uuid IS NOT NULL
                 AND EXISTS (
                    SELECT 1
                    FROM session_wake_claim
                    WHERE session_id = excluded.session_id
                 )
                THEN session_runtime.invocation_uuid
                ELSE excluded.invocation_uuid
            END,
            provider_name = excluded.provider_name,
            model_name = excluded.model_name,
            pty_control_path = excluded.pty_control_path,
            updated_at = excluded.updated_at,
            run_state = 'running',
            running_invocation_uuid = excluded.running_invocation_uuid,
            running_os_pid = excluded.running_os_pid,
            running_os_boot_id = excluded.running_os_boot_id,
            running_os_pid_starttime_ticks = excluded.running_os_pid_starttime_ticks,
            turn_started_at = excluded.turn_started_at,
            turn_ended_at = NULL,
            turn_start_max_mailbox_seq = excluded.turn_start_max_mailbox_seq,
            last_exit_code = NULL,
            models_dir = COALESCE(excluded.models_dir, session_runtime.models_dir),
            effective_cwd = COALESCE(excluded.effective_cwd, session_runtime.effective_cwd)",
        params![
            input.session_id,
            input.mode,
            input.invocation_uuid,
            input.provider_name,
            input.model_name,
            input.pty_control_path,
            now,
            input.identity.os_pid,
            &input.identity.os_boot_id,
            input.identity.os_pid_starttime_ticks,
            turn_start_max_mailbox_seq,
            input.models_dir,
            input.effective_cwd,
        ],
    )
    .map_err(|err| format!("Failed to mark session runtime running: {err}"))?;
    Ok(())
}

fn settle_runtime_compatibility_row(
    conn: &Connection,
    input: LegacyRuntimeProjectionSettlement<'_>,
    now: &str,
) -> Result<bool, String> {
    let changed = settle_runtime_compatibility_row_count(conn, input, now)?;
    Ok(row_changed(changed))
}

fn settle_runtime_compatibility_row_count(
    conn: &Connection,
    input: LegacyRuntimeProjectionSettlement<'_>,
    now: &str,
) -> Result<usize, String> {
    conn.execute(
        "UPDATE session_runtime
         SET run_state = 'idle',
             updated_at = ?3,
             pty_control_path = NULL,
             running_invocation_uuid = NULL,
             running_os_pid = NULL,
             running_os_boot_id = NULL,
             running_os_pid_starttime_ticks = NULL,
             turn_ended_at = ?3,
             last_exit_code = ?4
         WHERE session_id = ?1
           AND running_invocation_uuid = ?2",
        params![
            input.session_id,
            input.invocation_uuid,
            now,
            input.last_exit_code,
        ],
    )
    .map_err(|err| format!("Failed to mark session runtime idle: {err}"))
}

fn row_changed(changed: usize) -> bool {
    changed > 0
}

fn session_metadata_row(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<SessionMetadataRow>, String> {
    conn.query_row(
        "SELECT session_id, mode, invocation_uuid, provider_name, model_name, updated_at,
                models_dir, effective_cwd, auto_wake_count
         FROM session_runtime
         WHERE session_id = ?1",
        params![session_id],
        map_session_metadata_row,
    )
    .optional()
    .map_err(|err| format!("Failed to read session metadata row: {err}"))
}

fn legacy_runtime_projection_row(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<LegacyRuntimeProjectionRow>, String> {
    conn.query_row(
        "SELECT session_id, pty_control_path, updated_at, run_state,
                running_invocation_uuid, running_os_pid, running_os_boot_id,
                running_os_pid_starttime_ticks, turn_started_at, turn_ended_at,
                turn_start_max_mailbox_seq, last_exit_code
         FROM session_runtime
         WHERE session_id = ?1",
        params![session_id],
        map_legacy_runtime_projection_row,
    )
    .optional()
    .map_err(|err| format!("Failed to read legacy runtime projection row: {err}"))
}

fn max_mailbox_seq_on(conn: &Connection, session_id: &str) -> Result<Option<i64>, String> {
    conn.query_row(
        "SELECT MAX(seq) FROM mailbox WHERE session_id = ?1",
        params![session_id],
        |row| row.get(0),
    )
    .map_err(|err| format!("Failed to read mailbox max seq: {err}"))
}

enum GenerationLivenessObservation {
    Busy,
    Stale,
}

fn generation_liveness_observation(
    generation: &RuntimeGenerationRow,
) -> GenerationLivenessObservation {
    let ExactProcessEvidence::Recorded(recorded) = generation_liveness_process(generation) else {
        return GenerationLivenessObservation::Busy;
    };
    match pid_identity::observe_live_process_identity(recorded.os_pid) {
        pid_identity::ProcessIdentityObservation::ExactLive(live) if live == *recorded => {
            GenerationLivenessObservation::Busy
        }
        pid_identity::ProcessIdentityObservation::ExactLive(_)
        | pid_identity::ProcessIdentityObservation::Dead => GenerationLivenessObservation::Stale,
        pid_identity::ProcessIdentityObservation::Unsupported
        | pid_identity::ProcessIdentityObservation::ReadError(_) => {
            GenerationLivenessObservation::Busy
        }
    }
}

fn generation_liveness_process(generation: &RuntimeGenerationRow) -> &ExactProcessEvidence {
    if generation.lifecycle_state == RuntimeLifecycleState::Starting {
        &generation.creator_process_evidence
    } else {
        &generation.exact_process_evidence
    }
}

fn classify_generation_liveness_read_only(
    generations: &[RuntimeGenerationRow],
) -> RuntimeGenerationReadOnlyLiveness {
    if generations.is_empty() {
        return RuntimeGenerationReadOnlyLiveness::Idle;
    }
    let mut stale = RuntimeGenerationReadOnlyLiveness::StaleMissingIdentity;
    for generation in generations {
        let ExactProcessEvidence::Recorded(recorded) = generation_liveness_process(generation)
        else {
            continue;
        };
        match pid_identity::observe_live_process_identity(recorded.os_pid) {
            pid_identity::ProcessIdentityObservation::ExactLive(live) if live == *recorded => {
                return RuntimeGenerationReadOnlyLiveness::Busy;
            }
            pid_identity::ProcessIdentityObservation::ExactLive(_) => {
                stale = RuntimeGenerationReadOnlyLiveness::StalePidReused;
            }
            pid_identity::ProcessIdentityObservation::Dead => {
                stale = RuntimeGenerationReadOnlyLiveness::StaleDead;
            }
            pid_identity::ProcessIdentityObservation::Unsupported
            | pid_identity::ProcessIdentityObservation::ReadError(_) => {
                return RuntimeGenerationReadOnlyLiveness::Busy;
            }
        }
    }
    stale
}

fn validate_legacy_runtime_projection(
    row: Option<&LegacyRuntimeProjectionRow>,
) -> Result<(), String> {
    let Some(row) = row else {
        return Ok(());
    };
    validate_legacy_run_state(&row.run_state)
}

fn validate_running_run_state() -> Result<(), String> {
    validate_legacy_run_state("running")
}

fn validate_idle_run_state() -> Result<(), String> {
    validate_legacy_run_state("idle")
}

fn fresh_in_flight_wake_claim(
    tx: &rusqlite::Transaction<'_>,
    existing: Option<WakeClaimRow>,
    stale_after_seconds: i64,
    renew_token: Option<&str>,
) -> Result<Option<WakeClaimRow>, String> {
    let Some(existing) = existing else {
        return Ok(None);
    };
    let renews_existing = renew_token == Some(existing.claim_token.as_str());
    if renews_existing || wake_claim_is_reclaimable(tx, &existing, stale_after_seconds)? {
        return Ok(None);
    }
    Ok(Some(existing))
}

fn wake_claim_is_reclaimable(
    conn: &Connection,
    claim: &WakeClaimRow,
    stale_after_seconds: i64,
) -> Result<bool, String> {
    if claim.wake_pid.is_some() {
        return wake_claim_pid_is_reclaimable(conn, claim);
    }
    Ok(claim_is_stale(claim, stale_after_seconds))
}

fn wake_claim_is_releasable_for_manual_resume(
    conn: &Connection,
    claim: &WakeClaimRow,
) -> Result<bool, String> {
    if claim.wake_invocation_uuid.is_none() {
        return Ok(true);
    }
    let Some(wake_pid) = claim.wake_pid else {
        return Ok(false);
    };
    let Some(live) = wake_claim_live_process_identity(wake_pid)? else {
        return Ok(true);
    };
    if !wake_claim_has_persisted_process_identity(conn, claim)? {
        return Ok(false);
    }
    wake_claim_has_matching_live_process_identity(conn, claim, &live).map(|matched| !matched)
}

fn wake_claim_has_persisted_process_identity(
    conn: &Connection,
    claim: &WakeClaimRow,
) -> Result<bool, String> {
    conn.query_row(
        "SELECT wake_pid IS NOT NULL
                AND wake_os_boot_id IS NOT NULL
                AND wake_os_pid_starttime_ticks IS NOT NULL
         FROM session_wake_claim
         WHERE session_id = ?1
           AND claim_token = ?2",
        params![&claim.session_id, &claim.claim_token],
        |row| row.get::<_, bool>(0),
    )
    .optional()
    .map(|recorded| recorded.unwrap_or(false))
    .map_err(|err| format!("Failed to read wake claim process identity: {err}"))
}

fn wake_claim_pid_is_reclaimable(conn: &Connection, claim: &WakeClaimRow) -> Result<bool, String> {
    let Some(wake_pid) = claim.wake_pid else {
        return Ok(false);
    };
    wake_claim_pid_is_live_identity_matched(conn, claim, wake_pid).map(|matched| !matched)
}

fn wake_claim_pid_is_live_identity_matched(
    conn: &Connection,
    claim: &WakeClaimRow,
    wake_pid: i64,
) -> Result<bool, String> {
    let Some(live) = wake_claim_live_process_identity(wake_pid)? else {
        return Ok(false);
    };
    wake_claim_has_matching_live_process_identity(conn, claim, &live)
}

fn wake_claim_live_process_identity(wake_pid: i64) -> Result<Option<ProcessIdentity>, String> {
    pid_identity::read_live_process_identity(wake_pid)
}

fn wake_claim_has_matching_live_process_identity(
    conn: &Connection,
    claim: &WakeClaimRow,
    live: &ProcessIdentity,
) -> Result<bool, String> {
    let exists = wake_claim_matching_live_process_identity_exists(conn, claim, live)?;
    Ok(sqlite_exists_value_to_bool(exists))
}

fn wake_claim_matching_live_process_identity_exists(
    conn: &Connection,
    claim: &WakeClaimRow,
    live: &ProcessIdentity,
) -> Result<i64, String> {
    conn.query_row(
        "SELECT EXISTS(
            SELECT 1
            FROM session_wake_claim
            WHERE session_id = ?1
              AND claim_token = ?2
              AND wake_pid = ?3
              AND wake_os_boot_id = ?4
              AND wake_os_pid_starttime_ticks = ?5
        )",
        params![
            &claim.session_id,
            &claim.claim_token,
            live.os_pid,
            &live.os_boot_id,
            live.os_pid_starttime_ticks,
        ],
        |row| row.get::<_, i64>(0),
    )
    .map_err(|err| format!("Failed to verify wake claim process identity: {err}"))
}

fn sqlite_exists_value_to_bool(value: i64) -> bool {
    value != 0
}

fn acquire_wake_claim_tx(
    tx: &rusqlite::Transaction<'_>,
    input: WakeClaimRequest<'_>,
    now: &str,
    min_seq: i64,
    max_seq: i64,
) -> Result<WakeClaimRow, String> {
    upsert_wake_claim_tx(tx, input, now, min_seq, max_seq)?;
    update_session_runtime_auto_wake_count_tx(tx, input.session_id, input.auto_wake_count)?;
    read_acquired_wake_claim_tx(tx, input.session_id)
}

fn upsert_wake_claim_tx(
    tx: &rusqlite::Transaction<'_>,
    input: WakeClaimRequest<'_>,
    now: &str,
    min_seq: i64,
    max_seq: i64,
) -> Result<(), String> {
    tx.execute(
        "INSERT INTO session_wake_claim (
            session_id,
            claim_token,
            claimed_at,
            wake_pid,
            wake_invocation_uuid,
            reason,
            auto_wake_count,
            min_pending_seq_at_claim,
            max_pending_seq_at_claim
         ) VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(session_id)
         DO UPDATE SET
            claim_token = excluded.claim_token,
            claimed_at = excluded.claimed_at,
            wake_pid = excluded.wake_pid,
            wake_os_boot_id = NULL,
            wake_os_pid_starttime_ticks = NULL,
            wake_invocation_uuid = excluded.wake_invocation_uuid,
            reason = excluded.reason,
            auto_wake_count = excluded.auto_wake_count,
            min_pending_seq_at_claim = excluded.min_pending_seq_at_claim,
            max_pending_seq_at_claim = excluded.max_pending_seq_at_claim",
        params![
            input.session_id,
            input.claim_token,
            now,
            input.wake_invocation_uuid,
            input.reason,
            input.auto_wake_count,
            min_seq,
            max_seq,
        ],
    )
    .map_err(format_acquire_wake_claim_error)?;
    Ok(())
}

fn format_acquire_wake_claim_error(err: rusqlite::Error) -> String {
    format!("Failed to acquire wake claim: {err}")
}

fn read_acquired_wake_claim_tx(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
) -> Result<WakeClaimRow, String> {
    wake_claim_tx(tx, session_id)?
        .ok_or_else(|| "Wake claim missing immediately after acquisition".to_string())
}

fn update_session_runtime_auto_wake_count_tx(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
    auto_wake_count: i64,
) -> Result<(), String> {
    tx.execute(
        "UPDATE session_runtime
         SET auto_wake_count = MAX(auto_wake_count, ?2)
         WHERE session_id = ?1",
        params![session_id, auto_wake_count],
    )
    .map_err(|err| format!("Failed to update session auto wake count: {err}"))?;
    Ok(())
}

fn wake_claim_matches_child(claim: &WakeClaimRow, claim_token: &str) -> bool {
    claim.claim_token == claim_token
}

pub(crate) fn validate_completion_event_registration(
    input: &CompletionEventRegistrationInput<'_>,
) -> Result<(), String> {
    if input.event_id.is_empty() {
        return Err("Completion event ID must not be empty".to_string());
    }
    if !matches!(input.delivery_mode, "sync" | "async") {
        return Err(format!(
            "Invalid completion event delivery mode: {}",
            input.delivery_mode
        ));
    }
    if [
        input.state_dir,
        input.meta_path,
        input.log_path,
        input.rc_path,
    ]
    .iter()
    .any(|value| value.is_empty())
    {
        return Err("Completion event artifact paths must not be empty".to_string());
    }
    match (input.owner_session_id, input.owner_invocation_uuid) {
        (Some(session_id), Some(invocation_uuid))
            if !session_id.is_empty() && !invocation_uuid.is_empty() =>
        {
            Ok(())
        }
        _ => Err("Completion event owner session and invocation are both required".to_string()),
    }
}

fn sidecar_generation_on(conn: &Connection) -> Result<String, String> {
    let generation = conn
        .query_row(
            "SELECT generation_uuid
             FROM mailbox_sidecar_identity
             WHERE singleton = 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .map_err(|err| format!("Failed to read PID mailbox sidecar generation: {err}"))?;
    let parsed = Uuid::parse_str(&generation)
        .map_err(|_| "PID mailbox sidecar generation is invalid".to_string())?;
    if parsed.to_string() != generation {
        return Err("PID mailbox sidecar generation is not canonical".to_string());
    }
    Ok(generation)
}

fn completion_continuity_head_on(
    conn: &Connection,
) -> Result<Option<CompletionContinuityHead>, String> {
    #[cfg(test)]
    COMPLETION_CONTINUITY_HEAD_QUERIES.with(|count| count.set(count.get() + 1));
    conn.query_row(
        "SELECT authority_ordinal, admission_id, sidecar_generation,
                invocation_uuid, event_id, owner_invocation_uuid, owner_session_id,
                previous_continuity_digest, continuity_digest
         FROM completion_authority_continuity
         ORDER BY authority_ordinal DESC
         LIMIT 1",
        [],
        map_completion_continuity_head,
    )
    .optional()
    .map_err(|err| format!("Failed to read completion continuity head: {err}"))
}

fn completion_continuity_by_admission_on(
    conn: &Connection,
    admission_id: &str,
) -> Result<Option<CompletionContinuityHead>, String> {
    conn.query_row(
        "SELECT authority_ordinal, admission_id, sidecar_generation,
                invocation_uuid, event_id, owner_invocation_uuid, owner_session_id,
                previous_continuity_digest, continuity_digest
         FROM completion_authority_continuity
         WHERE admission_id = ?1",
        params![admission_id],
        map_completion_continuity_head,
    )
    .optional()
    .map_err(|err| format!("Failed to read completion continuity admission: {err}"))
}

fn map_completion_continuity_head(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<CompletionContinuityHead> {
    Ok(CompletionContinuityHead {
        authority_ordinal: row.get(0)?,
        admission_id: row.get(1)?,
        sidecar_generation: row.get(2)?,
        invocation_uuid: row.get(3)?,
        event_id: row.get(4)?,
        owner_invocation_uuid: row.get(5)?,
        owner_session_id: row.get(6)?,
        previous_continuity_digest: row.get(7)?,
        continuity_digest: row.get(8)?,
    })
}

fn append_completion_continuity_on(
    conn: &Connection,
    continuity: &CompletionContinuityHead,
) -> Result<(), String> {
    if let Some(existing) = completion_continuity_by_admission_on(conn, &continuity.admission_id)? {
        return if existing == *continuity {
            Ok(())
        } else {
            Err(format!(
                "Completion continuity admission {} conflicts with its durable identity",
                continuity.admission_id
            ))
        };
    }
    conn.execute(
        "INSERT INTO completion_authority_continuity (
            authority_ordinal, admission_id, sidecar_generation, invocation_uuid,
            event_id, owner_invocation_uuid, owner_session_id,
            previous_continuity_digest, continuity_digest
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            continuity.authority_ordinal,
            continuity.admission_id,
            continuity.sidecar_generation,
            continuity.invocation_uuid,
            continuity.event_id,
            continuity.owner_invocation_uuid,
            continuity.owner_session_id,
            continuity.previous_continuity_digest,
            continuity.continuity_digest,
        ],
    )
    .map(|_| ())
    .map_err(|err| format!("Failed to append completion continuity: {err}"))
}

#[cfg(test)]
thread_local! {
    static COMPLETION_CONTINUITY_HEAD_QUERIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static COMPLETION_FINALIZATION_VM_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static COUNT_COMPLETION_FINALIZATION_VM_STEPS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(crate) fn install_completion_finalization_vm_counter(conn: &Connection) {
    conn.progress_handler(
        1,
        Some(|| {
            COUNT_COMPLETION_FINALIZATION_VM_STEPS.with(|enabled| {
                if enabled.get() {
                    COMPLETION_FINALIZATION_VM_STEPS.with(|count| count.set(count.get() + 1));
                }
            });
            false
        }),
    )
    .expect("install completion finalization SQLite VM counter");
}

#[cfg(test)]
pub(crate) fn begin_completion_finalization_vm_count() {
    COMPLETION_FINALIZATION_VM_STEPS.with(|count| count.set(0));
    COUNT_COMPLETION_FINALIZATION_VM_STEPS.with(|enabled| enabled.set(true));
}

#[cfg(test)]
pub(crate) fn end_completion_finalization_vm_count() -> usize {
    COUNT_COMPLETION_FINALIZATION_VM_STEPS.with(|enabled| enabled.set(false));
    COMPLETION_FINALIZATION_VM_STEPS.with(std::cell::Cell::get)
}

#[cfg(test)]
pub(crate) fn reset_completion_continuity_head_query_count() {
    COMPLETION_CONTINUITY_HEAD_QUERIES.with(|count| count.set(0));
}

#[cfg(test)]
pub(crate) fn completion_continuity_head_query_count() -> usize {
    COMPLETION_CONTINUITY_HEAD_QUERIES.with(std::cell::Cell::get)
}

fn contains_completion_obligation_on(
    conn: &Connection,
    event_id: &str,
    owner_invocation_uuid: &str,
    owner_session_id: &str,
) -> Result<bool, String> {
    let Some(event) = completion_event_by_id_on(conn, event_id)? else {
        return Ok(false);
    };
    if event.kind != AGENT_BASH_COMPLETE_KIND {
        return Ok(false);
    }
    completion_event_listener_on(conn, event_id, owner_invocation_uuid).map(|listener| {
        listener.is_some_and(|listener| {
            listener.owner_invocation_uuid == owner_invocation_uuid
                && listener.session_id == owner_session_id
        })
    })
}

fn completion_materialization_summary_on(
    conn: &Connection,
    invocation_uuid: &str,
) -> Result<Option<CompletionMaterializationSummary>, String> {
    conn.query_row(
        "SELECT materialized_count, authority_ordinal, sidecar_generation, continuity_digest
         FROM completion_authority_materialization_summary
         WHERE invocation_uuid = ?1",
        params![invocation_uuid],
        |row| {
            Ok(CompletionMaterializationSummary {
                materialized_count: row.get(0)?,
                authority_ordinal: row.get(1)?,
                sidecar_generation: row.get(2)?,
                continuity_digest: row.get(3)?,
            })
        },
    )
    .optional()
    .map_err(|error| format!("Failed to read completion materialization summary: {error}"))
}

fn preflight_completion_event_registration_on(
    conn: &Connection,
    input: &CompletionEventRegistrationInput<'_>,
) -> Result<(), String> {
    validate_completion_event_registration(input)?;
    let Some(event) = completion_event_by_id_on(conn, input.event_id)? else {
        return Ok(());
    };
    validate_completion_event_registration_replay(&event, input)?;
    let owner_invocation_uuid = input
        .owner_invocation_uuid
        .expect("validated completion registration has an owner");
    let owner_session_id = input
        .owner_session_id
        .expect("validated completion registration has a session");
    if let Some(listener) =
        completion_event_listener_on(conn, input.event_id, owner_invocation_uuid)?
    {
        return validate_completion_event_listener_replay(
            &listener,
            owner_session_id,
            owner_invocation_uuid,
        );
    }
    if event.state == "triggered" {
        return Err(format!(
            "Completion event {} cannot register a listener after it was triggered",
            input.event_id
        ));
    }
    Ok(())
}

fn register_completion_event_on(
    tx: &Transaction<'_>,
    input: &CompletionEventRegistrationInput<'_>,
    now: &str,
) -> Result<bool, String> {
    let existing = completion_event_by_id_on(tx, input.event_id)?;
    let inserted = if let Some(existing) = existing.as_ref() {
        validate_completion_event_registration_replay(existing, input)?;
        false
    } else {
        insert_completion_event(tx, input, now)?;
        true
    };
    register_completion_event_listener(
        tx,
        input,
        existing.is_some_and(|event| event.state == "triggered"),
        now,
    )?;
    Ok(inserted)
}

fn completion_event_registration_on(
    conn: &Connection,
    event_id: &str,
    inserted: bool,
) -> Result<CompletionEventRegistrationResult, String> {
    let event = completion_event_by_id_on(conn, event_id)?
        .ok_or_else(|| format!("Completion event {event_id} disappeared"))?;
    let listeners = completion_event_listeners_on(conn, event_id)?;
    Ok(CompletionEventRegistrationResult {
        inserted,
        event,
        listeners,
    })
}

fn validate_completion_event_trigger(
    input: &CompletionEventTriggerInput<'_>,
) -> Result<(), String> {
    if input.event_id.is_empty() {
        return Err("Completion event ID must not be empty".to_string());
    }
    if input.payload_json.is_empty() {
        return Err("Completion event payload must not be empty".to_string());
    }
    Ok(())
}

fn completion_event_by_id_on(
    conn: &Connection,
    event_id: &str,
) -> Result<Option<CompletionEventRow>, String> {
    conn.query_row(
        "SELECT event_id, kind, state, delivery_mode, state_dir, meta_path, log_path,
                rc_path, rc, payload_json, payload_file_path, payload_sha256,
                payload_byte_len, payload_retention_policy, created_at, triggered_at,
                payload_reclaimed_at
         FROM completion_event
         WHERE event_id = ?1",
        params![event_id],
        |row| {
            Ok(CompletionEventRow {
                event_id: row.get(0)?,
                kind: row.get(1)?,
                state: row.get(2)?,
                delivery_mode: row.get(3)?,
                state_dir: row.get(4)?,
                meta_path: row.get(5)?,
                log_path: row.get(6)?,
                rc_path: row.get(7)?,
                rc: row.get(8)?,
                payload_json: row.get(9)?,
                payload_file_path: row.get(10)?,
                payload_sha256: row.get(11)?,
                payload_byte_len: row.get(12)?,
                payload_retention_policy: row.get(13)?,
                created_at: row.get(14)?,
                triggered_at: row.get(15)?,
                payload_reclaimed_at: row.get(16)?,
            })
        },
    )
    .optional()
    .map_err(|err| format!("Failed to read completion event: {err}"))
}

fn completion_event_listeners_on(
    conn: &Connection,
    event_id: &str,
) -> Result<Vec<CompletionEventListenerRow>, String> {
    let mut statement = conn
        .prepare(
            "SELECT event_id, listener_id, session_id, owner_invocation_uuid, active,
                    mailbox_seq, acknowledged_at, acknowledgement_reason
             FROM completion_event_listener
             WHERE event_id = ?1
             ORDER BY listener_id",
        )
        .map_err(|err| format!("Failed to prepare completion event listener query: {err}"))?;
    let rows = statement
        .query_map(params![event_id], |row| {
            Ok(CompletionEventListenerRow {
                event_id: row.get(0)?,
                listener_id: row.get(1)?,
                session_id: row.get(2)?,
                owner_invocation_uuid: row.get(3)?,
                active: row.get(4)?,
                mailbox_seq: row.get(5)?,
                acknowledged_at: row.get(6)?,
                acknowledgement_reason: row.get(7)?,
            })
        })
        .map_err(|err| format!("Failed to query completion event listeners: {err}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("Failed to read completion event listener: {err}"))
}

fn insert_completion_event(
    tx: &Transaction<'_>,
    input: &CompletionEventRegistrationInput<'_>,
    now: &str,
) -> Result<(), String> {
    tx.execute(
        "INSERT INTO completion_event (
            event_id, kind, state, delivery_mode, state_dir, meta_path, log_path,
            rc_path, created_at
         ) VALUES (?1, ?2, 'pending', ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            input.event_id,
            AGENT_BASH_COMPLETE_KIND,
            input.delivery_mode,
            input.state_dir,
            input.meta_path,
            input.log_path,
            input.rc_path,
            now,
        ],
    )
    .map(|_| ())
    .map_err(|err| format!("Failed to insert completion event: {err}"))
}

fn validate_completion_event_registration_replay(
    event: &CompletionEventRow,
    input: &CompletionEventRegistrationInput<'_>,
) -> Result<(), String> {
    let matches = event.kind == AGENT_BASH_COMPLETE_KIND
        && event.delivery_mode == input.delivery_mode
        && event.state_dir == input.state_dir
        && event.meta_path == input.meta_path
        && event.log_path == input.log_path
        && event.rc_path == input.rc_path;
    if matches {
        Ok(())
    } else {
        Err(format!(
            "Completion event {} registration conflicts with its durable identity",
            input.event_id
        ))
    }
}

fn register_completion_event_listener(
    tx: &Transaction<'_>,
    input: &CompletionEventRegistrationInput<'_>,
    listeners_frozen: bool,
    now: &str,
) -> Result<(), String> {
    let (Some(session_id), Some(invocation_uuid)) =
        (input.owner_session_id, input.owner_invocation_uuid)
    else {
        return Err("Completion event owner session and invocation are both required".to_string());
    };
    if let Some(listener) = completion_event_listener_on(tx, input.event_id, invocation_uuid)? {
        return validate_completion_event_listener_replay(&listener, session_id, invocation_uuid);
    }
    if listeners_frozen {
        return Err(format!(
            "Completion event {} cannot register a listener after it was triggered",
            input.event_id
        ));
    }
    tx.execute(
        "INSERT OR IGNORE INTO completion_event_listener (
            event_id, listener_id, session_id, owner_invocation_uuid, active, created_at
         ) VALUES (?1, ?2, ?3, ?2, ?4, ?5)",
        params![
            input.event_id,
            invocation_uuid,
            session_id,
            input.delivery_mode == "async",
            now,
        ],
    )
    .map_err(|err| format!("Failed to register completion event listener: {err}"))?;
    let listener = completion_event_listener_on(tx, input.event_id, invocation_uuid)?
        .ok_or_else(|| "Registered completion listener disappeared".to_string())?;
    validate_completion_event_listener_replay(&listener, session_id, invocation_uuid)
}

fn completion_event_listener_on(
    conn: &Connection,
    event_id: &str,
    listener_id: &str,
) -> Result<Option<CompletionEventListenerRow>, String> {
    conn.query_row(
        "SELECT event_id, listener_id, session_id, owner_invocation_uuid, active,
                mailbox_seq, acknowledged_at, acknowledgement_reason
         FROM completion_event_listener
         WHERE event_id = ?1 AND listener_id = ?2",
        params![event_id, listener_id],
        |row| {
            Ok(CompletionEventListenerRow {
                event_id: row.get(0)?,
                listener_id: row.get(1)?,
                session_id: row.get(2)?,
                owner_invocation_uuid: row.get(3)?,
                active: row.get(4)?,
                mailbox_seq: row.get(5)?,
                acknowledged_at: row.get(6)?,
                acknowledgement_reason: row.get(7)?,
            })
        },
    )
    .optional()
    .map_err(|err| format!("Failed to read registered completion listener: {err}"))
}

fn validate_completion_event_listener_replay(
    listener: &CompletionEventListenerRow,
    session_id: &str,
    invocation_uuid: &str,
) -> Result<(), String> {
    if listener.session_id == session_id && listener.owner_invocation_uuid == invocation_uuid {
        Ok(())
    } else {
        Err(format!(
            "Completion event {} listener registration conflicts with its durable identity",
            listener.event_id
        ))
    }
}

fn validate_completion_event_trigger_source(
    event: &CompletionEventRow,
    input: &CompletionEventTriggerInput<'_>,
) -> Result<(), String> {
    if event.kind == AGENT_BASH_COMPLETE_KIND
        && event.state_dir == input.state_dir
        && event.meta_path == input.meta_path
        && event.log_path == input.log_path
        && event.rc_path == input.rc_path
    {
        Ok(())
    } else {
        Err(format!(
            "Completion event {} trigger does not match its registered source",
            input.event_id
        ))
    }
}

fn trigger_completion_event_row(
    tx: &Transaction<'_>,
    input: &CompletionEventTriggerInput<'_>,
    payload_json: &str,
    published: &PublishedMailboxPayload,
    now: &str,
) -> Result<(), String> {
    let payload_len = i64::try_from(published.byte_len)
        .map_err(|_| "Completion event payload length does not fit SQLite INTEGER".to_string())?;
    let changed = tx
        .execute(
            "UPDATE completion_event
             SET state = 'triggered', rc = ?2, payload_json = ?3,
                 payload_file_path = ?4, payload_sha256 = ?5, payload_byte_len = ?6,
                 payload_retention_policy = ?7, triggered_at = ?8
             WHERE event_id = ?1 AND state = 'pending'",
            params![
                input.event_id,
                input.rc,
                payload_json,
                published.file_path.to_string_lossy().as_ref(),
                published.sha256,
                payload_len,
                published.retention_policy,
                now,
            ],
        )
        .map_err(|err| format!("Failed to trigger completion event: {err}"))?;
    if changed == 1 {
        Ok(())
    } else {
        Err(format!(
            "Completion event {} changed while it was being triggered",
            input.event_id
        ))
    }
}

fn validate_completion_event_trigger_replay(
    event: &CompletionEventRow,
    input: &CompletionEventTriggerInput<'_>,
    published: &PublishedMailboxPayload,
) -> Result<(), String> {
    if event.rc == Some(input.rc)
        && event.payload_sha256.as_deref() == Some(published.sha256.as_str())
        && event.payload_byte_len == i64::try_from(published.byte_len).ok()
    {
        Ok(())
    } else {
        Err(format!(
            "Completion event {} was already triggered with a different payload",
            input.event_id
        ))
    }
}

fn acknowledge_consumed_completion_event_listeners(
    tx: &Transaction<'_>,
    event_id: &str,
    now: &str,
) -> Result<(), String> {
    let session_ids = completion_event_listener_session_ids(tx, event_id)?;
    tx.execute(
        "UPDATE mailbox
         SET delivered_at = COALESCE(delivered_at, ?2),
             delivered_by_invocation_uuid = COALESCE(
                 delivered_by_invocation_uuid,
                 (SELECT owner_invocation_uuid
                  FROM completion_event_listener
                  WHERE completion_event_listener.mailbox_seq = mailbox.seq)
             ),
             delivery_attempts = delivery_attempts + 1,
             delivery_error = NULL
         WHERE seq IN (
             SELECT mailbox_seq
             FROM completion_event_listener
             WHERE event_id = ?1 AND mailbox_seq IS NOT NULL
         ) AND delivered_at IS NULL",
        params![event_id, now],
    )
    .map_err(|err| format!("Failed to consume completion event mailbox rows: {err}"))?;
    tx.execute(
        "UPDATE completion_event_listener
         SET active = 0,
             acknowledged_at = COALESCE(acknowledged_at, ?2),
             acknowledgement_reason = COALESCE(acknowledgement_reason, 'consumed_in_call')
         WHERE event_id = ?1",
        params![event_id, now],
    )
    .map_err(|err| format!("Failed to acknowledge consumed completion listeners: {err}"))?;
    for session_id in session_ids {
        resolve_completed_delivery_attempts(tx, &session_id, now, None)?;
    }
    Ok(())
}

fn completion_event_listener_session_ids(
    conn: &Connection,
    event_id: &str,
) -> Result<Vec<String>, String> {
    let mut statement = conn
        .prepare(
            "SELECT DISTINCT session_id
             FROM completion_event_listener
             WHERE event_id = ?1 AND mailbox_seq IS NOT NULL",
        )
        .map_err(|err| format!("Failed to prepare completion listener session query: {err}"))?;
    let rows = statement
        .query_map(params![event_id], |row| row.get(0))
        .map_err(|err| format!("Failed to query completion listener sessions: {err}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("Failed to read completion listener session: {err}"))
}

fn materialize_completion_event_listeners(
    tx: &Transaction<'_>,
    event: &CompletionEventRow,
    now: &str,
) -> Result<(), String> {
    if event.state != "triggered" {
        return Ok(());
    }
    let listeners = completion_event_listeners_on(tx, &event.event_id)?;
    let listener_count = listeners.len();
    for listener in listeners.into_iter().filter(|listener| {
        listener.active && listener.acknowledged_at.is_none() && listener.mailbox_seq.is_none()
    }) {
        let handle = completion_listener_mailbox_handle(event, &listener, listener_count);
        let changed = insert_completion_listener_mailbox_row(tx, event, &listener, &handle, now)?;
        let row = query_mailbox_by_kind_handle_tx(tx, AGENT_BASH_COMPLETE_KIND, &handle)?
            .ok_or_else(|| "Completion listener mailbox row disappeared".to_string())?;
        if changed == 0
            && (row.session_id != listener.session_id
                || row.owner_invocation_uuid.as_deref()
                    != Some(listener.owner_invocation_uuid.as_str())
                || row.payload_sha256 != event.payload_sha256)
        {
            return Err(format!(
                "Completion event {} mailbox identity conflicts with an existing row",
                event.event_id
            ));
        }
        tx.execute(
            "UPDATE completion_event_listener
             SET mailbox_seq = ?3
             WHERE event_id = ?1 AND listener_id = ?2 AND mailbox_seq IS NULL",
            params![event.event_id, listener.listener_id, row.seq],
        )
        .map_err(|err| format!("Failed to bind completion listener mailbox row: {err}"))?;
    }
    Ok(())
}

fn completion_listener_mailbox_handle(
    event: &CompletionEventRow,
    listener: &CompletionEventListenerRow,
    listener_count: usize,
) -> String {
    if listener_count == 1 {
        event.event_id.clone()
    } else {
        format!("{}:{}", event.event_id, listener.listener_id)
    }
}

fn insert_completion_listener_mailbox_row(
    tx: &Transaction<'_>,
    event: &CompletionEventRow,
    listener: &CompletionEventListenerRow,
    handle: &str,
    now: &str,
) -> Result<usize, String> {
    tx.execute(
        "INSERT OR IGNORE INTO mailbox (
            session_id, kind, handle, payload_json, enqueued_at,
            owner_invocation_uuid, state_dir, meta_path, log_path, rc_path, rc,
            payload_file_path, payload_sha256, payload_byte_len,
            payload_retention_policy, payload_compacted_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        params![
            listener.session_id,
            AGENT_BASH_COMPLETE_KIND,
            handle,
            event.payload_json.as_deref().ok_or_else(|| {
                format!(
                    "Triggered completion event {} has no payload",
                    event.event_id
                )
            })?,
            now,
            listener.owner_invocation_uuid,
            event.state_dir,
            event.meta_path,
            event.log_path,
            event.rc_path,
            event.rc.ok_or_else(|| {
                format!(
                    "Triggered completion event {} has no exit code",
                    event.event_id
                )
            })?,
            event.payload_file_path,
            event.payload_sha256,
            event.payload_byte_len,
            event.payload_retention_policy,
            event.triggered_at,
        ],
    )
    .map_err(|err| format!("Failed to materialize completion event listener: {err}"))
}

fn completion_event_mailbox_rows_on(
    conn: &Connection,
    event_id: &str,
) -> Result<Vec<MailboxRow>, String> {
    let mut statement = conn
        .prepare(
            "SELECT mailbox.seq, mailbox.session_id, mailbox.kind, mailbox.handle,
                    mailbox.payload_json, mailbox.enqueued_at, mailbox.delivered_at,
                    mailbox.delivered_by_invocation_uuid, mailbox.delivery_attempts,
                    mailbox.delivery_error, mailbox.owner_invocation_uuid,
                    mailbox.matched_os_pid, mailbox.matched_os_boot_id,
                    mailbox.matched_os_pid_starttime_ticks, mailbox.matched_chain_index,
                    mailbox.state_dir, mailbox.meta_path, mailbox.log_path, mailbox.rc_path,
                    mailbox.rc, mailbox.payload_file_path, mailbox.payload_sha256,
                    mailbox.payload_byte_len, mailbox.payload_retention_policy,
                    mailbox.payload_compacted_at, mailbox.submission_token,
                    mailbox.target_kind, mailbox.target_id
             FROM completion_event_listener
             JOIN mailbox ON mailbox.seq = completion_event_listener.mailbox_seq
             WHERE completion_event_listener.event_id = ?1
             ORDER BY mailbox.seq",
        )
        .map_err(|err| format!("Failed to prepare completion event mailbox query: {err}"))?;
    let rows = statement
        .query_map(params![event_id], map_mailbox_row)
        .map_err(|err| format!("Failed to query completion event mailbox rows: {err}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("Failed to read completion event mailbox row: {err}"))
}

fn acknowledge_completion_event_listeners_for_seqs(
    tx: &Transaction<'_>,
    session_id: &str,
    chain_id: Option<&str>,
    seqs: &[i64],
    now: &str,
) -> Result<(), String> {
    let sql = format!(
        "UPDATE completion_event_listener
         SET acknowledged_at = COALESCE(acknowledged_at, ?4),
             acknowledgement_reason = COALESCE(acknowledgement_reason, 'injected')
         WHERE mailbox_seq = ?3
           AND EXISTS (
               SELECT 1 FROM mailbox
               WHERE mailbox.seq = ?3 AND {PENDING_MAILBOX_TARGET_PREDICATE}
           )"
    );
    for seq in seqs {
        tx.execute(&sql, params![session_id, chain_id, seq, now])
            .map_err(|err| format!("Failed to acknowledge completion event listener: {err}"))?;
    }
    Ok(())
}

fn query_mailbox_by_kind_handle_tx(
    tx: &rusqlite::Transaction<'_>,
    kind: &str,
    handle: &str,
) -> Result<Option<MailboxRow>, String> {
    tx.query_row(
        "SELECT seq, session_id, kind, handle, payload_json, enqueued_at,
                delivered_at, delivered_by_invocation_uuid, delivery_attempts,
                delivery_error, owner_invocation_uuid, matched_os_pid,
                matched_os_boot_id, matched_os_pid_starttime_ticks,
                matched_chain_index, state_dir, meta_path, log_path, rc_path, rc,
                payload_file_path, payload_sha256, payload_byte_len,
                payload_retention_policy, payload_compacted_at,
                submission_token, target_kind, target_id
         FROM mailbox
         WHERE kind = ?1 AND handle = ?2",
        params![kind, handle],
        map_mailbox_row,
    )
    .optional()
    .map_err(|err| format!("Failed to read mailbox row by handle: {err}"))
}

fn ensure_parent_dir(path: &Path) -> Result<(), String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        ensure_durable_directory(parent)
            .map_err(|err| format!("Failed to create PID mailbox sidecar directory: {err}"))?;
    }
    Ok(())
}

pub(crate) fn set_wal_mode(conn: &Connection) -> Result<(), String> {
    const RETRY_INTERVAL: StdDuration = StdDuration::from_millis(10);
    const TIMEOUT: StdDuration = StdDuration::from_secs(5);

    let deadline = Instant::now() + TIMEOUT;
    loop {
        match conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;") {
            Ok(()) => return Ok(()),
            Err(error) if sqlite_error_is_contention(&error) && Instant::now() < deadline => {
                std::thread::sleep(RETRY_INTERVAL);
            }
            Err(error) => {
                return Err(format!(
                    "Failed to set durable PID mailbox sidecar mode: {error}"
                ));
            }
        }
    }
}

pub(crate) fn configure_writable_sidecar_connection(conn: &Connection) -> Result<(), String> {
    conn.pragma_update(None, "foreign_keys", true)
        .map_err(|err| format!("Failed to enable PID mailbox sidecar foreign keys: {err}"))?;
    conn.busy_timeout(mailbox_writer_sqlite_timeout())
        .map_err(|err| format!("Failed to configure PID mailbox writer wait: {err}"))?;
    #[cfg(test)]
    install_completion_finalization_vm_counter(conn);
    Ok(())
}

pub(crate) fn ensure_shared_sidecar_schema(conn: &mut Connection) -> Result<(), String> {
    schema::ensure(conn)
}

fn mailbox_schema_definition() -> &'static str {
    "CREATE TABLE IF NOT EXISTS mailbox_sidecar_identity (
            singleton       INTEGER PRIMARY KEY CHECK(singleton = 1),
            generation_uuid TEXT NOT NULL UNIQUE,
            created_at      TEXT NOT NULL
        );

        CREATE TRIGGER IF NOT EXISTS trg_mailbox_sidecar_identity_update
        BEFORE UPDATE ON mailbox_sidecar_identity
        BEGIN
            SELECT RAISE(ABORT, 'mailbox sidecar identity is immutable');
        END;

        CREATE TRIGGER IF NOT EXISTS trg_mailbox_sidecar_identity_insert
        BEFORE INSERT ON mailbox_sidecar_identity
        WHEN EXISTS (SELECT 1 FROM mailbox_sidecar_identity)
        BEGIN
            SELECT RAISE(ABORT, 'mailbox sidecar identity is immutable');
        END;

        CREATE TRIGGER IF NOT EXISTS trg_mailbox_sidecar_identity_delete
        BEFORE DELETE ON mailbox_sidecar_identity
        BEGIN
            SELECT RAISE(ABORT, 'mailbox sidecar identity is immutable');
        END;

        CREATE TABLE IF NOT EXISTS mailbox (
            seq                          INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id                   TEXT    NOT NULL,
            kind                         TEXT    NOT NULL,
            handle                       TEXT    NOT NULL,
            payload_json                 TEXT    NOT NULL,
            enqueued_at                  TEXT    NOT NULL,
            delivered_at                 TEXT,
            delivered_by_invocation_uuid TEXT,
            delivery_attempts            INTEGER NOT NULL DEFAULT 0,
            delivery_error               TEXT,
            owner_invocation_uuid        TEXT,
            matched_os_pid               INTEGER,
            matched_os_boot_id           TEXT,
            matched_os_pid_starttime_ticks INTEGER,
            matched_chain_index          INTEGER,
            state_dir                    TEXT    NOT NULL,
            meta_path                    TEXT    NOT NULL,
            log_path                     TEXT    NOT NULL,
            rc_path                      TEXT    NOT NULL,
            rc                           INTEGER NOT NULL,
            payload_file_path            TEXT,
            payload_sha256               TEXT,
            payload_byte_len             INTEGER,
            payload_retention_policy     TEXT,
            payload_compacted_at         TEXT,
            submission_token             TEXT,
            target_kind                  TEXT,
            target_id                    TEXT,
            UNIQUE(kind, handle)
        );

        CREATE INDEX IF NOT EXISTS idx_mailbox_pending
            ON mailbox(session_id, delivered_at, seq);

        CREATE TABLE IF NOT EXISTS mailbox_delivery_attempts (
            attempt_id                    TEXT PRIMARY KEY,
            session_id                    TEXT NOT NULL,
            delivery_invocation_uuid      TEXT NOT NULL,
            created_at                    TEXT NOT NULL,
            prepared_remaining_count      INTEGER NOT NULL,
            submission_started_at         TEXT,
            acknowledged_at               TEXT,
            resolved_at                   TEXT,
            resolved_by_attempt_id        TEXT,
            evidence_turn_generation_id   TEXT,
            evidence_observed_at           INTEGER,
            evidence_reconciled_at         TEXT,
            evidence_disposition           TEXT,
            observation_provider_name      TEXT,
            observation_provider_instance_id TEXT,
            observation_settings_id        TEXT,
            observation_session_id         TEXT,
            observation_anchor_token       TEXT,
            observation_expected_sha256    TEXT,
            observation_error              TEXT,
            observation_confirmed_turn_id  TEXT,
            observation_confirmed_at       TEXT
        );

        CREATE TABLE IF NOT EXISTS mailbox_delivery_attempt_items (
            attempt_id                    TEXT NOT NULL,
            mailbox_seq                   INTEGER NOT NULL,
            PRIMARY KEY(attempt_id, mailbox_seq),
            FOREIGN KEY(attempt_id) REFERENCES mailbox_delivery_attempts(attempt_id),
            FOREIGN KEY(mailbox_seq) REFERENCES mailbox(seq)
        );

        CREATE INDEX IF NOT EXISTS idx_mailbox_delivery_attempt_items_seq
            ON mailbox_delivery_attempt_items(mailbox_seq, attempt_id);

        CREATE TABLE IF NOT EXISTS completion_event (
            event_id                 TEXT PRIMARY KEY,
            kind                     TEXT NOT NULL,
            state                    TEXT NOT NULL CHECK(state IN ('pending', 'triggered')),
            delivery_mode            TEXT NOT NULL CHECK(delivery_mode IN ('sync', 'async')),
            state_dir                TEXT NOT NULL,
            meta_path                TEXT NOT NULL,
            log_path                 TEXT NOT NULL,
            rc_path                  TEXT NOT NULL,
            rc                       INTEGER,
            payload_json             TEXT,
            payload_file_path        TEXT,
            payload_sha256           TEXT,
            payload_byte_len         INTEGER,
            payload_retention_policy TEXT,
            created_at               TEXT NOT NULL,
            triggered_at             TEXT,
            payload_reclaimed_at     TEXT,
            CHECK (
                (state = 'pending' AND rc IS NULL AND payload_json IS NULL
                    AND payload_file_path IS NULL AND payload_sha256 IS NULL
                    AND payload_byte_len IS NULL AND payload_retention_policy IS NULL
                    AND triggered_at IS NULL)
                OR
                (state = 'triggered' AND rc IS NOT NULL AND payload_json IS NOT NULL
                    AND payload_file_path IS NOT NULL AND payload_sha256 IS NOT NULL
                    AND payload_byte_len IS NOT NULL AND payload_retention_policy IS NOT NULL
                    AND triggered_at IS NOT NULL)
            )
        );

        CREATE TABLE IF NOT EXISTS completion_event_listener (
            event_id                    TEXT NOT NULL,
            listener_id                 TEXT NOT NULL,
            session_id                  TEXT NOT NULL,
            owner_invocation_uuid       TEXT NOT NULL,
            active                      INTEGER NOT NULL CHECK(active IN (0, 1)),
            mailbox_seq                 INTEGER UNIQUE,
            acknowledged_at             TEXT,
            acknowledgement_reason      TEXT,
            created_at                  TEXT NOT NULL,
            PRIMARY KEY(event_id, listener_id),
            FOREIGN KEY(event_id) REFERENCES completion_event(event_id),
            FOREIGN KEY(mailbox_seq) REFERENCES mailbox(seq),
            CHECK (
                (acknowledged_at IS NULL AND acknowledgement_reason IS NULL)
                OR
                (acknowledged_at IS NOT NULL AND acknowledgement_reason IS NOT NULL)
            )
        );

        CREATE INDEX IF NOT EXISTS idx_completion_event_listener_pending
            ON completion_event_listener(session_id, active, acknowledged_at, event_id);

        CREATE TRIGGER IF NOT EXISTS trg_completion_event_identity_update
        BEFORE UPDATE OF event_id, kind, delivery_mode, state_dir, meta_path, log_path, rc_path, created_at
        ON completion_event
        WHEN OLD.event_id IS NOT NEW.event_id
          OR OLD.kind IS NOT NEW.kind
          OR OLD.delivery_mode IS NOT NEW.delivery_mode
          OR OLD.state_dir IS NOT NEW.state_dir
          OR OLD.meta_path IS NOT NEW.meta_path
          OR OLD.log_path IS NOT NEW.log_path
          OR OLD.rc_path IS NOT NEW.rc_path
          OR OLD.created_at IS NOT NEW.created_at
        BEGIN
            SELECT RAISE(ABORT, 'completion event identity is immutable');
        END;

        CREATE TRIGGER IF NOT EXISTS trg_completion_event_listener_identity_update
        BEFORE UPDATE OF event_id, listener_id, session_id, owner_invocation_uuid, created_at
        ON completion_event_listener
        WHEN OLD.event_id IS NOT NEW.event_id
          OR OLD.listener_id IS NOT NEW.listener_id
          OR OLD.session_id IS NOT NEW.session_id
          OR OLD.owner_invocation_uuid IS NOT NEW.owner_invocation_uuid
          OR OLD.created_at IS NOT NEW.created_at
        BEGIN
            SELECT RAISE(ABORT, 'completion listener identity is immutable');
        END;

        CREATE TABLE IF NOT EXISTS completion_authority_continuity (
            authority_ordinal          INTEGER PRIMARY KEY CHECK(authority_ordinal > 0),
            admission_id              TEXT NOT NULL UNIQUE,
            sidecar_generation        TEXT NOT NULL,
            invocation_uuid           TEXT NOT NULL,
            event_id                  TEXT NOT NULL,
            owner_invocation_uuid     TEXT NOT NULL,
            owner_session_id          TEXT NOT NULL,
            previous_continuity_digest TEXT NOT NULL
                CHECK(length(previous_continuity_digest) = 64
                    AND previous_continuity_digest NOT GLOB '*[^0-9a-f]*'),
            continuity_digest         TEXT NOT NULL UNIQUE
                CHECK(length(continuity_digest) = 64
                    AND continuity_digest NOT GLOB '*[^0-9a-f]*'),
            FOREIGN KEY(sidecar_generation)
                REFERENCES mailbox_sidecar_identity(generation_uuid),
            FOREIGN KEY(event_id, owner_invocation_uuid)
                REFERENCES completion_event_listener(event_id, listener_id)
        );

        CREATE INDEX IF NOT EXISTS idx_completion_authority_continuity_head
            ON completion_authority_continuity(authority_ordinal DESC);

        CREATE INDEX IF NOT EXISTS idx_completion_authority_continuity_invocation
            ON completion_authority_continuity(invocation_uuid, authority_ordinal);

        CREATE TABLE IF NOT EXISTS completion_authority_materialization_summary (
            invocation_uuid    TEXT PRIMARY KEY,
            materialized_count INTEGER NOT NULL CHECK(materialized_count > 0),
            authority_ordinal  INTEGER NOT NULL CHECK(authority_ordinal > 0),
            sidecar_generation TEXT NOT NULL,
            continuity_digest  TEXT NOT NULL
                CHECK(length(continuity_digest) = 64
                    AND continuity_digest NOT GLOB '*[^0-9a-f]*'),
            FOREIGN KEY(sidecar_generation)
                REFERENCES mailbox_sidecar_identity(generation_uuid)
        ) STRICT;

        INSERT OR IGNORE INTO completion_authority_materialization_summary (
            invocation_uuid,
            materialized_count,
            authority_ordinal,
            sidecar_generation,
            continuity_digest
        )
        SELECT
            head.invocation_uuid,
            (
                SELECT COUNT(*)
                FROM completion_authority_continuity AS counted
                WHERE counted.invocation_uuid = head.invocation_uuid
            ),
            head.authority_ordinal,
            head.sidecar_generation,
            head.continuity_digest
        FROM completion_authority_continuity AS head
        WHERE head.authority_ordinal = (
            SELECT MAX(candidate.authority_ordinal)
            FROM completion_authority_continuity AS candidate
            WHERE candidate.invocation_uuid = head.invocation_uuid
        )
        AND (
            SELECT COUNT(*)
            FROM completion_authority_continuity AS counted
            WHERE counted.invocation_uuid = head.invocation_uuid
        ) = (
            SELECT COUNT(*)
            FROM completion_authority_continuity AS continuity
            JOIN completion_event AS event
              ON event.event_id = continuity.event_id
             AND event.kind = 'agent_bash_complete'
            JOIN completion_event_listener AS listener
              ON listener.event_id = continuity.event_id
             AND listener.listener_id = continuity.owner_invocation_uuid
             AND listener.owner_invocation_uuid = continuity.owner_invocation_uuid
             AND listener.session_id = continuity.owner_session_id
            WHERE continuity.invocation_uuid = head.invocation_uuid
        );

        CREATE TRIGGER IF NOT EXISTS trg_completion_authority_continuity_insert
        BEFORE INSERT ON completion_authority_continuity
        BEGIN
            SELECT CASE
                WHEN NEW.authority_ordinal <> COALESCE(
                    (
                        SELECT authority_ordinal + 1
                        FROM completion_authority_continuity
                        ORDER BY authority_ordinal DESC
                        LIMIT 1
                    ),
                    1
                ) THEN RAISE(ABORT, 'completion continuity ordinal is not append-only')
                WHEN NEW.previous_continuity_digest <> COALESCE(
                    (
                        SELECT continuity_digest
                        FROM completion_authority_continuity
                        ORDER BY authority_ordinal DESC
                        LIMIT 1
                    ),
                    '0000000000000000000000000000000000000000000000000000000000000000'
                ) THEN RAISE(ABORT, 'completion continuity previous digest mismatch')
                WHEN NEW.sidecar_generation <> (
                    SELECT generation_uuid
                    FROM mailbox_sidecar_identity
                    WHERE singleton = 1
                ) THEN RAISE(ABORT, 'completion continuity sidecar generation mismatch')
                WHEN NOT EXISTS (
                    SELECT 1
                    FROM completion_event AS event
                    JOIN completion_event_listener AS listener
                      ON listener.event_id = event.event_id
                    WHERE event.event_id = NEW.event_id
                      AND event.kind = 'agent_bash_complete'
                      AND listener.listener_id = NEW.owner_invocation_uuid
                      AND listener.owner_invocation_uuid = NEW.owner_invocation_uuid
                      AND listener.session_id = NEW.owner_session_id
                ) THEN RAISE(ABORT, 'completion continuity event/listener identity mismatch')
            END;
        END;

        CREATE TRIGGER IF NOT EXISTS trg_completion_authority_materialization_summary_insert
        AFTER INSERT ON completion_authority_continuity
        BEGIN
            INSERT INTO completion_authority_materialization_summary (
                invocation_uuid,
                materialized_count,
                authority_ordinal,
                sidecar_generation,
                continuity_digest
            ) VALUES (
                NEW.invocation_uuid,
                1,
                NEW.authority_ordinal,
                NEW.sidecar_generation,
                NEW.continuity_digest
            )
            ON CONFLICT(invocation_uuid) DO UPDATE SET
                materialized_count = materialized_count + 1,
                authority_ordinal = NEW.authority_ordinal,
                sidecar_generation = NEW.sidecar_generation,
                continuity_digest = NEW.continuity_digest;
        END;

        CREATE TRIGGER IF NOT EXISTS trg_completion_authority_continuity_update
        BEFORE UPDATE ON completion_authority_continuity
        BEGIN
            SELECT RAISE(ABORT, 'completion continuity is append-only: update forbidden');
        END;

        CREATE TRIGGER IF NOT EXISTS trg_completion_authority_continuity_delete
        BEFORE DELETE ON completion_authority_continuity
        BEGIN
            SELECT RAISE(ABORT, 'completion continuity is append-only: delete forbidden');
        END;

        CREATE TRIGGER IF NOT EXISTS trg_completion_event_listener_continuity_delete
        BEFORE DELETE ON completion_event_listener
        WHEN EXISTS (
            SELECT 1
            FROM completion_authority_continuity
            WHERE event_id = OLD.event_id
              AND owner_invocation_uuid = OLD.listener_id
        )
        BEGIN
            SELECT RAISE(ABORT, 'completion listener continuity identity is immutable');
        END;

        CREATE TRIGGER IF NOT EXISTS trg_completion_event_materialization_delete
        AFTER DELETE ON completion_event
        BEGIN
            DELETE FROM completion_authority_materialization_summary
            WHERE materialized_count = 1
              AND invocation_uuid IN (
                  SELECT invocation_uuid
                  FROM completion_authority_continuity
                  WHERE event_id = OLD.event_id
              );
            UPDATE completion_authority_materialization_summary
            SET materialized_count = materialized_count - 1
            WHERE invocation_uuid IN (
                SELECT invocation_uuid
                FROM completion_authority_continuity
                WHERE event_id = OLD.event_id
            );
        END;

        CREATE TRIGGER IF NOT EXISTS trg_completion_event_materialization_update
        AFTER UPDATE OF event_id, kind ON completion_event
        WHEN OLD.event_id IS NOT NEW.event_id OR OLD.kind IS NOT NEW.kind
        BEGIN
            DELETE FROM completion_authority_materialization_summary
            WHERE materialized_count = 1
              AND invocation_uuid IN (
                  SELECT invocation_uuid
                  FROM completion_authority_continuity
                  WHERE event_id = OLD.event_id
              );
            UPDATE completion_authority_materialization_summary
            SET materialized_count = materialized_count - 1
            WHERE invocation_uuid IN (
                SELECT invocation_uuid
                FROM completion_authority_continuity
                WHERE event_id = OLD.event_id
            );
        END;

        CREATE TRIGGER IF NOT EXISTS trg_completion_listener_materialization_delete
        AFTER DELETE ON completion_event_listener
        BEGIN
            DELETE FROM completion_authority_materialization_summary
            WHERE materialized_count = 1
              AND invocation_uuid IN (
                  SELECT invocation_uuid
                  FROM completion_authority_continuity
                  WHERE event_id = OLD.event_id
                    AND owner_invocation_uuid = OLD.listener_id
              );
            UPDATE completion_authority_materialization_summary
            SET materialized_count = materialized_count - 1
            WHERE invocation_uuid IN (
                SELECT invocation_uuid
                FROM completion_authority_continuity
                WHERE event_id = OLD.event_id
                  AND owner_invocation_uuid = OLD.listener_id
            );
        END;

        CREATE TRIGGER IF NOT EXISTS trg_completion_listener_materialization_update
        AFTER UPDATE OF event_id, listener_id, session_id, owner_invocation_uuid
        ON completion_event_listener
        WHEN OLD.event_id IS NOT NEW.event_id
          OR OLD.listener_id IS NOT NEW.listener_id
          OR OLD.session_id IS NOT NEW.session_id
          OR OLD.owner_invocation_uuid IS NOT NEW.owner_invocation_uuid
        BEGIN
            DELETE FROM completion_authority_materialization_summary
            WHERE materialized_count = 1
              AND invocation_uuid IN (
                  SELECT invocation_uuid
                  FROM completion_authority_continuity
                  WHERE event_id = OLD.event_id
                    AND owner_invocation_uuid = OLD.listener_id
              );
            UPDATE completion_authority_materialization_summary
            SET materialized_count = materialized_count - 1
            WHERE invocation_uuid IN (
                SELECT invocation_uuid
                FROM completion_authority_continuity
                WHERE event_id = OLD.event_id
                  AND owner_invocation_uuid = OLD.listener_id
            );
        END;

        CREATE TABLE IF NOT EXISTS mailbox_notification_control (
            session_id                    TEXT PRIMARY KEY,
            paused                       INTEGER NOT NULL DEFAULT 0,
            updated_at                   TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS wake_sweep_progress (
            singleton                     INTEGER PRIMARY KEY CHECK(singleton = 1),
            after_pending_seq             INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS runtime_generation (
            generation_uuid                    TEXT PRIMARY KEY,
            lifecycle_state                   TEXT NOT NULL CHECK(lifecycle_state IN ('starting', 'running', 'draining', 'exited')),
            spawn_invocation_uuid              TEXT NOT NULL,
            session_id                         TEXT,
            runtime_mode                       TEXT NOT NULL CHECK(runtime_mode IN ('headless', 'pty_interactive')),
            provider_name                      TEXT NOT NULL,
            model_name                         TEXT,
            pty_control_path                    TEXT,
            models_dir                         TEXT,
            effective_cwd                      TEXT,
            spawned_os_pid                     INTEGER,
            identity_os_pid                    INTEGER,
            identity_os_boot_id                TEXT,
            identity_os_pid_starttime_ticks    INTEGER,
            created_at                         TEXT NOT NULL,
            running_at                         TEXT,
            draining_at                        TEXT,
            exited_at                          TEXT,
            terminal_reason                    TEXT CHECK(terminal_reason IS NULL OR terminal_reason IN ('startup_failed', 'orderly_completion', 'abnormal_termination', 'cancelled', 'recovered_dead')),
            exit_code                          INTEGER,
            drain_request_uuid                 TEXT,
            drain_requested_at                 TEXT,
            drain_requested_by_invocation_uuid TEXT,
            active_delivery_claim_uuid         TEXT,
            active_delivery_claimed_at          TEXT,
            active_delivery_seqs_json           TEXT,
            creator_identity_os_pid             INTEGER,
            creator_identity_os_boot_id         TEXT,
            creator_identity_os_pid_starttime_ticks INTEGER,
            CHECK (
                (identity_os_pid IS NULL AND identity_os_boot_id IS NULL AND identity_os_pid_starttime_ticks IS NULL)
                OR
                (identity_os_pid IS NOT NULL AND identity_os_boot_id IS NOT NULL AND identity_os_pid_starttime_ticks IS NOT NULL)
            ),
            CHECK (identity_os_pid IS NULL OR identity_os_pid = spawned_os_pid),
            CHECK (
                (creator_identity_os_pid IS NULL
                    AND creator_identity_os_boot_id IS NULL
                    AND creator_identity_os_pid_starttime_ticks IS NULL)
                OR
                (creator_identity_os_pid IS NOT NULL
                    AND creator_identity_os_boot_id IS NOT NULL
                    AND creator_identity_os_pid_starttime_ticks IS NOT NULL)
            ),
            CHECK (
                (active_delivery_claim_uuid IS NULL
                    AND active_delivery_claimed_at IS NULL
                    AND active_delivery_seqs_json IS NULL)
                OR
                (active_delivery_claim_uuid IS NOT NULL
                    AND active_delivery_claimed_at IS NOT NULL
                    AND active_delivery_seqs_json IS NOT NULL)
            ),
            CHECK (
                lifecycle_state != 'starting'
                OR (running_at IS NULL AND draining_at IS NULL AND exited_at IS NULL
                    AND terminal_reason IS NULL AND drain_request_uuid IS NULL
                    AND active_delivery_claim_uuid IS NULL)
            ),
            CHECK (
                lifecycle_state != 'running'
                OR (spawned_os_pid IS NOT NULL AND running_at IS NOT NULL
                    AND draining_at IS NULL AND exited_at IS NULL AND terminal_reason IS NULL)
            ),
            CHECK (
                lifecycle_state != 'draining'
                OR (running_at IS NOT NULL AND drain_request_uuid IS NOT NULL
                    AND drain_requested_at IS NOT NULL AND draining_at IS NOT NULL
                    AND exited_at IS NULL AND terminal_reason IS NULL
                    AND active_delivery_claim_uuid IS NULL)
            ),
            CHECK (
                lifecycle_state != 'exited'
                OR (exited_at IS NOT NULL AND terminal_reason IS NOT NULL)
            )
        );

        CREATE UNIQUE INDEX IF NOT EXISTS idx_runtime_generation_exact_identity
            ON runtime_generation(identity_os_pid, identity_os_boot_id, identity_os_pid_starttime_ticks)
            WHERE identity_os_pid IS NOT NULL
              AND identity_os_boot_id IS NOT NULL
              AND identity_os_pid_starttime_ticks IS NOT NULL;

        CREATE INDEX IF NOT EXISTS idx_runtime_generation_spawn_invocation
            ON runtime_generation(spawn_invocation_uuid);

        CREATE INDEX IF NOT EXISTS idx_runtime_generation_session_lifecycle
            ON runtime_generation(session_id, lifecycle_state);

        CREATE INDEX IF NOT EXISTS idx_runtime_generation_drain_request
            ON runtime_generation(drain_request_uuid);

        CREATE INDEX IF NOT EXISTS idx_runtime_generation_active_claim
            ON runtime_generation(active_delivery_claim_uuid);

        CREATE TABLE IF NOT EXISTS session_runtime (
            session_id                       TEXT PRIMARY KEY,
            mode                             TEXT NOT NULL CHECK(mode IN ('headless', 'pty_interactive')),
            invocation_uuid                  TEXT,
            provider_name                    TEXT,
            model_name                       TEXT,
            pty_control_path                 TEXT,
            updated_at                       TEXT NOT NULL,
            run_state                        TEXT NOT NULL DEFAULT 'idle',
            running_invocation_uuid          TEXT,
            running_os_pid                   INTEGER,
            running_os_boot_id               TEXT,
            running_os_pid_starttime_ticks   INTEGER,
            turn_started_at                  TEXT,
            turn_ended_at                    TEXT,
            turn_start_max_mailbox_seq       INTEGER,
            last_exit_code                   INTEGER,
            models_dir                       TEXT,
            effective_cwd                    TEXT,
            auto_wake_count                  INTEGER NOT NULL DEFAULT 0,
            selected_auto_wake_max           INTEGER
        );

        CREATE TABLE IF NOT EXISTS session_wake_claim (
            session_id                       TEXT PRIMARY KEY,
            claim_token                      TEXT NOT NULL,
            claimed_at                       TEXT NOT NULL,
            wake_pid                         INTEGER,
            wake_os_boot_id                  TEXT,
            wake_os_pid_starttime_ticks      INTEGER,
            wake_invocation_uuid             TEXT,
            reason                           TEXT NOT NULL,
            auto_wake_count                  INTEGER NOT NULL,
            min_pending_seq_at_claim         INTEGER,
            max_pending_seq_at_claim         INTEGER
        );

        CREATE INDEX IF NOT EXISTS idx_session_wake_claim_claimed_at
            ON session_wake_claim(claimed_at);"
}

pub(super) fn wake_sweep_progress_schema_definition() -> &'static str {
    "CREATE TABLE IF NOT EXISTS wake_sweep_progress (
        singleton         INTEGER PRIMARY KEY CHECK(singleton = 1),
        after_pending_seq INTEGER NOT NULL
     );"
}

pub(super) fn session_admission_schema_definition() -> &'static str {
    "CREATE TABLE IF NOT EXISTS session_admission_queue (
        queue_sequence          INTEGER PRIMARY KEY AUTOINCREMENT,
        admission_id            TEXT NOT NULL UNIQUE,
        registration_identity   TEXT NOT NULL UNIQUE,
        session_id              TEXT,
        state                   TEXT NOT NULL CHECK(state IN ('queued', 'admitted', 'launching', 'settled', 'cancelled')),
        queue_reason            TEXT NOT NULL,
        claim_token             TEXT,
        claimed_at_unix_ms      INTEGER,
        runtime_generation_uuid TEXT UNIQUE,
        launcher_os_pid         INTEGER NOT NULL,
        launcher_os_boot_id     TEXT NOT NULL,
        launcher_os_pid_starttime_ticks INTEGER NOT NULL,
        created_at_unix_ms      INTEGER NOT NULL,
        updated_at_unix_ms      INTEGER NOT NULL,
        CHECK (
            (state IN ('queued', 'cancelled') AND claim_token IS NULL AND claimed_at_unix_ms IS NULL)
            OR
            (state IN ('admitted', 'launching', 'settled') AND claim_token IS NOT NULL AND claimed_at_unix_ms IS NOT NULL)
        )
     );

     CREATE INDEX IF NOT EXISTS idx_session_admission_fifo
       ON session_admission_queue(state, queue_sequence);

     CREATE INDEX IF NOT EXISTS idx_session_admission_claim
       ON session_admission_queue(claim_token, claimed_at_unix_ms);"
}

pub(super) fn session_admission_scaling_indexes_definition() -> &'static str {
    "CREATE INDEX IF NOT EXISTS idx_session_admission_state_runtime
       ON session_admission_queue(state, runtime_generation_uuid);

     CREATE INDEX IF NOT EXISTS idx_runtime_generation_lifecycle_created
       ON runtime_generation(lifecycle_state, created_at);"
}

pub(super) fn ensure_session_admission_launcher_identity_schema(
    conn: &Connection,
) -> Result<(), String> {
    let columns = table_columns(
        conn,
        "session_admission_queue",
        &format_table_columns_pragma("session_admission_queue"),
    )?;
    if columns
        .iter()
        .any(|column| column == "launcher_os_pid_starttime_ticks")
    {
        return Ok(());
    }
    let active_runtime: bool = conn
        .query_row(
            "SELECT EXISTS (
                SELECT 1 FROM runtime_generation WHERE lifecycle_state != 'exited'
             )",
            [],
            |row| row.get(0),
        )
        .map_err(|err| format!("Failed to inspect runtime quiescence before v7: {err}"))?;
    let active_admission: bool = conn
        .query_row(
            "SELECT EXISTS (
                SELECT 1 FROM session_admission_queue WHERE state != 'settled'
             )",
            [],
            |row| row.get(0),
        )
        .map_err(|err| format!("Failed to inspect admission quiescence before v7: {err}"))?;
    if active_runtime || active_admission {
        return Err(
            "PID mailbox sidecar v7 migration requires a quiescent runner; stop all provider launchers before upgrading"
                .to_string(),
        );
    }

    conn.execute_batch(
        "ALTER TABLE session_admission_queue RENAME TO session_admission_queue_v6;
         DROP INDEX IF EXISTS idx_session_admission_fifo;
         DROP INDEX IF EXISTS idx_session_admission_claim;",
    )
    .map_err(|err| format!("Failed to preserve v6 session admission queue: {err}"))?;
    conn.execute_batch(session_admission_schema_definition())
        .map_err(|err| format!("Failed to create launcher-owned session admission queue: {err}"))?;
    conn.execute_batch(
        "INSERT INTO session_admission_queue (
            queue_sequence, admission_id, registration_identity, session_id, state,
            queue_reason, claim_token, claimed_at_unix_ms, runtime_generation_uuid,
            launcher_os_pid, launcher_os_boot_id,
            launcher_os_pid_starttime_ticks, created_at_unix_ms, updated_at_unix_ms
         )
         SELECT queue_sequence, admission_id, registration_identity, session_id,
                CASE WHEN state = 'queued' THEN 'cancelled' ELSE state END,
                CASE WHEN state = 'queued' THEN 'upgrade_cancelled' ELSE state END,
                CASE WHEN state = 'queued' THEN NULL ELSE claim_token END,
                CASE WHEN state = 'queued' THEN NULL ELSE claimed_at_unix_ms END,
                runtime_generation_uuid, 0, 'legacy-unverifiable', 0,
                created_at_unix_ms, updated_at_unix_ms
         FROM session_admission_queue_v6;
         DROP TABLE session_admission_queue_v6;",
    )
    .map_err(|err| format!("Failed to migrate v6 session admission rows: {err}"))
}

fn validate_session_admission_identity(value: &str, field: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err(format!("Session admission {field} must not be empty"))
    } else {
        Ok(())
    }
}

fn validate_optional_session_admission_identity(
    value: Option<&str>,
    field: &str,
) -> Result<(), String> {
    match value {
        Some(value) => validate_session_admission_identity(value, field),
        None => Ok(()),
    }
}

fn next_session_admission_on(conn: &Connection) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT admission.registration_identity
         FROM session_admission_queue admission
         WHERE admission.state = 'queued'
           AND (
               admission.session_id IS NULL
               OR NOT EXISTS (
                   SELECT 1
                   FROM runtime_generation generation
                   WHERE generation.session_id = admission.session_id
                     AND generation.lifecycle_state != 'exited'
               )
           )
         ORDER BY admission.queue_sequence ASC
         LIMIT 1",
        [],
        |row| row.get(0),
    )
    .optional()
    .map_err(|err| format!("Failed to select next eligible session admission: {err}"))
}

fn unmaterialized_session_admission_exists_on(conn: &Connection) -> Result<bool, String> {
    conn.query_row(
        "SELECT
             EXISTS (
                 SELECT 1 FROM session_admission_queue
                 WHERE state = 'admitted'
             )
             OR EXISTS (
                 SELECT 1 FROM session_admission_queue
                 WHERE state = 'launching' AND runtime_generation_uuid IS NULL
             )
             OR EXISTS (
                 SELECT 1
                 FROM runtime_generation generation
                 JOIN session_admission_queue admission
                   ON admission.runtime_generation_uuid = generation.generation_uuid
                 WHERE generation.lifecycle_state = 'starting'
                   AND admission.state = 'launching'
             )",
        [],
        |row| row.get(0),
    )
    .map_err(|err| format!("Failed to inspect materializing session admissions: {err}"))
}

fn session_admission_by_registration_on(
    conn: &Connection,
    registration_identity: &str,
) -> Result<Option<SessionAdmissionRow>, String> {
    conn.query_row(
        "SELECT queue_sequence, admission_id, registration_identity, session_id,
                state, queue_reason, claim_token, claimed_at_unix_ms, runtime_generation_uuid,
                launcher_os_pid, launcher_os_boot_id,
                launcher_os_pid_starttime_ticks
         FROM session_admission_queue
         WHERE registration_identity = ?1",
        params![registration_identity],
        |row| {
            Ok(SessionAdmissionRow {
                queue_sequence: row.get(0)?,
                admission_id: row.get(1)?,
                registration_identity: row.get(2)?,
                session_id: row.get(3)?,
                state: row.get(4)?,
                queue_reason: row.get(5)?,
                claim_token: row.get(6)?,
                claimed_at_unix_ms: row.get(7)?,
                runtime_generation_uuid: row.get(8)?,
                launcher_os_pid: row.get(9)?,
                launcher_os_boot_id: row.get(10)?,
                launcher_os_pid_starttime_ticks: row.get(11)?,
            })
        },
    )
    .optional()
    .map_err(|err| format!("Failed to read session admission row: {err}"))
}

fn recover_stale_session_admissions_on(
    conn: &Connection,
    stale_before_unix_ms: i64,
    now_unix_ms: i64,
) -> Result<(), String> {
    let exited_generation = conn
        .query_row(
            "SELECT generation.generation_uuid
             FROM runtime_generation generation
             JOIN session_admission_queue admission
               ON admission.runtime_generation_uuid = generation.generation_uuid
             WHERE generation.lifecycle_state = 'exited'
               AND admission.state IN ('admitted', 'launching')
             ORDER BY generation.created_at
             LIMIT 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|err| format!("Failed to inspect exited session admissions: {err}"))?;
    if let Some(generation_uuid) = exited_generation {
        conn.execute(
            "UPDATE session_admission_queue
             SET state = 'settled', queue_reason = 'settled', updated_at_unix_ms = ?2
             WHERE runtime_generation_uuid = ?1
               AND state IN ('admitted', 'launching')",
            params![generation_uuid, now_unix_ms],
        )
        .map_err(|err| format!("Failed to settle exited session admission: {err}"))?;
    }
    conn.execute(
        "UPDATE session_admission_queue
         SET state = 'queued', queue_reason = 'fifo_wait',
             claim_token = NULL, claimed_at_unix_ms = NULL,
             runtime_generation_uuid = NULL, updated_at_unix_ms = ?2
         WHERE state = 'admitted'
           AND runtime_generation_uuid IS NULL
           AND claimed_at_unix_ms <= ?1",
        params![stale_before_unix_ms, now_unix_ms],
    )
    .map_err(|err| format!("Failed to recover stale session admission reservations: {err}"))?;
    Ok(())
}

fn cancel_dead_session_admission_head_on(
    conn: &Connection,
    now_unix_ms: i64,
) -> Result<bool, String> {
    let owner = conn
        .query_row(
            "SELECT registration_identity, launcher_os_pid, launcher_os_boot_id,
                    launcher_os_pid_starttime_ticks
             FROM session_admission_queue
             WHERE state = 'launching' AND runtime_generation_uuid IS NULL
             ORDER BY queue_sequence
             LIMIT 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    ProcessIdentity {
                        os_pid: row.get(1)?,
                        os_boot_id: row.get(2)?,
                        os_pid_starttime_ticks: row.get(3)?,
                    },
                ))
            },
        )
        .optional()
        .map_err(|err| format!("Failed to read unbound launcher identity: {err}"))?;
    let owner = match owner {
        Some(owner) => Some(owner),
        None => conn
            .query_row(
                "SELECT registration_identity, launcher_os_pid, launcher_os_boot_id,
                        launcher_os_pid_starttime_ticks
                 FROM session_admission_queue
                 WHERE state = 'queued'
                 ORDER BY queue_sequence
                 LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        ProcessIdentity {
                            os_pid: row.get(1)?,
                            os_boot_id: row.get(2)?,
                            os_pid_starttime_ticks: row.get(3)?,
                        },
                    ))
                },
            )
            .optional()
            .map_err(|err| format!("Failed to read FIFO launcher identity: {err}"))?,
    };
    let Some((registration_identity, recorded)) = owner else {
        return Ok(false);
    };
    let live = pid_identity::read_live_process_identity(recorded.os_pid)?;
    if live.as_ref() == Some(&recorded) {
        return Ok(false);
    }
    let changed = conn
        .execute(
            "UPDATE session_admission_queue
             SET state = 'cancelled', queue_reason = 'launcher_exited', claim_token = NULL,
                 claimed_at_unix_ms = NULL, updated_at_unix_ms = ?2
             WHERE registration_identity = ?1
               AND (state = 'queued'
                    OR (state = 'launching' AND runtime_generation_uuid IS NULL))
               AND launcher_os_pid = ?3
               AND launcher_os_boot_id = ?4
               AND launcher_os_pid_starttime_ticks = ?5",
            params![
                &registration_identity,
                now_unix_ms,
                recorded.os_pid,
                &recorded.os_boot_id,
                recorded.os_pid_starttime_ticks,
            ],
        )
        .map_err(|err| format!("Failed to cancel dead queued launcher: {err}"))?;
    Ok(changed == 1)
}

fn reconcile_dead_starting_generations_on(
    conn: &Connection,
    now_unix_ms: i64,
) -> Result<(), String> {
    let generation = conn
        .query_row(
            "SELECT generation_uuid, creator_identity_os_pid,
                    creator_identity_os_boot_id,
                    creator_identity_os_pid_starttime_ticks
             FROM runtime_generation
             WHERE lifecycle_state = 'starting'
               AND identity_os_pid IS NULL
               AND creator_identity_os_pid IS NOT NULL
             ORDER BY created_at
             LIMIT 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    ProcessIdentity {
                        os_pid: row.get(1)?,
                        os_boot_id: row.get(2)?,
                        os_pid_starttime_ticks: row.get(3)?,
                    },
                ))
            },
        )
        .optional()
        .map_err(|err| format!("Failed to read starting-generation creator: {err}"))?;
    let Some((generation_uuid, creator)) = generation else {
        return Ok(());
    };
    let live = pid_identity::read_live_process_identity(creator.os_pid)?;
    if live.as_ref() == Some(&creator) {
        return Ok(());
    }
    conn.execute(
        "UPDATE runtime_generation
         SET lifecycle_state = 'exited', exited_at = ?2,
             terminal_reason = 'recovered_dead'
         WHERE generation_uuid = ?1
           AND lifecycle_state = 'starting'
           AND identity_os_pid IS NULL
           AND creator_identity_os_pid = ?3
           AND creator_identity_os_boot_id = ?4
           AND creator_identity_os_pid_starttime_ticks = ?5",
        params![
            &generation_uuid,
            now_rfc3339(),
            creator.os_pid,
            &creator.os_boot_id,
            creator.os_pid_starttime_ticks,
        ],
    )
    .map_err(|err| format!("Failed to reconcile dead starting generation: {err}"))?;
    conn.execute(
        "UPDATE session_admission_queue
          SET state = 'settled', queue_reason = 'settled', updated_at_unix_ms = ?2
         WHERE runtime_generation_uuid = ?1
           AND state IN ('admitted', 'launching')",
        params![&generation_uuid, now_unix_ms],
    )
    .map_err(|err| format!("Failed to settle dead starting admission: {err}"))?;
    Ok(())
}

fn bind_runtime_generation_admission_on(
    conn: &Connection,
    spawn_invocation_uuid: &str,
    generation_id: &RuntimeGenerationId,
    creator_identity: &ProcessIdentity,
) -> Result<(), GenerationStorageError> {
    let admission_exists = conn
        .query_row(
            "SELECT EXISTS (
                SELECT 1 FROM session_admission_queue
                WHERE registration_identity = ?1
             )",
            params![spawn_invocation_uuid],
            |row| row.get::<_, bool>(0),
        )
        .map_err(generation_storage_error(
            "inspect runtime generation session admission",
        ))?;
    if !admission_exists {
        return Ok(());
    }
    let changed = conn
        .execute(
            "UPDATE session_admission_queue
         SET runtime_generation_uuid = ?2
         WHERE registration_identity = ?1
           AND state = 'launching'
           AND runtime_generation_uuid IS NULL
           AND launcher_os_pid = ?3
           AND launcher_os_boot_id = ?4
           AND launcher_os_pid_starttime_ticks = ?5",
            params![
                spawn_invocation_uuid,
                generation_id.to_string(),
                creator_identity.os_pid,
                &creator_identity.os_boot_id,
                creator_identity.os_pid_starttime_ticks,
            ],
        )
        .map_err(generation_storage_error(
            "bind runtime generation to session admission",
        ))?;
    if changed != 1 {
        return Err(GenerationStorageError::new(
            "Runtime generation session admission is stale or owned by another launcher"
                .to_string(),
        ));
    }
    Ok(())
}

fn bind_runtime_generation_admission_session_on(
    conn: &Connection,
    generation_id: &RuntimeGenerationId,
    session_id: &str,
) -> Result<(), GenerationStorageError> {
    let conflicting = conn
        .query_row(
            "SELECT session_id
             FROM session_admission_queue
             WHERE runtime_generation_uuid = ?1 AND session_id IS NOT NULL",
            params![generation_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(generation_storage_error(
            "inspect runtime generation admission session",
        ))?;
    if conflicting
        .as_deref()
        .is_some_and(|current| current != session_id)
    {
        return Err(GenerationStorageError::new(
            "Runtime generation admission belongs to another session".to_string(),
        ));
    }
    conn.execute(
        "UPDATE session_admission_queue
         SET session_id = ?2
         WHERE runtime_generation_uuid = ?1 AND session_id IS NULL",
        params![generation_id.to_string(), session_id],
    )
    .map(|_| ())
    .map_err(generation_storage_error(
        "bind runtime generation admission session",
    ))
}

fn settle_runtime_generation_admission_on(
    conn: &Connection,
    generation_id: &RuntimeGenerationId,
) -> Result<(), GenerationStorageError> {
    let now_unix_ms = now_unix_millis().map_err(GenerationStorageError::new)?;
    conn.execute(
        "UPDATE session_admission_queue
         SET state = 'settled', queue_reason = 'settled', updated_at_unix_ms = ?2
         WHERE runtime_generation_uuid = ?1
           AND state IN ('admitted', 'launching')",
        params![generation_id.to_string(), now_unix_ms],
    )
    .map(|_| ())
    .map_err(generation_storage_error(
        "settle runtime generation session admission",
    ))
}

fn normalized_mailbox_path(sidecar_path: &Path) -> std::io::Result<PathBuf> {
    match fs::canonicalize(sidecar_path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let parent = sidecar_path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            let file_name = sidecar_path.file_name().ok_or_else(|| {
                std::io::Error::new(
                    ErrorKind::InvalidInput,
                    "PID mailbox sidecar path must name a file",
                )
            })?;
            Ok(fs::canonicalize(parent)?.join(file_name))
        }
        Err(error) => Err(error),
    }
}

fn mailbox_authority_path(sidecar_path: &Path) -> PathBuf {
    let mut path = sidecar_path.as_os_str().to_owned();
    path.push(".authority.lock");
    PathBuf::from(path)
}

fn validate_mailbox_storage_path(path: &Path) -> std::io::Result<()> {
    let valid_role = path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .map(str::to_ascii_lowercase)
        .is_some_and(|file_name| {
            file_name == "pid-identity.db" || file_name.ends_with(".pid-identity.db")
        });
    if valid_role {
        Ok(())
    } else {
        Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            "PID mailbox sidecar path must use a reserved sidecar storage role",
        ))
    }
}

fn validate_mailbox_source(
    source_path: &Path,
    retained_path: &Path,
    expected_identity: Option<MailboxFileIdentity>,
) -> std::io::Result<Option<MailboxFileIdentity>> {
    if normalized_mailbox_path(source_path)? != retained_path {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            "PID mailbox sidecar source changed during authority acquisition",
        ));
    }
    let observed_identity = inspect_mailbox_storage_file(retained_path)?;
    if expected_identity.is_some() && observed_identity != expected_identity {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            "PID mailbox sidecar target changed during authority acquisition",
        ));
    }
    validate_mailbox_sqlite_artifacts(retained_path)?;
    Ok(observed_identity)
}

#[cfg(any(unix, windows))]
fn inspect_mailbox_storage_file(path: &Path) -> std::io::Result<Option<MailboxFileIdentity>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let identity = match crate::filesystem_identity::path_file_identity(path, &metadata) {
        Ok(identity) => identity,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    classify_mailbox_storage_metadata(path, &metadata, identity)
}

fn classify_mailbox_storage_metadata(
    path: &Path,
    metadata: &std::fs::Metadata,
    identity: crate::filesystem_identity::OpenFileIdentity,
) -> std::io::Result<Option<MailboxFileIdentity>> {
    if !metadata.is_file() {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            format!(
                "PID mailbox storage requires a regular file with exactly one hard link: {}",
                path.display()
            ),
        ));
    }
    // SQLite may unlink a WAL or journal between path lookup and metadata
    // inspection. A zero-link inode is already outside the namespace; actual
    // hard-link aliases still have more than one link and remain rejected.
    if identity.links == 0 {
        return Ok(None);
    }
    if identity.links != 1 {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            format!(
                "PID mailbox storage requires a regular file with exactly one hard link: {}",
                path.display()
            ),
        ));
    }
    Ok(Some(mailbox_identity(identity)))
}

#[cfg(not(any(unix, windows)))]
fn inspect_mailbox_storage_file(path: &Path) -> std::io::Result<Option<MailboxFileIdentity>> {
    Err(std::io::Error::new(
        ErrorKind::Unsupported,
        format!(
            "PID mailbox sidecar file identity is unsupported on this platform: {}",
            path.display()
        ),
    ))
}

fn validate_mailbox_sqlite_artifacts(sidecar_path: &Path) -> std::io::Result<()> {
    for artifact in mailbox_sqlite_artifact_paths(sidecar_path) {
        inspect_mailbox_storage_file(&artifact)?;
    }
    Ok(())
}

fn mailbox_sqlite_artifact_paths(sidecar_path: &Path) -> [PathBuf; 3] {
    [
        path_with_storage_suffix(sidecar_path, "-journal"),
        path_with_storage_suffix(sidecar_path, "-wal"),
        path_with_storage_suffix(sidecar_path, "-shm"),
    ]
}

fn path_with_storage_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn opened_mailbox_file_identity(
    file: &std::fs::File,
    path: &Path,
) -> std::io::Result<MailboxFileIdentity> {
    let metadata = file.metadata()?;
    let identity = crate::filesystem_identity::open_file_identity(file)?;
    if !metadata.is_file() || identity.links != 1 {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            format!(
                "PID mailbox storage requires a regular file with exactly one hard link: {}",
                path.display()
            ),
        ));
    }
    Ok(mailbox_identity(identity))
}

fn mailbox_identity(identity: crate::filesystem_identity::OpenFileIdentity) -> MailboxFileIdentity {
    MailboxFileIdentity {
        volume: identity.storage,
        file: identity.file,
    }
}

fn ensure_mailbox_sidecar_identity_locked(conn: &Connection) -> Result<(), String> {
    let count = conn
        .query_row("SELECT COUNT(*) FROM mailbox_sidecar_identity", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|err| format!("Failed to validate PID mailbox sidecar identity: {err}"))?;
    if count == 0 {
        conn.execute(
            "INSERT INTO mailbox_sidecar_identity (
                singleton, generation_uuid, created_at
             ) VALUES (1, ?1, ?2)",
            params![Uuid::new_v4().to_string(), now_rfc3339()],
        )
        .map_err(|err| format!("Failed to initialize PID mailbox sidecar identity: {err}"))?;
        return Ok(());
    }
    if count != 1 {
        return Err(format!(
            "PID mailbox sidecar identity is not a singleton: {count} rows"
        ));
    }
    Ok(())
}

fn ensure_mailbox_columns(conn: &Connection) -> Result<(), String> {
    let sql = format_table_columns_pragma("mailbox");
    let columns = table_columns(conn, "mailbox", &sql)?;
    for (name, definition) in missing_mailbox_columns(&columns) {
        add_sidecar_column(conn, "mailbox", name, definition)?;
    }
    Ok(())
}

fn ensure_mailbox_delivery_attempt_columns(conn: &Connection) -> Result<(), String> {
    let sql = format_table_columns_pragma("mailbox_delivery_attempts");
    let columns = table_columns(conn, "mailbox_delivery_attempts", &sql)?;
    if columns.is_empty() {
        return Ok(());
    }
    for (name, definition) in [
        ("submission_started_at", "TEXT"),
        ("evidence_turn_generation_id", "TEXT"),
        ("evidence_observed_at", "INTEGER"),
        ("evidence_reconciled_at", "TEXT"),
        ("evidence_disposition", "TEXT"),
        ("observation_provider_name", "TEXT"),
        ("observation_provider_instance_id", "TEXT"),
        ("observation_settings_id", "TEXT"),
        ("observation_session_id", "TEXT"),
        ("observation_anchor_token", "TEXT"),
        ("observation_expected_sha256", "TEXT"),
        ("observation_error", "TEXT"),
        ("observation_confirmed_turn_id", "TEXT"),
        ("observation_confirmed_at", "TEXT"),
    ] {
        if !columns.iter().any(|column| column == name) {
            add_sidecar_column(conn, "mailbox_delivery_attempts", name, definition)?;
        }
    }
    conn.execute_batch(
        "UPDATE mailbox_delivery_attempts
         SET submission_started_at = acknowledged_at
         WHERE acknowledged_at IS NOT NULL
           AND submission_started_at IS NULL;

         UPDATE mailbox_delivery_attempts AS attempts
         SET evidence_turn_generation_id = (
                 SELECT generation_uuid
                 FROM runtime_generation AS generations
                 WHERE generations.session_id = attempts.session_id
                   AND generations.spawn_invocation_uuid = attempts.delivery_invocation_uuid
             ),
             evidence_observed_at = CAST(strftime('%s', acknowledged_at) AS INTEGER) * 1000,
             evidence_disposition = 'legacy_pending'
         WHERE acknowledged_at IS NOT NULL
           AND evidence_turn_generation_id IS NULL
           AND evidence_observed_at IS NULL
           AND evidence_disposition IS NULL
           AND 1 = (
                 SELECT COUNT(*)
                 FROM runtime_generation AS generations
                 WHERE generations.session_id = attempts.session_id
                   AND generations.spawn_invocation_uuid = attempts.delivery_invocation_uuid
              );

         UPDATE mailbox_delivery_attempts AS attempts
         SET evidence_disposition = CASE (
                 SELECT COUNT(*)
                 FROM runtime_generation AS generations
                 WHERE generations.session_id = attempts.session_id
                   AND generations.spawn_invocation_uuid = attempts.delivery_invocation_uuid
             )
             WHEN 0 THEN 'legacy_unmatched_generation'
             ELSE 'legacy_ambiguous_generation'
             END
         WHERE acknowledged_at IS NOT NULL
           AND evidence_disposition IS NULL;",
    )
    .map_err(|err| format!("Failed to backfill mailbox delivery settlement: {err}"))?;
    Ok(())
}

fn mailbox_column_additions() -> [(&'static str, &'static str); 8] {
    [
        ("payload_file_path", "TEXT"),
        ("payload_sha256", "TEXT"),
        ("payload_byte_len", "INTEGER"),
        ("payload_retention_policy", "TEXT"),
        ("payload_compacted_at", "TEXT"),
        ("submission_token", "TEXT"),
        ("target_kind", "TEXT"),
        ("target_id", "TEXT"),
    ]
}

fn missing_mailbox_columns(columns: &[String]) -> Vec<(&'static str, &'static str)> {
    mailbox_column_additions()
        .into_iter()
        .filter(|(name, _)| !columns.iter().any(|column| column == name))
        .collect()
}

fn ensure_mailbox_target_index(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_mailbox_pending_target
             ON mailbox(target_kind, target_id, delivered_at, seq);",
    )
    .map_err(|err| format!("Failed to ensure mailbox target index: {err}"))
}

fn ensure_mailbox_compaction_index(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_mailbox_delivered_compaction
             ON mailbox(kind, payload_compacted_at, delivered_at, seq);",
    )
    .map_err(|err| format!("Failed to ensure mailbox compaction index: {err}"))
}

fn ensure_terminal_history_retention_schema(conn: &Connection) -> Result<(), String> {
    let columns = table_columns(
        conn,
        "completion_event",
        &format_table_columns_pragma("completion_event"),
    )?;
    if !columns
        .iter()
        .any(|column| column == "payload_reclaimed_at")
    {
        add_sidecar_column(conn, "completion_event", "payload_reclaimed_at", "TEXT")?;
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_mailbox_terminal_retention
             ON mailbox(delivered_at, seq)
             WHERE delivered_at IS NOT NULL;
         CREATE INDEX IF NOT EXISTS idx_mailbox_delivery_attempt_terminal_retention
             ON mailbox_delivery_attempts(resolved_at, created_at, attempt_id)
             WHERE resolved_at IS NOT NULL;
         CREATE INDEX IF NOT EXISTS idx_completion_event_payload_retention
             ON completion_event(payload_reclaimed_at, triggered_at, event_id)
             WHERE state = 'triggered';",
    )
    .map_err(|err| format!("Failed to ensure terminal history retention indexes: {err}"))
}

fn ensure_terminal_payload_lookup_indexes(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_mailbox_payload_reference
             ON mailbox(payload_sha256)
             WHERE payload_sha256 IS NOT NULL;
         CREATE INDEX IF NOT EXISTS idx_completion_event_payload_reference
             ON completion_event(payload_sha256, event_id);",
    )
    .map_err(|err| format!("Failed to ensure terminal payload lookup indexes: {err}"))
}

fn ensure_mailbox_delivery_owner_index(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_mailbox_delivery_owner
             ON mailbox(owner_invocation_uuid, delivered_by_invocation_uuid, seq)
             WHERE delivered_by_invocation_uuid IS NOT NULL;",
    )
    .map_err(|err| format!("Failed to ensure mailbox delivery-owner index: {err}"))
}

fn ensure_session_runtime_columns(conn: &Connection) -> Result<(), String> {
    let sql = format_table_columns_pragma("session_runtime");
    let columns = table_columns(conn, "session_runtime", &sql)?;
    for (name, definition) in missing_session_runtime_columns(&columns) {
        add_session_runtime_column(conn, name, definition)?;
    }
    Ok(())
}

fn ensure_wake_claim_process_identity_columns(conn: &Connection) -> Result<(), String> {
    let sql = format_table_columns_pragma("session_wake_claim");
    let columns = table_columns(conn, "session_wake_claim", &sql)?;
    for (name, definition) in [
        ("wake_os_boot_id", "TEXT"),
        ("wake_os_pid_starttime_ticks", "INTEGER"),
    ] {
        if !columns.iter().any(|column| column == name) {
            add_sidecar_column(conn, "session_wake_claim", name, definition)?;
        }
    }
    Ok(())
}

fn ensure_runtime_generation_columns(conn: &Connection) -> Result<(), String> {
    let sql = format_table_columns_pragma("runtime_generation");
    let columns = table_columns(conn, "runtime_generation", &sql)?;
    for (name, definition) in missing_runtime_generation_columns(&columns) {
        add_sidecar_column(conn, "runtime_generation", name, definition)?;
    }
    Ok(())
}

fn settle_unverifiable_runtime_generations(conn: &Connection) -> Result<(), String> {
    let now = now_rfc3339();
    conn.execute(
        "UPDATE runtime_generation
         SET lifecycle_state = 'exited',
             exited_at = ?1,
             terminal_reason = CASE lifecycle_state
                 WHEN 'starting' THEN 'startup_failed'
                 ELSE 'abnormal_termination'
             END,
             active_delivery_claim_uuid = NULL,
             active_delivery_claimed_at = NULL,
             active_delivery_seqs_json = NULL
         WHERE (lifecycle_state = 'starting'
                AND creator_identity_os_pid IS NULL
                AND creator_identity_os_boot_id IS NULL
                AND creator_identity_os_pid_starttime_ticks IS NULL)
            OR (lifecycle_state IN ('running', 'draining')
                AND identity_os_pid IS NULL
                AND identity_os_boot_id IS NULL
                AND identity_os_pid_starttime_ticks IS NULL)",
        params![&now],
    )
    .map_err(|err| format!("Failed to settle unverifiable runtime generations: {err}"))?;
    Ok(())
}

fn runtime_generation_column_additions() -> [(&'static str, &'static str); 6] {
    [
        ("active_delivery_claimed_at", "TEXT"),
        ("active_delivery_seqs_json", "TEXT"),
        ("creator_identity_os_pid", "INTEGER"),
        ("creator_identity_os_boot_id", "TEXT"),
        ("creator_identity_os_pid_starttime_ticks", "INTEGER"),
        ("created_at", "TEXT NOT NULL DEFAULT '1970-01-01T00:00:00Z'"),
    ]
}

fn missing_runtime_generation_columns(columns: &[String]) -> Vec<(&'static str, &'static str)> {
    runtime_generation_column_additions()
        .into_iter()
        .filter(|(name, _)| !columns.iter().any(|column| column == name))
        .collect()
}

fn session_runtime_column_additions() -> [(&'static str, &'static str); 13] {
    [
        ("run_state", "TEXT NOT NULL DEFAULT 'idle'"),
        ("running_invocation_uuid", "TEXT"),
        ("running_os_pid", "INTEGER"),
        ("running_os_boot_id", "TEXT"),
        ("running_os_pid_starttime_ticks", "INTEGER"),
        ("turn_started_at", "TEXT"),
        ("turn_ended_at", "TEXT"),
        ("turn_start_max_mailbox_seq", "INTEGER"),
        ("last_exit_code", "INTEGER"),
        ("models_dir", "TEXT"),
        ("effective_cwd", "TEXT"),
        ("auto_wake_count", "INTEGER NOT NULL DEFAULT 0"),
        ("selected_auto_wake_max", "INTEGER"),
    ]
}

fn missing_session_runtime_columns(columns: &[String]) -> Vec<(&'static str, &'static str)> {
    session_runtime_column_additions()
        .into_iter()
        .filter(|(name, _)| !columns.iter().any(|column| column == name))
        .collect()
}

fn add_session_runtime_column(
    conn: &Connection,
    name: &str,
    definition: &str,
) -> Result<(), String> {
    add_sidecar_column(conn, "session_runtime", name, definition)
}

fn add_sidecar_column(
    conn: &Connection,
    table: &str,
    name: &str,
    definition: &str,
) -> Result<(), String> {
    conn.execute_batch(&sidecar_add_column_sql(table, name, definition))
        .map_err(|err| format!("Failed to add {table}.{name}: {err}"))
}

fn sidecar_add_column_sql(table: &str, name: &str, definition: &str) -> String {
    format!("ALTER TABLE {table} ADD COLUMN {name} {definition};")
}

fn format_table_columns_pragma(table: &str) -> String {
    format!("PRAGMA table_info({table})")
}

fn table_columns(conn: &Connection, table: &str, sql: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare(sql)
        .map_err(|err| format!("Failed to inspect {table} columns: {err}"))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|err| format!("Failed to query {table} columns: {err}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("Failed to read {table} column: {err}"))
}

fn validate_legacy_run_state(run_state: &str) -> Result<(), String> {
    match run_state {
        "idle" | "running" => Ok(()),
        other => Err(format!("Invalid session_runtime.run_state value: {other}")),
    }
}

fn pending_seq_bounds_on(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<(i64, i64)>, String> {
    conn.query_row(
        "SELECT MIN(seq), MAX(seq)
         FROM mailbox
         WHERE session_id = ?1
           AND delivered_at IS NULL
           AND (delivery_error IS NULL OR delivery_error != ?2)",
        params![session_id, WAKE_SWEEP_ABANDONED_ERROR],
        |row| {
            let min_seq: Option<i64> = row.get(0)?;
            let max_seq: Option<i64> = row.get(1)?;
            Ok(min_seq.zip(max_seq))
        },
    )
    .map_err(|err| format!("Failed to read pending mailbox seq bounds: {err}"))
}

fn wake_claim(conn: &Connection, session_id: &str) -> Result<Option<WakeClaimRow>, String> {
    conn.query_row(
        "SELECT session_id, claim_token, claimed_at, wake_pid, wake_invocation_uuid,
                reason, auto_wake_count, min_pending_seq_at_claim, max_pending_seq_at_claim
         FROM session_wake_claim
         WHERE session_id = ?1",
        params![session_id],
        map_wake_claim_row,
    )
    .optional()
    .map_err(|err| format!("Failed to read wake claim row: {err}"))
}

fn wake_claim_tx(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
) -> Result<Option<WakeClaimRow>, String> {
    tx.query_row(
        "SELECT session_id, claim_token, claimed_at, wake_pid, wake_invocation_uuid,
                reason, auto_wake_count, min_pending_seq_at_claim, max_pending_seq_at_claim
         FROM session_wake_claim
         WHERE session_id = ?1",
        params![session_id],
        map_wake_claim_row,
    )
    .optional()
    .map_err(|err| format!("Failed to read wake claim row: {err}"))
}

fn claim_is_stale(claim: &WakeClaimRow, stale_after_seconds: i64) -> bool {
    let Some(claimed_at) = parse_claimed_at(&claim.claimed_at) else {
        return true;
    };
    claim_age_exceeds_stale_after(claimed_at, stale_after_seconds)
}

fn parse_claimed_at(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn claim_age_exceeds_stale_after(claimed_at: DateTime<Utc>, stale_after_seconds: i64) -> bool {
    let age = Utc::now().signed_duration_since(claimed_at);
    age > Duration::seconds(stale_after_seconds)
}

fn map_session_metadata_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionMetadataRow> {
    Ok(SessionMetadataRow {
        session_id: row.get(0)?,
        mode: row.get(1)?,
        invocation_uuid: row.get(2)?,
        provider_name: row.get(3)?,
        model_name: row.get(4)?,
        updated_at: row.get(5)?,
        models_dir: row.get(6)?,
        effective_cwd: row.get(7)?,
        auto_wake_count: row.get(8)?,
    })
}

fn map_legacy_runtime_projection_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<LegacyRuntimeProjectionRow> {
    Ok(LegacyRuntimeProjectionRow {
        session_id: row.get(0)?,
        pty_control_path: row.get(1)?,
        updated_at: row.get(2)?,
        run_state: row.get(3)?,
        running_invocation_uuid: row.get(4)?,
        running_os_pid: row.get(5)?,
        running_os_boot_id: row.get(6)?,
        running_os_pid_starttime_ticks: row.get(7)?,
        turn_started_at: row.get(8)?,
        turn_ended_at: row.get(9)?,
        turn_start_max_mailbox_seq: row.get(10)?,
        last_exit_code: row.get(11)?,
    })
}

fn map_wake_claim_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WakeClaimRow> {
    Ok(WakeClaimRow {
        session_id: row.get(0)?,
        claim_token: row.get(1)?,
        claimed_at: row.get(2)?,
        wake_pid: row.get(3)?,
        wake_invocation_uuid: row.get(4)?,
        reason: row.get(5)?,
        auto_wake_count: row.get(6)?,
        min_pending_seq_at_claim: row.get(7)?,
        max_pending_seq_at_claim: row.get(8)?,
    })
}

fn map_mailbox_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MailboxRow> {
    Ok(MailboxRow {
        seq: row.get(0)?,
        session_id: row.get(1)?,
        kind: row.get(2)?,
        handle: row.get(3)?,
        payload_json: row.get(4)?,
        enqueued_at: row.get(5)?,
        delivered_at: row.get(6)?,
        delivered_by_invocation_uuid: row.get(7)?,
        delivery_attempts: row.get(8)?,
        delivery_error: row.get(9)?,
        owner_invocation_uuid: row.get(10)?,
        matched_os_pid: row.get(11)?,
        matched_os_boot_id: row.get(12)?,
        matched_os_pid_starttime_ticks: row.get(13)?,
        matched_chain_index: row.get(14)?,
        state_dir: row.get(15)?,
        meta_path: row.get(16)?,
        log_path: row.get(17)?,
        rc_path: row.get(18)?,
        rc: row.get(19)?,
        payload_file_path: row.get(20)?,
        payload_sha256: row.get(21)?,
        payload_byte_len: row.get(22)?,
        payload_retention_policy: row.get(23)?,
        payload_compacted_at: row.get(24)?,
        submission_token: row.get(25)?,
        target_kind: row.get(26)?,
        target_id: row.get(27)?,
    })
}

fn collect_rows<F>(rows: rusqlite::MappedRows<'_, F>) -> Result<Vec<MailboxRow>, String>
where
    F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<MailboxRow>,
{
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("Failed to read mailbox row: {err}"))
}

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn now_unix_millis() -> Result<i64, String> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| format!("system clock precedes Unix epoch: {error}"))?
        .as_millis()
        .try_into()
        .map_err(|_| "mailbox evidence timestamp exceeds i64".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StateDb;

    const STARTING_GENERATION_FIXTURE_PATH: &str = "OULIPOLY_TEST_STARTING_GENERATION_FIXTURE_PATH";
    const WAKE_CLAIM_FOREIGN_CHILD_FIXTURE: &str = "OULIPOLY_TEST_WAKE_CLAIM_FOREIGN_CHILD";

    #[test]
    fn pid_and_mailbox_readers_share_one_snapshot_lifetime() {
        let directory = tempfile::tempdir().unwrap();
        let sidecar_path = directory.path().join("pid-identity.db");
        drop(MailboxDb::open(&sidecar_path).unwrap());

        let (pid, mailbox) = MailboxDb::open_read_only_with_pid_identity_and_work_timeout(
            &sidecar_path,
            StdDuration::from_millis(250),
            StdDuration::from_secs(5),
            &|| false,
        )
        .unwrap();

        assert_eq!(pid.path(), sidecar_path);
        assert_eq!(mailbox.path(), sidecar_path);
        drop(pid);
        let schema_version: i64 = mailbox
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(schema_version, schema::CURRENT_VERSION);
    }

    #[test]
    fn starting_runtime_generation_fixture() {
        let Some(path) = std::env::var_os(STARTING_GENERATION_FIXTURE_PATH) else {
            return;
        };
        let generation_id =
            RuntimeGenerationId::parse("90111111-1111-4111-8111-111111111111").unwrap();
        let mut db = MailboxDb::open(Path::new(&path)).unwrap();
        let GenerationMutation::Applied(row) = db
            .runtime_lifecycle()
            .create_runtime_generation(CreateRuntimeGeneration {
                generation_id: &generation_id,
                spawn_invocation_uuid: "starting-fixture-invocation",
                session_id: Some("starting-fixture-session"),
                runtime_mode: "headless",
                provider_name: "provider-a",
                model_name: Some("model-a"),
                pty_control_path: None,
                models_dir: None,
                effective_cwd: None,
            })
            .unwrap()
        else {
            panic!("fixture generation was not created");
        };
        assert!(matches!(
            row.creator_process_evidence,
            ExactProcessEvidence::Recorded(_)
        ));
    }

    #[test]
    fn wake_claim_foreign_child_fixture() {
        if std::env::var_os(WAKE_CLAIM_FOREIGN_CHILD_FIXTURE).is_none() {
            return;
        }
        std::thread::sleep(StdDuration::from_secs(30));
    }

    #[test]
    fn dead_starting_generation_creator_is_recovered_after_restart() {
        let directory = tempfile::tempdir().unwrap();
        let sidecar_path = directory.path().join("pid-identity.db");
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("mailbox::tests::starting_runtime_generation_fixture")
            .arg("--nocapture")
            .env(STARTING_GENERATION_FIXTURE_PATH, &sidecar_path)
            .status()
            .unwrap();
        assert!(status.success());

        let generation_id =
            RuntimeGenerationId::parse("90111111-1111-4111-8111-111111111111").unwrap();
        let mut db = MailboxDb::open(&sidecar_path).unwrap();
        let before = db
            .runtime_lifecycle_reader()
            .runtime_generation(&generation_id)
            .unwrap()
            .unwrap();
        assert_eq!(before.lifecycle_state, RuntimeLifecycleState::Starting);
        assert!(matches!(
            before.creator_process_evidence,
            ExactProcessEvidence::Recorded(_)
        ));
        assert_eq!(
            db.runtime_lifecycle()
                .reconcile_session_liveness("starting-fixture-session")
                .unwrap(),
            SessionLiveness::Idle
        );
        let after = db
            .runtime_lifecycle_reader()
            .runtime_generation(&generation_id)
            .unwrap()
            .unwrap();
        assert_eq!(after.lifecycle_state, RuntimeLifecycleState::Exited);
        assert_eq!(
            after.terminal_reason,
            Some(RuntimeTerminalReason::RecoveredDead)
        );
    }

    #[cfg(unix)]
    #[test]
    fn unlinked_sqlite_artifact_is_absent_not_a_hard_link_violation() {
        let directory = tempfile::tempdir().unwrap();
        let artifact_path = directory.path().join("pid-identity.db-wal");
        let artifact = File::create(&artifact_path).unwrap();
        fs::remove_file(&artifact_path).unwrap();
        let metadata = artifact.metadata().unwrap();
        let identity = crate::filesystem_identity::open_file_identity(&artifact).unwrap();

        assert_eq!(identity.links, 0);
        assert_eq!(
            classify_mailbox_storage_metadata(&artifact_path, &metadata, identity).unwrap(),
            None
        );
    }

    #[test]
    fn one_hundred_concurrent_startups_share_fresh_sidecar_authority() {
        assert_one_hundred_concurrent_startups(false);
    }

    #[test]
    fn one_hundred_mixed_mailbox_and_pid_startups_share_fresh_sidecar_authority() {
        assert_one_hundred_concurrent_startups(true);
    }

    fn assert_one_hundred_concurrent_startups(mixed_pid_handles: bool) {
        const STARTUPS: usize = 100;

        enum SidecarHandle {
            Mailbox(MailboxDb),
            Pid(crate::pid_identity::PidIdentityDb),
        }

        let directory = tempfile::tempdir().unwrap();
        let sidecar_path = directory.path().join("pid-identity.db");
        let start = std::sync::Arc::new(std::sync::Barrier::new(STARTUPS + 1));
        let release = std::sync::Arc::new(std::sync::Barrier::new(STARTUPS + 1));
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        let mut threads = Vec::with_capacity(STARTUPS);

        for ordinal in 0..STARTUPS {
            let sidecar_path = sidecar_path.clone();
            let start = std::sync::Arc::clone(&start);
            let release = std::sync::Arc::clone(&release);
            let result_tx = result_tx.clone();
            threads.push(std::thread::spawn(move || {
                start.wait();
                let handle = if mixed_pid_handles && ordinal % 2 == 1 {
                    crate::pid_identity::PidIdentityDb::open(&sidecar_path).map(SidecarHandle::Pid)
                } else {
                    MailboxDb::open(&sidecar_path).map(SidecarHandle::Mailbox)
                };
                result_tx
                    .send(handle.as_ref().map(|_| ()).map_err(Clone::clone))
                    .unwrap();
                release.wait();
                match handle {
                    Ok(SidecarHandle::Mailbox(mailbox)) => drop(mailbox),
                    Ok(SidecarHandle::Pid(pid_identity)) => drop(pid_identity),
                    Err(_) => {}
                }
            }));
        }
        drop(result_tx);

        start.wait();
        let results = result_rx.iter().take(STARTUPS).collect::<Vec<_>>();
        release.wait();
        for thread in threads {
            thread.join().unwrap();
        }

        assert_eq!(results.len(), STARTUPS);
        let failures = results
            .into_iter()
            .filter_map(Result::err)
            .collect::<Vec<_>>();
        assert!(failures.is_empty(), "startup failures: {failures:#?}");
    }

    #[test]
    fn ordinary_mailbox_connections_wait_for_bounded_writer_contention() {
        let directory = tempfile::tempdir().unwrap();
        let sidecar_path = directory.path().join("pid-identity.db");
        let blocker = MailboxDb::open(&sidecar_path).unwrap();
        let mut launch = MailboxDb::open(&sidecar_path).unwrap();
        let timeout_ms = launch
            .connection()
            .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
            .unwrap();
        assert!(
            timeout_ms >= 500,
            "configured busy timeout was {timeout_ms}ms"
        );
        blocker
            .connection()
            .execute_batch("BEGIN IMMEDIATE")
            .unwrap();

        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (created_tx, created_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let generation_id =
                RuntimeGenerationId::parse("91111111-1111-4111-8111-111111111111").unwrap();
            let result = launch
                .runtime_lifecycle()
                .create_runtime_generation(CreateRuntimeGeneration {
                    generation_id: &generation_id,
                    spawn_invocation_uuid: "launch-contention-invocation",
                    session_id: Some("launch-contention-session"),
                    runtime_mode: "headless",
                    provider_name: "provider-a",
                    model_name: Some("model-a"),
                    pty_control_path: None,
                    models_dir: None,
                    effective_cwd: None,
                })
                .map(|mutation| matches!(mutation, GenerationMutation::Applied(_)))
                .map_err(|error| error.to_string());
            created_tx.send((launch, result)).unwrap();
        });
        started_rx.recv().unwrap();
        assert!(
            created_rx
                .recv_timeout(StdDuration::from_millis(25))
                .is_err(),
            "runtime generation creation must wait while the sweep-side writer is held"
        );
        blocker.connection().execute_batch("ROLLBACK").unwrap();
        let (mut launch, created) = created_rx.recv_timeout(StdDuration::from_secs(1)).unwrap();
        assert_eq!(created, Ok(true));

        blocker
            .connection()
            .execute_batch("BEGIN IMMEDIATE")
            .unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (bound_tx, bound_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let identity = ProcessIdentity {
                os_pid: 901,
                os_boot_id: "launch-contention-boot".to_string(),
                os_pid_starttime_ticks: 1,
            };
            let generation_id =
                RuntimeGenerationId::parse("91111111-1111-4111-8111-111111111111").unwrap();
            let result = launch
                .runtime_lifecycle()
                .bind_runtime_generation_running(BindRuntimeGenerationRunning {
                    fence: RuntimeGenerationFence {
                        generation_id: &generation_id,
                        spawn_invocation_uuid: "launch-contention-invocation",
                    },
                    spawned_os_pid: 901,
                    exact_process_identity: &identity,
                    os_pgid: None,
                })
                .map(|mutation| matches!(mutation, GenerationMutation::Applied(_)))
                .map_err(|error| error.to_string());
            bound_tx.send(result).unwrap();
        });
        started_rx.recv().unwrap();
        assert!(
            bound_rx.recv_timeout(StdDuration::from_millis(25)).is_err(),
            "runtime generation binding must wait while the sweep-side writer is held"
        );
        blocker.connection().execute_batch("ROLLBACK").unwrap();
        assert_eq!(
            bound_rx.recv_timeout(StdDuration::from_secs(1)).unwrap(),
            Ok(true)
        );
    }

    #[test]
    fn every_writable_sidecar_constructor_enforces_foreign_keys() {
        let directory = tempfile::tempdir().unwrap();
        let sidecar_path = directory.path().join("pid-identity.db");
        let authority = MailboxAuthorityFence::acquire(&sidecar_path).unwrap();
        let mut first_admission = MailboxDb::open_with_authority(&authority).unwrap();
        assert_writable_sidecar_rejects_orphan(first_admission.connection(), "first-admission");
        first_admission
            .register_completion_event(completion_registration(
                "foreign-key-normal-event",
                "async",
                "foreign-key-session",
                "foreign-key-invocation",
            ))
            .unwrap();
        drop(first_admission);
        drop(authority);

        let authority = MailboxAuthorityFence::acquire(&sidecar_path).unwrap();
        let existing = MailboxDb::open_existing_for_completion_authority(&authority).unwrap();
        let completion_authority_timeout = existing
            .connection()
            .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
            .unwrap();
        assert!(
            completion_authority_timeout >= 5_000,
            "completion authority busy timeout was {completion_authority_timeout}ms"
        );
        assert_writable_sidecar_rejects_orphan(existing.connection(), "existing-authority");
        drop(existing);
        drop(authority);

        let authority = MailboxAuthorityFence::acquire(&sidecar_path).unwrap();
        let pid = crate::pid_identity::PidIdentityDb::open_with_authority(&authority).unwrap();
        assert_writable_sidecar_rejects_orphan(pid.connection(), "pid-identity");
        let identity = crate::pid_identity::ProcessIdentity {
            os_pid: 101,
            os_boot_id: "foreign-key-boot".to_string(),
            os_pid_starttime_ticks: 202,
        };
        pid.record_identity(crate::pid_identity::PidIdentityRecord {
            identity: &identity,
            os_pgid: Some(101),
            invocation_uuid: "foreign-key-invocation",
            session_id: Some("foreign-key-session"),
            provider_name: Some("test-provider"),
            model_name: Some("test-model"),
            recorded_at: "2026-08-14T00:00:00Z",
        })
        .unwrap();
    }

    #[test]
    fn current_sidecar_open_is_bounded_and_upgrade_backfills_once() {
        let directory = tempfile::tempdir().unwrap();
        let sidecar_path = directory.path().join("pid-identity.db");
        let mut mailbox = MailboxDb::open(&sidecar_path).unwrap();
        append_test_completion_history(&mut mailbox, 32);
        drop(mailbox);

        let connection = Connection::open(&sidecar_path).unwrap();
        connection
            .execute(
                "DELETE FROM completion_authority_materialization_summary",
                [],
            )
            .unwrap();
        connection.pragma_update(None, "user_version", 1).unwrap();
        drop(connection);

        drop(crate::pid_identity::PidIdentityDb::open(&sidecar_path).unwrap());
        assert_eq!(materialization_summary_count(&sidecar_path), 0);

        drop(MailboxDb::open(&sidecar_path).unwrap());
        let connection = Connection::open(&sidecar_path).unwrap();
        let (version, count): (i64, i64) = (
            connection
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .unwrap(),
            connection
                .query_row(
                    "SELECT materialized_count
                     FROM completion_authority_materialization_summary
                     WHERE invocation_uuid = 'mature-open-invocation'",
                    [],
                    |row| row.get(0),
                )
                .unwrap(),
        );
        assert_eq!(version, schema::CURRENT_VERSION);
        assert_eq!(count, 32);

        connection
            .execute(
                "DELETE FROM completion_authority_materialization_summary",
                [],
            )
            .unwrap();
        drop(connection);
        begin_completion_finalization_vm_count();
        drop(MailboxDb::open(&sidecar_path).unwrap());
        let current_open_steps = end_completion_finalization_vm_count();
        eprintln!("current-schema ordinary open VM steps: {current_open_steps}");
        assert_eq!(materialization_summary_count(&sidecar_path), 0);
        assert!(
            current_open_steps < 576,
            "current-schema open performed unexpected SQLite work: {current_open_steps}"
        );
    }

    #[test]
    fn v8_upgrade_adds_session_admission_scaling_indexes() {
        let directory = tempfile::tempdir().unwrap();
        let sidecar_path = directory.path().join("pid-identity.db");
        drop(MailboxDb::open(&sidecar_path).unwrap());

        let connection = Connection::open(&sidecar_path).unwrap();
        connection
            .execute_batch(
                "DROP INDEX idx_session_admission_state_runtime;
                 DROP INDEX idx_runtime_generation_lifecycle_created;
                 PRAGMA user_version = 8;",
            )
            .unwrap();
        drop(connection);

        drop(MailboxDb::open(&sidecar_path).unwrap());
        let connection = Connection::open(&sidecar_path).unwrap();
        let index_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'index'
                   AND name IN (
                     'idx_session_admission_state_runtime',
                     'idx_runtime_generation_lifecycle_created'
                   )",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(index_count, 2);
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            schema::CURRENT_VERSION
        );
    }

    #[test]
    fn v9_upgrade_adds_terminal_history_retention_schema() {
        let directory = tempfile::tempdir().unwrap();
        let sidecar_path = directory.path().join("pid-identity.db");
        drop(MailboxDb::open(&sidecar_path).unwrap());

        let connection = Connection::open(&sidecar_path).unwrap();
        connection
            .execute_batch(
                "DROP INDEX idx_mailbox_terminal_retention;
                 DROP INDEX idx_mailbox_delivery_attempt_terminal_retention;
                 DROP INDEX idx_completion_event_payload_retention;
                 DROP INDEX idx_mailbox_payload_reference;
                 DROP INDEX idx_completion_event_payload_reference;
                 ALTER TABLE completion_event DROP COLUMN payload_reclaimed_at;
                 PRAGMA user_version = 9;",
            )
            .unwrap();
        drop(connection);

        drop(MailboxDb::open(&sidecar_path).unwrap());
        let connection = Connection::open(&sidecar_path).unwrap();
        let index_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'index'
                   AND name IN (
                     'idx_mailbox_terminal_retention',
                     'idx_mailbox_delivery_attempt_terminal_retention',
                     'idx_completion_event_payload_retention'
                   )",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let columns = table_columns(
            &connection,
            "completion_event",
            &format_table_columns_pragma("completion_event"),
        )
        .unwrap();
        assert_eq!(index_count, 3);
        assert!(
            columns
                .iter()
                .any(|column| column == "payload_reclaimed_at")
        );
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            schema::CURRENT_VERSION
        );
    }

    #[test]
    fn v10_upgrade_adds_terminal_payload_lookup_indexes() {
        let directory = tempfile::tempdir().unwrap();
        let sidecar_path = directory.path().join("pid-identity.db");
        drop(MailboxDb::open(&sidecar_path).unwrap());

        let connection = Connection::open(&sidecar_path).unwrap();
        connection
            .execute_batch(
                "DROP INDEX idx_mailbox_payload_reference;
                 DROP INDEX idx_completion_event_payload_reference;
                 PRAGMA user_version = 10;",
            )
            .unwrap();
        drop(connection);

        drop(MailboxDb::open(&sidecar_path).unwrap());
        let connection = Connection::open(&sidecar_path).unwrap();
        let index_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'index'
                   AND name IN (
                     'idx_mailbox_payload_reference',
                     'idx_completion_event_payload_reference'
                   )",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(index_count, 2);
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            schema::CURRENT_VERSION
        );
    }

    #[test]
    fn v11_upgrade_adds_delivery_observation_columns() {
        let directory = tempfile::tempdir().unwrap();
        let sidecar_path = directory.path().join("pid-identity.db");
        drop(MailboxDb::open(&sidecar_path).unwrap());

        let connection = Connection::open(&sidecar_path).unwrap();
        connection
            .execute_batch(
                "ALTER TABLE mailbox_delivery_attempts DROP COLUMN observation_provider_name;
                 ALTER TABLE mailbox_delivery_attempts DROP COLUMN observation_provider_instance_id;
                 ALTER TABLE mailbox_delivery_attempts DROP COLUMN observation_settings_id;
                 ALTER TABLE mailbox_delivery_attempts DROP COLUMN observation_session_id;
                 ALTER TABLE mailbox_delivery_attempts DROP COLUMN observation_anchor_token;
                 ALTER TABLE mailbox_delivery_attempts DROP COLUMN observation_expected_sha256;
                 ALTER TABLE mailbox_delivery_attempts DROP COLUMN observation_error;
                 ALTER TABLE mailbox_delivery_attempts DROP COLUMN observation_confirmed_turn_id;
                 ALTER TABLE mailbox_delivery_attempts DROP COLUMN observation_confirmed_at;
                 PRAGMA user_version = 11;",
            )
            .unwrap();
        drop(connection);

        drop(MailboxDb::open(&sidecar_path).unwrap());
        let connection = Connection::open(&sidecar_path).unwrap();
        let columns = table_columns(
            &connection,
            "mailbox_delivery_attempts",
            &format_table_columns_pragma("mailbox_delivery_attempts"),
        )
        .unwrap();
        for column in [
            "observation_provider_name",
            "observation_provider_instance_id",
            "observation_settings_id",
            "observation_session_id",
            "observation_anchor_token",
            "observation_expected_sha256",
            "observation_error",
            "observation_confirmed_turn_id",
            "observation_confirmed_at",
        ] {
            assert!(columns.iter().any(|candidate| candidate == column));
        }
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            schema::CURRENT_VERSION
        );
    }

    fn waiting_admission_drain_vm_steps(queued: usize) -> usize {
        let directory = tempfile::tempdir().unwrap();
        let sidecar_path = directory.path().join("pid-identity.db");
        let mut mailbox = MailboxDb::open(&sidecar_path).unwrap();
        let identity = current_identity();
        for index in 0..queued {
            mailbox
                .session_admissions()
                .enqueue(
                    &format!("bounded-admission-{index}"),
                    &format!("bounded-registration-{index}"),
                    None,
                    &identity,
                    index as i64,
                )
                .unwrap();
        }

        begin_completion_finalization_vm_count();
        let result = mailbox
            .session_admissions()
            .try_admit_registration(
                &format!("bounded-registration-{}", queued - 1),
                "bounded-claim",
                i64::MAX,
                i64::MIN,
            )
            .unwrap();
        let steps = end_completion_finalization_vm_count();
        assert_eq!(result, SessionAdmissionAttempt::Waiting);
        steps
    }

    #[test]
    fn waiting_admission_drain_work_is_bounded_independently_of_queue_length() {
        let small_steps = waiting_admission_drain_vm_steps(16);
        let large_steps = waiting_admission_drain_vm_steps(1_024);
        eprintln!("waiting admission VM steps: small={small_steps}, large={large_steps}");
        assert!(
            large_steps <= small_steps + 256,
            "waiting drain grew with queue length: small={small_steps}, large={large_steps}"
        );
    }

    #[test]
    fn v2_upgrade_adds_wake_claim_process_identity() {
        let directory = tempfile::tempdir().unwrap();
        let sidecar_path = directory.path().join("pid-identity.db");
        drop(MailboxDb::open(&sidecar_path).unwrap());

        let connection = Connection::open(&sidecar_path).unwrap();
        connection
            .execute_batch(
                "DROP TABLE session_wake_claim;
                 CREATE TABLE session_wake_claim (
                    session_id                       TEXT PRIMARY KEY,
                    claim_token                      TEXT NOT NULL,
                    claimed_at                       TEXT NOT NULL,
                    wake_pid                         INTEGER,
                    wake_invocation_uuid             TEXT,
                    reason                           TEXT NOT NULL,
                    auto_wake_count                  INTEGER NOT NULL,
                    min_pending_seq_at_claim         INTEGER,
                    max_pending_seq_at_claim         INTEGER
                 );
                 CREATE INDEX idx_session_wake_claim_claimed_at
                    ON session_wake_claim(claimed_at);
                 PRAGMA user_version = 2;",
            )
            .unwrap();
        drop(connection);

        drop(MailboxDb::open(&sidecar_path).unwrap());
        let connection = Connection::open(&sidecar_path).unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            schema::CURRENT_VERSION
        );
        let columns = table_columns(
            &connection,
            "session_wake_claim",
            "PRAGMA table_info(session_wake_claim)",
        )
        .unwrap();
        assert!(columns.iter().any(|column| column == "wake_os_boot_id"));
        assert!(
            columns
                .iter()
                .any(|column| column == "wake_os_pid_starttime_ticks")
        );
    }

    #[test]
    fn v5_upgrade_adds_durable_session_admission_queue() {
        let directory = tempfile::tempdir().unwrap();
        let sidecar_path = directory.path().join("pid-identity.db");
        drop(MailboxDb::open(&sidecar_path).unwrap());

        let connection = Connection::open(&sidecar_path).unwrap();
        connection
            .execute_batch(
                "DROP TABLE session_admission_queue;
                 PRAGMA user_version = 5;",
            )
            .unwrap();
        drop(connection);

        let mut mailbox = MailboxDb::open(&sidecar_path).unwrap();
        let row = mailbox
            .session_admissions()
            .enqueue(
                "admission-id",
                "registration-id",
                None,
                &current_identity(),
                1,
            )
            .unwrap();
        assert_eq!(row.state, "queued");
        assert_eq!(row.queue_sequence, 1);
        let connection = Connection::open(&sidecar_path).unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            schema::CURRENT_VERSION
        );
    }

    #[test]
    fn v6_upgrade_requires_quiescent_admission_queue() {
        let directory = tempfile::tempdir().unwrap();
        let sidecar_path = directory.path().join("pid-identity.db");
        drop(MailboxDb::open(&sidecar_path).unwrap());

        let connection = Connection::open(&sidecar_path).unwrap();
        connection
            .execute_batch(
                "DROP TABLE session_admission_queue;
                 CREATE TABLE session_admission_queue (
                    queue_sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                    admission_id TEXT NOT NULL UNIQUE,
                    registration_identity TEXT NOT NULL UNIQUE,
                    session_id TEXT,
                    state TEXT NOT NULL CHECK(state IN ('queued', 'admitted', 'launching', 'settled')),
                    claim_token TEXT,
                    claimed_at_unix_ms INTEGER,
                    runtime_generation_uuid TEXT UNIQUE,
                    created_at_unix_ms INTEGER NOT NULL,
                    updated_at_unix_ms INTEGER NOT NULL
                 );
                 INSERT INTO session_admission_queue (
                    admission_id, registration_identity, state,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES ('legacy-admission', 'legacy-registration', 'queued', 1, 1);
                 PRAGMA user_version = 6;",
            )
            .unwrap();
        drop(connection);

        let error = match MailboxDb::open(&sidecar_path) {
            Ok(_) => panic!("non-quiescent v6 sidecar must not upgrade"),
            Err(error) => error,
        };
        assert!(error.contains("requires a quiescent runner"), "{error}");
        let connection = Connection::open(&sidecar_path).unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            6
        );
        connection
            .execute("DELETE FROM session_admission_queue", [])
            .unwrap();
        drop(connection);

        let mailbox = MailboxDb::open(&sidecar_path).unwrap();
        assert!(
            table_columns(
                &mailbox.conn,
                "session_admission_queue",
                "PRAGMA table_info(session_admission_queue)",
            )
            .unwrap()
            .iter()
            .any(|column| column == "launcher_os_pid_starttime_ticks")
        );
    }

    #[test]
    fn admission_drain_reconciles_dead_sessionless_starting_generation() {
        let directory = tempfile::tempdir().unwrap();
        let sidecar_path = directory.path().join("pid-identity.db");
        let mut mailbox = MailboxDb::open(&sidecar_path).unwrap();
        let generation_id =
            RuntimeGenerationId::parse("97777777-7777-4777-8777-777777777777").unwrap();
        mailbox
            .runtime_lifecycle()
            .create_runtime_generation(CreateRuntimeGeneration {
                generation_id: &generation_id,
                spawn_invocation_uuid: "dead-starting-invocation",
                session_id: None,
                runtime_mode: "headless",
                provider_name: "test-provider",
                model_name: None,
                pty_control_path: None,
                models_dir: None,
                effective_cwd: None,
            })
            .unwrap();
        mailbox
            .conn
            .execute(
                "UPDATE runtime_generation
                 SET creator_identity_os_pid = ?2,
                     creator_identity_os_boot_id = 'dead-boot',
                     creator_identity_os_pid_starttime_ticks = 1
                 WHERE generation_uuid = ?1",
                params![generation_id.to_string(), i64::MAX],
            )
            .unwrap();
        mailbox
            .session_admissions()
            .enqueue(
                "live-admission",
                "live-registration",
                None,
                &current_identity(),
                2,
            )
            .unwrap();

        let admitted = mailbox
            .session_admissions()
            .try_admit_next("live-claim", 0, 3)
            .unwrap();
        let SessionAdmissionAttempt::Admitted(admitted) = admitted else {
            panic!("live admission was not admitted after dead generation recovery");
        };
        assert_eq!(admitted.registration_identity, "live-registration");
        assert_eq!(
            mailbox
                .runtime_lifecycle_reader()
                .runtime_generation(&generation_id)
                .unwrap()
                .unwrap()
                .lifecycle_state,
            RuntimeLifecycleState::Exited
        );
    }

    #[test]
    fn v3_upgrade_settles_unverifiable_runtime_authorities() {
        let directory = tempfile::tempdir().unwrap();
        let sidecar_path = directory.path().join("pid-identity.db");
        drop(MailboxDb::open(&sidecar_path).unwrap());
        let connection = Connection::open(&sidecar_path).unwrap();
        connection
            .execute_batch(
                "DROP TABLE runtime_generation;
                 CREATE TABLE runtime_generation (
                    generation_uuid TEXT PRIMARY KEY,
                    lifecycle_state TEXT NOT NULL,
                    spawn_invocation_uuid TEXT NOT NULL DEFAULT 'legacy-spawn',
                    session_id TEXT,
                    identity_os_pid INTEGER,
                    identity_os_boot_id TEXT,
                    identity_os_pid_starttime_ticks INTEGER,
                    created_at TEXT NOT NULL DEFAULT '1970-01-01T00:00:00Z',
                    exited_at TEXT,
                    terminal_reason TEXT,
                    active_delivery_claim_uuid TEXT,
                    active_delivery_claimed_at TEXT,
                    active_delivery_seqs_json TEXT
                 );
                 INSERT INTO runtime_generation (generation_uuid, lifecycle_state)
                 VALUES ('legacy-starting', 'starting');
                 INSERT INTO runtime_generation (generation_uuid, lifecycle_state)
                 VALUES ('legacy-unverified-running', 'running');
                 INSERT INTO runtime_generation (
                    generation_uuid, lifecycle_state, identity_os_pid,
                    identity_os_boot_id, identity_os_pid_starttime_ticks
                 ) VALUES ('legacy-verified-running', 'running', 42, 'legacy-boot', 7);
                 PRAGMA user_version = 3;",
            )
            .unwrap();
        drop(connection);

        drop(MailboxDb::open(&sidecar_path).unwrap());
        let connection = Connection::open(&sidecar_path).unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            schema::CURRENT_VERSION
        );
        let columns = table_columns(
            &connection,
            "runtime_generation",
            "PRAGMA table_info(runtime_generation)",
        )
        .unwrap();
        for column in [
            "creator_identity_os_pid",
            "creator_identity_os_boot_id",
            "creator_identity_os_pid_starttime_ticks",
            "created_at",
        ] {
            assert!(columns.iter().any(|candidate| candidate == column));
        }
        for (generation_uuid, expected_state, expected_reason) in [
            ("legacy-starting", "exited", Some("startup_failed")),
            (
                "legacy-unverified-running",
                "exited",
                Some("abnormal_termination"),
            ),
            ("legacy-verified-running", "running", None),
        ] {
            let observed = connection
                .query_row(
                    "SELECT lifecycle_state, terminal_reason
                     FROM runtime_generation
                     WHERE generation_uuid = ?1",
                    params![generation_uuid],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
                )
                .unwrap();
            assert_eq!(
                observed,
                (
                    expected_state.to_string(),
                    expected_reason.map(str::to_string)
                )
            );
        }
    }

    #[test]
    fn v4_upgrade_preserves_acknowledged_attempt_retry_exclusion_and_evidence_obligation() {
        let directory = tempfile::tempdir().unwrap();
        let sidecar_path = directory.path().join("pid-identity.db");
        let mut mailbox = MailboxDb::open(&sidecar_path).unwrap();
        let unresolved_row = inserted_row(
            mailbox.enqueue_agent_bash_complete(&input("legacy-unresolved", "session-a")),
        );
        let resolved_row = inserted_row(
            mailbox.enqueue_agent_bash_complete(&input("legacy-resolved", "session-b")),
        );
        let unmatched_row = inserted_row(
            mailbox.enqueue_agent_bash_complete(&input("legacy-unmatched", "session-c")),
        );
        let ambiguous_row = inserted_row(
            mailbox.enqueue_agent_bash_complete(&input("legacy-ambiguous", "session-d")),
        );
        mailbox
            .register_delivery_attempt(
                "legacy-unresolved-attempt",
                "session-a",
                "legacy-unresolved-invocation",
                &[unresolved_row.seq],
                0,
            )
            .unwrap();
        mailbox
            .register_delivery_attempt(
                "legacy-resolved-attempt",
                "session-b",
                "legacy-resolved-invocation",
                &[resolved_row.seq],
                0,
            )
            .unwrap();
        for (attempt_id, session_id, invocation_id, seq) in [
            (
                "legacy-unmatched-attempt",
                "session-c",
                "legacy-unmatched-invocation",
                unmatched_row.seq,
            ),
            (
                "legacy-ambiguous-attempt",
                "session-d",
                "legacy-ambiguous-invocation",
                ambiguous_row.seq,
            ),
        ] {
            mailbox
                .register_delivery_attempt(attempt_id, session_id, invocation_id, &[seq], 0)
                .unwrap();
            mailbox
                .record_delivery_attempt_transport_ack(attempt_id)
                .unwrap();
        }
        mailbox
            .record_delivery_attempt_transport_ack("legacy-unresolved-attempt")
            .unwrap();
        mailbox
            .record_delivery_attempt_transport_ack("legacy-resolved-attempt")
            .unwrap();
        assert!(
            mailbox
                .confirm_delivery_attempt("legacy-resolved-attempt")
                .unwrap()
        );
        let unresolved_generation_id =
            RuntimeGenerationId::parse("93333333-3333-4333-8333-333333333333").unwrap();
        mailbox
            .runtime_lifecycle()
            .create_runtime_generation(CreateRuntimeGeneration {
                generation_id: &unresolved_generation_id,
                spawn_invocation_uuid: "legacy-unresolved-invocation",
                session_id: Some("session-a"),
                runtime_mode: "pty_interactive",
                provider_name: "legacy-provider",
                model_name: None,
                pty_control_path: None,
                models_dir: None,
                effective_cwd: None,
            })
            .unwrap();
        let resolved_generation_id =
            RuntimeGenerationId::parse("94444444-4444-4444-8444-444444444444").unwrap();
        mailbox
            .runtime_lifecycle()
            .create_runtime_generation(CreateRuntimeGeneration {
                generation_id: &resolved_generation_id,
                spawn_invocation_uuid: "legacy-resolved-invocation",
                session_id: Some("session-b"),
                runtime_mode: "pty_interactive",
                provider_name: "legacy-provider",
                model_name: None,
                pty_control_path: None,
                models_dir: None,
                effective_cwd: None,
            })
            .unwrap();
        for generation_uuid in [
            "95555555-5555-4555-8555-555555555555",
            "96666666-6666-4666-8666-666666666666",
        ] {
            let generation_id = RuntimeGenerationId::parse(generation_uuid).unwrap();
            mailbox
                .runtime_lifecycle()
                .create_runtime_generation(CreateRuntimeGeneration {
                    generation_id: &generation_id,
                    spawn_invocation_uuid: "legacy-ambiguous-invocation",
                    session_id: Some("session-d"),
                    runtime_mode: "pty_interactive",
                    provider_name: "legacy-provider",
                    model_name: None,
                    pty_control_path: None,
                    models_dir: None,
                    effective_cwd: None,
                })
                .unwrap();
        }
        drop(mailbox);

        let connection = Connection::open(&sidecar_path).unwrap();
        for column in [
            "evidence_reconciled_at",
            "evidence_observed_at",
            "evidence_turn_generation_id",
            "submission_started_at",
            "evidence_disposition",
        ] {
            connection
                .execute_batch(&format!(
                    "ALTER TABLE mailbox_delivery_attempts DROP COLUMN {column};"
                ))
                .unwrap();
        }
        connection.pragma_update(None, "user_version", 4).unwrap();
        drop(connection);

        let mut mailbox = MailboxDb::open(&sidecar_path).unwrap();
        let unresolved = mailbox
            .delivery_attempt_window("legacy-unresolved-attempt")
            .unwrap()
            .unwrap();
        assert!(unresolved.submission_started_at.is_some());
        assert!(unresolved.acknowledged_at.is_some());
        assert!(unresolved.resolved_at.is_none());
        let unresolved_obligations = mailbox
            .pending_delivery_evidence_obligations("session-a")
            .unwrap();
        assert_eq!(unresolved_obligations.len(), 1);
        assert_eq!(
            unresolved_obligations[0].turn_generation_id,
            unresolved_generation_id.to_string()
        );
        assert!(unresolved_obligations[0].legacy);
        assert_eq!(
            mailbox
                .accepted_delivery_attempt_windows("session-a")
                .unwrap()
                .len(),
            1
        );
        assert!(
            mailbox
                .register_or_reuse_delivery_attempt(
                    "replacement-attempt",
                    "session-a",
                    "replacement-invocation",
                    "replacement-generation",
                    &[unresolved_row.seq],
                    0,
                )
                .unwrap_err()
                .contains("legacy-unresolved-attempt")
        );
        let resolved = mailbox
            .delivery_attempt_window("legacy-resolved-attempt")
            .unwrap()
            .unwrap();
        assert!(resolved.submission_started_at.is_some());
        assert!(resolved.resolved_at.is_some());
        let obligations = mailbox
            .pending_delivery_evidence_obligations("session-b")
            .unwrap();
        assert_eq!(obligations.len(), 1);
        assert_eq!(obligations[0].attempt_id, "legacy-resolved-attempt");
        assert_eq!(obligations[0].session_id, "session-b");
        assert_eq!(
            obligations[0].turn_generation_id,
            resolved_generation_id.to_string()
        );
        assert!(obligations[0].observed_at > 0);
        assert!(obligations[0].legacy);
        let connection = Connection::open(&sidecar_path).unwrap();
        for (attempt_id, expected) in [
            ("legacy-unmatched-attempt", "legacy_unmatched_generation"),
            ("legacy-ambiguous-attempt", "legacy_ambiguous_generation"),
        ] {
            let disposition = connection
                .query_row(
                    "SELECT evidence_disposition FROM mailbox_delivery_attempts WHERE attempt_id = ?1",
                    params![attempt_id],
                    |row| row.get::<_, String>(0),
                )
                .unwrap();
            assert_eq!(disposition, expected);
            assert!(!mailbox.confirm_delivery_attempt(attempt_id).unwrap());
            let attempt = mailbox
                .delivery_attempt_window(attempt_id)
                .unwrap()
                .unwrap();
            assert!(attempt.resolved_at.is_none());
            assert!(attempt.rows[0].delivered_at.is_none());
        }
        assert!(
            mailbox
                .pending_delivery_evidence_obligations("session-c")
                .unwrap()
                .is_empty()
        );
        assert!(
            mailbox
                .pending_delivery_evidence_obligations("session-d")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn v1_upgrade_promotes_complete_legacy_runtime_authority_once() {
        let directory = tempfile::tempdir().unwrap();
        let sidecar_path = directory.path().join("pid-identity.db");
        let identity = current_identity();
        let mut mailbox = MailboxDb::open(&sidecar_path).unwrap();
        mailbox
            .wake_sessions()
            .project_legacy_runtime_running(LegacyRuntimeProjection {
                session_id: "legacy-session",
                mode: "pty_interactive",
                invocation_uuid: "legacy-invocation",
                provider_name: Some("legacy-provider"),
                model_name: Some("legacy-model"),
                identity: &identity,
                pty_control_path: Some("/tmp/legacy-control.sock"),
                turn_start_max_mailbox_seq: None,
                models_dir: Some("/tmp/legacy-models"),
                effective_cwd: Some("/tmp/legacy-work"),
            })
            .unwrap();
        mailbox
            .connection()
            .pragma_update(None, "user_version", 1)
            .unwrap();
        drop(mailbox);

        let mailbox = MailboxDb::open(&sidecar_path).unwrap();
        let SessionGenerationProjection::One(generation) = mailbox
            .runtime_lifecycle_reader()
            .session_generation_projection("legacy-session")
            .unwrap()
        else {
            panic!("legacy runtime authority was not promoted");
        };
        assert_eq!(generation.lifecycle_state, RuntimeLifecycleState::Running);
        assert_eq!(generation.spawn_invocation_uuid, "legacy-invocation");
        assert_eq!(generation.provider_name, "legacy-provider");
        assert_eq!(
            generation.pty_control_path.as_deref(),
            Some("/tmp/legacy-control.sock")
        );
        assert_eq!(
            mailbox
                .connection()
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            schema::CURRENT_VERSION
        );
        drop(mailbox);

        let mailbox = MailboxDb::open(&sidecar_path).unwrap();
        let SessionGenerationProjection::One(_) = mailbox
            .runtime_lifecycle_reader()
            .session_generation_projection("legacy-session")
            .unwrap()
        else {
            panic!("promoted runtime authority changed on current-schema reopen");
        };
    }

    #[test]
    fn v1_upgrade_does_not_revive_an_exited_runtime_generation() {
        let directory = tempfile::tempdir().unwrap();
        let sidecar_path = directory.path().join("pid-identity.db");
        let identity = current_identity();
        let generation_id =
            RuntimeGenerationId::parse("91111111-1111-4111-8111-111111111111").unwrap();
        let fence = RuntimeGenerationFence {
            generation_id: &generation_id,
            spawn_invocation_uuid: "legacy-invocation",
        };
        let mut mailbox = MailboxDb::open(&sidecar_path).unwrap();
        mailbox
            .runtime_lifecycle()
            .create_runtime_generation(CreateRuntimeGeneration {
                generation_id: &generation_id,
                spawn_invocation_uuid: "legacy-invocation",
                session_id: Some("legacy-session"),
                runtime_mode: "headless",
                provider_name: "legacy-provider",
                model_name: None,
                pty_control_path: None,
                models_dir: None,
                effective_cwd: None,
            })
            .unwrap();
        mailbox
            .runtime_lifecycle()
            .bind_runtime_generation_running(BindRuntimeGenerationRunning {
                fence,
                spawned_os_pid: identity.os_pid,
                exact_process_identity: &identity,
                os_pgid: None,
            })
            .unwrap();
        mailbox
            .runtime_lifecycle()
            .exit_runtime_generation_non_orderly(ExitRuntimeGenerationNonOrderly {
                fence,
                reason: RuntimeTerminalReason::AbnormalTermination,
                exit_code: Some(17),
            })
            .unwrap();
        mailbox
            .connection()
            .execute(
                "UPDATE session_runtime
                 SET run_state = 'running',
                     running_invocation_uuid = 'legacy-invocation',
                     running_os_pid = ?2,
                     running_os_boot_id = ?3,
                     running_os_pid_starttime_ticks = ?4,
                     turn_started_at = updated_at,
                     turn_ended_at = NULL,
                     last_exit_code = NULL
                 WHERE session_id = ?1",
                params![
                    "legacy-session",
                    identity.os_pid,
                    &identity.os_boot_id,
                    identity.os_pid_starttime_ticks,
                ],
            )
            .unwrap();
        mailbox
            .connection()
            .pragma_update(None, "user_version", 1)
            .unwrap();
        drop(mailbox);

        let mailbox = MailboxDb::open(&sidecar_path).unwrap();
        let generation = mailbox
            .runtime_lifecycle_reader()
            .runtime_generation(&generation_id)
            .unwrap()
            .expect("exited runtime generation was not retained");
        assert_eq!(generation.lifecycle_state, RuntimeLifecycleState::Exited);
        let projection = mailbox
            .wake_session_reader()
            .legacy_runtime_projection("legacy-session")
            .unwrap()
            .unwrap();
        assert_eq!(projection.run_state, "idle");
        assert!(projection.running_invocation_uuid.is_none());
        assert_eq!(projection.last_exit_code, Some(17));
    }

    #[test]
    fn mature_history_does_not_increase_ordinary_mailbox_open_work() {
        let directory = tempfile::tempdir().unwrap();
        let sidecar_path = directory.path().join("pid-identity.db");
        let mut mailbox = MailboxDb::open(&sidecar_path).unwrap();
        append_test_completion_history(&mut mailbox, 1);
        drop(mailbox);

        begin_completion_finalization_vm_count();
        let mut mailbox = MailboxDb::open(&sidecar_path).unwrap();
        let small_open_steps = end_completion_finalization_vm_count();
        append_test_completion_history(&mut mailbox, 255);
        drop(mailbox);

        begin_completion_finalization_vm_count();
        drop(MailboxDb::open(&sidecar_path).unwrap());
        let mature_open_steps = end_completion_finalization_vm_count();
        eprintln!("ordinary open VM steps: small={small_open_steps}, mature={mature_open_steps}");

        assert!(
            mature_open_steps <= small_open_steps + 64,
            "ordinary sidecar open grew with retained history: small={small_open_steps}, mature={mature_open_steps}"
        );
    }

    fn append_test_completion_history(mailbox: &mut MailboxDb, additional: usize) {
        let generation = mailbox.sidecar_generation().unwrap();
        let retained_head = completion_continuity_head_on(mailbox.connection()).unwrap();
        let mut ordinal = retained_head
            .as_ref()
            .map_or(1, |head| head.authority_ordinal + 1);
        let mut previous_digest = retained_head.map_or_else(
            || COMPLETION_CONTINUITY_GENESIS_DIGEST.to_string(),
            |head| head.continuity_digest,
        );
        for _ in 0..additional {
            let event_id = format!("mature-open-event-{ordinal}");
            mailbox
                .register_completion_event(completion_registration(
                    &event_id,
                    "async",
                    "mature-open-session",
                    "mature-open-owner",
                ))
                .unwrap();
            let digest = sha256_hex(format!("mature-open-digest-{ordinal}").as_bytes());
            append_completion_continuity_on(
                mailbox.connection(),
                &CompletionContinuityHead {
                    authority_ordinal: ordinal,
                    admission_id: format!("mature-open-admission-{ordinal}"),
                    sidecar_generation: generation.clone(),
                    invocation_uuid: "mature-open-invocation".to_string(),
                    event_id,
                    owner_invocation_uuid: "mature-open-owner".to_string(),
                    owner_session_id: "mature-open-session".to_string(),
                    previous_continuity_digest: previous_digest,
                    continuity_digest: digest.clone(),
                },
            )
            .unwrap();
            previous_digest = digest;
            ordinal += 1;
        }
    }

    fn materialization_summary_count(path: &Path) -> i64 {
        Connection::open(path)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM completion_authority_materialization_summary",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[test]
    fn idle_writable_handles_exclude_rebuild_until_release_and_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        drop(StateDb::open(&state_path).unwrap());
        let sidecar_path = MailboxDb::path_for_state_db(&state_path);
        let mut mailbox = MailboxDb::open(&sidecar_path).unwrap();
        let generation = mailbox.sidecar_generation().unwrap();
        let state_authority = StateDb::acquire_rebuild_authority(&state_path).unwrap();

        let error = MailboxDb::acquire_rebuild_authority(&state_authority)
            .err()
            .expect("idle writable sidecar handle must exclude rebuild");
        assert!(error.contains("completion_authority_contention"), "{error}");
        assert_eq!(mailbox.sidecar_generation().unwrap(), generation);
        mailbox
            .wake_sessions()
            .upsert_session_metadata(SessionMetadataUpsert {
                session_id: "idle-handle-session",
                mode: "headless",
                invocation_uuid: Some("idle-handle-invocation"),
                provider_name: None,
                model_name: None,
                models_dir: None,
                effective_cwd: None,
            })
            .unwrap();
        drop(mailbox);

        let mut rebuild = MailboxDb::acquire_rebuild_authority(&state_authority).unwrap();
        rebuild.reset().unwrap();
        rebuild.initialize_after_rebuild().unwrap();
        drop(rebuild);
        drop(state_authority);

        let reopened = MailboxDb::open(&sidecar_path).unwrap();
        assert_ne!(reopened.sidecar_generation().unwrap(), generation);
        assert!(
            reopened
                .wake_session_reader()
                .session_metadata("idle-handle-session")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn idle_pid_identity_handle_excludes_rebuild_until_release_and_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        drop(StateDb::open(&state_path).unwrap());
        let sidecar_path = MailboxDb::path_for_state_db(&state_path);
        let pid = crate::pid_identity::PidIdentityDb::open(&sidecar_path).unwrap();
        let identity = crate::pid_identity::ProcessIdentity {
            os_pid: 301,
            os_boot_id: "idle-pid-handle-boot".to_string(),
            os_pid_starttime_ticks: 401,
        };
        let state_authority = StateDb::acquire_rebuild_authority(&state_path).unwrap();

        let error = MailboxDb::acquire_rebuild_authority(&state_authority)
            .err()
            .expect("idle PID identity handle must exclude rebuild");
        assert!(error.contains("completion_authority_contention"), "{error}");
        pid.record_identity(crate::pid_identity::PidIdentityRecord {
            identity: &identity,
            os_pgid: Some(301),
            invocation_uuid: "idle-pid-handle-invocation",
            session_id: Some("idle-pid-handle-session"),
            provider_name: Some("fixture-provider"),
            model_name: Some("fixture-model"),
            recorded_at: "2026-08-15T00:00:00Z",
        })
        .unwrap();
        drop(pid);

        let mut rebuild = MailboxDb::acquire_rebuild_authority(&state_authority).unwrap();
        rebuild.reset().unwrap();
        rebuild.initialize_after_rebuild().unwrap();
        drop(rebuild);
        drop(state_authority);

        let reopened = crate::pid_identity::PidIdentityDb::open(&sidecar_path).unwrap();
        assert!(reopened.lookup_by_identity(&identity).unwrap().is_none());
    }

    #[test]
    fn rebuild_writer_error_classification_allows_only_explicit_corruption() {
        let corrupt = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ffi::ErrorCode::NotADatabase,
                extended_code: 0,
            },
            Some("not a database".to_string()),
        );
        let unavailable = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ffi::ErrorCode::SystemIoFailure,
                extended_code: 0,
            },
            Some("I/O unavailable".to_string()),
        );

        assert!(sqlite_error_is_corrupt(&corrupt));
        assert!(!sqlite_error_is_corrupt(&unavailable));
    }

    fn assert_writable_sidecar_rejects_orphan(conn: &Connection, listener_id: &str) {
        let foreign_keys = conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))
            .unwrap();
        assert_eq!(foreign_keys, 1);
        let error = conn
            .execute(
                "INSERT INTO completion_event_listener (
                    event_id, listener_id, session_id, owner_invocation_uuid, active, created_at
                 ) VALUES ('missing-event', ?1, 'session', 'invocation', 0, 'now')",
                params![listener_id],
            )
            .unwrap_err();
        assert!(
            error.to_string().contains("FOREIGN KEY constraint failed"),
            "{error}"
        );
    }

    #[test]
    fn sidecar_writers_reject_a_state_database_storage_role() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        let state = StateDb::open(&state_path).unwrap();

        for error in [
            MailboxDb::open(&state_path).err().unwrap(),
            crate::pid_identity::PidIdentityDb::open(&state_path)
                .err()
                .unwrap(),
        ] {
            assert!(error.contains("reserved sidecar storage role"), "{error}");
        }
        assert!(!directory.path().join("state.db.authority.lock").exists());
        assert_eq!(state.path(), state_path);
    }

    #[cfg(unix)]
    #[test]
    fn sidecar_writers_reject_hard_links_to_state_and_control_storage() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        let state = StateDb::open(&state_path).unwrap();
        let sidecar_path = MailboxDb::path_for_state_db(&state_path);
        drop(MailboxDb::open(&sidecar_path).unwrap());
        let journal_path = path_with_suffix(&state_path, "-journal");
        fs::write(&journal_path, b"journal-role").unwrap();

        for (index, target) in [
            state_path.clone(),
            path_with_suffix(&state_path, ".namespace.lock"),
            journal_path,
            path_with_suffix(&state_path, "-wal"),
            path_with_suffix(&state_path, "-shm"),
            sidecar_path.clone(),
            path_with_suffix(&sidecar_path, ".authority.lock"),
        ]
        .into_iter()
        .enumerate()
        {
            assert!(target.exists(), "missing storage role {}", target.display());
            for kind in ["mailbox", "pid"] {
                let alias = directory
                    .path()
                    .join(format!("hard-link-{index}-{kind}.pid-identity.db"));
                fs::hard_link(&target, &alias).unwrap();
                let error = if kind == "mailbox" {
                    MailboxDb::open(&alias).err().unwrap()
                } else {
                    crate::pid_identity::PidIdentityDb::open(&alias)
                        .err()
                        .unwrap()
                };
                assert!(error.contains("exactly one hard link"), "{error}");
                assert!(!path_with_suffix(&alias, ".authority.lock").exists());
                fs::remove_file(alias).unwrap();
            }
        }
        drop(state);
    }

    #[cfg(unix)]
    #[test]
    fn sidecar_writers_reject_aliased_authority_fences_and_sqlite_artifacts() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        drop(StateDb::open(&state_path).unwrap());
        let state_before = fs::read(&state_path).unwrap();
        let sidecar_path = directory.path().join("pid-identity.db");
        drop(MailboxDb::open(&sidecar_path).unwrap());
        let authority_path = path_with_suffix(&sidecar_path, ".authority.lock");

        fs::remove_file(&authority_path).unwrap();
        symlink(&state_path, &authority_path).unwrap();
        for error in [
            MailboxDb::open(&sidecar_path).err().unwrap(),
            crate::pid_identity::PidIdentityDb::open(&sidecar_path)
                .err()
                .unwrap(),
        ] {
            assert!(error.contains("exactly one hard link"), "{error}");
        }
        assert_eq!(fs::read(&state_path).unwrap(), state_before);

        fs::remove_file(&authority_path).unwrap();
        drop(MailboxDb::open(&sidecar_path).unwrap());
        for suffix in ["-journal", "-wal", "-shm"] {
            let artifact = path_with_suffix(&sidecar_path, suffix);
            if artifact.exists() {
                fs::remove_file(&artifact).unwrap();
            }
            fs::hard_link(&state_path, &artifact).unwrap();
            for error in [
                MailboxDb::open(&sidecar_path).err().unwrap(),
                crate::pid_identity::PidIdentityDb::open(&sidecar_path)
                    .err()
                    .unwrap(),
            ] {
                assert!(error.contains("exactly one hard link"), "{error}");
            }
            fs::remove_file(artifact).unwrap();
        }
        assert_eq!(fs::read(&state_path).unwrap(), state_before);
    }

    #[cfg(unix)]
    #[test]
    fn sidecar_authority_retains_its_target_across_parent_retargeting() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let first_parent = directory.path().join("first");
        let second_parent = directory.path().join("second");
        let alias_parent = directory.path().join("current");
        fs::create_dir(&first_parent).unwrap();
        fs::create_dir(&second_parent).unwrap();
        let first_path = first_parent.join("pid-identity.db");
        let second_path = second_parent.join("pid-identity.db");
        drop(MailboxDb::open(&first_path).unwrap());
        drop(MailboxDb::open(&second_path).unwrap());
        symlink(&first_parent, &alias_parent).unwrap();
        let alias_path = alias_parent.join("pid-identity.db");

        let first_authority = MailboxAuthorityFence::acquire(&alias_path).unwrap();
        fs::remove_file(&alias_parent).unwrap();
        symlink(&second_parent, &alias_parent).unwrap();
        let first = MailboxDb::open_with_authority(&first_authority).unwrap();
        let second_authority = MailboxAuthorityFence::acquire(&alias_path).unwrap();
        let second =
            crate::pid_identity::PidIdentityDb::open_with_authority(&second_authority).unwrap();

        assert_eq!(first.path(), first_path.canonicalize().unwrap());
        assert_eq!(second.path(), second_path.canonicalize().unwrap());
    }

    fn path_with_suffix(path: &Path, suffix: &str) -> PathBuf {
        let mut value = path.as_os_str().to_os_string();
        value.push(suffix);
        PathBuf::from(value)
    }

    fn input<'a>(handle: &'a str, session_id: &'a str) -> AgentBashCompleteEnqueue<'a> {
        AgentBashCompleteEnqueue {
            session_id,
            handle,
            payload_json: r#"{"schema_version":1}"#,
            owner_invocation_uuid: Some("11111111-1111-4111-8111-111111111111"),
            matched_os_pid: Some(4242),
            matched_os_boot_id: Some("boot-a"),
            matched_os_pid_starttime_ticks: Some(99),
            matched_chain_index: Some(0),
            state_dir: "/tmp/state",
            meta_path: "/tmp/state/meta.json",
            log_path: "/tmp/state/log",
            rc_path: "/tmp/state/rc",
            rc: 0,
        }
    }

    fn completion_registration<'a>(
        event_id: &'a str,
        delivery_mode: &'a str,
        session_id: &'a str,
        invocation_uuid: &'a str,
    ) -> CompletionEventRegistrationInput<'a> {
        CompletionEventRegistrationInput {
            event_id,
            delivery_mode,
            owner_session_id: Some(session_id),
            owner_invocation_uuid: Some(invocation_uuid),
            state_dir: "/tmp/state",
            meta_path: "/tmp/state/meta.json",
            log_path: "/tmp/state/log",
            rc_path: "/tmp/state/rc",
        }
    }

    fn completion_trigger<'a>(
        event_id: &'a str,
        payload_json: &'a str,
        consumed: bool,
    ) -> CompletionEventTriggerInput<'a> {
        CompletionEventTriggerInput {
            event_id,
            payload_json,
            state_dir: "/tmp/state",
            meta_path: "/tmp/state/meta.json",
            log_path: "/tmp/state/log",
            rc_path: "/tmp/state/rc",
            rc: 0,
            consumed,
        }
    }

    fn submitted_input<'a>(
        submission_token: &'a str,
        target_kind: InboxTargetKind,
        target_id: &'a str,
        payload: &'a [u8],
    ) -> SubmittedInputEnqueue<'a> {
        SubmittedInputEnqueue {
            submission_token,
            target: InboxTarget {
                kind: target_kind,
                id: target_id,
            },
            input: payload,
        }
    }

    #[test]
    fn enqueue_transaction_rollback_has_no_partial_row() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let input = input("handle-a", "session-a");
        let payload = db
            .payloads()
            .payload_reference(input.payload_json.as_bytes())
            .unwrap();

        let err = db
            .enqueue_agent_bash_complete_then_rollback(&input)
            .unwrap_err();

        assert_eq!(err, "forced rollback before commit");
        assert!(db.list_mailbox("session-a", true).unwrap().is_empty());
        assert!(payload.file_path.exists());
        db.payloads().verify_published_payload(&payload).unwrap();
    }

    #[test]
    fn immutable_payload_publication_is_content_addressed_and_detects_tampering() {
        let dir = tempfile::tempdir().unwrap();
        let db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let first = db
            .payloads()
            .publish_immutable_payload(b"payload-a")
            .unwrap();
        let second = db
            .payloads()
            .publish_immutable_payload(b"payload-a")
            .unwrap();

        assert_eq!(first, second);
        assert_eq!(fs::read(&first.file_path).unwrap(), b"payload-a");
        let mut permissions = fs::metadata(&first.file_path).unwrap().permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(0o600);
        }
        #[cfg(not(unix))]
        permissions.set_readonly(false);
        fs::set_permissions(&first.file_path, permissions).unwrap();
        fs::write(&first.file_path, b"payload-b").unwrap();
        let mut permissions = fs::metadata(&first.file_path).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&first.file_path, permissions).unwrap();
        assert!(
            fs::metadata(&first.file_path)
                .unwrap()
                .permissions()
                .readonly()
        );

        let error = db.payloads().verify_published_payload(&first).unwrap_err();
        assert!(
            error.contains("Mailbox payload integrity mismatch"),
            "{error}"
        );
    }

    #[test]
    fn completion_event_trigger_is_idempotent_and_acknowledges_each_listener() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let event_id = "ab_event";
        let first_invocation = "11111111-1111-4111-8111-111111111111";
        let second_invocation = "22222222-2222-4222-8222-222222222222";

        let first = db
            .register_completion_event(completion_registration(
                event_id,
                "async",
                "session-a",
                first_invocation,
            ))
            .unwrap();
        assert!(first.inserted);
        let replay = db
            .register_completion_event(completion_registration(
                event_id,
                "async",
                "session-b",
                second_invocation,
            ))
            .unwrap();
        assert!(!replay.inserted);
        assert_eq!(replay.listeners.len(), 2);

        let triggered = db
            .trigger_completion_event(completion_trigger(
                event_id,
                r#"{"schema_version":2,"handle":"ab_event"}"#,
                false,
            ))
            .unwrap();
        assert!(triggered.triggered);
        assert_eq!(triggered.event.state, "triggered");
        assert_eq!(triggered.mailbox_rows.len(), 2);
        assert_ne!(
            triggered.mailbox_rows[0].handle,
            triggered.mailbox_rows[1].handle
        );

        let duplicate = db
            .trigger_completion_event(completion_trigger(
                event_id,
                r#"{"schema_version":2,"handle":"ab_event"}"#,
                false,
            ))
            .unwrap();
        assert!(!duplicate.triggered);
        assert_eq!(
            duplicate
                .mailbox_rows
                .iter()
                .map(|row| row.seq)
                .collect::<Vec<_>>(),
            triggered
                .mailbox_rows
                .iter()
                .map(|row| row.seq)
                .collect::<Vec<_>>()
        );
        assert!(
            db.trigger_completion_event(completion_trigger(
                event_id,
                r#"{"schema_version":2,"handle":"different"}"#,
                false,
            ))
            .is_err()
        );

        let session_a_seq = triggered
            .mailbox_rows
            .iter()
            .find(|row| row.session_id == "session-a")
            .unwrap()
            .seq;
        db.mark_delivered("session-a", None, &[session_a_seq], "delivery-a")
            .unwrap();
        let listeners = db.completion_event_listeners(event_id).unwrap();
        let first = listeners
            .iter()
            .find(|listener| listener.session_id == "session-a")
            .unwrap();
        let second = listeners
            .iter()
            .find(|listener| listener.session_id == "session-b")
            .unwrap();
        assert!(first.acknowledged_at.is_some());
        assert_eq!(first.acknowledgement_reason.as_deref(), Some("injected"));
        assert!(second.acknowledged_at.is_none());
        let late_listener = db
            .register_completion_event(completion_registration(
                event_id,
                "async",
                "session-c",
                "33333333-3333-4333-8333-333333333333",
            ))
            .unwrap_err();
        assert!(late_listener.contains("after it was triggered"));
    }

    #[test]
    fn completion_event_rejects_registration_and_trigger_identity_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let event_id = "ab_identity";
        db.register_completion_event(completion_registration(
            event_id,
            "async",
            "session-a",
            "11111111-1111-4111-8111-111111111111",
        ))
        .unwrap();

        let registration_error = db
            .register_completion_event(CompletionEventRegistrationInput {
                delivery_mode: "sync",
                ..completion_registration(
                    event_id,
                    "async",
                    "session-a",
                    "11111111-1111-4111-8111-111111111111",
                )
            })
            .unwrap_err();
        assert!(registration_error.contains("conflicts with its durable identity"));

        let trigger_error = db
            .trigger_completion_event(CompletionEventTriggerInput {
                state_dir: "/tmp/different-state",
                ..completion_trigger(
                    event_id,
                    r#"{"schema_version":2,"handle":"ab_identity"}"#,
                    false,
                )
            })
            .unwrap_err();
        assert!(trigger_error.contains("does not match its registered source"));
        assert_eq!(
            db.completion_event(event_id).unwrap().unwrap().state,
            "pending"
        );
        assert!(db.list_mailbox("session-a", true).unwrap().is_empty());
    }

    #[test]
    fn completion_event_registration_requires_an_owner() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let event_id = "ab_ownerless";

        let error = db
            .register_completion_event(CompletionEventRegistrationInput {
                owner_session_id: None,
                owner_invocation_uuid: None,
                ..completion_registration(
                    event_id,
                    "async",
                    "session-a",
                    "11111111-1111-4111-8111-111111111111",
                )
            })
            .unwrap_err();

        assert!(error.contains("both required"));
        assert!(db.completion_event(event_id).unwrap().is_none());
    }

    #[test]
    fn sync_completion_waits_for_activation_before_materializing_listener() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let event_id = "ab_sync";
        db.register_completion_event(completion_registration(
            event_id,
            "sync",
            "session-a",
            "11111111-1111-4111-8111-111111111111",
        ))
        .unwrap();

        let triggered = db
            .trigger_completion_event(completion_trigger(
                event_id,
                r#"{"schema_version":2,"handle":"ab_sync"}"#,
                false,
            ))
            .unwrap();
        assert!(triggered.triggered);
        assert!(triggered.mailbox_rows.is_empty());
        assert!(!triggered.listeners[0].active);

        let activated = db.activate_completion_event_listeners(event_id).unwrap();
        assert_eq!(activated.mailbox_rows.len(), 1);
        assert!(activated.listeners[0].active);
    }

    #[test]
    fn consumed_completion_acknowledges_listener_without_mailbox_delivery() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let event_id = "ab_consumed";
        db.register_completion_event(completion_registration(
            event_id,
            "async",
            "session-a",
            "11111111-1111-4111-8111-111111111111",
        ))
        .unwrap();

        let triggered = db
            .trigger_completion_event(completion_trigger(
                event_id,
                r#"{"schema_version":2,"handle":"ab_consumed"}"#,
                true,
            ))
            .unwrap();
        assert!(triggered.triggered);
        assert!(triggered.mailbox_rows.is_empty());
        assert!(!triggered.listeners[0].active);
        assert!(triggered.listeners[0].acknowledged_at.is_some());
        assert_eq!(
            triggered.listeners[0].acknowledgement_reason.as_deref(),
            Some("consumed_in_call")
        );
    }

    #[test]
    fn consumed_completion_resolves_an_existing_delivery_attempt() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let event_id = "ab_consumed_replay";
        let invocation_uuid = "11111111-1111-4111-8111-111111111111";
        let payload = r#"{"schema_version":2,"handle":"ab_consumed_replay"}"#;
        db.register_completion_event(completion_registration(
            event_id,
            "async",
            "session-a",
            invocation_uuid,
        ))
        .unwrap();
        let triggered = db
            .trigger_completion_event(completion_trigger(event_id, payload, false))
            .unwrap();
        let seq = triggered.mailbox_rows[0].seq;
        db.register_delivery_attempt("attempt-consumed", "session-a", invocation_uuid, &[seq], 0)
            .unwrap();
        assert!(
            db.record_delivery_attempt_transport_ack("attempt-consumed")
                .unwrap()
        );
        db.conn
            .execute(
                "UPDATE mailbox SET delivery_attempts = 2, delivery_error = 'transport_error'
                 WHERE seq = ?1",
                params![seq],
            )
            .unwrap();

        let consumed = db
            .trigger_completion_event(completion_trigger(event_id, payload, true))
            .unwrap();

        let row = &consumed.mailbox_rows[0];
        assert!(row.delivered_at.is_some());
        assert_eq!(row.delivery_attempts, 3);
        assert_eq!(row.delivery_error, None);
        assert_eq!(
            db.delivery_attempt_disposition("attempt-consumed").unwrap(),
            Some(MailboxDeliveryAttemptDisposition::Resolved)
        );
        assert_eq!(
            consumed.listeners[0].acknowledgement_reason.as_deref(),
            Some("consumed_in_call")
        );
    }

    struct LateConsumedCompletionFixture {
        _dir: tempfile::TempDir,
        db: MailboxDb,
        event_id: &'static str,
        unrelated_event_id: &'static str,
        invocation_uuid: &'static str,
        sibling_invocation_uuid: &'static str,
        seq: i64,
        sibling_seq: i64,
    }

    const LATE_CONSUMED_EVENT_ID: &str = "ab_late_consumed";
    const UNRELATED_PENDING_EVENT_ID: &str = "ab_unrelated_pending";
    const UNRELATED_COMPLETED_EVENT_ID: &str = "ab_unrelated_completed";
    const LATE_CONSUMED_INVOCATION_UUID: &str = "11111111-1111-4111-8111-111111111111";
    const LATE_CONSUMED_SIBLING_INVOCATION_UUID: &str = "22222222-2222-4222-8222-222222222222";

    fn late_consumed_completion_fixture() -> LateConsumedCompletionFixture {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let (seq, sibling_seq) = seed_late_consumed_listeners(&mut db);
        seed_unrelated_pending_completion(&mut db, seq);
        seed_unrelated_completed_attempt(&mut db);
        map_late_consumed_completion_fixture(dir, db, seq, sibling_seq)
    }

    fn seed_late_consumed_listeners(db: &mut MailboxDb) -> (i64, i64) {
        db.register_completion_event(completion_registration(
            LATE_CONSUMED_EVENT_ID,
            "async",
            "session-a",
            LATE_CONSUMED_INVOCATION_UUID,
        ))
        .unwrap();
        db.register_completion_event(completion_registration(
            LATE_CONSUMED_EVENT_ID,
            "async",
            "session-b",
            LATE_CONSUMED_SIBLING_INVOCATION_UUID,
        ))
        .unwrap();
        let triggered = db
            .trigger_completion_event(completion_trigger(
                LATE_CONSUMED_EVENT_ID,
                r#"{"schema_version":2,"handle":"ab_late_consumed"}"#,
                false,
            ))
            .unwrap();
        (
            triggered_mailbox_seq(&triggered, "session-a"),
            triggered_mailbox_seq(&triggered, "session-b"),
        )
    }

    fn triggered_mailbox_seq(triggered: &CompletionEventTriggerResult, session_id: &str) -> i64 {
        triggered
            .mailbox_rows
            .iter()
            .find(|row| row.session_id == session_id)
            .unwrap()
            .seq
    }

    fn seed_unrelated_pending_completion(db: &mut MailboxDb, consumed_seq: i64) {
        db.register_completion_event(completion_registration(
            UNRELATED_PENDING_EVENT_ID,
            "async",
            "session-a",
            LATE_CONSUMED_INVOCATION_UUID,
        ))
        .unwrap();
        let unrelated = db
            .trigger_completion_event(completion_trigger(
                UNRELATED_PENDING_EVENT_ID,
                r#"{"schema_version":2,"handle":"ab_unrelated_pending"}"#,
                false,
            ))
            .unwrap();
        let unrelated_seq = unrelated.mailbox_rows[0].seq;
        db.register_delivery_attempt(
            "attempt-late-consumed",
            "session-a",
            LATE_CONSUMED_INVOCATION_UUID,
            &[consumed_seq],
            0,
        )
        .unwrap();
        db.register_delivery_attempt(
            "attempt-unrelated",
            "session-a",
            LATE_CONSUMED_INVOCATION_UUID,
            &[unrelated_seq],
            0,
        )
        .unwrap();
    }

    fn seed_unrelated_completed_attempt(db: &mut MailboxDb) {
        db.register_completion_event(completion_registration(
            UNRELATED_COMPLETED_EVENT_ID,
            "async",
            "session-a",
            LATE_CONSUMED_INVOCATION_UUID,
        ))
        .unwrap();
        let unrelated_completed = db
            .trigger_completion_event(completion_trigger(
                UNRELATED_COMPLETED_EVENT_ID,
                r#"{"schema_version":2,"handle":"ab_unrelated_completed"}"#,
                false,
            ))
            .unwrap();
        let unrelated_completed_seq = unrelated_completed.mailbox_rows[0].seq;
        db.register_delivery_attempt(
            "attempt-unrelated-completed",
            "session-a",
            LATE_CONSUMED_INVOCATION_UUID,
            &[unrelated_completed_seq],
            0,
        )
        .unwrap();
        db.conn
            .execute(
                "UPDATE mailbox SET delivered_at = '2026-08-10T00:00:00Z' WHERE seq = ?1",
                params![unrelated_completed_seq],
            )
            .unwrap();
    }

    fn map_late_consumed_completion_fixture(
        dir: tempfile::TempDir,
        db: MailboxDb,
        seq: i64,
        sibling_seq: i64,
    ) -> LateConsumedCompletionFixture {
        LateConsumedCompletionFixture {
            _dir: dir,
            db,
            event_id: LATE_CONSUMED_EVENT_ID,
            unrelated_event_id: UNRELATED_PENDING_EVENT_ID,
            invocation_uuid: LATE_CONSUMED_INVOCATION_UUID,
            sibling_invocation_uuid: LATE_CONSUMED_SIBLING_INVOCATION_UUID,
            seq,
            sibling_seq,
        }
    }

    #[test]
    fn late_consumed_completion_acknowledges_materialized_mailbox_row_once() {
        let fixture = late_consumed_completion_fixture();
        let event_id = fixture.event_id;
        let unrelated_event_id = fixture.unrelated_event_id;
        let invocation_uuid = fixture.invocation_uuid;
        let sibling_invocation_uuid = fixture.sibling_invocation_uuid;
        let seq = fixture.seq;
        let sibling_seq = fixture.sibling_seq;
        let mut db = fixture.db;

        assert_eq!(
            db.acknowledge_consumed_completion_event_for_mailbox_seq(
                sibling_seq,
                "session-a",
                invocation_uuid,
            )
            .unwrap(),
            None
        );
        assert_eq!(
            db.acknowledge_consumed_completion_event_for_mailbox_seq(
                seq,
                "session-a",
                invocation_uuid,
            )
            .unwrap()
            .as_deref(),
            Some(event_id)
        );
        let first_acknowledged_at = db
            .completion_event_listeners(event_id)
            .unwrap()
            .into_iter()
            .find(completion_listener_for_session_a)
            .unwrap()
            .acknowledged_at
            .unwrap();
        assert_eq!(
            db.acknowledge_consumed_completion_event_for_mailbox_seq(
                seq,
                "session-a",
                invocation_uuid,
            )
            .unwrap(),
            None
        );
        assert_eq!(
            db.acknowledge_consumed_completion_event_for_mailbox_seq(
                sibling_seq,
                "session-a",
                invocation_uuid,
            )
            .unwrap(),
            None
        );

        let rows = db.list_mailbox("session-a", true).unwrap();
        assert_eq!(rows.len(), 3);
        let consumed_row = mailbox_row_for_seq(&rows, seq);
        assert!(consumed_row.delivered_at.is_some());
        assert_eq!(consumed_row.delivery_attempts, 1);
        assert_eq!(
            consumed_row.delivered_by_invocation_uuid.as_deref(),
            Some(invocation_uuid)
        );
        assert_eq!(
            db.delivery_attempt_disposition("attempt-late-consumed")
                .unwrap(),
            Some(MailboxDeliveryAttemptDisposition::Resolved)
        );
        assert!(
            db.delivery_attempt_window("attempt-late-consumed")
                .unwrap()
                .unwrap()
                .resolved_at
                .is_some()
        );
        assert_eq!(
            db.delivery_attempt_disposition("attempt-unrelated")
                .unwrap(),
            Some(MailboxDeliveryAttemptDisposition::Pending)
        );
        assert!(
            db.delivery_attempt_window("attempt-unrelated-completed")
                .unwrap()
                .unwrap()
                .resolved_at
                .is_none()
        );
        let pending = db.list_pending("session-a").unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].handle, unrelated_event_id);
        let listeners = db.completion_event_listeners(event_id).unwrap();
        let consumed_listener = completion_listener_for_session(&listeners, "session-a");
        assert_eq!(
            consumed_listener.acknowledgement_reason.as_deref(),
            Some("consumed_in_call")
        );
        assert!(!consumed_listener.active);
        assert_eq!(
            consumed_listener.acknowledged_at.as_deref(),
            Some(first_acknowledged_at.as_str())
        );
        let sibling_listener = completion_listener_for_session(&listeners, "session-b");
        assert!(sibling_listener.active);
        assert!(sibling_listener.acknowledged_at.is_none());
        let sibling_pending = db.list_pending("session-b").unwrap();
        assert_eq!(sibling_pending.len(), 1);
        assert_eq!(
            sibling_pending[0].owner_invocation_uuid.as_deref(),
            Some(sibling_invocation_uuid)
        );
        let unrelated = db.completion_event_listeners(unrelated_event_id).unwrap();
        assert!(unrelated[0].active);
        assert!(unrelated[0].acknowledged_at.is_none());
    }

    fn completion_listener_for_session_a(listener: &CompletionEventListenerRow) -> bool {
        listener.session_id == "session-a"
    }

    fn completion_listener_for_session<'a>(
        listeners: &'a [CompletionEventListenerRow],
        session_id: &str,
    ) -> &'a CompletionEventListenerRow {
        listeners
            .iter()
            .find(|listener| listener.session_id == session_id)
            .unwrap()
    }

    fn mailbox_row_for_seq(rows: &[MailboxRow], seq: i64) -> &MailboxRow {
        rows.iter().find(|row| row.seq == seq).unwrap()
    }

    #[test]
    fn digest_directory_and_publication_helpers_preserve_payload_contracts() {
        let dir = tempfile::tempdir().unwrap();
        let digest_path = dir.path().join("digest.txt");
        fs::write(&digest_path, b"abc").unwrap();
        assert_eq!(
            format_sha256_digest(&sha256_file(&digest_path).unwrap()),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            validate_durable_directory_state(dir.path()).unwrap(),
            DurableDirectoryState::Existing
        );
        assert_eq!(validate_payload_publication(&digest_path).unwrap(), None);
        assert_eq!(
            validate_durable_directory_state(&digest_path).unwrap_err(),
            format!("Path is not a directory: {}", digest_path.display())
        );
    }

    #[test]
    fn delivery_sequence_parsing_and_claim_predicates_preserve_fail_closed_behavior() {
        let decoded = parse_delivery_seqs("[0]").unwrap();
        assert_eq!(decoded, vec![0]);
        assert_eq!(
            validate_delivery_claim_seqs(&decoded)
                .unwrap_err()
                .to_string(),
            "Delivery claim requires positive mailbox sequence numbers"
        );
        assert!(parse_delivery_seqs("not-json").is_err());
        let missing_encoding = parse_active_delivery_claim_batches(vec![None]).unwrap();
        assert!(delivery_claim_batches_overlap(&missing_encoding, &[1]));
        assert!(all_mailbox_seqs_pending(&[Some(None), Some(None)]));
        assert!(!all_mailbox_seqs_pending(&[Some(None), None]));
    }

    #[test]
    fn runtime_field_parser_rejects_malformed_encoded_fields() {
        let malformed = RawRuntimeGenerationFields {
            generation_uuid: "not-a-uuid".to_string(),
            lifecycle_state: "starting".to_string(),
            identity_os_pid: None,
            identity_os_boot_id: None,
            identity_os_pid_starttime_ticks: None,
            creator_identity_os_pid: None,
            creator_identity_os_boot_id: None,
            creator_identity_os_pid_starttime_ticks: None,
            terminal_reason: None,
            drain_request_uuid: None,
            active_delivery_claim_uuid: None,
            active_delivery_seqs_json: None,
        };
        assert!(
            format!(
                "{:?}",
                parse_runtime_generation_fields(malformed).unwrap_err()
            )
            .contains("Invalid runtime generation UUID")
        );

        let malformed = RawRuntimeGenerationFields {
            generation_uuid: RuntimeGenerationId::new().to_string(),
            lifecycle_state: "invalid".to_string(),
            identity_os_pid: Some(42),
            identity_os_boot_id: None,
            identity_os_pid_starttime_ticks: None,
            creator_identity_os_pid: None,
            creator_identity_os_boot_id: None,
            creator_identity_os_pid_starttime_ticks: None,
            terminal_reason: None,
            drain_request_uuid: None,
            active_delivery_claim_uuid: None,
            active_delivery_seqs_json: Some("[0]".to_string()),
        };
        assert!(parse_runtime_generation_fields(malformed).is_err());

        let malformed = RawRuntimeGenerationFields {
            generation_uuid: RuntimeGenerationId::new().to_string(),
            lifecycle_state: "starting".to_string(),
            identity_os_pid: Some(42),
            identity_os_boot_id: None,
            identity_os_pid_starttime_ticks: None,
            creator_identity_os_pid: None,
            creator_identity_os_boot_id: None,
            creator_identity_os_pid_starttime_ticks: None,
            terminal_reason: None,
            drain_request_uuid: None,
            active_delivery_claim_uuid: None,
            active_delivery_seqs_json: None,
        };
        assert!(
            format!(
                "{:?}",
                parse_runtime_generation_fields(malformed).unwrap_err()
            )
            .contains("partial exact process identity")
        );

        let malformed = RawRuntimeGenerationFields {
            generation_uuid: RuntimeGenerationId::new().to_string(),
            lifecycle_state: "starting".to_string(),
            identity_os_pid: None,
            identity_os_boot_id: None,
            identity_os_pid_starttime_ticks: None,
            creator_identity_os_pid: None,
            creator_identity_os_boot_id: None,
            creator_identity_os_pid_starttime_ticks: None,
            terminal_reason: None,
            drain_request_uuid: None,
            active_delivery_claim_uuid: None,
            active_delivery_seqs_json: Some("[0]".to_string()),
        };
        assert!(
            format!(
                "{:?}",
                parse_runtime_generation_fields(malformed).unwrap_err()
            )
            .contains("positive mailbox sequence numbers")
        );
    }

    #[test]
    fn schema_mapper_and_table_accessor_cover_fresh_sidecar_and_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        let db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        assert!(mailbox_schema_definition().contains("CREATE TABLE IF NOT EXISTS mailbox"));
        for table in [
            "mailbox",
            "runtime_generation",
            "session_runtime",
            "completion_event",
            "completion_event_listener",
        ] {
            let sql = format_table_columns_pragma(table);
            assert!(!table_columns(&db.conn, table, &sql).unwrap().is_empty());
        }
        assert!(
            table_columns(&db.conn, "runtime_generation", "NOT VALID SQL")
                .unwrap_err()
                .starts_with("Failed to inspect runtime_generation columns:")
        );
    }

    #[test]
    fn submitted_input_key_is_stable_and_target_domain_separated() {
        let session = submitted_input_handle(
            "caller-token",
            InboxTarget {
                kind: InboxTargetKind::Session,
                id: "same-id",
            },
        )
        .unwrap();
        let session_retry = submitted_input_handle(
            "caller-token",
            InboxTarget {
                kind: InboxTargetKind::Session,
                id: "same-id",
            },
        )
        .unwrap();
        let chain = submitted_input_handle(
            "caller-token",
            InboxTarget {
                kind: InboxTargetKind::Chain,
                id: "same-id",
            },
        )
        .unwrap();
        let separate_submission = submitted_input_handle(
            "different-token",
            InboxTarget {
                kind: InboxTargetKind::Session,
                id: "same-id",
            },
        )
        .unwrap();

        assert_eq!(session, session_retry);
        assert_ne!(session, chain);
        assert_ne!(session, separate_submission);
        assert_eq!(session.len(), 64);
    }

    #[test]
    fn submitted_input_retry_returns_original_row_and_payload_collision_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let first = db
            .enqueue_submitted_input(&submitted_input(
                "caller-token",
                InboxTargetKind::Chain,
                "chain-a",
                b"first payload",
            ))
            .unwrap();
        let EnqueueResult::Inserted(first_row) = first else {
            panic!("expected inserted input row, got {first:?}");
        };

        let retry = db
            .enqueue_submitted_input(&submitted_input(
                "caller-token",
                InboxTargetKind::Chain,
                "chain-a",
                b"first payload",
            ))
            .unwrap();
        let EnqueueResult::AlreadyEnqueued(retry_row) = retry else {
            panic!("expected duplicate input row, got {retry:?}");
        };
        assert_eq!(retry_row.seq, first_row.seq);
        assert_eq!(retry_row.kind, SUBMITTED_INPUT_KIND);
        assert_eq!(retry_row.submission_token.as_deref(), Some("caller-token"));
        assert_eq!(retry_row.target_kind.as_deref(), Some("chain"));
        assert_eq!(retry_row.target_id.as_deref(), Some("chain-a"));

        let conflict = db
            .enqueue_submitted_input(&submitted_input(
                "caller-token",
                InboxTargetKind::Chain,
                "chain-a",
                b"different payload",
            ))
            .unwrap();
        assert!(matches!(conflict, EnqueueResult::Conflict { .. }));
        assert_eq!(
            db.list_pending_for_delivery("session-b", Some("chain-a"))
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn legacy_notification_rows_have_no_input_identity_or_target() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let row = inserted_row(db.enqueue_agent_bash_complete(&input("handle-a", "session-a")));

        assert!(row.submission_token.is_none());
        assert!(row.target_kind.is_none());
        assert!(row.target_id.is_none());
        assert_eq!(
            db.list_pending_for_delivery("session-a", Some("chain-a"))
                .unwrap(),
            vec![row]
        );
    }

    #[test]
    fn predecessor_notification_row_remains_readable_after_input_columns_are_added() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE mailbox (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                handle TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                enqueued_at TEXT NOT NULL,
                delivered_at TEXT,
                delivered_by_invocation_uuid TEXT,
                delivery_attempts INTEGER NOT NULL DEFAULT 0,
                delivery_error TEXT,
                owner_invocation_uuid TEXT,
                matched_os_pid INTEGER,
                matched_os_boot_id TEXT,
                matched_os_pid_starttime_ticks INTEGER,
                matched_chain_index INTEGER,
                state_dir TEXT NOT NULL,
                meta_path TEXT NOT NULL,
                log_path TEXT NOT NULL,
                rc_path TEXT NOT NULL,
                rc INTEGER NOT NULL,
                UNIQUE(kind, handle)
             );
             INSERT INTO mailbox (
                session_id, kind, handle, payload_json, enqueued_at,
                state_dir, meta_path, log_path, rc_path, rc
             ) VALUES (
                'session-a', 'agent_bash_complete', 'legacy-handle', '{}',
                '2026-07-16T00:00:00Z', '/tmp/state', '/tmp/meta', '/tmp/log', '/tmp/rc', 0
             );",
        )
        .unwrap();
        drop(conn);

        let db = MailboxDb::open(&path).unwrap();
        let rows = db.list_pending("session-a").unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].handle, "legacy-handle");
        assert!(rows[0].payload_file_path.is_none());
        assert!(rows[0].submission_token.is_none());
        assert!(rows[0].target_kind.is_none());
        assert!(rows[0].target_id.is_none());
    }

    #[test]
    fn mark_delivered_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let row = inserted_row(db.enqueue_agent_bash_complete(&input("handle-a", "session-a")));

        db.mark_delivered("session-a", None, &[row.seq], "resume-1")
            .unwrap();
        let first = db.list_mailbox("session-a", true).unwrap().remove(0);
        db.mark_delivered("session-a", None, &[row.seq], "resume-2")
            .unwrap();
        let second = db.list_mailbox("session-a", true).unwrap().remove(0);

        assert_eq!(second.delivered_at, first.delivered_at);
        assert_eq!(
            second.delivered_by_invocation_uuid.as_deref(),
            Some("resume-1")
        );
        assert_eq!(second.delivery_attempts, 1);
    }

    #[test]
    fn mark_delivered_rejects_a_mixed_session_batch_without_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let owned = inserted_row(db.enqueue_agent_bash_complete(&input("owned", "session-a")));
        let foreign = inserted_row(db.enqueue_agent_bash_complete(&input("foreign", "session-b")));

        let error = db
            .mark_delivered("session-a", None, &[owned.seq, foreign.seq], "invocation-a")
            .unwrap_err();

        assert!(error.contains("missing or foreign-session row"));
        for session_id in ["session-a", "session-b"] {
            let unchanged = db.list_mailbox(session_id, true).unwrap().remove(0);
            assert!(unchanged.delivered_at.is_none());
            assert!(unchanged.delivered_by_invocation_uuid.is_none());
            assert_eq!(unchanged.delivery_attempts, 0);
        }
    }

    #[test]
    fn late_attempt_confirmation_contracts_overlapping_delivery_window() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let rows = (1..=6)
            .map(|index| {
                inserted_row(
                    db.enqueue_agent_bash_complete(&input(&format!("handle-{index}"), "session-a")),
                )
            })
            .collect::<Vec<_>>();
        let first_window = rows[..3].iter().map(|row| row.seq).collect::<Vec<_>>();
        let expanded_window = rows.iter().map(|row| row.seq).collect::<Vec<_>>();
        db.register_delivery_attempt("attempt-1", "session-a", "invocation-a", &first_window, 3)
            .unwrap();
        db.register_delivery_attempt(
            "attempt-2",
            "session-a",
            "invocation-a",
            &expanded_window,
            0,
        )
        .unwrap();
        assert!(
            db.record_delivery_attempt_transport_ack("attempt-1")
                .unwrap()
        );
        assert!(db.confirm_delivery_attempt("attempt-1").unwrap());

        assert_eq!(db.list_pending("session-a").unwrap().len(), 3);
        assert_eq!(
            db.delivery_attempt_disposition("attempt-1").unwrap(),
            Some(MailboxDeliveryAttemptDisposition::Resolved)
        );
        assert_eq!(
            db.delivery_attempt_disposition("attempt-2").unwrap(),
            Some(MailboxDeliveryAttemptDisposition::Stale)
        );
        let contracted = db.delivery_attempt_window("attempt-2").unwrap().unwrap();
        assert_eq!(contracted.rows.len(), 3);
        assert_eq!(contracted.rows[0].handle, "handle-4");
        assert_eq!(contracted.remaining_count, 0);
        assert!(db.confirm_delivery_attempt("attempt-1").unwrap());
        assert_eq!(db.list_pending("session-a").unwrap().len(), 3);
        assert!(
            db.list_mailbox("session-a", true)
                .unwrap()
                .iter()
                .take(3)
                .all(|row| row.delivery_attempts == 1)
        );
        assert!(
            db.list_mailbox("session-a", true)
                .unwrap()
                .iter()
                .take(3)
                .all(|row| row.delivered_by_invocation_uuid.as_deref() == Some("invocation-a"))
        );
    }

    #[test]
    fn delivery_observation_anchor_and_confirmation_are_attempt_scoped() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let row = inserted_row(db.enqueue_agent_bash_complete(&input("handle", "session-a")));
        db.register_delivery_attempt(
            "attempt-observation",
            "session-a",
            "invocation-a",
            &[row.seq],
            0,
        )
        .unwrap();
        let anchor = MailboxDeliveryObservationAnchor {
            provider_name: "provider-a".to_string(),
            provider_instance_id: "provider-instance-a".to_string(),
            settings_id: "settings-a".to_string(),
            provider_session_id: "session-a".to_string(),
            resume_token: "opaque-tail-anchor".to_string(),
            expected_sha256: "1111111111111111111111111111111111111111111111111111111111111111"
                .to_string(),
        };

        db.record_delivery_observation_anchor("attempt-observation", "session-a", &anchor)
            .unwrap();
        assert_eq!(
            db.delivery_observation_anchor("attempt-observation")
                .unwrap(),
            Some(anchor.clone())
        );
        let retry_anchor = MailboxDeliveryObservationAnchor {
            resume_token: "later-retry-anchor".to_string(),
            ..anchor.clone()
        };
        db.record_delivery_observation_anchor("attempt-observation", "session-a", &retry_anchor)
            .unwrap();
        assert_eq!(
            db.delivery_observation_anchor("attempt-observation")
                .unwrap(),
            Some(anchor)
        );
        assert_eq!(
            db.delivery_observation_confirmation("attempt-observation")
                .unwrap(),
            None
        );
        assert_eq!(
            db.pending_delivery_observations("session-a", 1).unwrap(),
            vec![PendingMailboxDeliveryObservation {
                attempt_id: "attempt-observation".to_string(),
                anchor: db
                    .delivery_observation_anchor("attempt-observation")
                    .unwrap()
                    .unwrap(),
            }]
        );

        db.record_delivery_observation_confirmation("attempt-observation", "turn-new")
            .unwrap();
        assert_eq!(
            db.delivery_observation_confirmation("attempt-observation")
                .unwrap()
                .as_deref(),
            Some("turn-new")
        );
        assert!(
            db.pending_delivery_observations("session-a", 1)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn confirmation_from_any_retry_resolves_notification_roots_and_siblings() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let rows = ["handle-a", "handle-b"].map(|handle| {
            inserted_row(db.enqueue_agent_bash_complete(&input(handle, "session-a")))
        });
        let seqs = rows.iter().map(|row| row.seq).collect::<Vec<_>>();
        db.register_delivery_attempt("attempt-1", "session-a", "invocation-a", &seqs, 0)
            .unwrap();
        db.register_delivery_attempt("attempt-2", "session-a", "invocation-a", &seqs, 0)
            .unwrap();
        assert!(
            db.record_delivery_attempt_transport_ack("attempt-2")
                .unwrap()
        );
        assert!(db.confirm_delivery_attempt("attempt-2").unwrap());

        assert!(db.list_pending("session-a").unwrap().is_empty());
        assert_eq!(
            db.delivery_attempt_disposition("attempt-1").unwrap(),
            Some(MailboxDeliveryAttemptDisposition::Resolved)
        );
        assert_eq!(
            db.delivery_attempt_disposition("attempt-2").unwrap(),
            Some(MailboxDeliveryAttemptDisposition::Resolved)
        );
    }

    #[test]
    fn retry_registration_after_late_confirmation_is_resolved_without_redelivery() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let row = inserted_row(db.enqueue_agent_bash_complete(&input("handle-a", "session-a")));
        let seqs = [row.seq];
        db.register_delivery_attempt("attempt-1", "session-a", "invocation-a", &seqs, 0)
            .unwrap();
        db.record_delivery_attempt_transport_ack("attempt-1")
            .unwrap();
        db.confirm_delivery_attempt("attempt-1").unwrap();

        db.register_delivery_attempt("attempt-2", "session-a", "invocation-a", &seqs, 0)
            .unwrap();

        let window = db.delivery_attempt_window("attempt-2").unwrap().unwrap();
        assert!(window.rows.is_empty());
        assert_eq!(
            db.delivery_attempt_disposition("attempt-2").unwrap(),
            Some(MailboxDeliveryAttemptDisposition::Resolved)
        );
        assert!(
            !db.record_delivery_attempt_transport_ack("attempt-2")
                .unwrap()
        );
        let delivered = db.list_mailbox("session-a", true).unwrap().remove(0);
        assert_eq!(delivered.delivery_attempts, 1);
        assert_eq!(
            delivered.delivered_by_invocation_uuid.as_deref(),
            Some("invocation-a")
        );
    }

    #[test]
    fn transport_acknowledgement_is_nonterminal_and_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let row = inserted_row(db.enqueue_agent_bash_complete(&input("handle-a", "session-a")));
        db.register_delivery_attempt("attempt-1", "session-a", "invocation-a", &[row.seq], 0)
            .unwrap();

        assert!(
            db.record_delivery_attempt_transport_ack("attempt-1")
                .unwrap()
        );
        let first_ack = db
            .delivery_attempt_window("attempt-1")
            .unwrap()
            .unwrap()
            .acknowledged_at;
        assert!(first_ack.is_some());
        assert!(
            db.record_delivery_attempt_transport_ack("attempt-1")
                .unwrap()
        );
        let window = db.delivery_attempt_window("attempt-1").unwrap().unwrap();
        assert_eq!(window.acknowledged_at, first_ack);
        assert_eq!(window.rows, vec![row.clone()]);
        let persisted = db.list_mailbox("session-a", true).unwrap().remove(0);
        assert!(persisted.delivered_at.is_none());
        assert!(persisted.delivered_by_invocation_uuid.is_none());
        assert_eq!(persisted.delivery_attempts, 0);
        assert!(persisted.delivery_error.is_none());
        let (resolved_at, resolved_by): (Option<String>, Option<String>) = db
            .connection()
            .query_row(
                "SELECT resolved_at, resolved_by_attempt_id
                 FROM mailbox_delivery_attempts WHERE attempt_id = 'attempt-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(resolved_at.is_none());
        assert!(resolved_by.is_none());
        assert!(
            !db.record_delivery_attempt_transport_ack("missing-attempt")
                .unwrap()
        );
    }

    #[test]
    fn protocol_retry_reuses_unresolved_delivery_attempt() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let row = inserted_row(db.enqueue_agent_bash_complete(&input("handle-a", "session-a")));

        let first = db
            .register_or_reuse_delivery_attempt(
                "attempt-1",
                "session-a",
                "invocation-a",
                "generation-a",
                &[row.seq],
                0,
            )
            .unwrap();
        let retry = db
            .register_or_reuse_delivery_attempt(
                "attempt-2",
                "session-a",
                "invocation-a",
                "generation-a",
                &[row.seq],
                0,
            )
            .unwrap();

        assert_eq!(first, "attempt-1");
        assert_eq!(retry, first);
        assert!(db.delivery_attempt_window("attempt-2").unwrap().is_none());
        assert!(
            db.delivery_attempt_window(&retry)
                .unwrap()
                .is_some_and(|window| window.resolved_at.is_none())
        );
    }

    #[test]
    fn submission_started_retains_exact_attempt_and_blocks_replacement_after_confirmation_fault() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let mut db = MailboxDb::open(&path).unwrap();
        let row = inserted_row(db.enqueue_agent_bash_complete(&input("handle-a", "session-a")));
        let attempt = db
            .register_or_reuse_delivery_attempt(
                "attempt-1",
                "session-a",
                "invocation-a",
                "generation-a",
                &[row.seq],
                0,
            )
            .unwrap();
        assert!(db.begin_delivery_attempt_submission(&attempt).unwrap());
        db.connection()
            .execute_batch(
                "CREATE TRIGGER fail_delivery_confirmation
                 BEFORE UPDATE OF acknowledged_at ON mailbox_delivery_attempts
                 BEGIN SELECT RAISE(FAIL, 'injected confirmation failure'); END;",
            )
            .unwrap();

        assert!(db.confirm_delivery_attempt(&attempt).is_err());
        let retained = db.delivery_attempt_window(&attempt).unwrap().unwrap();
        assert!(retained.submission_started_at.is_some());
        assert!(retained.acknowledged_at.is_none());
        assert!(retained.resolved_at.is_none());
        assert_eq!(retained.rows, vec![row.clone()]);
        assert!(
            db.register_or_reuse_delivery_attempt(
                "replacement",
                "session-a",
                "invocation-b",
                "generation-b",
                &[row.seq],
                0,
            )
            .unwrap_err()
            .contains("mailbox_delivery_submission_uncertain:attempt-1")
        );
        assert_eq!(
            db.register_or_reuse_delivery_attempt(
                "retry",
                "session-a",
                "invocation-a",
                "generation-a",
                &[row.seq],
                0,
            )
            .unwrap(),
            attempt
        );
        db.connection()
            .execute_batch("DROP TRIGGER fail_delivery_confirmation")
            .unwrap();

        assert!(db.confirm_delivery_attempt(&attempt).unwrap());
        assert!(db.list_pending("session-a").unwrap().is_empty());
        let obligations = db
            .pending_delivery_evidence_obligations("session-a")
            .unwrap();
        assert_eq!(obligations.len(), 1);
        assert_eq!(obligations[0].attempt_id, attempt);
        assert_eq!(obligations[0].turn_generation_id, "generation-a");
    }

    #[test]
    fn pre_submission_fault_remains_genuinely_unobserved_and_replaceable() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let row = inserted_row(db.enqueue_agent_bash_complete(&input("handle-a", "session-a")));
        db.register_or_reuse_delivery_attempt(
            "attempt-1",
            "session-a",
            "invocation-a",
            "generation-a",
            &[row.seq],
            0,
        )
        .unwrap();
        db.connection()
            .execute_batch(
                "CREATE TRIGGER fail_submission_start
                 BEFORE UPDATE OF submission_started_at ON mailbox_delivery_attempts
                 BEGIN SELECT RAISE(FAIL, 'injected submission-start failure'); END;",
            )
            .unwrap();

        assert!(db.begin_delivery_attempt_submission("attempt-1").is_err());
        assert!(!db.delivery_attempt_submission_started("attempt-1").unwrap());
        assert!(
            db.resolve_unacknowledged_delivery_attempt("attempt-1")
                .unwrap()
        );
        db.connection()
            .execute_batch("DROP TRIGGER fail_submission_start")
            .unwrap();
        assert_eq!(
            db.register_or_reuse_delivery_attempt(
                "attempt-2",
                "session-a",
                "invocation-b",
                "generation-b",
                &[row.seq],
                0,
            )
            .unwrap(),
            "attempt-2"
        );
    }

    #[test]
    fn resolved_delivery_keeps_evidence_obligation_until_one_idempotent_clear() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let row = inserted_row(db.enqueue_agent_bash_complete(&input("handle-a", "session-a")));
        db.register_or_reuse_delivery_attempt(
            "attempt-1",
            "session-a",
            "invocation-a",
            "generation-a",
            &[row.seq],
            0,
        )
        .unwrap();
        db.begin_delivery_attempt_submission("attempt-1").unwrap();
        db.confirm_delivery_attempt("attempt-1").unwrap();

        let obligation = db
            .pending_delivery_evidence_obligations("session-a")
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(obligation.attempt_id, "attempt-1");
        assert!(
            db.delivery_attempt_window("attempt-1")
                .unwrap()
                .unwrap()
                .resolved_at
                .is_some()
        );
        assert!(db.mark_delivery_evidence_reconciled("attempt-1").unwrap());
        assert!(
            db.pending_delivery_evidence_obligations("session-a")
                .unwrap()
                .is_empty()
        );
        assert!(!db.mark_delivery_evidence_reconciled("attempt-1").unwrap());
    }

    #[test]
    fn unacknowledged_attempt_resolution_is_terminal_but_never_resolves_an_ack() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let row = inserted_row(db.enqueue_agent_bash_complete(&input("handle-a", "session-a")));
        db.register_delivery_attempt("attempt-1", "session-a", "invocation-a", &[row.seq], 0)
            .unwrap();

        assert!(
            db.resolve_unacknowledged_delivery_attempt("attempt-1")
                .unwrap()
        );
        assert!(
            !db.resolve_unacknowledged_delivery_attempt("attempt-1")
                .unwrap()
        );
        assert!(
            !db.record_delivery_attempt_transport_ack("attempt-1")
                .unwrap()
        );

        db.register_delivery_attempt("attempt-2", "session-a", "invocation-a", &[row.seq], 0)
            .unwrap();
        db.record_delivery_attempt_transport_ack("attempt-2")
            .unwrap();
        assert!(
            !db.resolve_unacknowledged_delivery_attempt("attempt-2")
                .unwrap()
        );
        assert_eq!(
            db.accepted_delivery_attempt_windows("session-a")
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn new_invocation_registration_resolves_only_prior_unacknowledged_attempts() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let row = inserted_row(db.enqueue_agent_bash_complete(&input("handle-a", "session-a")));
        db.register_delivery_attempt("old-unacked", "session-a", "old-invocation", &[row.seq], 0)
            .unwrap();
        db.register_delivery_attempt("old-acked", "session-a", "old-invocation", &[row.seq], 0)
            .unwrap();
        db.record_delivery_attempt_transport_ack("old-acked")
            .unwrap();

        db.register_delivery_attempt(
            "current-first",
            "session-a",
            "current-invocation",
            &[row.seq],
            0,
        )
        .unwrap();
        db.register_delivery_attempt(
            "current-second",
            "session-a",
            "current-invocation",
            &[row.seq],
            0,
        )
        .unwrap();

        let resolved_at = |attempt_id: &str| -> Option<String> {
            db.connection()
                .query_row(
                    "SELECT resolved_at FROM mailbox_delivery_attempts WHERE attempt_id = ?1",
                    params![attempt_id],
                    |row| row.get(0),
                )
                .unwrap()
        };
        assert!(resolved_at("old-unacked").is_some());
        assert!(resolved_at("old-acked").is_none());
        assert!(resolved_at("current-first").is_none());
        assert!(resolved_at("current-second").is_none());
    }

    #[test]
    fn provider_confirmation_marks_only_pending_items_and_resolves_attempts() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let rows = ["handle-a", "handle-b"].map(|handle| {
            inserted_row(db.enqueue_agent_bash_complete(&input(handle, "session-a")))
        });
        let seqs = rows.iter().map(|row| row.seq).collect::<Vec<_>>();
        db.register_delivery_attempt("attempt-1", "session-a", "invocation-a", &seqs, 0)
            .unwrap();
        assert!(!db.confirm_delivery_attempt("attempt-1").unwrap());
        db.mark_delivered("session-a", None, &[rows[0].seq], "sibling-invocation")
            .unwrap();
        db.record_delivery_attempt_transport_ack("attempt-1")
            .unwrap();

        assert!(db.confirm_delivery_attempt("attempt-1").unwrap());
        assert!(db.confirm_delivery_attempt("attempt-1").unwrap());
        let delivered = db.list_mailbox("session-a", true).unwrap();
        assert_eq!(delivered[0].delivery_attempts, 1);
        assert_eq!(
            delivered[0].delivered_by_invocation_uuid.as_deref(),
            Some("sibling-invocation")
        );
        assert_eq!(delivered[1].delivery_attempts, 1);
        assert_eq!(
            delivered[1].delivered_by_invocation_uuid.as_deref(),
            Some("invocation-a")
        );
        let (resolved_at, resolved_by): (Option<String>, Option<String>) = db
            .connection()
            .query_row(
                "SELECT resolved_at, resolved_by_attempt_id
                 FROM mailbox_delivery_attempts WHERE attempt_id = 'attempt-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(resolved_at.is_some());
        assert_eq!(resolved_by.as_deref(), Some("attempt-1"));
    }

    #[test]
    fn unobserved_delivery_failure_releases_owner_and_records_one_retry() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let row = inserted_row(db.enqueue_agent_bash_complete(&input("handle-a", "session-a")));
        db.register_delivery_attempt("attempt-1", "session-a", "invocation-a", &[row.seq], 0)
            .unwrap();

        assert!(
            !db.fail_unobserved_delivery_attempt("attempt-1", MAILBOX_DELIVERY_UNCONFIRMED_ERROR)
                .unwrap()
        );
        db.record_delivery_attempt_transport_ack("attempt-1")
            .unwrap();
        assert!(
            db.fail_unobserved_delivery_attempt("attempt-1", MAILBOX_DELIVERY_UNCONFIRMED_ERROR)
                .unwrap()
        );
        assert!(
            !db.fail_unobserved_delivery_attempt("attempt-1", MAILBOX_DELIVERY_UNCONFIRMED_ERROR)
                .unwrap()
        );

        let pending = db.list_pending("session-a").unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].delivery_attempts, 1);
        assert_eq!(
            pending[0].delivery_error.as_deref(),
            Some(MAILBOX_DELIVERY_UNCONFIRMED_ERROR)
        );
        assert!(
            db.accepted_delivery_attempt_windows("session-a")
                .unwrap()
                .is_empty()
        );
        let resolved_at: Option<String> = db
            .connection()
            .query_row(
                "SELECT resolved_at FROM mailbox_delivery_attempts WHERE attempt_id = 'attempt-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(resolved_at.is_some());
    }

    #[test]
    fn unobserved_delivery_failure_rejects_terminal_wake_abandonment() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let row = inserted_row(db.enqueue_agent_bash_complete(&input("handle-a", "session-a")));
        db.register_delivery_attempt("attempt-1", "session-a", "invocation-a", &[row.seq], 0)
            .unwrap();
        db.record_delivery_attempt_transport_ack("attempt-1")
            .unwrap();

        let error = db
            .fail_unobserved_delivery_attempt("attempt-1", WAKE_SWEEP_ABANDONED_ERROR)
            .unwrap_err();

        assert!(error.contains("dedicated authority-bearing disposition"));
        let unchanged = db.list_mailbox("session-a", true).unwrap().remove(0);
        assert_eq!(unchanged.delivery_attempts, 0);
        assert!(unchanged.delivery_error.is_none());
        let resolved_at: Option<String> = db
            .connection()
            .query_row(
                "SELECT resolved_at FROM mailbox_delivery_attempts WHERE attempt_id = 'attempt-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(resolved_at.is_none());
    }

    #[test]
    fn accepted_attempt_owner_requires_transport_ack_and_oldest_pending_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let rows = ["handle-a", "handle-b"].map(|handle| {
            inserted_row(db.enqueue_agent_bash_complete(&input(handle, "session-a")))
        });
        db.register_delivery_attempt(
            "prefix-attempt",
            "session-a",
            "invocation-a",
            &[rows[0].seq],
            1,
        )
        .unwrap();
        db.register_delivery_attempt(
            "suffix-attempt",
            "session-a",
            "invocation-a",
            &[rows[1].seq],
            0,
        )
        .unwrap();
        db.record_delivery_attempt_transport_ack("suffix-attempt")
            .unwrap();
        assert!(
            db.accepted_delivery_attempt_windows("session-a")
                .unwrap()
                .is_empty()
        );

        db.record_delivery_attempt_transport_ack("prefix-attempt")
            .unwrap();
        let owners = db.accepted_delivery_attempt_windows("session-a").unwrap();
        assert_eq!(owners.len(), 1);
        assert_eq!(owners[0].attempt_id, "prefix-attempt");
        assert_eq!(owners[0].rows, vec![rows[0].clone()]);
    }

    #[test]
    fn twice_unconfirmed_oldest_row_remains_the_delivery_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let abandoned =
            inserted_row(db.enqueue_agent_bash_complete(&input("abandoned", "session-a")));
        let unconfirmed =
            inserted_row(db.enqueue_agent_bash_complete(&input("unconfirmed", "session-a")));
        let newer = inserted_row(db.enqueue_agent_bash_complete(&input("newer", "session-a")));
        db.force_pending_abandoned_for_test("session-a", 1).unwrap();
        for _ in 0..2 {
            db.mark_delivery_failed(
                "session-a",
                None,
                &[unconfirmed.seq],
                MAILBOX_DELIVERY_UNCONFIRMED_ERROR,
            )
            .unwrap();
        }
        let unconfirmed = db
            .list_pending("session-a")
            .unwrap()
            .into_iter()
            .find(|row| row.seq == unconfirmed.seq)
            .unwrap();
        assert_eq!(unconfirmed.delivery_attempts, 2);
        assert_eq!(
            unconfirmed.delivery_error.as_deref(),
            Some(MAILBOX_DELIVERY_UNCONFIRMED_ERROR)
        );
        assert_eq!(
            db.list_pending_for_delivery_after("session-a", None, 0, 1)
                .unwrap(),
            vec![unconfirmed.clone()],
            "attempt count cannot remove the oldest pending row from FIFO selection"
        );
        db.register_delivery_attempt(
            "newer-attempt",
            "session-a",
            "invocation-a",
            &[newer.seq],
            0,
        )
        .unwrap();
        db.record_delivery_attempt_transport_ack("newer-attempt")
            .unwrap();
        assert!(
            db.accepted_delivery_attempt_windows("session-a")
                .unwrap()
                .is_empty(),
            "a newer accepted attempt cannot bypass the unconfirmed oldest row"
        );

        db.register_delivery_attempt(
            "unconfirmed-attempt",
            "session-a",
            "invocation-a",
            &[unconfirmed.seq],
            0,
        )
        .unwrap();
        db.record_delivery_attempt_transport_ack("unconfirmed-attempt")
            .unwrap();

        let owners = db.accepted_delivery_attempt_windows("session-a").unwrap();
        assert_eq!(owners.len(), 1);
        assert_eq!(owners[0].attempt_id, "unconfirmed-attempt");
        assert_eq!(owners[0].rows, vec![unconfirmed]);
        assert_eq!(owners[0].remaining_count, 1);
        let abandoned = db
            .list_pending("session-a")
            .unwrap()
            .into_iter()
            .find(|row| row.seq == abandoned.seq)
            .unwrap();
        assert!(!mailbox_row_is_deliverable_pending(&abandoned));
    }

    #[test]
    fn late_transport_ack_after_sibling_confirmation_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let row = inserted_row(db.enqueue_agent_bash_complete(&input("handle-a", "session-a")));
        for attempt_id in ["attempt-1", "attempt-2"] {
            db.register_delivery_attempt(attempt_id, "session-a", "invocation-a", &[row.seq], 0)
                .unwrap();
        }
        db.record_delivery_attempt_transport_ack("attempt-2")
            .unwrap();
        db.confirm_delivery_attempt("attempt-2").unwrap();
        let before = db.list_mailbox("session-a", true).unwrap().remove(0);

        assert!(
            !db.record_delivery_attempt_transport_ack("attempt-1")
                .unwrap()
        );
        let after = db.list_mailbox("session-a", true).unwrap().remove(0);
        assert_eq!(after, before);
        let late = db.delivery_attempt_window("attempt-1").unwrap().unwrap();
        assert!(late.acknowledged_at.is_none());
        assert!(late.rows.is_empty());
    }

    #[test]
    fn deployed_sidecar_attempt_rows_reopen_without_schema_or_historical_rewrite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let mut db = MailboxDb::open(&path).unwrap();
        let old = inserted_row(db.enqueue_agent_bash_complete(&input("old-handle", "session-a")));
        db.register_delivery_attempt("old-attempt", "session-a", "old-invocation", &[old.seq], 0)
            .unwrap();
        db.record_delivery_attempt_transport_ack("old-attempt")
            .unwrap();
        db.confirm_delivery_attempt("old-attempt").unwrap();
        let old_mailbox = db.list_mailbox("session-a", true).unwrap();
        let old_attempt: (Option<String>, Option<String>, Option<String>) = db
            .connection()
            .query_row(
                "SELECT acknowledged_at, resolved_at, resolved_by_attempt_id
                 FROM mailbox_delivery_attempts WHERE attempt_id = 'old-attempt'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        let attempt_ddl: String = db
            .connection()
            .query_row(
                "SELECT sql FROM sqlite_master
                 WHERE type = 'table' AND name = 'mailbox_delivery_attempts'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        drop(db);

        let mut reopened = MailboxDb::open(&path).unwrap();
        let new =
            inserted_row(reopened.enqueue_agent_bash_complete(&input("new-handle", "session-a")));
        reopened
            .register_delivery_attempt("new-attempt", "session-a", "new-invocation", &[new.seq], 0)
            .unwrap();
        reopened
            .record_delivery_attempt_transport_ack("new-attempt")
            .unwrap();
        assert!(reopened.list_pending("session-a").unwrap().contains(&new));
        assert_eq!(
            reopened
                .list_mailbox("session-a", true)
                .unwrap()
                .into_iter()
                .filter(|row| row.handle == "old-handle")
                .collect::<Vec<_>>(),
            old_mailbox
        );
        let reopened_old_attempt: (Option<String>, Option<String>, Option<String>) = reopened
            .connection()
            .query_row(
                "SELECT acknowledged_at, resolved_at, resolved_by_attempt_id
                 FROM mailbox_delivery_attempts WHERE attempt_id = 'old-attempt'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(reopened_old_attempt, old_attempt);
        let reopened_ddl: String = reopened
            .connection()
            .query_row(
                "SELECT sql FROM sqlite_master
                 WHERE type = 'table' AND name = 'mailbox_delivery_attempts'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(reopened_ddl, attempt_ddl);
    }

    #[test]
    fn manual_range_ack_leaves_newer_rows_and_contracts_attempts() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let rows = ["handle-a", "handle-b", "handle-c"].map(|handle| {
            inserted_row(db.enqueue_agent_bash_complete(&input(handle, "session-a")))
        });
        let seqs = rows.iter().map(|row| row.seq).collect::<Vec<_>>();
        db.register_delivery_attempt("attempt-1", "session-a", "invocation-a", &seqs, 0)
            .unwrap();

        let changed = db
            .acknowledge_range("session-a", rows[0].seq, rows[1].seq, "manual-test")
            .unwrap();

        assert_eq!(changed, 2);
        let pending = db.list_pending("session-a").unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].seq, rows[2].seq);
        let window = db.delivery_attempt_window("attempt-1").unwrap().unwrap();
        assert_eq!(window.rows.len(), 1);
        assert_eq!(window.rows[0].seq, rows[2].seq);
        assert_eq!(
            db.delivery_attempt_disposition("attempt-1").unwrap(),
            Some(MailboxDeliveryAttemptDisposition::Stale)
        );
    }

    #[test]
    fn delivered_payload_compaction_preserves_bytes_and_leaves_pending_rows_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let delivered_payload = format!(
            r#"{{"schema_version":1,"kind":"agent_bash_complete","padding":"{}"}}"#,
            "é".repeat(4096)
        );
        let delivered_input = AgentBashCompleteEnqueue {
            payload_json: &delivered_payload,
            ..input("handle-delivered", "session-a")
        };
        let delivered = inserted_row(db.enqueue_agent_bash_complete(&delivered_input));
        let pending =
            inserted_row(db.enqueue_agent_bash_complete(&input("handle-pending", "session-a")));
        db.connection()
            .execute(
                "UPDATE mailbox
                 SET payload_json = ?2, payload_compacted_at = NULL
                 WHERE seq = ?1",
                params![delivered.seq, &delivered_payload],
            )
            .unwrap();
        db.mark_delivered("session-a", None, &[delivered.seq], "resume-1")
            .unwrap();

        let before = db.payloads().delivered_payload_compaction_stats().unwrap();
        assert_eq!(before.eligible_rows, 1);
        assert_eq!(before.inline_bytes, delivered_payload.len() as u64);

        let report = db.payloads().compact_delivered_payloads(1).unwrap();
        assert_eq!(report.scanned_rows, 1);
        assert_eq!(report.compacted_rows, 1);
        assert_eq!(
            report.retained_payload_bytes,
            delivered_payload.len() as u64
        );
        assert!(report.inline_bytes_reclaimed > 0);

        let rows = db.list_mailbox("session-a", true).unwrap();
        let compacted = rows.iter().find(|row| row.seq == delivered.seq).unwrap();
        assert_ne!(compacted.payload_json, delivered_payload);
        assert!(compacted.payload_compacted_at.is_some());
        assert_eq!(
            db.payloads()
                .hydrate_agent_bash_payload_json(compacted)
                .unwrap(),
            delivered_payload
        );
        let still_pending = rows.iter().find(|row| row.seq == pending.seq).unwrap();
        assert_eq!(
            db.payloads()
                .hydrate_agent_bash_payload_json(still_pending)
                .unwrap(),
            input("x", "y").payload_json
        );
        assert!(still_pending.delivered_at.is_none());
        assert!(
            db.connection()
                .query_row(
                    "SELECT payload_compacted_at FROM mailbox WHERE seq = ?1",
                    params![delivered.seq],
                    |row| row.get::<_, Option<String>>(0),
                )
                .unwrap()
                .is_some()
        );

        assert_eq!(
            db.payloads().delivered_payload_compaction_stats().unwrap(),
            DeliveredPayloadCompactionStats::default()
        );
        assert_eq!(
            db.payloads().compact_delivered_payloads(1).unwrap(),
            DeliveredPayloadCompactionReport::default()
        );
    }

    #[test]
    fn delivered_payload_compaction_externalizes_legacy_inline_payload() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let payload = format!(r#"{{"legacy":"{}"}}"#, "y".repeat(2048));
        let enqueue = AgentBashCompleteEnqueue {
            payload_json: &payload,
            ..input("legacy-delivered", "session-a")
        };
        let row = inserted_row(db.enqueue_agent_bash_complete(&enqueue));
        db.connection()
            .execute(
                "UPDATE mailbox
                 SET payload_json = ?2,
                     payload_compacted_at = NULL,
                     payload_file_path = NULL,
                     payload_sha256 = NULL,
                     payload_byte_len = NULL,
                     payload_retention_policy = NULL
                 WHERE seq = ?1",
                params![row.seq, &payload],
            )
            .unwrap();
        db.mark_delivered("session-a", None, &[row.seq], "resume-1")
            .unwrap();

        let report = db.payloads().compact_delivered_payloads(1).unwrap();
        assert_eq!(report.compacted_rows, 1);
        let compacted = db.list_mailbox("session-a", true).unwrap().remove(0);
        assert_eq!(
            fs::read_to_string(compacted.payload_file_path.as_deref().unwrap()).unwrap(),
            payload
        );
        assert_eq!(
            db.payloads()
                .hydrate_agent_bash_payload_json(&compacted)
                .unwrap(),
            payload
        );
    }

    #[test]
    fn terminal_history_pruning_is_bounded_and_preserves_live_authority() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let old = inserted_row(db.enqueue_agent_bash_complete(&input("old", "session-a")));
        let protected =
            inserted_row(db.enqueue_agent_bash_complete(&input("protected", "session-a")));
        db.register_completion_event(completion_registration(
            "terminal-event",
            "async",
            "session-a",
            "terminal-owner",
        ))
        .unwrap();
        let event_result = db
            .trigger_completion_event(completion_trigger(
                "terminal-event",
                r#"{"terminal":"payload"}"#,
                false,
            ))
            .unwrap();
        let event_row = event_result.mailbox_rows.into_iter().next().unwrap();
        let event_payload_path = PathBuf::from(event_row.payload_file_path.as_deref().unwrap());
        let newest = inserted_row(db.enqueue_agent_bash_complete(&input("newest", "session-a")));
        let pending = inserted_row(db.enqueue_agent_bash_complete(&input("pending", "session-a")));
        db.mark_delivered(
            "session-a",
            None,
            &[old.seq, protected.seq, event_row.seq, newest.seq],
            "delivery",
        )
        .unwrap();
        db.connection()
            .execute_batch(&format!(
                "INSERT INTO mailbox_delivery_attempts (
                     attempt_id, session_id, delivery_invocation_uuid, created_at,
                     prepared_remaining_count
                 ) VALUES
                     ('resolved-evidence', 'session-a', 'delivery', '2026-07-31T00:00:00Z', 0),
                     ('resolved-old', 'session-a', 'delivery', '2026-08-01T00:00:00Z', 0),
                     ('resolved-middle', 'session-a', 'delivery', '2026-08-02T00:00:00Z', 0),
                     ('resolved-new', 'session-a', 'delivery', '2026-08-03T00:00:00Z', 0),
                     ('unresolved', 'session-a', 'delivery', '2026-08-04T00:00:00Z', 0);
                 UPDATE mailbox_delivery_attempts
                  SET resolved_at = created_at
                  WHERE attempt_id LIKE 'resolved-%';
                  UPDATE mailbox_delivery_attempts
                  SET acknowledged_at = created_at,
                      evidence_turn_generation_id = 'generation-a',
                      evidence_observed_at = 1,
                      evidence_disposition = 'pending'
                  WHERE attempt_id = 'resolved-evidence';
                  INSERT INTO mailbox_delivery_attempt_items (attempt_id, mailbox_seq)
                  VALUES ('unresolved', {});",
                protected.seq
            ))
            .unwrap();

        let before = terminal_history_retention_stats_on(db.connection(), 1).unwrap();
        assert_eq!(before.terminal_mailbox_rows, 4);
        assert_eq!(before.prunable_mailbox_rows, 2);
        assert_eq!(before.prunable_delivery_attempts, 2);

        let report = db.prune_terminal_history_with_keep(10, 1).unwrap();

        assert_eq!(report.mailbox_rows_deleted, 2);
        assert_eq!(report.listeners_detached, 1);
        assert_eq!(report.delivery_attempts_deleted, 2);
        assert!(
            db.delivery_attempt_window("resolved-evidence")
                .unwrap()
                .is_some()
        );
        assert!(!event_payload_path.exists());
        let remaining = db.list_mailbox("session-a", true).unwrap();
        assert_eq!(
            remaining.iter().map(|row| row.seq).collect::<Vec<_>>(),
            vec![protected.seq, newest.seq, pending.seq]
        );
        assert!(
            db.completion_event_listeners("terminal-event").unwrap()[0]
                .mailbox_seq
                .is_none()
        );
        assert!(
            db.completion_event("terminal-event")
                .unwrap()
                .unwrap()
                .payload_reclaimed_at
                .is_some()
        );
        assert!(
            db.connection()
                .query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM mailbox_delivery_attempt_items
                         WHERE attempt_id = 'unresolved' AND mailbox_seq = ?1
                     )",
                    params![protected.seq],
                    |row| row.get::<_, bool>(0),
                )
                .unwrap()
        );
        let replay = db
            .trigger_completion_event(completion_trigger(
                "terminal-event",
                r#"{"terminal":"payload"}"#,
                false,
            ))
            .unwrap();
        assert!(!replay.triggered);
        assert!(!event_payload_path.exists());

        db.vacuum_terminal_history().unwrap();
        let after = inserted_row(db.enqueue_agent_bash_complete(&input("after", "session-a")));
        assert!(after.seq > pending.seq);
    }

    #[test]
    fn terminal_history_candidate_discovery_does_not_hold_writer_lock() {
        let dir = tempfile::tempdir().unwrap();
        let sidecar_path = dir.path().join("pid-identity.db");
        let mut maintenance = MailboxDb::open(&sidecar_path).unwrap();
        let old = inserted_row(maintenance.enqueue_agent_bash_complete(&input("old", "session-a")));
        maintenance
            .mark_delivered("session-a", None, &[old.seq], "delivery")
            .unwrap();

        let paused = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let handler_paused = std::sync::Arc::clone(&paused);
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let release_rx = std::sync::Arc::new(std::sync::Mutex::new(release_rx));
        let handler_release_rx = std::sync::Arc::clone(&release_rx);
        maintenance
            .connection()
            .progress_handler(
                1,
                Some(move || {
                    if !handler_paused.swap(true, std::sync::atomic::Ordering::SeqCst) {
                        entered_tx.send(()).unwrap();
                        handler_release_rx.lock().unwrap().recv().unwrap();
                    }
                    false
                }),
            )
            .unwrap();

        let prune = std::thread::spawn(move || maintenance.prune_terminal_history_with_keep(1, 0));
        entered_rx
            .recv_timeout(StdDuration::from_secs(2))
            .expect("terminal history candidate query did not start");

        let writer = Connection::open(&sidecar_path).unwrap();
        writer.busy_timeout(StdDuration::from_millis(100)).unwrap();
        let write_result = writer.execute(
            "INSERT INTO mailbox (
                 session_id, kind, handle, payload_json, enqueued_at,
                 state_dir, meta_path, log_path, rc_path, rc
             ) VALUES (
                 'session-b', 'input', 'concurrent-writer', '{}',
                 '2026-08-28T00:00:00Z', '/state', '/meta', '/log', '/rc', 0
             )",
            [],
        );
        release_tx.send(()).unwrap();

        assert_eq!(write_result.unwrap(), 1);
        prune.join().unwrap().unwrap();
    }

    #[test]
    fn terminal_payload_reclamation_lookup_work_is_bounded() {
        const HISTORY_PER_REFERENCE_KIND: usize = 2_048;

        let dir = tempfile::tempdir().unwrap();
        let db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        db.connection()
            .execute_batch(&format!(
                "WITH RECURSIVE counter(value) AS (
                     VALUES(1)
                     UNION ALL
                     SELECT value + 1 FROM counter
                     WHERE value < {HISTORY_PER_REFERENCE_KIND}
                 )
                 INSERT INTO completion_event (
                     event_id, kind, state, delivery_mode, state_dir, meta_path,
                     log_path, rc_path, rc, payload_json, payload_file_path,
                     payload_sha256, payload_byte_len, payload_retention_policy,
                     created_at, triggered_at
                 )
                 SELECT printf('mailbox-event-%d', value), 'agent_bash_complete',
                        'triggered', 'async', '/state', '/meta', '/log', '/rc', 0,
                        '{{}}', printf('/payload/mailbox-%d', value),
                        printf('%064x', value), 2, 'immutable',
                        '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z'
                 FROM counter;

                 WITH RECURSIVE counter(value) AS (
                     VALUES(1)
                     UNION ALL
                     SELECT value + 1 FROM counter
                     WHERE value < {HISTORY_PER_REFERENCE_KIND}
                 )
                 INSERT INTO mailbox (
                     session_id, kind, handle, payload_json, enqueued_at,
                     delivered_at, state_dir, meta_path, log_path, rc_path, rc,
                     payload_file_path, payload_sha256, payload_byte_len,
                     payload_retention_policy, payload_compacted_at
                 )
                 SELECT 'session-a', 'agent_bash_complete',
                        printf('mailbox-handle-%d', value), '{{}}',
                        '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z',
                        '/state', '/meta', '/log', '/rc', 0,
                        printf('/payload/mailbox-%d', value),
                        printf('%064x', value), 2, 'immutable',
                        '2026-08-01T00:00:00Z'
                 FROM counter;

                 WITH RECURSIVE counter(value) AS (
                     VALUES(1)
                     UNION ALL
                     SELECT value + 1 FROM counter
                     WHERE value < {HISTORY_PER_REFERENCE_KIND}
                 )
                 INSERT INTO completion_event (
                     event_id, kind, state, delivery_mode, state_dir, meta_path,
                     log_path, rc_path, rc, payload_json, payload_file_path,
                     payload_sha256, payload_byte_len, payload_retention_policy,
                     created_at, triggered_at
                 )
                 SELECT printf('listener-event-%d', value), 'agent_bash_complete',
                        'triggered', 'async', '/state', '/meta', '/log', '/rc', 0,
                        '{{}}', printf('/payload/listener-%d', value),
                        printf('%064x', value + {HISTORY_PER_REFERENCE_KIND}),
                        2, 'immutable', '2026-08-01T00:00:00Z',
                        '2026-08-01T00:00:00Z'
                 FROM counter;

                 WITH RECURSIVE counter(value) AS (
                     VALUES(1)
                     UNION ALL
                     SELECT value + 1 FROM counter
                     WHERE value < {HISTORY_PER_REFERENCE_KIND}
                 )
                 INSERT INTO completion_event_listener (
                     event_id, listener_id, session_id, owner_invocation_uuid,
                     active, created_at
                 )
                 SELECT printf('listener-event-%d', value),
                        printf('listener-%d', value), 'session-a',
                        printf('listener-%d', value), 1,
                        '2026-08-01T00:00:00Z'
                 FROM counter;"
            ))
            .unwrap();

        begin_completion_finalization_vm_count();
        let reclaimable = reclaimable_completion_payloads(db.connection(), 256).unwrap();
        let steps = end_completion_finalization_vm_count();
        eprintln!("terminal payload lookup VM steps: {steps}");

        assert!(reclaimable.is_empty());
        assert!(
            steps < 500_000,
            "terminal payload lookup exceeded its bounded VM budget: {steps}"
        );
    }

    #[test]
    fn terminal_mailbox_pruning_work_is_independent_of_unrelated_resolved_attempts() {
        const ATTEMPT_HISTORY: usize = 4_096;
        const MAILBOX_HISTORY: usize = 256;

        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        db.connection()
            .execute_batch(&format!(
                "WITH RECURSIVE counter(value) AS (
                     VALUES(1)
                     UNION ALL
                     SELECT value + 1 FROM counter
                     WHERE value < {ATTEMPT_HISTORY}
                 )
                 INSERT INTO mailbox_delivery_attempts (
                     attempt_id, session_id, delivery_invocation_uuid, created_at,
                     prepared_remaining_count, resolved_at, evidence_disposition
                 )
                 SELECT printf('protected-attempt-%d', value), 'session-a',
                        'delivery', '2026-08-01T00:00:00Z', 0,
                        '2026-08-01T00:00:00Z', 'pending'
                 FROM counter;

                 WITH RECURSIVE counter(value) AS (
                     VALUES(1)
                     UNION ALL
                     SELECT value + 1 FROM counter
                     WHERE value < {MAILBOX_HISTORY}
                 )
                 INSERT INTO mailbox (
                     session_id, kind, handle, payload_json, enqueued_at,
                     delivered_at, state_dir, meta_path, log_path, rc_path, rc
                 )
                 SELECT 'session-a', 'agent_bash_complete',
                        printf('terminal-handle-%d', value), '{{}}',
                        '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z',
                        '/state', '/meta', '/log', '/rc', 0
                 FROM counter;"
            ))
            .unwrap();

        begin_completion_finalization_vm_count();
        let report = db
            .prune_terminal_history_with_keep(MAILBOX_HISTORY, 0)
            .unwrap();
        let steps = end_completion_finalization_vm_count();

        assert_eq!(report.mailbox_rows_deleted, MAILBOX_HISTORY);
        assert_eq!(report.delivery_attempts_deleted, 0);
        assert!(
            steps < 500_000,
            "terminal mailbox pruning exceeded its history-independent VM budget: {steps}"
        );
    }

    #[test]
    fn terminal_history_vacuum_rejects_a_busy_wal_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let sidecar_path = dir.path().join("pid-identity.db");
        let mut db = MailboxDb::open(&sidecar_path).unwrap();
        db.connection()
            .execute(
                "INSERT INTO mailbox (
                     session_id, kind, handle, payload_json, enqueued_at,
                     state_dir, meta_path, log_path, rc_path, rc
                 ) VALUES (
                     'session-a', 'input', 'before-reader', '{}',
                     '2026-08-28T00:00:00Z', '/state', '/meta', '/log', '/rc', 0
                 )",
                [],
            )
            .unwrap();

        let reader = Connection::open(&sidecar_path).unwrap();
        reader.execute_batch("BEGIN").unwrap();
        reader
            .query_row("SELECT COUNT(*) FROM mailbox", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();
        db.connection()
            .execute(
                "INSERT INTO mailbox (
                     session_id, kind, handle, payload_json, enqueued_at,
                     state_dir, meta_path, log_path, rc_path, rc
                 ) VALUES (
                     'session-a', 'input', 'after-reader', '{}',
                     '2026-08-28T00:00:00Z', '/state', '/meta', '/log', '/rc', 0
                 )",
                [],
            )
            .unwrap();
        db.connection()
            .busy_timeout(StdDuration::from_millis(10))
            .unwrap();

        let error = db.vacuum_terminal_history().unwrap_err();

        assert!(
            error.contains("checkpoint remained busy before VACUUM"),
            "{error}"
        );
        reader.execute_batch("ROLLBACK").unwrap();
        db.connection()
            .busy_timeout(mailbox_writer_sqlite_timeout())
            .unwrap();
        db.vacuum_terminal_history().unwrap();
    }

    #[test]
    fn terminal_history_retains_submitted_input_idempotency() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let enqueue = submitted_input(
            "submission-token",
            InboxTargetKind::Session,
            "session-a",
            b"input payload",
        );
        let EnqueueResult::Inserted(row) = db.enqueue_submitted_input(&enqueue).unwrap() else {
            panic!("expected inserted submitted input");
        };
        db.mark_delivered("session-a", None, &[row.seq], "delivery")
            .unwrap();

        let report = db.prune_terminal_history_with_keep(10, 0).unwrap();

        assert_eq!(report.mailbox_rows_deleted, 0);
        let EnqueueResult::AlreadyEnqueued(retry) = db.enqueue_submitted_input(&enqueue).unwrap()
        else {
            panic!("expected submitted input retry to retain its durable identity");
        };
        assert_eq!(retry.seq, row.seq);
    }

    #[test]
    fn ordinary_delivery_runs_bounded_terminal_history_maintenance() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let mut seqs = Vec::new();
        for index in 0..(TERMINAL_HISTORY_KEEP_ROWS + 2) {
            let handle = format!("maintenance-{index}");
            seqs.push(
                inserted_row(db.enqueue_agent_bash_complete(&input(&handle, "session-a"))).seq,
            );
        }

        db.mark_delivered("session-a", None, &seqs, "delivery")
            .unwrap();

        let rows = db.list_mailbox("session-a", true).unwrap();
        assert_eq!(rows.len(), TERMINAL_HISTORY_KEEP_ROWS);
        assert_eq!(rows[0].seq, seqs[2]);
        assert_eq!(
            db.terminal_history_retention_stats()
                .unwrap()
                .prunable_mailbox_rows,
            0
        );
    }

    #[test]
    fn legacy_inline_payload_hydration_does_not_require_retained_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let payload = r#"{"schema_version":1,"kind":"agent_bash_complete","legacy":true}"#;
        let enqueue = AgentBashCompleteEnqueue {
            payload_json: payload,
            ..input("legacy-inline", "session-a")
        };
        let row = inserted_row(db.enqueue_agent_bash_complete(&enqueue));
        db.connection()
            .execute(
                "UPDATE mailbox
                 SET payload_json = ?2, payload_compacted_at = NULL
                 WHERE seq = ?1",
                params![row.seq, payload],
            )
            .unwrap();
        fs::remove_file(row.payload_file_path.as_deref().unwrap()).unwrap();

        let legacy = db.list_mailbox("session-a", true).unwrap().remove(0);
        assert_eq!(legacy.payload_compacted_at, None);
        assert_eq!(
            db.payloads()
                .hydrate_agent_bash_payload_json(&legacy)
                .unwrap(),
            payload
        );
    }

    #[test]
    fn notification_pause_state_defaults_false_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();

        assert!(!db.notifications_paused("session-a").unwrap());
        db.set_notifications_paused("session-a", true).unwrap();
        assert!(db.notifications_paused("session-a").unwrap());
        db.set_notifications_paused("session-a", false).unwrap();
        assert!(!db.notifications_paused("session-a").unwrap());
    }

    #[test]
    fn mark_delivery_failed_records_attempt_without_delivery() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let row = inserted_row(db.enqueue_agent_bash_complete(&input("handle-a", "session-a")));

        db.mark_delivery_failed(
            "session-a",
            None,
            &[row.seq],
            "mailbox_delivery_unconfirmed",
        )
        .unwrap();
        let failed = db.list_mailbox("session-a", true).unwrap().remove(0);

        assert!(failed.delivered_at.is_none());
        assert_eq!(failed.delivery_attempts, 1);
        assert_eq!(
            failed.delivery_error.as_deref(),
            Some("mailbox_delivery_unconfirmed")
        );
    }

    #[test]
    fn mark_delivery_failed_rejects_a_mixed_foreign_target_batch_without_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let owned = inserted_row(db.enqueue_agent_bash_complete(&input("handle-a", "session-a")));
        let foreign = inserted_row(db.enqueue_agent_bash_complete(&input("handle-b", "session-b")));

        let error = db
            .mark_delivery_failed(
                "session-a",
                None,
                &[owned.seq, foreign.seq],
                "mailbox_delivery_unconfirmed",
            )
            .unwrap_err();

        assert!(error.contains("missing, settled, or foreign-target row"));
        for session_id in ["session-a", "session-b"] {
            let unchanged = db.list_mailbox(session_id, true).unwrap().remove(0);
            assert_eq!(unchanged.delivery_attempts, 0);
            assert!(unchanged.delivery_error.is_none());
        }
    }

    #[test]
    fn mark_delivery_failed_accepts_an_authorized_chain_target() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let EnqueueResult::Inserted(row) = db
            .enqueue_submitted_input(&submitted_input(
                "chain-failure-token",
                InboxTargetKind::Chain,
                "chain-a",
                b"chain input",
            ))
            .unwrap()
        else {
            panic!("expected chain-targeted input to be inserted");
        };

        db.mark_delivery_failed(
            "active-session-a",
            Some("chain-a"),
            &[row.seq],
            "mailbox_delivery_unconfirmed",
        )
        .unwrap();

        let failed = db.list_mailbox("chain-a", true).unwrap().remove(0);
        assert_eq!(failed.delivery_attempts, 1);
        assert_eq!(
            failed.delivery_error.as_deref(),
            Some("mailbox_delivery_unconfirmed")
        );
    }

    #[test]
    fn generic_delivery_failure_rejects_terminal_wake_abandonment() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let row = inserted_row(db.enqueue_agent_bash_complete(&input("handle-a", "session-a")));

        let error = db
            .mark_delivery_failed("session-a", None, &[row.seq], WAKE_SWEEP_ABANDONED_ERROR)
            .unwrap_err();

        assert!(error.contains("dedicated authority-bearing disposition"));
        let unchanged = db.list_mailbox("session-a", true).unwrap().remove(0);
        assert_eq!(unchanged.delivery_attempts, 0);
        assert!(unchanged.delivery_error.is_none());
    }

    #[test]
    fn list_pending_excludes_delivered() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let delivered =
            inserted_row(db.enqueue_agent_bash_complete(&input("handle-a", "session-a")));
        let pending = inserted_row(db.enqueue_agent_bash_complete(&input("handle-b", "session-a")));

        db.mark_delivered("session-a", None, &[delivered.seq], "resume-1")
            .unwrap();

        let pending_rows = db.list_pending("session-a").unwrap();
        assert_eq!(pending_rows.len(), 1);
        assert_eq!(pending_rows[0].seq, pending.seq);
        assert_eq!(pending_rows[0].handle, "handle-b");
    }

    #[test]
    fn bounded_pending_session_and_chain_plans_use_supporting_mailbox_indexes() {
        let dir = tempfile::tempdir().unwrap();
        let db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();

        for chain_id in [None, Some("chain-a")] {
            let query = format!("EXPLAIN QUERY PLAN {}", bounded_pending_mailbox_query());
            let mut statement = db.connection().prepare(&query).unwrap();
            let details = statement
                .query_map(
                    params![
                        "session-a",
                        chain_id,
                        0,
                        1,
                        WAKE_SWEEP_ABANDONED_ERROR,
                        MAILBOX_PAYLOAD_VERIFICATION_FAILED_ERROR,
                        MAILBOX_INGRESS_EXPIRED_ERROR,
                    ],
                    |row| row.get::<_, String>(3),
                )
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();

            assert!(
                details
                    .iter()
                    .any(|detail| detail.contains("idx_mailbox_pending (")),
                "bounded pending plan did not use the session index: {details:?}"
            );
            assert!(
                details
                    .iter()
                    .any(|detail| detail.contains("idx_mailbox_pending_target")),
                "bounded pending plan did not use the target index: {details:?}"
            );
        }
    }

    #[test]
    fn pending_list_does_not_mutate_rows_for_crash_redelivery() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let row = inserted_row(db.enqueue_agent_bash_complete(&input("handle-a", "session-a")));

        let first = db.list_pending("session-a").unwrap();
        let second = db.list_pending("session-a").unwrap();

        assert_eq!(first[0].seq, row.seq);
        assert_eq!(second[0].seq, row.seq);
        assert!(second[0].delivered_at.is_none());
        assert_eq!(second[0].delivery_attempts, 0);
    }

    #[test]
    fn legacy_selected_auto_wake_max_column_remains_for_nondestructive_repair() {
        let legacy_columns = session_runtime_column_additions()
            .into_iter()
            .map(|(name, _)| name.to_string())
            .filter(|name| name != "selected_auto_wake_max")
            .collect::<Vec<_>>();

        assert_eq!(
            missing_session_runtime_columns(&legacy_columns),
            vec![("selected_auto_wake_max", "INTEGER")]
        );

        let dir = tempfile::tempdir().unwrap();
        let db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let sql = format_table_columns_pragma("session_runtime");
        assert!(
            table_columns(&db.conn, "session_runtime", &sql)
                .unwrap()
                .contains(&"selected_auto_wake_max".to_string())
        );
    }

    #[test]
    fn runtime_mark_running_records_pid_identity() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let identity = current_identity();

        db.wake_sessions()
            .project_legacy_runtime_running(LegacyRuntimeProjection {
                session_id: "session-a",
                mode: "headless",
                invocation_uuid: "invocation-a",
                provider_name: Some("provider-a"),
                model_name: Some("model-a"),
                identity: &identity,
                pty_control_path: None,
                turn_start_max_mailbox_seq: Some(7),
                models_dir: Some("/tmp/models"),
                effective_cwd: Some("/tmp/work"),
            })
            .unwrap();

        let metadata = db
            .wake_session_reader()
            .session_metadata("session-a")
            .unwrap()
            .unwrap();
        let projection = db
            .wake_session_reader()
            .legacy_runtime_projection("session-a")
            .unwrap()
            .unwrap();
        assert_eq!(projection.run_state, "running");
        assert_eq!(metadata.mode, "headless");
        assert_eq!(metadata.invocation_uuid.as_deref(), Some("invocation-a"));
        assert_eq!(
            projection.running_invocation_uuid.as_deref(),
            Some("invocation-a")
        );
        assert_eq!(metadata.provider_name.as_deref(), Some("provider-a"));
        assert_eq!(metadata.model_name.as_deref(), Some("model-a"));
        assert_eq!(projection.running_os_pid, Some(identity.os_pid));
        assert_eq!(
            projection.running_os_boot_id.as_deref(),
            Some(identity.os_boot_id.as_str())
        );
        assert_eq!(
            projection.running_os_pid_starttime_ticks,
            Some(identity.os_pid_starttime_ticks)
        );
        assert_eq!(projection.turn_start_max_mailbox_seq, Some(7));
        assert_eq!(metadata.models_dir.as_deref(), Some("/tmp/models"));
        assert_eq!(metadata.effective_cwd.as_deref(), Some("/tmp/work"));
        assert!(projection.turn_started_at.is_some());
        assert!(projection.turn_ended_at.is_none());
    }

    #[test]
    fn auto_wake_keeps_owner_separate_from_running_invocation() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let identity = current_identity();

        db.wake_sessions()
            .project_legacy_runtime_running(LegacyRuntimeProjection {
                session_id: "session-a",
                mode: "headless",
                invocation_uuid: "owner-invocation",
                provider_name: Some("provider-a"),
                model_name: Some("model-a"),
                identity: &identity,
                pty_control_path: None,
                turn_start_max_mailbox_seq: None,
                models_dir: Some("/tmp/models"),
                effective_cwd: Some("/tmp/work"),
            })
            .unwrap();
        assert!(
            db.wake_sessions()
                .settle_legacy_runtime_projection(LegacyRuntimeProjectionSettlement {
                    session_id: "session-a",
                    invocation_uuid: "owner-invocation",
                    last_exit_code: Some(0),
                })
                .unwrap()
        );

        db.wake_sessions()
            .upsert_session_metadata(SessionMetadataUpsert {
                session_id: "session-a",
                mode: "headless",
                invocation_uuid: None,
                provider_name: Some("provider-a"),
                model_name: Some("model-a"),
                models_dir: None,
                effective_cwd: None,
            })
            .unwrap();
        db.enqueue_agent_bash_complete(&input("handle-a", "session-a"))
            .unwrap();
        assert!(matches!(
            db.wake_sessions()
                .try_acquire_wake_claim(WakeClaimRequest {
                    session_id: "session-a",
                    claim_token: "wake-token",
                    reason: "notify_idle",
                    auto_wake_count: 1,
                    wake_invocation_uuid: None,
                    stale_after_seconds: 600,
                })
                .unwrap(),
            WakeClaimAcquireResult::Acquired(_)
        ));

        db.wake_sessions()
            .project_legacy_runtime_running(LegacyRuntimeProjection {
                session_id: "session-a",
                mode: "headless",
                invocation_uuid: "wake-invocation",
                provider_name: Some("provider-a"),
                model_name: Some("model-a"),
                identity: &identity,
                pty_control_path: None,
                turn_start_max_mailbox_seq: None,
                models_dir: None,
                effective_cwd: None,
            })
            .unwrap();

        let metadata = db
            .wake_session_reader()
            .session_metadata("session-a")
            .unwrap()
            .unwrap();
        let projection = db
            .wake_session_reader()
            .legacy_runtime_projection("session-a")
            .unwrap()
            .unwrap();
        assert_eq!(
            metadata.invocation_uuid.as_deref(),
            Some("owner-invocation")
        );
        assert_eq!(
            projection.running_invocation_uuid.as_deref(),
            Some("wake-invocation")
        );
        assert!(
            db.wake_sessions()
                .settle_legacy_runtime_projection(LegacyRuntimeProjectionSettlement {
                    session_id: "session-a",
                    invocation_uuid: "wake-invocation",
                    last_exit_code: Some(0),
                })
                .unwrap()
        );
    }

    #[test]
    fn runtime_mark_running_records_pty_control_path_without_schema_change() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let identity = current_identity();

        db.wake_sessions()
            .project_legacy_runtime_running(LegacyRuntimeProjection {
                session_id: "session-a",
                mode: "pty_interactive",
                invocation_uuid: "invocation-a",
                provider_name: Some("provider-a"),
                model_name: Some("model-a"),
                identity: &identity,
                pty_control_path: Some("/tmp/oulipoly-a.sock"),
                turn_start_max_mailbox_seq: None,
                models_dir: None,
                effective_cwd: None,
            })
            .unwrap();

        let metadata = db
            .wake_session_reader()
            .session_metadata("session-a")
            .unwrap()
            .unwrap();
        let projection = db
            .wake_session_reader()
            .legacy_runtime_projection("session-a")
            .unwrap()
            .unwrap();
        assert_eq!(metadata.mode, "pty_interactive");
        assert_eq!(
            projection.pty_control_path.as_deref(),
            Some("/tmp/oulipoly-a.sock")
        );
    }

    #[test]
    fn runtime_mark_idle_is_invocation_guarded() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let identity = current_identity();

        db.wake_sessions()
            .project_legacy_runtime_running(LegacyRuntimeProjection {
                session_id: "session-a",
                mode: "headless",
                invocation_uuid: "new-invocation",
                provider_name: Some("provider-a"),
                model_name: Some("model-a"),
                identity: &identity,
                pty_control_path: Some("/tmp/oulipoly-test.sock"),
                turn_start_max_mailbox_seq: None,
                models_dir: None,
                effective_cwd: None,
            })
            .unwrap();

        assert!(
            !db.wake_sessions()
                .settle_legacy_runtime_projection(LegacyRuntimeProjectionSettlement {
                    session_id: "session-a",
                    invocation_uuid: "old-invocation",
                    last_exit_code: Some(0),
                })
                .unwrap()
        );
        assert_eq!(
            db.wake_session_reader()
                .legacy_runtime_projection("session-a")
                .unwrap()
                .unwrap()
                .run_state,
            "running"
        );

        assert!(
            db.wake_sessions()
                .settle_legacy_runtime_projection(LegacyRuntimeProjectionSettlement {
                    session_id: "session-a",
                    invocation_uuid: "new-invocation",
                    last_exit_code: Some(0),
                })
                .unwrap()
        );
        let row = db
            .wake_session_reader()
            .legacy_runtime_projection("session-a")
            .unwrap()
            .unwrap();
        assert_eq!(row.run_state, "idle");
        assert_eq!(row.last_exit_code, Some(0));
        assert!(row.turn_ended_at.is_some());
        assert!(row.running_invocation_uuid.is_none());
        assert!(row.running_os_pid.is_none());
        assert!(row.pty_control_path.is_none());
    }

    #[test]
    fn liveness_live_matching_identity_is_busy() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let identity = current_identity();
        let generation_id =
            RuntimeGenerationId::parse("91111111-1111-4111-8111-111111111111").unwrap();
        db.runtime_lifecycle()
            .create_runtime_generation(CreateRuntimeGeneration {
                generation_id: &generation_id,
                spawn_invocation_uuid: "invocation-a",
                session_id: Some("session-a"),
                runtime_mode: "headless",
                provider_name: "provider-a",
                model_name: None,
                pty_control_path: None,
                models_dir: None,
                effective_cwd: None,
            })
            .unwrap();
        db.runtime_lifecycle()
            .bind_runtime_generation_running(BindRuntimeGenerationRunning {
                fence: RuntimeGenerationFence {
                    generation_id: &generation_id,
                    spawn_invocation_uuid: "invocation-a",
                },
                spawned_os_pid: identity.os_pid,
                exact_process_identity: &identity,
                os_pgid: None,
            })
            .unwrap();

        assert_eq!(
            db.runtime_lifecycle()
                .reconcile_session_liveness("session-a")
                .unwrap(),
            SessionLiveness::Busy
        );
        assert_eq!(
            db.wake_session_reader()
                .legacy_runtime_projection("session-a")
                .unwrap()
                .unwrap()
                .run_state,
            "running"
        );
    }

    #[test]
    fn liveness_dead_or_reused_identity_is_idle_and_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let mut identity = current_identity();
        identity.os_pid_starttime_ticks += 1;
        let generation_id =
            RuntimeGenerationId::parse("92222222-2222-4222-8222-222222222222").unwrap();
        db.runtime_lifecycle()
            .create_runtime_generation(CreateRuntimeGeneration {
                generation_id: &generation_id,
                spawn_invocation_uuid: "invocation-a",
                session_id: Some("session-a"),
                runtime_mode: "headless",
                provider_name: "provider-a",
                model_name: None,
                pty_control_path: Some("/tmp/stale.sock"),
                models_dir: None,
                effective_cwd: None,
            })
            .unwrap();
        db.runtime_lifecycle()
            .bind_runtime_generation_running(BindRuntimeGenerationRunning {
                fence: RuntimeGenerationFence {
                    generation_id: &generation_id,
                    spawn_invocation_uuid: "invocation-a",
                },
                spawned_os_pid: identity.os_pid,
                exact_process_identity: &identity,
                os_pgid: None,
            })
            .unwrap();

        assert_eq!(
            db.runtime_lifecycle()
                .reconcile_session_liveness("session-a")
                .unwrap(),
            SessionLiveness::Idle
        );
        let row = db
            .wake_session_reader()
            .legacy_runtime_projection("session-a")
            .unwrap()
            .unwrap();
        assert_eq!(row.run_state, "idle");
        assert!(row.running_invocation_uuid.is_none());
        assert!(row.running_os_pid.is_none());
        assert!(row.pty_control_path.is_none());
    }

    #[test]
    fn draining_reused_child_reconciles_once_and_preserves_drain_audit_identity() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let mut identity = current_identity();
        identity.os_pid_starttime_ticks += 1;
        let generation_id =
            RuntimeGenerationId::parse("93333333-3333-4333-8333-333333333333").unwrap();
        let drain_request_id =
            DrainRequestId::parse("94444444-4444-4444-8444-444444444444").unwrap();
        let fence = RuntimeGenerationFence {
            generation_id: &generation_id,
            spawn_invocation_uuid: "invocation-draining-stale",
        };
        db.runtime_lifecycle()
            .create_runtime_generation(CreateRuntimeGeneration {
                generation_id: &generation_id,
                spawn_invocation_uuid: fence.spawn_invocation_uuid,
                session_id: Some("session-draining-stale"),
                runtime_mode: "headless",
                provider_name: "provider-a",
                model_name: None,
                pty_control_path: None,
                models_dir: None,
                effective_cwd: None,
            })
            .unwrap();
        db.runtime_lifecycle()
            .bind_runtime_generation_running(BindRuntimeGenerationRunning {
                fence,
                spawned_os_pid: identity.os_pid,
                exact_process_identity: &identity,
                os_pgid: None,
            })
            .unwrap();
        db.runtime_lifecycle()
            .request_runtime_generation_drain(RequestRuntimeGenerationDrain {
                fence,
                drain_request_id: &drain_request_id,
                requested_by_invocation_uuid: "orderly-owner",
            })
            .unwrap();
        db.runtime_lifecycle()
            .advance_runtime_generation_drain(AdvanceRuntimeGenerationDrain {
                fence,
                drain_request_id: &drain_request_id,
            })
            .unwrap();

        assert_eq!(
            db.runtime_lifecycle()
                .reconcile_session_liveness("session-draining-stale")
                .unwrap(),
            SessionLiveness::Idle
        );
        assert_eq!(
            db.runtime_lifecycle()
                .reconcile_session_liveness("session-draining-stale")
                .unwrap(),
            SessionLiveness::Idle
        );
        let recovered = db
            .runtime_lifecycle_reader()
            .runtime_generation(&generation_id)
            .unwrap()
            .unwrap();
        assert_eq!(recovered.lifecycle_state, RuntimeLifecycleState::Exited);
        assert_eq!(
            recovered.terminal_reason,
            Some(RuntimeTerminalReason::RecoveredDead)
        );
        assert_eq!(recovered.drain_request_id, Some(drain_request_id));
        assert!(recovered.drain_requested_at.is_some());
        assert_eq!(
            recovered.drain_requested_by_invocation_uuid.as_deref(),
            Some("orderly-owner")
        );
        assert!(recovered.draining_at.is_some());
    }

    #[test]
    fn draining_live_child_and_foreign_fence_remain_nonterminal() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let identity = current_identity();
        let generation_id =
            RuntimeGenerationId::parse("95555555-5555-4555-8555-555555555555").unwrap();
        let drain_request_id =
            DrainRequestId::parse("96666666-6666-4666-8666-666666666666").unwrap();
        let fence = RuntimeGenerationFence {
            generation_id: &generation_id,
            spawn_invocation_uuid: "invocation-draining-live",
        };
        db.runtime_lifecycle()
            .create_runtime_generation(CreateRuntimeGeneration {
                generation_id: &generation_id,
                spawn_invocation_uuid: fence.spawn_invocation_uuid,
                session_id: Some("session-draining-live"),
                runtime_mode: "headless",
                provider_name: "provider-a",
                model_name: None,
                pty_control_path: None,
                models_dir: None,
                effective_cwd: None,
            })
            .unwrap();
        db.runtime_lifecycle()
            .bind_runtime_generation_running(BindRuntimeGenerationRunning {
                fence,
                spawned_os_pid: identity.os_pid,
                exact_process_identity: &identity,
                os_pgid: None,
            })
            .unwrap();
        db.runtime_lifecycle()
            .request_runtime_generation_drain(RequestRuntimeGenerationDrain {
                fence,
                drain_request_id: &drain_request_id,
                requested_by_invocation_uuid: "orderly-owner",
            })
            .unwrap();
        db.runtime_lifecycle()
            .advance_runtime_generation_drain(AdvanceRuntimeGenerationDrain {
                fence,
                drain_request_id: &drain_request_id,
            })
            .unwrap();

        assert_eq!(
            db.runtime_lifecycle()
                .reconcile_session_liveness("session-draining-live")
                .unwrap(),
            SessionLiveness::Busy
        );
        assert_eq!(
            db.runtime_lifecycle()
                .exit_runtime_generation_non_orderly(ExitRuntimeGenerationNonOrderly {
                    fence: RuntimeGenerationFence {
                        generation_id: &generation_id,
                        spawn_invocation_uuid: "foreign-invocation",
                    },
                    reason: RuntimeTerminalReason::RecoveredDead,
                    exit_code: None,
                })
                .unwrap(),
            GenerationMutation::Rejected(GenerationRejection::FenceMismatch)
        );
        assert_eq!(
            db.runtime_lifecycle()
                .exit_runtime_generation_non_orderly(ExitRuntimeGenerationNonOrderly {
                    fence,
                    reason: RuntimeTerminalReason::RecoveredDead,
                    exit_code: None,
                })
                .unwrap(),
            GenerationMutation::Rejected(GenerationRejection::ProcessIdentityConflict)
        );
        assert!(matches!(
            db.runtime_lifecycle()
                .exit_runtime_generation_non_orderly(ExitRuntimeGenerationNonOrderly {
                    fence,
                    reason: RuntimeTerminalReason::AbnormalTermination,
                    exit_code: None,
                })
                .unwrap(),
            GenerationMutation::Rejected(GenerationRejection::IllegalPredecessor {
                actual: RuntimeLifecycleState::Draining,
                ..
            })
        ));
        assert_eq!(
            db.runtime_lifecycle_reader()
                .runtime_generation(&generation_id)
                .unwrap()
                .unwrap()
                .lifecycle_state,
            RuntimeLifecycleState::Draining
        );
    }

    #[test]
    fn wake_idle_pending_acquires_claim() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        db.enqueue_agent_bash_complete(&input("handle-a", "session-a"))
            .unwrap();
        db.enqueue_agent_bash_complete(&input("handle-b", "session-a"))
            .unwrap();

        let result = db
            .wake_sessions()
            .try_acquire_wake_claim(WakeClaimRequest {
                session_id: "session-a",
                claim_token: "token-a",
                reason: "notify_idle",
                auto_wake_count: 1,
                wake_invocation_uuid: Some("wake-a"),
                stale_after_seconds: 600,
            })
            .unwrap();

        let WakeClaimAcquireResult::Acquired(claim) = result else {
            panic!("expected acquired claim, got {result:?}");
        };
        assert_eq!(claim.session_id, "session-a");
        assert_eq!(claim.claim_token, "token-a");
        assert_eq!(claim.reason, "notify_idle");
        assert_eq!(claim.auto_wake_count, 1);
        assert_eq!(claim.min_pending_seq_at_claim, Some(1));
        assert_eq!(claim.max_pending_seq_at_claim, Some(2));
    }

    #[test]
    fn wake_startable_claim_rechecks_runtime_generation_authority() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        db.enqueue_agent_bash_complete(&input("handle-a", "session-a"))
            .unwrap();
        let generation_id =
            RuntimeGenerationId::parse("91111111-1111-4111-8111-111111111111").unwrap();
        db.runtime_lifecycle()
            .create_runtime_generation(CreateRuntimeGeneration {
                generation_id: &generation_id,
                spawn_invocation_uuid: "runtime-invocation",
                session_id: Some("session-a"),
                runtime_mode: "headless",
                provider_name: "provider-a",
                model_name: Some("model-a"),
                pty_control_path: None,
                models_dir: None,
                effective_cwd: None,
            })
            .unwrap();

        let result = db
            .wake_sessions()
            .try_acquire_startable_wake_claim(
                WakeClaimRequest {
                    session_id: "session-a",
                    claim_token: "token-a",
                    reason: "notify_idle",
                    auto_wake_count: 1,
                    wake_invocation_uuid: Some("wake-a"),
                    stale_after_seconds: 600,
                },
                None,
            )
            .unwrap();

        assert!(matches!(result, WakeClaimAcquireResult::Busy));
        assert!(
            db.wake_session_reader()
                .wake_claim("session-a")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn wake_startable_claim_rechecks_notification_pause() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        db.enqueue_agent_bash_complete(&input("handle-a", "session-a"))
            .unwrap();
        db.set_notifications_paused("session-a", true).unwrap();

        let result = db
            .wake_sessions()
            .try_acquire_startable_wake_claim(
                WakeClaimRequest {
                    session_id: "session-a",
                    claim_token: "token-a",
                    reason: "notify_idle",
                    auto_wake_count: 1,
                    wake_invocation_uuid: Some("wake-a"),
                    stale_after_seconds: 600,
                },
                None,
            )
            .unwrap();

        assert!(matches!(result, WakeClaimAcquireResult::NoPending));
        assert!(
            db.wake_session_reader()
                .wake_claim("session-a")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn wake_startable_claim_does_not_apply_a_retry_budget() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        db.enqueue_agent_bash_complete(&input("handle-a", "session-a"))
            .unwrap();
        let first = db
            .wake_sessions()
            .try_acquire_startable_wake_claim(
                WakeClaimRequest {
                    session_id: "session-a",
                    claim_token: "token-a",
                    reason: "notify_idle",
                    auto_wake_count: 3,
                    wake_invocation_uuid: None,
                    stale_after_seconds: 600,
                },
                None,
            )
            .unwrap();
        assert!(matches!(first, WakeClaimAcquireResult::Acquired(_)));
        assert!(
            db.wake_sessions()
                .release_wake_claim("session-a", "token-a")
                .unwrap()
        );

        let result = db
            .wake_sessions()
            .try_acquire_startable_wake_claim(
                WakeClaimRequest {
                    session_id: "session-a",
                    claim_token: "token-b",
                    reason: "notify_idle",
                    auto_wake_count: 4,
                    wake_invocation_uuid: Some("wake-b"),
                    stale_after_seconds: 600,
                },
                None,
            )
            .unwrap();

        let WakeClaimAcquireResult::Acquired(claim) = result else {
            panic!("retry count must not act as a terminal budget: {result:?}");
        };
        assert_eq!(claim.auto_wake_count, 4);
    }

    #[test]
    fn wake_claim_count_persists_on_session_runtime_after_claim_release() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        db.wake_sessions()
            .upsert_session_metadata(SessionMetadataUpsert {
                session_id: "session-a",
                mode: "headless",
                invocation_uuid: None,
                provider_name: Some("provider-a"),
                model_name: Some("model-a"),
                models_dir: Some("/tmp/models"),
                effective_cwd: None,
            })
            .unwrap();
        db.enqueue_agent_bash_complete(&input("handle-a", "session-a"))
            .unwrap();

        let result = db
            .wake_sessions()
            .try_acquire_wake_claim(WakeClaimRequest {
                session_id: "session-a",
                claim_token: "token-a",
                reason: "notify_idle",
                auto_wake_count: 5,
                wake_invocation_uuid: None,
                stale_after_seconds: 600,
            })
            .unwrap();

        assert!(matches!(result, WakeClaimAcquireResult::Acquired(_)));
        assert_eq!(
            db.wake_session_reader()
                .session_metadata("session-a")
                .unwrap()
                .unwrap()
                .auto_wake_count,
            5
        );
        db.wake_sessions()
            .release_wake_claim("session-a", "token-a")
            .unwrap();
        let candidates = db.wake_sessions().wake_sweep_candidates(600, 10).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].auto_wake_count, 6);
    }

    #[test]
    fn wake_claim_release_requires_the_exact_current_token() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        db.enqueue_agent_bash_complete(&input("handle-a", "session-a"))
            .unwrap();
        db.wake_sessions()
            .try_acquire_wake_claim(WakeClaimRequest {
                session_id: "session-a",
                claim_token: "token-a",
                reason: "notify_idle",
                auto_wake_count: 1,
                wake_invocation_uuid: None,
                stale_after_seconds: 600,
            })
            .unwrap();

        assert!(
            !db.wake_sessions()
                .release_wake_claim("session-a", "token-b")
                .unwrap()
        );
        assert_eq!(
            db.wake_session_reader()
                .wake_claim("session-a")
                .unwrap()
                .unwrap()
                .claim_token,
            "token-a"
        );
        assert!(
            db.wake_sessions()
                .release_wake_claim("session-a", "token-a")
                .unwrap()
        );
        assert!(
            db.wake_session_reader()
                .wake_claim("session-a")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn admitted_wake_child_blocks_manual_claim_release() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        db.enqueue_agent_bash_complete(&input("handle-a", "session-a"))
            .unwrap();
        db.wake_sessions()
            .try_acquire_wake_claim(WakeClaimRequest {
                session_id: "session-a",
                claim_token: "token-a",
                reason: "notify_idle",
                auto_wake_count: 1,
                wake_invocation_uuid: None,
                stale_after_seconds: 600,
            })
            .unwrap();
        let admitted_identity = current_identity();

        assert!(
            !db.wake_sessions()
                .validate_wake_claim_for_child("session-a", "token-b", &admitted_identity)
                .unwrap()
        );
        assert!(
            db.wake_sessions()
                .validate_wake_claim_for_child("session-a", "token-a", &admitted_identity)
                .unwrap()
        );
        assert!(
            db.wake_sessions()
                .validate_wake_claim_for_child("session-a", "token-a", &admitted_identity)
                .unwrap(),
            "the exact admitted child must retain idempotent replay authority"
        );
        let mut foreign_child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("mailbox::tests::wake_claim_foreign_child_fixture")
            .arg("--nocapture")
            .env(WAKE_CLAIM_FOREIGN_CHILD_FIXTURE, "1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let foreign_identity =
            pid_identity::read_live_process_identity(i64::from(foreign_child.id()))
                .unwrap()
                .unwrap();
        let foreign_replay = db
            .wake_sessions()
            .validate_wake_claim_for_child("session-a", "token-a", &foreign_identity)
            .unwrap();
        foreign_child.kill().unwrap();
        foreign_child.wait().unwrap();
        assert!(
            !foreign_replay,
            "a different process identity must not replay an admitted wake token"
        );
        assert!(
            db.wake_session_reader()
                .wake_claim("session-a")
                .unwrap()
                .unwrap()
                .wake_invocation_uuid
                .is_some(),
            "child validation must durably reserve the wake claim"
        );
        assert_eq!(
            db.wake_session_reader()
                .wake_claim("session-a")
                .unwrap()
                .unwrap()
                .wake_pid,
            Some(i64::from(std::process::id())),
            "child admission must durably bind its own process identity"
        );
        assert!(
            !db.wake_sessions()
                .record_wake_claim_pid_identity("session-a", "token-a", i64::MAX)
                .unwrap(),
            "a failed parent observation must not overwrite child-owned identity"
        );
        assert_eq!(
            db.wake_session_reader()
                .wake_claim("session-a")
                .unwrap()
                .unwrap()
                .wake_pid,
            Some(i64::from(std::process::id()))
        );
        assert!(
            !db.wake_sessions()
                .release_wake_claim("session-a", "token-a")
                .unwrap(),
            "manual release must not erase a child-admitted wake claim"
        );
        assert!(
            !db.wake_sessions()
                .release_wake_claim_for_manual_resume("session-a", "token-a")
                .unwrap(),
            "manual resume must not erase a live child-admitted wake claim"
        );
        assert!(
            db.wake_sessions()
                .release_admitted_wake_claim("session-a", "token-a")
                .unwrap(),
            "the admitted child must retain exact-token cleanup authority"
        );
    }

    #[test]
    fn manual_resume_releases_an_exact_dead_admitted_wake_claim() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        db.enqueue_agent_bash_complete(&input("handle-a", "session-a"))
            .unwrap();
        db.wake_sessions()
            .try_acquire_wake_claim(WakeClaimRequest {
                session_id: "session-a",
                claim_token: "token-a",
                reason: "notify_idle",
                auto_wake_count: 1,
                wake_invocation_uuid: Some("wake-a"),
                stale_after_seconds: 600,
            })
            .unwrap();
        db.wake_sessions()
            .record_wake_claim_pid("session-a", "token-a", i64::MAX)
            .unwrap();

        assert!(
            db.wake_sessions()
                .release_wake_claim_for_manual_resume("session-a", "token-a")
                .unwrap()
        );
        assert!(
            db.wake_session_reader()
                .wake_claim("session-a")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn manual_resume_releases_an_exact_pid_reused_admitted_wake_claim() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        db.enqueue_agent_bash_complete(&input("handle-a", "session-a"))
            .unwrap();
        db.wake_sessions()
            .try_acquire_wake_claim(WakeClaimRequest {
                session_id: "session-a",
                claim_token: "token-a",
                reason: "notify_idle",
                auto_wake_count: 1,
                wake_invocation_uuid: Some("wake-a"),
                stale_after_seconds: 600,
            })
            .unwrap();
        let identity = current_identity();
        db.wake_sessions()
            .record_wake_claim_pid_identity("session-a", "token-a", identity.os_pid)
            .unwrap();
        db.conn
            .execute(
                "UPDATE session_wake_claim
                 SET wake_os_pid_starttime_ticks = ?2
                 WHERE session_id = ?1",
                params!["session-a", identity.os_pid_starttime_ticks + 1],
            )
            .unwrap();

        assert!(
            db.wake_sessions()
                .release_wake_claim_for_manual_resume("session-a", "token-a")
                .unwrap()
        );
        assert!(
            db.wake_session_reader()
                .wake_claim("session-a")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn manual_resume_retains_an_admitted_claim_without_process_identity() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        db.enqueue_agent_bash_complete(&input("handle-a", "session-a"))
            .unwrap();
        db.wake_sessions()
            .try_acquire_wake_claim(WakeClaimRequest {
                session_id: "session-a",
                claim_token: "token-a",
                reason: "notify_idle",
                auto_wake_count: 1,
                wake_invocation_uuid: Some("wake-a"),
                stale_after_seconds: 600,
            })
            .unwrap();

        assert!(
            !db.wake_sessions()
                .release_wake_claim_for_manual_resume("session-a", "token-a")
                .unwrap()
        );
        assert_eq!(
            db.wake_session_reader()
                .wake_claim("session-a")
                .unwrap()
                .unwrap()
                .claim_token,
            "token-a"
        );
    }

    #[test]
    fn wake_child_validation_rejects_claim_when_runtime_authority_is_busy() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        db.enqueue_agent_bash_complete(&input("handle-a", "session-a"))
            .unwrap();
        db.wake_sessions()
            .try_acquire_wake_claim(WakeClaimRequest {
                session_id: "session-a",
                claim_token: "token-a",
                reason: "notify_idle",
                auto_wake_count: 1,
                wake_invocation_uuid: None,
                stale_after_seconds: 600,
            })
            .unwrap();
        let generation_id =
            RuntimeGenerationId::parse("92222222-2222-4222-8222-222222222222").unwrap();
        db.runtime_lifecycle()
            .create_runtime_generation(CreateRuntimeGeneration {
                generation_id: &generation_id,
                spawn_invocation_uuid: "runtime-invocation",
                session_id: Some("session-a"),
                runtime_mode: "headless",
                provider_name: "provider-a",
                model_name: Some("model-a"),
                pty_control_path: None,
                models_dir: None,
                effective_cwd: None,
            })
            .unwrap();

        assert!(
            !db.wake_sessions()
                .validate_wake_claim_for_child("session-a", "token-a", &current_identity())
                .unwrap()
        );
        assert!(
            db.wake_session_reader()
                .wake_claim("session-a")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn wake_claim_ignores_legacy_runtime_projection_after_v2() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let identity = current_identity();
        db.enqueue_agent_bash_complete(&input("handle-a", "session-a"))
            .unwrap();
        db.wake_sessions()
            .project_legacy_runtime_running(LegacyRuntimeProjection {
                session_id: "session-a",
                mode: "headless",
                invocation_uuid: "invocation-a",
                provider_name: None,
                model_name: None,
                identity: &identity,
                pty_control_path: None,
                turn_start_max_mailbox_seq: None,
                models_dir: None,
                effective_cwd: None,
            })
            .unwrap();

        assert!(matches!(
            db.wake_sessions()
                .try_acquire_wake_claim(WakeClaimRequest {
                    session_id: "session-a",
                    claim_token: "token-a",
                    reason: "notify_idle",
                    auto_wake_count: 1,
                    wake_invocation_uuid: None,
                    stale_after_seconds: 600,
                })
                .unwrap(),
            WakeClaimAcquireResult::Acquired(_)
        ));
        assert!(
            db.wake_session_reader()
                .wake_claim("session-a")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn wake_existing_claim_is_single_flight() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        db.enqueue_agent_bash_complete(&input("handle-a", "session-a"))
            .unwrap();
        let first = db
            .wake_sessions()
            .try_acquire_wake_claim(WakeClaimRequest {
                session_id: "session-a",
                claim_token: "token-a",
                reason: "notify_idle",
                auto_wake_count: 1,
                wake_invocation_uuid: None,
                stale_after_seconds: 600,
            })
            .unwrap();
        assert!(matches!(first, WakeClaimAcquireResult::Acquired(_)));

        let second = db
            .wake_sessions()
            .try_acquire_wake_claim(WakeClaimRequest {
                session_id: "session-a",
                claim_token: "token-b",
                reason: "notify_idle",
                auto_wake_count: 1,
                wake_invocation_uuid: None,
                stale_after_seconds: 600,
            })
            .unwrap();

        let WakeClaimAcquireResult::AlreadyInFlight(claim) = second else {
            panic!("expected already in flight, got {second:?}");
        };
        assert_eq!(claim.claim_token, "token-a");
    }

    #[test]
    fn concurrent_wake_claim_attempts_have_one_exact_token_winner() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let mut db = MailboxDb::open(&path).unwrap();
        db.enqueue_agent_bash_complete(&input("handle-a", "session-a"))
            .unwrap();
        drop(db);

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let handles = ["token-a", "token-b"].map(|token| {
            let path = path.clone();
            let barrier = std::sync::Arc::clone(&barrier);
            std::thread::spawn(move || {
                let mut db = MailboxDb::open(&path).unwrap();
                barrier.wait();
                db.wake_sessions()
                    .try_acquire_wake_claim(WakeClaimRequest {
                        session_id: "session-a",
                        claim_token: token,
                        reason: "notify_idle",
                        auto_wake_count: 1,
                        wake_invocation_uuid: None,
                        stale_after_seconds: 600,
                    })
                    .unwrap()
            })
        });
        barrier.wait();
        let results = handles.map(|handle| handle.join().unwrap());

        let acquired = results
            .iter()
            .filter_map(|result| match result {
                WakeClaimAcquireResult::Acquired(claim) => Some(claim.claim_token.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(acquired.len(), 1, "{results:?}");
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, WakeClaimAcquireResult::AlreadyInFlight(_)))
                .count(),
            1,
            "{results:?}"
        );
        assert_eq!(
            MailboxDb::open(&path)
                .unwrap()
                .wake_session_reader()
                .wake_claim("session-a")
                .unwrap()
                .unwrap()
                .claim_token,
            acquired[0]
        );
    }

    #[test]
    fn wake_stale_claim_can_be_stolen() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        db.enqueue_agent_bash_complete(&input("handle-a", "session-a"))
            .unwrap();
        assert!(matches!(
            db.wake_sessions()
                .try_acquire_wake_claim(WakeClaimRequest {
                    session_id: "session-a",
                    claim_token: "token-a",
                    reason: "notify_idle",
                    auto_wake_count: 1,
                    wake_invocation_uuid: None,
                    stale_after_seconds: 600,
                })
                .unwrap(),
            WakeClaimAcquireResult::Acquired(_)
        ));
        db.wake_sessions()
            .force_wake_claim_age_for_test("session-a", 601)
            .unwrap();

        let stolen = db
            .wake_sessions()
            .try_acquire_wake_claim(WakeClaimRequest {
                session_id: "session-a",
                claim_token: "token-b",
                reason: "turn_end_recheck",
                auto_wake_count: 2,
                wake_invocation_uuid: None,
                stale_after_seconds: 600,
            })
            .unwrap();

        let WakeClaimAcquireResult::Acquired(claim) = stolen else {
            panic!("expected stolen claim, got {stolen:?}");
        };
        assert_eq!(claim.claim_token, "token-b");
        assert_eq!(claim.reason, "turn_end_recheck");
        assert_eq!(claim.auto_wake_count, 2);
    }

    #[test]
    fn wake_dead_pid_claim_can_be_stolen_before_ttl() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        db.enqueue_agent_bash_complete(&input("handle-a", "session-a"))
            .unwrap();
        assert!(matches!(
            db.wake_sessions()
                .try_acquire_wake_claim(WakeClaimRequest {
                    session_id: "session-a",
                    claim_token: "token-a",
                    reason: "notify_idle",
                    auto_wake_count: 1,
                    wake_invocation_uuid: None,
                    stale_after_seconds: 600,
                })
                .unwrap(),
            WakeClaimAcquireResult::Acquired(_)
        ));
        db.wake_sessions()
            .record_wake_claim_pid("session-a", "token-a", 999_999_999)
            .unwrap();

        let stolen = db
            .wake_sessions()
            .try_acquire_wake_claim(WakeClaimRequest {
                session_id: "session-a",
                claim_token: "token-b",
                reason: "wake_reclaim_sweep",
                auto_wake_count: 2,
                wake_invocation_uuid: None,
                stale_after_seconds: 600,
            })
            .unwrap();

        let WakeClaimAcquireResult::Acquired(claim) = stolen else {
            panic!("expected dead PID claim to be stolen, got {stolen:?}");
        };
        assert_eq!(claim.claim_token, "token-b");
    }

    #[test]
    fn wake_live_identity_matched_claim_is_not_stolen_after_ttl() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        db.enqueue_agent_bash_complete(&input("handle-a", "session-a"))
            .unwrap();
        assert!(matches!(
            db.wake_sessions()
                .try_acquire_wake_claim(WakeClaimRequest {
                    session_id: "session-a",
                    claim_token: "token-a",
                    reason: "notify_idle",
                    auto_wake_count: 1,
                    wake_invocation_uuid: None,
                    stale_after_seconds: 600,
                })
                .unwrap(),
            WakeClaimAcquireResult::Acquired(_)
        ));
        let identity = current_identity();
        db.wake_sessions()
            .record_wake_claim_pid_identity("session-a", "token-a", identity.os_pid)
            .unwrap();
        let sidecar = pid_identity::PidIdentityDb::open(db.path()).unwrap();
        assert!(sidecar.lookup_by_identity(&identity).unwrap().is_none());
        db.wake_sessions()
            .force_wake_claim_age_for_test("session-a", 601)
            .unwrap();

        let result = db
            .wake_sessions()
            .try_acquire_wake_claim(WakeClaimRequest {
                session_id: "session-a",
                claim_token: "token-b",
                reason: "wake_reclaim_sweep",
                auto_wake_count: 2,
                wake_invocation_uuid: None,
                stale_after_seconds: 600,
            })
            .unwrap();

        let WakeClaimAcquireResult::AlreadyInFlight(claim) = result else {
            panic!("expected live identity-matched claim to remain in flight, got {result:?}");
        };
        assert_eq!(claim.claim_token, "token-a");
    }

    #[test]
    fn runtime_generations_preserve_overlap_exact_lookup_and_late_session_attachment() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let first_id = RuntimeGenerationId::parse("11111111-1111-4111-8111-111111111111").unwrap();
        let second_id = RuntimeGenerationId::parse("22222222-2222-4222-8222-222222222222").unwrap();
        let first_identity = ProcessIdentity {
            os_pid: 101,
            os_boot_id: "boot-a".to_string(),
            os_pid_starttime_ticks: 1001,
        };
        let second_identity = ProcessIdentity {
            os_pid: 102,
            os_boot_id: "boot-a".to_string(),
            os_pid_starttime_ticks: 1002,
        };

        for (generation_id, invocation_uuid, session_id) in [
            (&first_id, "invocation-a", None),
            (&second_id, "invocation-b", Some("session-a")),
        ] {
            assert!(matches!(
                db.runtime_lifecycle()
                    .create_runtime_generation(CreateRuntimeGeneration {
                        generation_id,
                        spawn_invocation_uuid: invocation_uuid,
                        session_id,
                        runtime_mode: "headless",
                        provider_name: "provider-a",
                        model_name: Some("model-a"),
                        pty_control_path: None,
                        models_dir: None,
                        effective_cwd: None,
                    })
                    .unwrap(),
                GenerationMutation::Applied(_)
            ));
        }
        for (generation_id, invocation_uuid, identity) in [
            (&first_id, "invocation-a", &first_identity),
            (&second_id, "invocation-b", &second_identity),
        ] {
            assert!(matches!(
                db.runtime_lifecycle()
                    .bind_runtime_generation_running(BindRuntimeGenerationRunning {
                        fence: RuntimeGenerationFence {
                            generation_id,
                            spawn_invocation_uuid: invocation_uuid,
                        },
                        spawned_os_pid: identity.os_pid,
                        exact_process_identity: identity,
                        os_pgid: None,
                    })
                    .unwrap(),
                GenerationMutation::Applied(_)
            ));
        }
        assert!(matches!(
            db.runtime_lifecycle()
                .attach_runtime_generation_session(AttachRuntimeGenerationSession {
                    fence: RuntimeGenerationFence {
                        generation_id: &first_id,
                        spawn_invocation_uuid: "invocation-a",
                    },
                    session_id: "session-a",
                })
                .unwrap(),
            GenerationMutation::Applied(_)
        ));

        let SessionGenerationProjection::Multiple(rows) = db
            .runtime_lifecycle_reader()
            .session_generation_projection("session-a")
            .unwrap()
        else {
            panic!("overlapping generations must remain explicit");
        };
        assert_eq!(rows.len(), 2);
        assert!(matches!(
            db.runtime_lifecycle_reader().resolve_runtime_generation(RuntimeGenerationSelector::ProcessIdentity(
                &first_identity
            ))
            .unwrap(),
            RuntimeGenerationResolution::Found(row) if row.generation_id == first_id
        ));
    }

    #[test]
    fn generation_drain_is_a_durable_two_step_predecessor_transition() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let generation_id =
            RuntimeGenerationId::parse("33333333-3333-4333-8333-333333333333").unwrap();
        let drain_request_id =
            DrainRequestId::parse("44444444-4444-4444-8444-444444444444").unwrap();
        let identity = ProcessIdentity {
            os_pid: 103,
            os_boot_id: "drain-test-boot".to_string(),
            os_pid_starttime_ticks: 1,
        };
        db.runtime_lifecycle()
            .create_runtime_generation(CreateRuntimeGeneration {
                generation_id: &generation_id,
                spawn_invocation_uuid: "invocation-a",
                session_id: Some("session-a"),
                runtime_mode: "headless",
                provider_name: "provider-a",
                model_name: None,
                pty_control_path: None,
                models_dir: None,
                effective_cwd: None,
            })
            .unwrap();
        db.runtime_lifecycle()
            .bind_runtime_generation_running(BindRuntimeGenerationRunning {
                fence: RuntimeGenerationFence {
                    generation_id: &generation_id,
                    spawn_invocation_uuid: "invocation-a",
                },
                spawned_os_pid: 103,
                exact_process_identity: &identity,
                os_pgid: None,
            })
            .unwrap();
        let fence = RuntimeGenerationFence {
            generation_id: &generation_id,
            spawn_invocation_uuid: "invocation-a",
        };

        assert!(matches!(
            db.runtime_lifecycle()
                .request_runtime_generation_drain(RequestRuntimeGenerationDrain {
                    fence,
                    drain_request_id: &drain_request_id,
                    requested_by_invocation_uuid: "drainer-a",
                })
                .unwrap(),
            DrainRequestResult::Installed(_, DrainHandoff::Ready)
        ));
        assert!(matches!(
            db.runtime_lifecycle()
                .advance_runtime_generation_drain(AdvanceRuntimeGenerationDrain {
                fence,
                drain_request_id: &drain_request_id,
            })
            .unwrap(),
            DrainAdvanceResult::Advanced(ref row)
                if row.lifecycle_state == RuntimeLifecycleState::Draining
        ));
        assert!(matches!(
            db.runtime_lifecycle()
                .finish_runtime_generation_drain(FinishRuntimeGenerationDrain {
                    fence,
                    drain_request_id: &drain_request_id,
                    exit_code: Some(0),
                    compatibility_exit_code: Some(0),
                })
            .unwrap(),
            DrainFinishResult::Finished(ref row)
                if row.lifecycle_state == RuntimeLifecycleState::Exited
                    && row.terminal_reason == Some(RuntimeTerminalReason::OrderlyCompletion)
        ));
    }

    #[test]
    fn delivery_claim_wins_drain_race_then_confirmation_hands_off() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let generation_id =
            RuntimeGenerationId::parse("55555555-5555-4555-8555-555555555555").unwrap();
        let claim_id = DeliveryClaimId::parse("66666666-6666-4666-8666-666666666666").unwrap();
        let drain_request_id =
            DrainRequestId::parse("77777777-7777-4777-8777-777777777777").unwrap();
        let row = inserted_row(db.enqueue_agent_bash_complete(&input("claimed", "session-a")));
        create_running_generation(&mut db, &generation_id, "invocation-a", "session-a");
        let fence = RuntimeGenerationFence {
            generation_id: &generation_id,
            spawn_invocation_uuid: "invocation-a",
        };

        assert!(matches!(
            db.runtime_lifecycle()
                .acquire_runtime_generation_delivery(AcquireRuntimeGenerationDelivery {
                    fence,
                    claim_id: &claim_id,
                    seqs: &[row.seq],
                    stale_after_seconds: 30,
                })
                .unwrap(),
            DeliveryClaimAcquireResult::Acquired(_)
        ));
        assert!(matches!(
            db.runtime_lifecycle()
                .request_runtime_generation_drain(RequestRuntimeGenerationDrain {
                    fence,
                    drain_request_id: &drain_request_id,
                    requested_by_invocation_uuid: "drainer-a",
                })
                .unwrap(),
            DrainRequestResult::Installed(_, DrainHandoff::ClaimOutstanding { .. })
        ));
        assert_eq!(
            db.runtime_lifecycle()
                .advance_runtime_generation_drain(AdvanceRuntimeGenerationDrain {
                    fence,
                    drain_request_id: &drain_request_id,
                })
                .unwrap(),
            DrainAdvanceResult::WaitingOnClaim(claim_id.clone())
        );
        assert!(matches!(
            db.runtime_lifecycle()
                .confirm_runtime_generation_delivery(ConfirmRuntimeGenerationDelivery {
                    fence,
                    claim_id: &claim_id,
                    seqs: &[row.seq],
                    delivered_by_invocation_uuid: "invocation-a",
                })
                .unwrap(),
            GenerationMutation::Applied(_)
        ));
        assert!(matches!(
            db.runtime_lifecycle()
                .advance_runtime_generation_drain(AdvanceRuntimeGenerationDrain {
                    fence,
                    drain_request_id: &drain_request_id,
                })
                .unwrap(),
            DrainAdvanceResult::Advanced(_)
        ));
        assert!(db.list_pending("session-a").unwrap().is_empty());
    }

    #[test]
    fn runtime_generation_delivery_failure_is_session_bound_and_releases_its_claim() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let generation_id =
            RuntimeGenerationId::parse("56565656-5656-4656-8656-565656565656").unwrap();
        let claim_id = DeliveryClaimId::parse("78787878-7878-4878-8878-787878787878").unwrap();
        let row = inserted_row(db.enqueue_agent_bash_complete(&input("claimed", "session-a")));
        create_running_generation(&mut db, &generation_id, "invocation-a", "session-a");
        let fence = RuntimeGenerationFence {
            generation_id: &generation_id,
            spawn_invocation_uuid: "invocation-a",
        };
        db.runtime_lifecycle()
            .acquire_runtime_generation_delivery(AcquireRuntimeGenerationDelivery {
                fence,
                claim_id: &claim_id,
                seqs: &[row.seq],
                stale_after_seconds: 30,
            })
            .unwrap();

        assert!(matches!(
            db.runtime_lifecycle()
                .fail_runtime_generation_delivery(FailRuntimeGenerationDelivery {
                    fence,
                    claim_id: &claim_id,
                    seqs: &[row.seq],
                    delivery_error: MAILBOX_DELIVERY_UNCONFIRMED_ERROR,
                })
                .unwrap(),
            GenerationMutation::Applied(_)
        ));

        let failed = db.list_mailbox("session-a", true).unwrap().remove(0);
        assert_eq!(failed.delivery_attempts, 1);
        assert_eq!(
            failed.delivery_error.as_deref(),
            Some(MAILBOX_DELIVERY_UNCONFIRMED_ERROR)
        );
        assert!(
            db.runtime_lifecycle_reader()
                .runtime_generation(&generation_id)
                .unwrap()
                .unwrap()
                .active_delivery_claim_id
                .is_none()
        );
    }

    #[test]
    fn runtime_generation_delivery_failure_rejects_terminal_wake_abandonment() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let generation_id =
            RuntimeGenerationId::parse("abababab-abab-4bab-8bab-abababababab").unwrap();
        let claim_id = DeliveryClaimId::parse("cdcdcdcd-cdcd-4dcd-8dcd-cdcdcdcdcdcd").unwrap();
        let row = inserted_row(db.enqueue_agent_bash_complete(&input("claimed", "session-a")));
        create_running_generation(&mut db, &generation_id, "invocation-a", "session-a");
        let fence = RuntimeGenerationFence {
            generation_id: &generation_id,
            spawn_invocation_uuid: "invocation-a",
        };
        db.runtime_lifecycle()
            .acquire_runtime_generation_delivery(AcquireRuntimeGenerationDelivery {
                fence,
                claim_id: &claim_id,
                seqs: &[row.seq],
                stale_after_seconds: 30,
            })
            .unwrap();

        let error = db
            .runtime_lifecycle()
            .fail_runtime_generation_delivery(FailRuntimeGenerationDelivery {
                fence,
                claim_id: &claim_id,
                seqs: &[row.seq],
                delivery_error: WAKE_SWEEP_ABANDONED_ERROR,
            })
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("dedicated authority-bearing disposition")
        );
        let unchanged = db.list_mailbox("session-a", true).unwrap().remove(0);
        assert_eq!(unchanged.delivery_attempts, 0);
        assert!(unchanged.delivery_error.is_none());
        let generation = db
            .runtime_lifecycle_reader()
            .runtime_generation(&generation_id)
            .unwrap()
            .unwrap();
        assert_eq!(generation.active_delivery_claim_id, Some(claim_id));
        assert_eq!(generation.active_delivery_seqs, vec![row.seq]);
    }

    #[test]
    fn runtime_generation_delivery_authority_is_session_bound() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let generation_id =
            RuntimeGenerationId::parse("12121212-1212-4212-8212-121212121212").unwrap();
        let claim_id = DeliveryClaimId::parse("34343434-3434-4434-8434-343434343434").unwrap();
        let foreign = inserted_row(db.enqueue_agent_bash_complete(&input("foreign", "session-b")));
        create_running_generation(&mut db, &generation_id, "invocation-a", "session-a");
        let fence = RuntimeGenerationFence {
            generation_id: &generation_id,
            spawn_invocation_uuid: "invocation-a",
        };

        assert_eq!(
            db.runtime_lifecycle()
                .acquire_runtime_generation_delivery(AcquireRuntimeGenerationDelivery {
                    fence,
                    claim_id: &claim_id,
                    seqs: &[foreign.seq],
                    stale_after_seconds: 30,
                })
                .unwrap(),
            DeliveryClaimAcquireResult::Rejected(GenerationRejection::SessionConflict)
        );
        assert!(
            db.runtime_lifecycle_reader()
                .runtime_generation(&generation_id)
                .unwrap()
                .unwrap()
                .active_delivery_claim_id
                .is_none()
        );

        let seqs_json = serde_json::to_string(&[foreign.seq]).unwrap();
        db.connection()
            .execute(
                "UPDATE runtime_generation
                 SET active_delivery_claim_uuid = ?2,
                     active_delivery_claimed_at = '2026-08-18T00:00:00Z',
                     active_delivery_seqs_json = ?3
                 WHERE generation_uuid = ?1",
                params![generation_id.to_string(), claim_id.to_string(), seqs_json],
            )
            .unwrap();

        let confirmation_error = db
            .runtime_lifecycle()
            .confirm_runtime_generation_delivery(ConfirmRuntimeGenerationDelivery {
                fence,
                claim_id: &claim_id,
                seqs: &[foreign.seq],
                delivered_by_invocation_uuid: "invocation-a",
            })
            .unwrap_err();
        assert!(
            confirmation_error
                .to_string()
                .contains("not owned and pending")
        );
        for _ in 0..2 {
            let failure_error = db
                .runtime_lifecycle()
                .fail_runtime_generation_delivery(FailRuntimeGenerationDelivery {
                    fence,
                    claim_id: &claim_id,
                    seqs: &[foreign.seq],
                    delivery_error: MAILBOX_DELIVERY_UNCONFIRMED_ERROR,
                })
                .unwrap_err();
            assert!(failure_error.to_string().contains("not owned and pending"));
        }

        let unchanged = db.list_mailbox("session-b", true).unwrap().remove(0);
        assert!(unchanged.delivered_at.is_none());
        assert_eq!(unchanged.delivery_attempts, 0);
        assert!(unchanged.delivery_error.is_none());
        let generation = db
            .runtime_lifecycle_reader()
            .runtime_generation(&generation_id)
            .unwrap()
            .unwrap();
        assert_eq!(generation.active_delivery_claim_id, Some(claim_id));
        assert_eq!(generation.active_delivery_seqs, vec![foreign.seq]);
    }

    #[test]
    fn drain_request_wins_race_and_rejects_new_delivery_claim() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let generation_id =
            RuntimeGenerationId::parse("88888888-8888-4888-8888-888888888888").unwrap();
        let claim_id = DeliveryClaimId::parse("99999999-9999-4999-8999-999999999999").unwrap();
        let drain_request_id =
            DrainRequestId::parse("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa").unwrap();
        let row = inserted_row(db.enqueue_agent_bash_complete(&input("pending", "session-a")));
        create_running_generation(&mut db, &generation_id, "invocation-a", "session-a");
        let fence = RuntimeGenerationFence {
            generation_id: &generation_id,
            spawn_invocation_uuid: "invocation-a",
        };

        db.runtime_lifecycle()
            .request_runtime_generation_drain(RequestRuntimeGenerationDrain {
                fence,
                drain_request_id: &drain_request_id,
                requested_by_invocation_uuid: "drainer-a",
            })
            .unwrap();
        assert_eq!(
            db.runtime_lifecycle()
                .acquire_runtime_generation_delivery(AcquireRuntimeGenerationDelivery {
                    fence,
                    claim_id: &claim_id,
                    seqs: &[row.seq],
                    stale_after_seconds: 30,
                })
                .unwrap(),
            DeliveryClaimAcquireResult::Rejected(GenerationRejection::DrainRequestConflict)
        );
        assert_eq!(db.list_pending("session-a").unwrap().len(), 1);
    }

    #[test]
    fn stale_delivery_claim_recovery_reuses_nonce_and_batch() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let generation_id =
            RuntimeGenerationId::parse("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb").unwrap();
        let first_claim = DeliveryClaimId::parse("cccccccc-cccc-4ccc-8ccc-cccccccccccc").unwrap();
        let replacement_claim =
            DeliveryClaimId::parse("dddddddd-dddd-4ddd-8ddd-dddddddddddd").unwrap();
        let first = inserted_row(db.enqueue_agent_bash_complete(&input("first", "session-a")));
        create_running_generation(&mut db, &generation_id, "invocation-a", "session-a");
        let fence = RuntimeGenerationFence {
            generation_id: &generation_id,
            spawn_invocation_uuid: "invocation-a",
        };
        db.runtime_lifecycle()
            .acquire_runtime_generation_delivery(AcquireRuntimeGenerationDelivery {
                fence,
                claim_id: &first_claim,
                seqs: &[first.seq],
                stale_after_seconds: 30,
            })
            .unwrap();
        let second = inserted_row(db.enqueue_agent_bash_complete(&input("second", "session-a")));
        db.connection()
            .execute(
                "UPDATE runtime_generation
                 SET active_delivery_claimed_at = '2000-01-01T00:00:00Z'
                 WHERE generation_uuid = ?1",
                params![generation_id.to_string()],
            )
            .unwrap();

        let DeliveryClaimAcquireResult::Recovered(recovered) = db
            .runtime_lifecycle()
            .acquire_runtime_generation_delivery(AcquireRuntimeGenerationDelivery {
                fence,
                claim_id: &replacement_claim,
                seqs: &[first.seq, second.seq],
                stale_after_seconds: 30,
            })
            .unwrap()
        else {
            panic!("expected stale claim recovery");
        };
        assert_eq!(recovered.active_delivery_claim_id, Some(first_claim));
        assert_eq!(recovered.active_delivery_seqs, vec![first.seq]);
    }

    fn create_running_generation(
        db: &mut MailboxDb,
        generation_id: &RuntimeGenerationId,
        invocation_uuid: &str,
        session_id: &str,
    ) {
        let identity = ProcessIdentity {
            os_pid: 103,
            os_boot_id: "delivery-test-boot".to_string(),
            os_pid_starttime_ticks: 1,
        };
        db.runtime_lifecycle()
            .create_runtime_generation(CreateRuntimeGeneration {
                generation_id,
                spawn_invocation_uuid: invocation_uuid,
                session_id: Some(session_id),
                runtime_mode: "pty_interactive",
                provider_name: "provider-a",
                model_name: None,
                pty_control_path: Some("/tmp/control.sock"),
                models_dir: None,
                effective_cwd: None,
            })
            .unwrap();
        db.runtime_lifecycle()
            .bind_runtime_generation_running(BindRuntimeGenerationRunning {
                fence: RuntimeGenerationFence {
                    generation_id,
                    spawn_invocation_uuid: invocation_uuid,
                },
                spawned_os_pid: 103,
                exact_process_identity: &identity,
                os_pgid: None,
            })
            .unwrap();
    }

    #[test]
    fn mailbox_operations_do_not_change_state_db_schema() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("state.db");
        let state = StateDb::open(&state_path).unwrap();
        let baseline_version = user_version(state.raw_connection());
        let baseline_columns = invocation_columns(state.raw_connection());
        drop(state);

        let mut mailbox = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let row =
            inserted_row(mailbox.enqueue_agent_bash_complete(&input("handle-a", "session-a")));
        mailbox
            .mark_delivered("session-a", None, &[row.seq], "resume-1")
            .unwrap();
        let identity = current_identity();
        mailbox
            .wake_sessions()
            .project_legacy_runtime_running(LegacyRuntimeProjection {
                session_id: "session-a",
                mode: "headless",
                invocation_uuid: "invocation-a",
                provider_name: Some("provider-a"),
                model_name: Some("model-a"),
                identity: &identity,
                pty_control_path: None,
                turn_start_max_mailbox_seq: None,
                models_dir: None,
                effective_cwd: None,
            })
            .unwrap();
        mailbox
            .wake_sessions()
            .settle_legacy_runtime_projection(LegacyRuntimeProjectionSettlement {
                session_id: "session-a",
                invocation_uuid: "invocation-a",
                last_exit_code: Some(0),
            })
            .unwrap();
        assert!(matches!(
            mailbox
                .wake_sessions()
                .try_acquire_wake_claim(WakeClaimRequest {
                    session_id: "session-a",
                    claim_token: "token-a",
                    reason: "notify_idle",
                    auto_wake_count: 1,
                    wake_invocation_uuid: None,
                    stale_after_seconds: 600,
                })
                .unwrap(),
            WakeClaimAcquireResult::NoPending
        ));
        drop(mailbox);

        let state = StateDb::open(&state_path).unwrap();
        assert_eq!(user_version(state.raw_connection()), baseline_version);
        assert_eq!(invocation_columns(state.raw_connection()), baseline_columns);
    }

    fn inserted_row(result: Result<EnqueueResult, String>) -> MailboxRow {
        let result = result.unwrap();
        assert_inserted_result(&result);
        inserted_result_row(result)
    }

    fn assert_inserted_result(result: &EnqueueResult) {
        if !matches!(result, EnqueueResult::Inserted(_)) {
            panic!("expected inserted row, got {result:?}");
        }
    }

    fn inserted_result_row(result: EnqueueResult) -> MailboxRow {
        let EnqueueResult::Inserted(row) = result else {
            unreachable!("inserted result validated above");
        };
        row
    }

    fn current_identity() -> ProcessIdentity {
        expect_current_identity(read_current_identity().unwrap())
    }

    fn read_current_identity() -> Result<Option<ProcessIdentity>, String> {
        pid_identity::read_live_process_identity(std::process::id().into())
    }

    fn expect_current_identity(identity: Option<ProcessIdentity>) -> ProcessIdentity {
        identity.expect("test process should have a live identity")
    }

    fn user_version(conn: &Connection) -> i64 {
        conn.query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap()
    }

    fn invocation_columns(conn: &Connection) -> Vec<String> {
        read_invocation_columns(conn).unwrap()
    }

    fn read_invocation_columns(conn: &Connection) -> rusqlite::Result<Vec<String>> {
        let mut stmt = conn.prepare("PRAGMA table_info(invocations)").unwrap();
        stmt.query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
    }
}
