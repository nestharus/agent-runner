//! ## Declared roles
//!
//! `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `parser`,
//! `predicate`, `validator`
//!
//! Resume-backed notification mailbox storage in the PID identity sidecar DB.
//! This module deliberately owns only additive sidecar tables and never touches
//! the versioned `state.db` schema.

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::pid_identity::{self, ProcessIdentity};

pub const AGENT_BASH_COMPLETE_KIND: &str = "agent_bash_complete";
pub const MAILBOX_DELIVERY_UNCONFIRMED_ERROR: &str = "mailbox_delivery_unconfirmed";
pub const MAX_UNCONFIRMED_DELIVERY_ATTEMPTS: i64 = 2;
pub const SUBMITTED_INPUT_KIND: &str = "input";
pub const WAKE_SWEEP_ABANDONED_ERROR: &str = "wake_sweep_abandoned";
pub const MAILBOX_PAYLOAD_RETENTION_POLICY: &str = "until_terminal_disposition";
const COMPACTED_PAYLOAD_SCHEMA_VERSION: u8 = 1;

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
    pub exact_process_identity: Option<&'a ProcessIdentity>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnqueueResult {
    Inserted(MailboxRow),
    AlreadyEnqueued(MailboxRow),
    Conflict { existing: MailboxRow },
}

#[derive(Debug, Clone, Copy)]
pub struct SessionRuntimeUpsert<'a> {
    pub session_id: &'a str,
    pub mode: &'a str,
    pub invocation_uuid: Option<&'a str>,
    pub provider_name: Option<&'a str>,
    pub model_name: Option<&'a str>,
    pub pty_control_path: Option<&'a str>,
    pub models_dir: Option<&'a str>,
    pub effective_cwd: Option<&'a str>,
    pub selected_auto_wake_max: Option<i64>,
}

#[derive(Debug, Clone, Copy)]
pub struct SessionRuntimeRunningUpdate<'a> {
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
pub struct SessionRuntimeIdleUpdate<'a> {
    pub session_id: &'a str,
    pub invocation_uuid: &'a str,
    pub last_exit_code: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRuntimeRow {
    pub session_id: String,
    pub mode: String,
    pub invocation_uuid: Option<String>,
    pub provider_name: Option<String>,
    pub model_name: Option<String>,
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
    pub models_dir: Option<String>,
    pub effective_cwd: Option<String>,
    pub auto_wake_count: i64,
    pub selected_auto_wake_max: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionLiveness {
    Busy,
    Idle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionRuntimeReadOnlyLiveness {
    Busy,
    Idle,
    StaleMissingInvocation,
    StaleMissingIdentity,
    StaleDead,
    StalePidReused,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SessionRuntimeLivenessDecision {
    Busy,
    Idle,
    Stale {
        running_invocation_uuid: Option<String>,
    },
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
    pub resolved_at: Option<String>,
    pub rows: Vec<MailboxRow>,
    pub remaining_count: usize,
}

struct WakeSweepSessionState {
    session_id: String,
    min_pending_seq: i64,
    max_pending_seq: i64,
    claim: Option<WakeClaimRow>,
}

pub struct MailboxDb {
    conn: Connection,
    path: PathBuf,
}

enum BoundedMailboxRowsError {
    Prepare(rusqlite::Error),
    Query(rusqlite::Error),
    Row(rusqlite::Error),
}

impl MailboxDb {
    pub fn path_for_state_db(state_db_path: &Path) -> PathBuf {
        state_db_path.with_file_name("pid-identity.db")
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
        if !path.exists() {
            return Ok(None);
        }
        Self::open(&path).map(Some)
    }

    pub fn open(path: &Path) -> Result<Self, String> {
        ensure_parent_dir(path)?;
        let conn = Connection::open(path)
            .map_err(|err| format!("Failed to open PID mailbox sidecar: {err}"))?;
        set_wal_mode(&conn)?;
        pid_identity::ensure_identity_schema(&conn)?;
        ensure_mailbox_schema(&conn)?;
        Ok(Self {
            conn,
            path: path.to_path_buf(),
        })
    }

    pub fn open_read_only(path: &Path) -> Result<Self, String> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|err| format!("Failed to open PID mailbox sidecar read-only: {err}"))?;
        Ok(Self {
            conn,
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    pub fn create_runtime_generation(
        &mut self,
        request: CreateRuntimeGeneration<'_>,
    ) -> Result<GenerationMutation<RuntimeGenerationRow>, GenerationStorageError> {
        validate_runtime_generation_create(&request)?;
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
                    effective_cwd, created_at
                 ) VALUES (?1, 'starting', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
                ],
            )
            .map_err(generation_storage_error(
                "insert starting runtime generation",
            ))?;
        let row = runtime_generation_by_id_on(&tx, request.generation_id)?.ok_or_else(|| {
            GenerationStorageError::new("Runtime generation missing after create".to_string())
        })?;
        let result = map_runtime_generation_create(changed, row, &request);
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
                    identity.map(|identity| identity.os_pid),
                    identity.map(|identity| identity.os_boot_id.as_str()),
                    identity.map(|identity| identity.os_pid_starttime_ticks),
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
        tx.commit().map_err(generation_storage_error(
            "commit generation session attachment transaction",
        ))?;
        Ok(GenerationMutation::Applied(row))
    }

    pub fn resolve_runtime_generation(
        &self,
        selector: RuntimeGenerationSelector<'_>,
    ) -> Result<RuntimeGenerationResolution, GenerationStorageError> {
        let rows = match selector {
            RuntimeGenerationSelector::Exact(fence) => {
                runtime_generation_by_id_on(&self.conn, fence.generation_id)?
                    .filter(|row| row.spawn_invocation_uuid == fence.spawn_invocation_uuid)
                    .into_iter()
                    .collect()
            }
            RuntimeGenerationSelector::ProcessIdentity(identity) => {
                runtime_generations_by_process_identity_on(&self.conn, identity)?
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
        runtime_generation_by_id_on(&self.conn, generation_id)
    }

    pub fn session_generation_projection(
        &self,
        session_id: &str,
    ) -> Result<SessionGenerationProjection, GenerationStorageError> {
        let sql = format_runtime_generations_for_session_sql(true);
        let rows = runtime_generations_for_session_on(&self.conn, session_id, &sql)?;
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
        runtime_generations_for_session_on(&self.conn, session_id, &sql)
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
        let has_existing_claim = before.active_delivery_claim_id.is_some();
        if has_existing_claim && validate_existing_delivery_claim(&before).is_err() {
            tx.commit().map_err(generation_storage_error(
                "commit invalid existing delivery claim transaction",
            ))?;
            return Ok(DeliveryClaimAcquireResult::Rejected(
                GenerationRejection::InvariantViolation,
            ));
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
        let delivery_states = mailbox_delivery_states_on(&tx, request.seqs)?;
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
                     SET delivered_at = ?2,
                         delivered_by_invocation_uuid = ?3,
                         delivery_attempts = delivery_attempts + 1,
                         delivery_error = NULL
                     WHERE seq = ?1
                       AND delivered_at IS NULL",
                    params![seq, &now, request.delivered_by_invocation_uuid],
                )
                .map_err(generation_storage_error(
                    "confirm claimed mailbox row delivery",
                ))?;
            validate_confirmed_mailbox_row_change(*seq, changed)?;
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
        for seq in request.seqs {
            tx.execute(
                "UPDATE mailbox
                 SET delivery_attempts = delivery_attempts + 1,
                     delivery_error = ?2
                 WHERE seq = ?1
                   AND delivered_at IS NULL",
                params![seq, request.delivery_error],
            )
            .map_err(generation_storage_error(
                "record claimed mailbox row delivery failure",
            ))?;
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
        if let Err(rejection) = validate_non_orderly_predecessor(&before) {
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
               AND lifecycle_state IN ('starting', 'running')",
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
        let row =
            runtime_generation_by_id_on(&tx, request.fence.generation_id)?.ok_or_else(|| {
                GenerationStorageError::new("Runtime generation missing after exit".to_string())
            })?;
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
        let row =
            runtime_generation_by_id_on(&tx, request.fence.generation_id)?.ok_or_else(|| {
                GenerationStorageError::new(
                    "Runtime generation missing after drain finish".to_string(),
                )
            })?;
        tx.commit().map_err(generation_storage_error(
            "commit generation drain finish transaction",
        ))?;
        Ok(map_finished_drain(row))
    }

    pub fn enqueue_agent_bash_complete(
        &mut self,
        input: &AgentBashCompleteEnqueue<'_>,
    ) -> Result<EnqueueResult, String> {
        let published = self.publish_immutable_payload(input.payload_json.as_bytes())?;
        let payload_json = compacted_payload_json(AGENT_BASH_COMPLETE_KIND, &published)?;
        let now = now_rfc3339();
        let tx = self
            .conn
            .transaction()
            .map_err(|err| format!("Failed to start mailbox enqueue transaction: {err}"))?;
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
        let published = self.publish_immutable_payload(input.input)?;
        let handle = submitted_input_handle(input.submission_token, input.target)?;
        let payload_json = submitted_input_payload_json(input, &published)?;
        let now = now_rfc3339();
        let tx = self
            .conn
            .transaction()
            .map_err(|err| format!("Failed to start input enqueue transaction: {err}"))?;
        let result =
            enqueue_submitted_input_in_tx(&tx, input, &handle, &payload_json, &published, &now)?;
        tx.commit()
            .map_err(|err| format!("Failed to commit input enqueue transaction: {err}"))?;
        Ok(result)
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
        &mut self,
        limit: usize,
    ) -> Result<DeliveredPayloadCompactionReport, String> {
        if limit == 0 {
            return Ok(DeliveredPayloadCompactionReport::default());
        }
        let candidates = delivered_payload_compaction_candidates(&self.conn, limit)?;
        let mut report = DeliveredPayloadCompactionReport {
            scanned_rows: candidates.len(),
            ..DeliveredPayloadCompactionReport::default()
        };
        for candidate in candidates {
            let original_len = candidate.payload_json.len() as u64;
            let published = self.retained_payload_for_compaction(&candidate)?;
            let compacted_json = compacted_payload_json(&candidate.kind, &published)?;
            let changed =
                mark_payload_compacted(&self.conn, &candidate, &published, &compacted_json)?;
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

    pub fn list_pending(&self, session_id: &str) -> Result<Vec<MailboxRow>, String> {
        self.list_pending_for_delivery(session_id, None)
    }

    pub fn list_pending_for_delivery(
        &self,
        session_id: &str,
        chain_id: Option<&str>,
    ) -> Result<Vec<MailboxRow>, String> {
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
                 WHERE delivered_at IS NULL
                   AND (
                       (target_kind IS NULL AND session_id = ?1)
                       OR (target_kind = 'session' AND target_id = ?1)
                       OR (?2 IS NOT NULL AND target_kind = 'chain' AND target_id = ?2)
                   )
                 ORDER BY seq ASC",
            )
            .map_err(|err| format!("Failed to prepare pending mailbox query: {err}"))?;
        let rows = stmt
            .query_map(params![session_id, chain_id], map_mailbox_row)
            .map_err(|err| format!("Failed to query pending mailbox rows: {err}"))?;
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
        resolve_completed_delivery_attempts(&tx, session_id, &now, None)?;
        tx.commit().map_err(|err| {
            format!("Failed to commit mailbox range acknowledgement transaction: {err}")
        })?;
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
        seqs: &[i64],
        delivered_by_invocation_uuid: &str,
    ) -> Result<(), String> {
        if seqs.is_empty() {
            return Ok(());
        }
        let now = now_rfc3339();
        let tx = self
            .conn
            .transaction()
            .map_err(|err| format!("Failed to start mailbox delivery transaction: {err}"))?;
        for seq in seqs {
            tx.execute(
                "UPDATE mailbox
                 SET delivered_at = ?3,
                     delivered_by_invocation_uuid = ?4,
                     delivery_attempts = delivery_attempts + 1,
                     delivery_error = NULL
                 WHERE seq = ?2
                   AND delivered_at IS NULL",
                params![
                    rusqlite::types::Null,
                    seq,
                    &now,
                    delivered_by_invocation_uuid
                ],
            )
            .map_err(|err| format!("Failed to mark mailbox row delivered: {err}"))?;
        }
        resolve_completed_delivery_attempts(&tx, session_id, &now, None)?;
        tx.commit()
            .map_err(|err| format!("Failed to commit mailbox delivery transaction: {err}"))
    }

    pub fn register_delivery_attempt(
        &mut self,
        attempt_id: &str,
        session_id: &str,
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
        resolve_completed_delivery_attempts(&tx, session_id, &now, None)?;
        tx.commit()
            .map_err(|err| format!("Failed to commit mailbox delivery attempt: {err}"))
    }

    pub fn register_or_reuse_delivery_attempt(
        &mut self,
        attempt_id: &str,
        session_id: &str,
        delivery_invocation_uuid: &str,
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
                "SELECT attempt_id
                 FROM mailbox_delivery_attempts
                 WHERE session_id = ?1
                   AND delivery_invocation_uuid = ?2
                   AND resolved_at IS NULL
                 ORDER BY created_at, attempt_id
                 LIMIT 1",
                params![session_id, delivery_invocation_uuid],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|err| format!("Failed to query reusable mailbox delivery attempt: {err}"))?;
        if let Some(existing) = existing {
            tx.commit().map_err(|err| {
                format!("Failed to commit reused mailbox delivery attempt: {err}")
            })?;
            return Ok(existing);
        }
        tx.execute(
            "UPDATE mailbox_delivery_attempts
             SET resolved_at = ?3
             WHERE session_id = ?1
               AND delivery_invocation_uuid != ?2
               AND acknowledged_at IS NULL
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
        let Some((session_id, delivery_invocation_uuid, acknowledged_at, resolved_at)) = self
            .conn
            .query_row(
                "SELECT session_id, delivery_invocation_uuid, acknowledged_at, resolved_at
                 FROM mailbox_delivery_attempts WHERE attempt_id = ?1",
                params![attempt_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
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
            resolved_at,
            remaining_count: pending_count.saturating_sub(rows.len()),
            rows,
        }))
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
                 SET acknowledged_at = COALESCE(acknowledged_at, ?2)
                 WHERE attempt_id = ?1
                   AND resolved_at IS NULL",
                params![attempt_id, &now],
            )
            .map(|changed| changed > 0)
            .map_err(|err| format!("Failed to record mailbox delivery transport ACK: {err}"))
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
                   AND resolved_at IS NULL",
                params![attempt_id, &now],
            )
            .map(|changed| changed > 0)
            .map_err(|err| format!("Failed to resolve unacknowledged mailbox delivery: {err}"))
    }

    pub fn confirm_delivery_attempt(&mut self, attempt_id: &str) -> Result<bool, String> {
        let now = now_rfc3339();
        let tx = self.conn.transaction().map_err(|err| {
            format!("Failed to start mailbox delivery confirmation transaction: {err}")
        })?;
        let Some((session_id, delivery_invocation_uuid)) = tx
            .query_row(
                "SELECT session_id, delivery_invocation_uuid
                 FROM mailbox_delivery_attempts
                 WHERE attempt_id = ?1 AND acknowledged_at IS NOT NULL",
                params![attempt_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(|err| format!("Failed to query confirmed delivery attempt owner: {err}"))?
        else {
            return Ok(false);
        };
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
        resolve_completed_delivery_attempts(&tx, &session_id, &now, Some(attempt_id))?;
        tx.commit()
            .map_err(|err| format!("Failed to commit mailbox delivery confirmation: {err}"))?;
        Ok(true)
    }

    pub fn fail_unobserved_delivery_attempt(
        &mut self,
        attempt_id: &str,
        delivery_error: &str,
    ) -> Result<bool, String> {
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
        _session_id: &str,
        seqs: &[i64],
        delivery_error: &str,
    ) -> Result<(), String> {
        if seqs.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction().map_err(|err| {
            format!("Failed to start mailbox delivery failure transaction: {err}")
        })?;
        for seq in seqs {
            tx.execute(
                "UPDATE mailbox
                 SET delivery_attempts = delivery_attempts + 1,
                     delivery_error = ?3
                 WHERE seq = ?2
                   AND delivered_at IS NULL",
                params![rusqlite::types::Null, seq, delivery_error],
            )
            .map_err(|err| format!("Failed to mark mailbox row delivery failed: {err}"))?;
        }
        tx.commit()
            .map_err(|err| format!("Failed to commit mailbox delivery failure transaction: {err}"))
    }

    pub fn mark_pending_abandoned(
        &mut self,
        session_id: &str,
        delivery_error: &str,
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
                params![session_id, delivery_error, limit as i64],
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

    pub fn upsert_session_runtime(
        &mut self,
        input: SessionRuntimeUpsert<'_>,
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
                    pty_control_path,
                    updated_at,
                    models_dir,
                    effective_cwd,
                    selected_auto_wake_max
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(session_id)
                 DO UPDATE SET
                    mode = excluded.mode,
                    invocation_uuid = COALESCE(excluded.invocation_uuid, session_runtime.invocation_uuid),
                    provider_name = excluded.provider_name,
                    model_name = excluded.model_name,
                    pty_control_path = excluded.pty_control_path,
                    updated_at = excluded.updated_at,
                    models_dir = COALESCE(excluded.models_dir, session_runtime.models_dir),
                    effective_cwd = COALESCE(excluded.effective_cwd, session_runtime.effective_cwd),
                    selected_auto_wake_max = COALESCE(
                        session_runtime.selected_auto_wake_max,
                        excluded.selected_auto_wake_max
                    )",
                params![
                    input.session_id,
                    input.mode,
                    input.invocation_uuid,
                    input.provider_name,
                    input.model_name,
                    input.pty_control_path,
                    &now,
                    input.models_dir,
                    input.effective_cwd,
                    input.selected_auto_wake_max,
                ],
            )
            .map_err(|err| format!("Failed to upsert session runtime row: {err}"))?;
        Ok(())
    }

    pub fn mark_session_running(
        &mut self,
        input: SessionRuntimeRunningUpdate<'_>,
    ) -> Result<(), String> {
        validate_running_run_state()?;
        let now = now_rfc3339();
        let turn_start_max_mailbox_seq = self.running_turn_start_max_mailbox_seq(&input)?;
        mark_session_running_row(&self.conn, input, &now, turn_start_max_mailbox_seq)
    }

    pub fn mark_session_idle(
        &mut self,
        input: SessionRuntimeIdleUpdate<'_>,
    ) -> Result<bool, String> {
        validate_idle_run_state()?;
        let now = now_rfc3339();
        mark_session_idle_row(&self.conn, input, &now)
    }

    pub fn session_runtime(&self, session_id: &str) -> Result<Option<SessionRuntimeRow>, String> {
        let row = session_runtime_row(&self.conn, session_id)?;
        validate_session_runtime_row(row.as_ref())?;
        Ok(row)
    }

    pub fn session_liveness(&mut self, session_id: &str) -> Result<SessionLiveness, String> {
        let row = self.session_runtime(session_id)?;
        let decision = session_runtime_liveness_decision(row.as_ref())?;
        self.clear_stale_running_row_for_liveness(session_id, &decision)?;
        Ok(session_liveness_from_decision(&decision))
    }

    pub fn classify_session_runtime_read_only(
        &self,
        session_id: &str,
    ) -> Result<SessionRuntimeReadOnlyLiveness, String> {
        let row = self.session_runtime(session_id)?;
        classify_session_runtime_row_read_only(row.as_ref())
    }

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
        if let Some(result) = self.wake_claim_start_blocker(input.session_id)? {
            return Ok(result);
        }
        let now = now_rfc3339();
        let tx = begin_wake_claim_transaction(&mut self.conn)?;
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

    fn wake_claim_start_blocker(
        &mut self,
        session_id: &str,
    ) -> Result<Option<WakeClaimAcquireResult>, String> {
        if !self.session_has_pending_mailbox(session_id)? {
            return Ok(Some(WakeClaimAcquireResult::NoPending));
        }
        if self.session_is_busy(session_id)? {
            return Ok(Some(WakeClaimAcquireResult::Busy));
        }
        Ok(None)
    }

    pub fn wake_claim(&self, session_id: &str) -> Result<Option<WakeClaimRow>, String> {
        wake_claim(&self.conn, session_id)
    }

    pub fn release_wake_claim(
        &mut self,
        session_id: &str,
        claim_token: Option<&str>,
    ) -> Result<bool, String> {
        let changed = match claim_token {
            Some(token) => self.conn.execute(
                "DELETE FROM session_wake_claim WHERE session_id = ?1 AND claim_token = ?2",
                params![session_id, token],
            ),
            None => self.conn.execute(
                "DELETE FROM session_wake_claim WHERE session_id = ?1",
                params![session_id],
            ),
        }
        .map_err(|err| format!("Failed to release wake claim: {err}"))?;
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
                 SET wake_pid = ?3
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
        provider_name: Option<&str>,
        model_name: Option<&str>,
    ) -> Result<bool, String> {
        if let Some(identity) = pid_identity::read_live_process_identity(wake_pid)? {
            let recorded_at = now_rfc3339();
            let record = wake_claim_pid_identity_record(
                &identity,
                claim_token,
                session_id,
                provider_name,
                model_name,
                &recorded_at,
            );
            pid_identity::PidIdentityDb::open(self.path())?.record_identity(record)?;
        }
        self.record_wake_claim_pid(session_id, claim_token, wake_pid)
    }

    pub fn wake_sweep_candidates(
        &mut self,
        stale_after_seconds: i64,
        limit: usize,
    ) -> Result<Vec<WakeSweepCandidate>, String> {
        let session_ids = self.pending_wake_session_ids(limit)?;
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
        let claim = self.wake_claim(&session_id)?;
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
            Some(claim) => wake_claim_is_reclaimable(&self.conn, claim, stale_after_seconds),
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
        Ok(self
            .session_runtime(session_id)?
            .map(|runtime| runtime.auto_wake_count)
            .unwrap_or(0))
    }

    pub fn validate_wake_claim_for_child(
        &mut self,
        session_id: &str,
        claim_token: &str,
    ) -> Result<bool, String> {
        let claim = self.wake_claim(session_id)?;
        let valid = wake_claim_is_valid_for_child(claim.as_ref(), claim_token);
        self.release_after_child_claim_validation(session_id, claim_token, valid)
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

    fn release_after_child_claim_validation(
        &mut self,
        session_id: &str,
        claim_token: &str,
        valid: bool,
    ) -> Result<bool, String> {
        if !valid {
            return Ok(false);
        }
        self.release_busy_child_wake_claim(session_id, claim_token)
    }

    fn pending_seq_bounds(&self, session_id: &str) -> Result<Option<(i64, i64)>, String> {
        pending_seq_bounds_on(&self.conn, session_id)
    }

    #[cfg(test)]
    fn enqueue_agent_bash_complete_then_rollback(
        &mut self,
        input: &AgentBashCompleteEnqueue<'_>,
    ) -> Result<(), String> {
        let published = self.publish_immutable_payload(input.payload_json.as_bytes())?;
        let payload_json = compacted_payload_json(AGENT_BASH_COMPLETE_KIND, &published)?;
        let now = now_rfc3339();
        let tx = self
            .conn
            .transaction()
            .map_err(|err| format!("Failed to start mailbox rollback test transaction: {err}"))?;
        let _ = enqueue_agent_bash_complete_in_tx(&tx, input, &payload_json, &published, &now)?;
        Err("forced rollback before commit".to_string())
    }

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

    fn max_mailbox_seq(&self, session_id: &str) -> Result<Option<i64>, String> {
        self.conn
            .query_row(
                "SELECT MAX(seq) FROM mailbox WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .map_err(|err| format!("Failed to read mailbox max seq: {err}"))
    }

    fn running_turn_start_max_mailbox_seq(
        &self,
        input: &SessionRuntimeRunningUpdate<'_>,
    ) -> Result<Option<i64>, String> {
        let Some(seq) = input.turn_start_max_mailbox_seq else {
            return self.max_mailbox_seq(input.session_id);
        };
        Ok(Some(seq))
    }

    fn session_has_pending_mailbox(&self, session_id: &str) -> Result<bool, String> {
        self.list_pending(session_id)
            .map(|pending| !pending.is_empty())
    }

    fn session_is_busy(&mut self, session_id: &str) -> Result<bool, String> {
        self.session_liveness(session_id)
            .map(|liveness| liveness == SessionLiveness::Busy)
    }

    fn release_busy_child_wake_claim(
        &mut self,
        session_id: &str,
        claim_token: &str,
    ) -> Result<bool, String> {
        if self.busy_child_wake_claim_should_release(session_id)? {
            self.release_child_wake_claim(session_id, claim_token)?;
            return Ok(false);
        }
        Ok(true)
    }

    fn busy_child_wake_claim_should_release(&mut self, session_id: &str) -> Result<bool, String> {
        self.session_is_busy(session_id)
    }

    fn release_child_wake_claim(
        &mut self,
        session_id: &str,
        claim_token: &str,
    ) -> Result<(), String> {
        self.release_wake_claim(session_id, Some(claim_token))?;
        Ok(())
    }

    fn clear_stale_running_row_for_liveness(
        &mut self,
        session_id: &str,
        decision: &SessionRuntimeLivenessDecision,
    ) -> Result<(), String> {
        if let SessionRuntimeLivenessDecision::Stale {
            running_invocation_uuid,
        } = decision
        {
            self.clear_stale_running_row(session_id, running_invocation_uuid.as_deref())?;
        }
        Ok(())
    }

    fn clear_stale_running_row(
        &mut self,
        session_id: &str,
        running_invocation_uuid: Option<&str>,
    ) -> Result<(), String> {
        let now = now_rfc3339();
        self.conn
            .execute(
                "UPDATE session_runtime
                 SET run_state = 'idle',
                     updated_at = ?3,
                     pty_control_path = NULL,
                     running_invocation_uuid = NULL,
                     running_os_pid = NULL,
                     running_os_boot_id = NULL,
                     running_os_pid_starttime_ticks = NULL,
                     turn_ended_at = COALESCE(turn_ended_at, ?3)
                  WHERE session_id = ?1
                    AND run_state = 'running'
                    AND ((?2 IS NULL AND running_invocation_uuid IS NULL)
                         OR running_invocation_uuid = ?2)",
                params![session_id, running_invocation_uuid, &now],
            )
            .map_err(|err| format!("Failed to clear stale session runtime row: {err}"))?;
        Ok(())
    }
}

pub fn mailbox_row_is_deliverable_pending(row: &MailboxRow) -> bool {
    row.delivered_at.is_none()
        && row.delivery_error.as_deref() != Some(WAKE_SWEEP_ABANDONED_ERROR)
        && (row.delivery_error.as_deref() != Some(MAILBOX_DELIVERY_UNCONFIRMED_ERROR)
            || row.delivery_attempts < MAX_UNCONFIRMED_DELIVERY_ATTEMPTS)
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
    match fs::hard_link(&temp_path, path) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::AlreadyExists => {}
        Err(err) => {
            let _ = fs::remove_file(&temp_path);
            return Err(format!(
                "Failed to publish immutable mailbox payload: {err}"
            ));
        }
    }
    fs::remove_file(&temp_path)
        .map_err(|err| format!("Failed to remove mailbox payload temporary file: {err}"))?;
    sync_directory(directory)?;
    Ok(())
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
fn sync_directory(path: &Path) -> Result<(), String> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x02000000;
    OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|err| format!("Failed to sync directory {}: {err}", path.display()))
}

#[cfg(not(any(unix, windows)))]
fn sync_directory(path: &Path) -> Result<(), String> {
    Err(format!(
        "Durable mailbox payload directory publication is unsupported on this platform: {}",
        path.display()
    ))
}

fn wake_claim_pid_identity_record<'a>(
    identity: &'a ProcessIdentity,
    claim_token: &'a str,
    session_id: &'a str,
    provider_name: Option<&'a str>,
    model_name: Option<&'a str>,
    recorded_at: &'a str,
) -> pid_identity::PidIdentityRecord<'a> {
    pid_identity::PidIdentityRecord {
        identity,
        os_pgid: None,
        invocation_uuid: claim_token,
        session_id: Some(session_id),
        provider_name,
        model_name,
        recorded_at,
    }
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
    conn.transaction().map_err(format_start_wake_claim_tx_error)
}

fn format_start_wake_claim_tx_error(err: rusqlite::Error) -> String {
    format!("Failed to start wake claim transaction: {err}")
}

fn pending_seq_bounds_for_claim_tx(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
) -> Result<Option<(i64, i64)>, String> {
    pending_seq_bounds_tx(tx, session_id)
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
    if request
        .exact_process_identity
        .is_some_and(|identity| identity.os_pid != request.spawned_os_pid)
    {
        return Err(GenerationStorageError::new(
            "Exact process identity PID does not match spawned OS PID".to_string(),
        ));
    }
    Ok(())
}

fn runtime_generation_create_matches(
    row: &RuntimeGenerationRow,
    request: &CreateRuntimeGeneration<'_>,
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
}

fn runtime_generation_binding_matches(
    row: &RuntimeGenerationRow,
    request: &BindRuntimeGenerationRunning<'_>,
) -> bool {
    row.spawned_os_pid == Some(request.spawned_os_pid)
        && match (&row.exact_process_evidence, request.exact_process_identity) {
            (ExactProcessEvidence::NotRecorded, None) => true,
            (ExactProcessEvidence::Recorded(recorded), Some(requested)) => recorded == requested,
            _ => false,
        }
}

fn map_runtime_generation_create(
    changed: usize,
    row: RuntimeGenerationRow,
    request: &CreateRuntimeGeneration<'_>,
) -> GenerationMutation<RuntimeGenerationRow> {
    if changed == 1 {
        return GenerationMutation::Applied(row);
    }
    if runtime_generation_create_matches(&row, request) {
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
    let Some(identity) = request.exact_process_identity else {
        return Ok(true);
    };
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

fn validate_confirmed_mailbox_row_change(
    seq: i64,
    changed: usize,
) -> Result<(), GenerationStorageError> {
    if changed == 1 {
        return Ok(());
    }
    Err(GenerationStorageError::new(format!(
        "Claimed mailbox row {seq} was not pending at confirmation"
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

fn validate_non_orderly_predecessor(
    before: &RuntimeGenerationRow,
) -> Result<(), GenerationRejection> {
    if matches!(
        before.lifecycle_state,
        RuntimeLifecycleState::Starting | RuntimeLifecycleState::Running
    ) {
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
                active_delivery_seqs_json
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
            11,
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
    seqs: &[i64],
) -> Result<Vec<Option<Option<String>>>, GenerationStorageError> {
    let mut states = Vec::with_capacity(seqs.len());
    for seq in seqs {
        let state = conn
            .query_row(
                "SELECT delivered_at FROM mailbox WHERE seq = ?1",
                params![seq],
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

fn mark_session_running_row(
    conn: &Connection,
    input: SessionRuntimeRunningUpdate<'_>,
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

fn mark_session_idle_row(
    conn: &Connection,
    input: SessionRuntimeIdleUpdate<'_>,
    now: &str,
) -> Result<bool, String> {
    let changed = mark_session_idle_row_count(conn, input, now)?;
    Ok(row_changed(changed))
}

fn mark_session_idle_row_count(
    conn: &Connection,
    input: SessionRuntimeIdleUpdate<'_>,
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

fn session_runtime_row(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<SessionRuntimeRow>, String> {
    conn.query_row(
        "SELECT session_id, mode, invocation_uuid, provider_name, model_name,
                pty_control_path, updated_at, run_state, running_invocation_uuid,
                running_os_pid, running_os_boot_id, running_os_pid_starttime_ticks,
                turn_started_at, turn_ended_at, turn_start_max_mailbox_seq,
                last_exit_code, models_dir, effective_cwd, auto_wake_count,
                selected_auto_wake_max
         FROM session_runtime
         WHERE session_id = ?1",
        params![session_id],
        map_session_runtime_row,
    )
    .optional()
    .map_err(|err| format!("Failed to read session runtime row: {err}"))
}

fn validate_session_runtime_row(row: Option<&SessionRuntimeRow>) -> Result<(), String> {
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

fn runtime_row_is_idle(row: &SessionRuntimeRow) -> bool {
    row.run_state != "running"
}

fn live_process_identity_for_runtime(
    recorded: &ProcessIdentity,
) -> Result<Option<ProcessIdentity>, String> {
    pid_identity::read_live_process_identity(recorded.os_pid)
}

fn runtime_identity_is_live(live: Option<&ProcessIdentity>, recorded: &ProcessIdentity) -> bool {
    live.is_some_and(|live| live == recorded)
}

struct RuntimeLivenessEvidence {
    invocation_uuid: Option<String>,
    recorded: Option<ProcessIdentity>,
    live: Option<ProcessIdentity>,
}

fn session_runtime_liveness_decision(
    row: Option<&SessionRuntimeRow>,
) -> Result<SessionRuntimeLivenessDecision, String> {
    let Some(row) = row else {
        return Ok(SessionRuntimeLivenessDecision::Idle);
    };
    if runtime_row_is_idle(row) {
        return Ok(SessionRuntimeLivenessDecision::Idle);
    }
    let evidence = runtime_liveness_evidence(row)?;
    Ok(session_runtime_liveness_from_evidence(evidence))
}

fn classify_session_runtime_row_read_only(
    row: Option<&SessionRuntimeRow>,
) -> Result<SessionRuntimeReadOnlyLiveness, String> {
    let Some(row) = row else {
        return Ok(SessionRuntimeReadOnlyLiveness::Idle);
    };
    if runtime_row_is_idle(row) {
        return Ok(SessionRuntimeReadOnlyLiveness::Idle);
    }
    let evidence = runtime_liveness_evidence(row)?;
    Ok(read_only_liveness_from_evidence(evidence))
}

fn runtime_liveness_evidence(row: &SessionRuntimeRow) -> Result<RuntimeLivenessEvidence, String> {
    let invocation_uuid = row.running_invocation_uuid.clone();
    let recorded = runtime_liveness_recorded_identity(row, invocation_uuid.as_ref());
    let live = live_process_identity_for_evidence(recorded.as_ref())?;
    Ok(runtime_liveness_evidence_from_parts(
        invocation_uuid,
        recorded,
        live,
    ))
}

fn runtime_liveness_recorded_identity(
    row: &SessionRuntimeRow,
    invocation_uuid: Option<&String>,
) -> Option<ProcessIdentity> {
    invocation_uuid.and_then(|_| runtime_row_identity(row))
}

fn live_process_identity_for_evidence(
    recorded: Option<&ProcessIdentity>,
) -> Result<Option<ProcessIdentity>, String> {
    match recorded {
        Some(recorded) => live_process_identity_for_runtime(recorded),
        None => Ok(None),
    }
}

fn runtime_liveness_evidence_from_parts(
    invocation_uuid: Option<String>,
    recorded: Option<ProcessIdentity>,
    live: Option<ProcessIdentity>,
) -> RuntimeLivenessEvidence {
    RuntimeLivenessEvidence {
        invocation_uuid,
        recorded,
        live,
    }
}

fn session_runtime_liveness_from_evidence(
    evidence: RuntimeLivenessEvidence,
) -> SessionRuntimeLivenessDecision {
    if liveness_evidence_missing_invocation(&evidence) {
        return stale_liveness_decision(None);
    };
    let recorded_missing = liveness_evidence_missing_recorded(&evidence);
    let invocation_uuid = evidence.invocation_uuid.expect("invocation checked above");
    if recorded_missing {
        return stale_liveness_decision(Some(invocation_uuid));
    };
    let recorded = evidence.recorded.expect("recorded identity checked above");
    if liveness_evidence_is_busy(&evidence.live, &recorded) {
        return SessionRuntimeLivenessDecision::Busy;
    }
    stale_liveness_decision(Some(invocation_uuid))
}

fn liveness_evidence_missing_invocation(evidence: &RuntimeLivenessEvidence) -> bool {
    evidence.invocation_uuid.is_none()
}

fn liveness_evidence_missing_recorded(evidence: &RuntimeLivenessEvidence) -> bool {
    evidence.recorded.is_none()
}

fn liveness_evidence_is_busy(live: &Option<ProcessIdentity>, recorded: &ProcessIdentity) -> bool {
    runtime_identity_is_live(live.as_ref(), recorded)
}

fn stale_liveness_decision(
    running_invocation_uuid: Option<String>,
) -> SessionRuntimeLivenessDecision {
    SessionRuntimeLivenessDecision::Stale {
        running_invocation_uuid,
    }
}

fn read_only_liveness_from_evidence(
    evidence: RuntimeLivenessEvidence,
) -> SessionRuntimeReadOnlyLiveness {
    if liveness_evidence_missing_recorded(&evidence) {
        return read_only_missing_liveness(evidence.invocation_uuid.as_deref());
    };
    if read_only_liveness_evidence_missing_live(&evidence) {
        return SessionRuntimeReadOnlyLiveness::StaleDead;
    };
    let recorded = evidence.recorded.expect("recorded identity checked above");
    let live = evidence.live.expect("live identity checked above");
    read_only_liveness_from_live_identity(&live, &recorded)
}

fn read_only_liveness_evidence_missing_live(evidence: &RuntimeLivenessEvidence) -> bool {
    evidence.live.is_none()
}

fn read_only_liveness_from_live_identity(
    live: &ProcessIdentity,
    recorded: &ProcessIdentity,
) -> SessionRuntimeReadOnlyLiveness {
    if live == recorded {
        SessionRuntimeReadOnlyLiveness::Busy
    } else {
        SessionRuntimeReadOnlyLiveness::StalePidReused
    }
}

fn read_only_missing_liveness(invocation_uuid: Option<&str>) -> SessionRuntimeReadOnlyLiveness {
    if invocation_uuid.is_some() {
        SessionRuntimeReadOnlyLiveness::StaleMissingIdentity
    } else {
        SessionRuntimeReadOnlyLiveness::StaleMissingInvocation
    }
}

fn session_liveness_from_decision(decision: &SessionRuntimeLivenessDecision) -> SessionLiveness {
    match decision {
        SessionRuntimeLivenessDecision::Busy => SessionLiveness::Busy,
        SessionRuntimeLivenessDecision::Idle | SessionRuntimeLivenessDecision::Stale { .. } => {
            SessionLiveness::Idle
        }
    }
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
    wake_claim_live_identity_has_matching_sidecar_row(conn, claim, &live)
}

fn wake_claim_live_process_identity(wake_pid: i64) -> Result<Option<ProcessIdentity>, String> {
    pid_identity::read_live_process_identity(wake_pid)
}

fn wake_claim_live_identity_has_matching_sidecar_row(
    conn: &Connection,
    claim: &WakeClaimRow,
    live: &ProcessIdentity,
) -> Result<bool, String> {
    let exists = wake_claim_live_identity_matching_sidecar_exists(conn, claim, live)?;
    Ok(sqlite_exists_value_to_bool(exists))
}

fn wake_claim_live_identity_matching_sidecar_exists(
    conn: &Connection,
    claim: &WakeClaimRow,
    live: &ProcessIdentity,
) -> Result<i64, String> {
    conn.query_row(
        "SELECT EXISTS(
            SELECT 1
            FROM pid_identity
            WHERE os_pid = ?1
              AND os_boot_id = ?2
              AND os_pid_starttime_ticks = ?3
              AND invocation_uuid = ?4
              AND session_id = ?5
        )",
        params![
            live.os_pid,
            &live.os_boot_id,
            live.os_pid_starttime_ticks,
            &claim.claim_token,
            &claim.session_id,
        ],
        |row| row.get::<_, i64>(0),
    )
    .map_err(|err| format!("Failed to verify wake PID sidecar identity: {err}"))
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

fn wake_claim_is_valid_for_child(claim: Option<&WakeClaimRow>, claim_token: &str) -> bool {
    claim.is_some_and(|claim| wake_claim_matches_child(claim, claim_token))
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

fn set_wal_mode(conn: &Connection) -> Result<(), String> {
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")
        .map_err(|err| format!("Failed to set durable PID mailbox sidecar mode: {err}"))
}

fn mailbox_schema_definition() -> &'static str {
    "CREATE TABLE IF NOT EXISTS mailbox (
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
            acknowledged_at               TEXT,
            resolved_at                   TEXT,
            resolved_by_attempt_id        TEXT
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

        CREATE TABLE IF NOT EXISTS mailbox_notification_control (
            session_id                    TEXT PRIMARY KEY,
            paused                       INTEGER NOT NULL DEFAULT 0,
            updated_at                   TEXT NOT NULL
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
            CHECK (
                (identity_os_pid IS NULL AND identity_os_boot_id IS NULL AND identity_os_pid_starttime_ticks IS NULL)
                OR
                (identity_os_pid IS NOT NULL AND identity_os_boot_id IS NOT NULL AND identity_os_pid_starttime_ticks IS NOT NULL)
            ),
            CHECK (identity_os_pid IS NULL OR identity_os_pid = spawned_os_pid),
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
            wake_invocation_uuid             TEXT,
            reason                           TEXT NOT NULL,
            auto_wake_count                  INTEGER NOT NULL,
            min_pending_seq_at_claim         INTEGER,
            max_pending_seq_at_claim         INTEGER
        );

        CREATE INDEX IF NOT EXISTS idx_session_wake_claim_claimed_at
            ON session_wake_claim(claimed_at);"
}

fn ensure_mailbox_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(mailbox_schema_definition())
        .map_err(|err| format!("Failed to ensure PID mailbox sidecar schema: {err}"))?;
    ensure_mailbox_columns(conn)?;
    ensure_mailbox_target_index(conn)?;
    ensure_mailbox_compaction_index(conn)?;
    ensure_mailbox_delivery_owner_index(conn)?;
    ensure_session_runtime_columns(conn)?;
    ensure_runtime_generation_columns(conn)
}

fn ensure_mailbox_columns(conn: &Connection) -> Result<(), String> {
    let sql = format_table_columns_pragma("mailbox");
    let columns = table_columns(conn, "mailbox", &sql)?;
    for (name, definition) in missing_mailbox_columns(&columns) {
        add_sidecar_column(conn, "mailbox", name, definition)?;
    }
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

fn ensure_runtime_generation_columns(conn: &Connection) -> Result<(), String> {
    let sql = format_table_columns_pragma("runtime_generation");
    let columns = table_columns(conn, "runtime_generation", &sql)?;
    for (name, definition) in missing_runtime_generation_columns(&columns) {
        add_sidecar_column(conn, "runtime_generation", name, definition)?;
    }
    Ok(())
}

fn runtime_generation_column_additions() -> [(&'static str, &'static str); 2] {
    [
        ("active_delivery_claimed_at", "TEXT"),
        ("active_delivery_seqs_json", "TEXT"),
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

fn runtime_row_identity(row: &SessionRuntimeRow) -> Option<ProcessIdentity> {
    Some(ProcessIdentity {
        os_pid: row.running_os_pid?,
        os_boot_id: row.running_os_boot_id.clone()?,
        os_pid_starttime_ticks: row.running_os_pid_starttime_ticks?,
    })
}

fn pending_seq_bounds_tx(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
) -> Result<Option<(i64, i64)>, String> {
    tx.query_row(
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

fn map_session_runtime_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRuntimeRow> {
    Ok(SessionRuntimeRow {
        session_id: row.get(0)?,
        mode: row.get(1)?,
        invocation_uuid: row.get(2)?,
        provider_name: row.get(3)?,
        model_name: row.get(4)?,
        pty_control_path: row.get(5)?,
        updated_at: row.get(6)?,
        run_state: row.get(7)?,
        running_invocation_uuid: row.get(8)?,
        running_os_pid: row.get(9)?,
        running_os_boot_id: row.get(10)?,
        running_os_pid_starttime_ticks: row.get(11)?,
        turn_started_at: row.get(12)?,
        turn_ended_at: row.get(13)?,
        turn_start_max_mailbox_seq: row.get(14)?,
        last_exit_code: row.get(15)?,
        models_dir: row.get(16)?,
        effective_cwd: row.get(17)?,
        auto_wake_count: row.get(18)?,
        selected_auto_wake_max: row.get(19)?,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StateDb;

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
        let payload = db.payload_reference(input.payload_json.as_bytes()).unwrap();

        let err = db
            .enqueue_agent_bash_complete_then_rollback(&input)
            .unwrap_err();

        assert_eq!(err, "forced rollback before commit");
        assert!(db.list_mailbox("session-a", true).unwrap().is_empty());
        assert!(payload.file_path.exists());
        db.verify_published_payload(&payload).unwrap();
    }

    #[test]
    fn immutable_payload_publication_is_content_addressed_and_detects_tampering() {
        let dir = tempfile::tempdir().unwrap();
        let db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let first = db.publish_immutable_payload(b"payload-a").unwrap();
        let second = db.publish_immutable_payload(b"payload-a").unwrap();

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

        assert!(db.verify_published_payload(&first).is_err());
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
        for table in ["mailbox", "runtime_generation", "session_runtime"] {
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

        db.mark_delivered("session-a", &[row.seq], "resume-1")
            .unwrap();
        let first = db.list_mailbox("session-a", true).unwrap().remove(0);
        db.mark_delivered("session-a", &[row.seq], "resume-2")
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
                &[row.seq],
                0,
            )
            .unwrap();
        let retry = db
            .register_or_reuse_delivery_attempt(
                "attempt-2",
                "session-a",
                "invocation-a",
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
        db.mark_delivered("session-a", &[rows[0].seq], "sibling-invocation")
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
    fn accepted_attempt_owner_skips_undeliverable_older_rows() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let abandoned =
            inserted_row(db.enqueue_agent_bash_complete(&input("abandoned", "session-a")));
        let exhausted =
            inserted_row(db.enqueue_agent_bash_complete(&input("exhausted", "session-a")));
        let deliverable =
            inserted_row(db.enqueue_agent_bash_complete(&input("deliverable", "session-a")));
        db.mark_pending_abandoned("session-a", WAKE_SWEEP_ABANDONED_ERROR, 1)
            .unwrap();
        for _ in 0..MAX_UNCONFIRMED_DELIVERY_ATTEMPTS {
            db.mark_delivery_failed(
                "session-a",
                &[exhausted.seq],
                MAILBOX_DELIVERY_UNCONFIRMED_ERROR,
            )
            .unwrap();
        }
        db.register_delivery_attempt(
            "deliverable-attempt",
            "session-a",
            "invocation-a",
            &[deliverable.seq],
            0,
        )
        .unwrap();
        db.record_delivery_attempt_transport_ack("deliverable-attempt")
            .unwrap();

        let owners = db.accepted_delivery_attempt_windows("session-a").unwrap();
        assert_eq!(owners.len(), 1);
        assert_eq!(owners[0].attempt_id, "deliverable-attempt");
        assert_eq!(owners[0].rows, vec![deliverable]);
        assert_eq!(owners[0].remaining_count, 0);
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
        db.mark_delivered("session-a", &[delivered.seq], "resume-1")
            .unwrap();

        let before = db.delivered_payload_compaction_stats().unwrap();
        assert_eq!(before.eligible_rows, 1);
        assert_eq!(before.inline_bytes, delivered_payload.len() as u64);

        let report = db.compact_delivered_payloads(1).unwrap();
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
            db.hydrate_agent_bash_payload_json(compacted).unwrap(),
            delivered_payload
        );
        let still_pending = rows.iter().find(|row| row.seq == pending.seq).unwrap();
        assert_eq!(
            db.hydrate_agent_bash_payload_json(still_pending).unwrap(),
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
            db.delivered_payload_compaction_stats().unwrap(),
            DeliveredPayloadCompactionStats::default()
        );
        assert_eq!(
            db.compact_delivered_payloads(1).unwrap(),
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
        db.mark_delivered("session-a", &[row.seq], "resume-1")
            .unwrap();

        let report = db.compact_delivered_payloads(1).unwrap();
        assert_eq!(report.compacted_rows, 1);
        let compacted = db.list_mailbox("session-a", true).unwrap().remove(0);
        assert_eq!(
            fs::read_to_string(compacted.payload_file_path.as_deref().unwrap()).unwrap(),
            payload
        );
        assert_eq!(
            db.hydrate_agent_bash_payload_json(&compacted).unwrap(),
            payload
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
            db.hydrate_agent_bash_payload_json(&legacy).unwrap(),
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

        db.mark_delivery_failed("session-a", &[row.seq], "mailbox_delivery_unconfirmed")
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
    fn list_pending_excludes_delivered() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let delivered =
            inserted_row(db.enqueue_agent_bash_complete(&input("handle-a", "session-a")));
        let pending = inserted_row(db.enqueue_agent_bash_complete(&input("handle-b", "session-a")));

        db.mark_delivered("session-a", &[delivered.seq], "resume-1")
            .unwrap();

        let pending_rows = db.list_pending("session-a").unwrap();
        assert_eq!(pending_rows.len(), 1);
        assert_eq!(pending_rows[0].seq, pending.seq);
        assert_eq!(pending_rows[0].handle, "handle-b");
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
    fn session_runtime_selected_auto_wake_max_round_trips_and_is_write_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();

        db.upsert_session_runtime(SessionRuntimeUpsert {
            session_id: "session-max",
            mode: "headless",
            invocation_uuid: Some("owner-invocation"),
            provider_name: Some("provider-a"),
            model_name: Some("model-a"),
            pty_control_path: None,
            models_dir: None,
            effective_cwd: None,
            selected_auto_wake_max: Some(32),
        })
        .unwrap();
        db.upsert_session_runtime(SessionRuntimeUpsert {
            session_id: "session-max",
            mode: "headless",
            invocation_uuid: None,
            provider_name: Some("provider-a"),
            model_name: Some("model-a"),
            pty_control_path: None,
            models_dir: None,
            effective_cwd: None,
            selected_auto_wake_max: Some(99),
        })
        .unwrap();

        let row = db.session_runtime("session-max").unwrap().unwrap();
        assert_eq!(row.selected_auto_wake_max, Some(32));
        assert_eq!(row.invocation_uuid.as_deref(), Some("owner-invocation"));
    }

    #[test]
    fn session_runtime_legacy_null_accepts_first_selected_auto_wake_max() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        db.upsert_session_runtime(SessionRuntimeUpsert {
            session_id: "session-legacy",
            mode: "headless",
            invocation_uuid: Some("owner-invocation"),
            provider_name: Some("provider-a"),
            model_name: Some("model-a"),
            pty_control_path: None,
            models_dir: None,
            effective_cwd: None,
            selected_auto_wake_max: None,
        })
        .unwrap();

        assert_eq!(
            db.session_runtime("session-legacy")
                .unwrap()
                .unwrap()
                .selected_auto_wake_max,
            None
        );
        db.upsert_session_runtime(SessionRuntimeUpsert {
            session_id: "session-legacy",
            mode: "headless",
            invocation_uuid: None,
            provider_name: Some("provider-a"),
            model_name: Some("model-a"),
            pty_control_path: None,
            models_dir: None,
            effective_cwd: None,
            selected_auto_wake_max: Some(32),
        })
        .unwrap();
        assert_eq!(
            db.session_runtime("session-legacy")
                .unwrap()
                .unwrap()
                .selected_auto_wake_max,
            Some(32)
        );
    }

    #[test]
    fn session_runtime_sidecar_repair_declares_selected_auto_wake_max() {
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

        db.mark_session_running(SessionRuntimeRunningUpdate {
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

        let row = db.session_runtime("session-a").unwrap().unwrap();
        assert_eq!(row.run_state, "running");
        assert_eq!(row.mode, "headless");
        assert_eq!(row.invocation_uuid.as_deref(), Some("invocation-a"));
        assert_eq!(row.running_invocation_uuid.as_deref(), Some("invocation-a"));
        assert_eq!(row.provider_name.as_deref(), Some("provider-a"));
        assert_eq!(row.model_name.as_deref(), Some("model-a"));
        assert_eq!(row.running_os_pid, Some(identity.os_pid));
        assert_eq!(
            row.running_os_boot_id.as_deref(),
            Some(identity.os_boot_id.as_str())
        );
        assert_eq!(
            row.running_os_pid_starttime_ticks,
            Some(identity.os_pid_starttime_ticks)
        );
        assert_eq!(row.turn_start_max_mailbox_seq, Some(7));
        assert_eq!(row.models_dir.as_deref(), Some("/tmp/models"));
        assert_eq!(row.effective_cwd.as_deref(), Some("/tmp/work"));
        assert!(row.turn_started_at.is_some());
        assert!(row.turn_ended_at.is_none());
    }

    #[test]
    fn auto_wake_keeps_owner_separate_from_running_invocation() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let identity = current_identity();

        db.mark_session_running(SessionRuntimeRunningUpdate {
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
            db.mark_session_idle(SessionRuntimeIdleUpdate {
                session_id: "session-a",
                invocation_uuid: "owner-invocation",
                last_exit_code: Some(0),
            })
            .unwrap()
        );

        db.upsert_session_runtime(SessionRuntimeUpsert {
            session_id: "session-a",
            mode: "headless",
            invocation_uuid: None,
            provider_name: Some("provider-a"),
            model_name: Some("model-a"),
            pty_control_path: None,
            models_dir: None,
            effective_cwd: None,
            selected_auto_wake_max: None,
        })
        .unwrap();
        db.enqueue_agent_bash_complete(&input("handle-a", "session-a"))
            .unwrap();
        assert!(matches!(
            db.try_acquire_wake_claim(WakeClaimRequest {
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

        db.mark_session_running(SessionRuntimeRunningUpdate {
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

        let running = db.session_runtime("session-a").unwrap().unwrap();
        assert_eq!(running.invocation_uuid.as_deref(), Some("owner-invocation"));
        assert_eq!(
            running.running_invocation_uuid.as_deref(),
            Some("wake-invocation")
        );
        assert!(
            db.mark_session_idle(SessionRuntimeIdleUpdate {
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

        db.mark_session_running(SessionRuntimeRunningUpdate {
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

        let row = db.session_runtime("session-a").unwrap().unwrap();
        assert_eq!(row.mode, "pty_interactive");
        assert_eq!(
            row.pty_control_path.as_deref(),
            Some("/tmp/oulipoly-a.sock")
        );
    }

    #[test]
    fn runtime_mark_idle_is_invocation_guarded() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let identity = current_identity();

        db.mark_session_running(SessionRuntimeRunningUpdate {
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
            !db.mark_session_idle(SessionRuntimeIdleUpdate {
                session_id: "session-a",
                invocation_uuid: "old-invocation",
                last_exit_code: Some(0),
            })
            .unwrap()
        );
        assert_eq!(
            db.session_runtime("session-a").unwrap().unwrap().run_state,
            "running"
        );

        assert!(
            db.mark_session_idle(SessionRuntimeIdleUpdate {
                session_id: "session-a",
                invocation_uuid: "new-invocation",
                last_exit_code: Some(0),
            })
            .unwrap()
        );
        let row = db.session_runtime("session-a").unwrap().unwrap();
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
        db.mark_session_running(SessionRuntimeRunningUpdate {
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

        assert_eq!(
            db.session_liveness("session-a").unwrap(),
            SessionLiveness::Busy
        );
        assert_eq!(
            db.session_runtime("session-a").unwrap().unwrap().run_state,
            "running"
        );
    }

    #[test]
    fn liveness_dead_or_reused_identity_is_idle_and_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let mut identity = current_identity();
        identity.os_pid_starttime_ticks += 1;
        db.mark_session_running(SessionRuntimeRunningUpdate {
            session_id: "session-a",
            mode: "headless",
            invocation_uuid: "invocation-a",
            provider_name: None,
            model_name: None,
            identity: &identity,
            pty_control_path: Some("/tmp/stale.sock"),
            turn_start_max_mailbox_seq: None,
            models_dir: None,
            effective_cwd: None,
        })
        .unwrap();

        assert_eq!(
            db.session_liveness("session-a").unwrap(),
            SessionLiveness::Idle
        );
        let row = db.session_runtime("session-a").unwrap().unwrap();
        assert_eq!(row.run_state, "idle");
        assert!(row.running_invocation_uuid.is_none());
        assert!(row.running_os_pid.is_none());
        assert!(row.pty_control_path.is_none());
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
    fn wake_claim_count_persists_on_session_runtime_after_claim_release() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        db.upsert_session_runtime(SessionRuntimeUpsert {
            session_id: "session-a",
            mode: "headless",
            invocation_uuid: None,
            provider_name: Some("provider-a"),
            model_name: Some("model-a"),
            pty_control_path: None,
            models_dir: Some("/tmp/models"),
            effective_cwd: None,
            selected_auto_wake_max: None,
        })
        .unwrap();
        db.enqueue_agent_bash_complete(&input("handle-a", "session-a"))
            .unwrap();

        let result = db
            .try_acquire_wake_claim(WakeClaimRequest {
                session_id: "session-a",
                claim_token: "token-a",
                reason: "notify_idle",
                auto_wake_count: 5,
                wake_invocation_uuid: Some("wake-a"),
                stale_after_seconds: 600,
            })
            .unwrap();

        assert!(matches!(result, WakeClaimAcquireResult::Acquired(_)));
        assert_eq!(
            db.session_runtime("session-a")
                .unwrap()
                .unwrap()
                .auto_wake_count,
            5
        );
        db.release_wake_claim("session-a", Some("token-a")).unwrap();
        let candidates = db.wake_sweep_candidates(600, 10).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].auto_wake_count, 6);
    }

    #[test]
    fn wake_busy_pending_skips_claim() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let identity = current_identity();
        db.enqueue_agent_bash_complete(&input("handle-a", "session-a"))
            .unwrap();
        db.mark_session_running(SessionRuntimeRunningUpdate {
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
            db.try_acquire_wake_claim(WakeClaimRequest {
                session_id: "session-a",
                claim_token: "token-a",
                reason: "notify_idle",
                auto_wake_count: 1,
                wake_invocation_uuid: None,
                stale_after_seconds: 600,
            })
            .unwrap(),
            WakeClaimAcquireResult::Busy
        ));
        assert!(db.wake_claim("session-a").unwrap().is_none());
    }

    #[test]
    fn wake_existing_claim_is_single_flight() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        db.enqueue_agent_bash_complete(&input("handle-a", "session-a"))
            .unwrap();
        let first = db
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
    fn wake_stale_claim_can_be_stolen() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        db.enqueue_agent_bash_complete(&input("handle-a", "session-a"))
            .unwrap();
        assert!(matches!(
            db.try_acquire_wake_claim(WakeClaimRequest {
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
        db.force_wake_claim_age_for_test("session-a", 601).unwrap();

        let stolen = db
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
            db.try_acquire_wake_claim(WakeClaimRequest {
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
        db.record_wake_claim_pid("session-a", "token-a", 999_999_999)
            .unwrap();

        let stolen = db
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
            db.try_acquire_wake_claim(WakeClaimRequest {
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
        let sidecar = pid_identity::PidIdentityDb::open(db.path()).unwrap();
        sidecar
            .record_identity(pid_identity::PidIdentityRecord {
                identity: &identity,
                os_pgid: None,
                invocation_uuid: "token-a",
                session_id: Some("session-a"),
                provider_name: Some("wake"),
                model_name: Some("model-a"),
                recorded_at: "2026-06-08T00:00:00Z",
            })
            .unwrap();
        db.record_wake_claim_pid("session-a", "token-a", identity.os_pid)
            .unwrap();
        db.force_wake_claim_age_for_test("session-a", 601).unwrap();

        let result = db
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
                db.create_runtime_generation(CreateRuntimeGeneration {
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
                db.bind_runtime_generation_running(BindRuntimeGenerationRunning {
                    fence: RuntimeGenerationFence {
                        generation_id,
                        spawn_invocation_uuid: invocation_uuid,
                    },
                    spawned_os_pid: identity.os_pid,
                    exact_process_identity: Some(identity),
                    os_pgid: None,
                })
                .unwrap(),
                GenerationMutation::Applied(_)
            ));
        }
        assert!(matches!(
            db.attach_runtime_generation_session(AttachRuntimeGenerationSession {
                fence: RuntimeGenerationFence {
                    generation_id: &first_id,
                    spawn_invocation_uuid: "invocation-a",
                },
                session_id: "session-a",
            })
            .unwrap(),
            GenerationMutation::Applied(_)
        ));

        let SessionGenerationProjection::Multiple(rows) =
            db.session_generation_projection("session-a").unwrap()
        else {
            panic!("overlapping generations must remain explicit");
        };
        assert_eq!(rows.len(), 2);
        assert!(matches!(
            db.resolve_runtime_generation(RuntimeGenerationSelector::ProcessIdentity(
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
        db.create_runtime_generation(CreateRuntimeGeneration {
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
        db.bind_runtime_generation_running(BindRuntimeGenerationRunning {
            fence: RuntimeGenerationFence {
                generation_id: &generation_id,
                spawn_invocation_uuid: "invocation-a",
            },
            spawned_os_pid: 103,
            exact_process_identity: None,
            os_pgid: None,
        })
        .unwrap();
        let fence = RuntimeGenerationFence {
            generation_id: &generation_id,
            spawn_invocation_uuid: "invocation-a",
        };

        assert!(matches!(
            db.request_runtime_generation_drain(RequestRuntimeGenerationDrain {
                fence,
                drain_request_id: &drain_request_id,
                requested_by_invocation_uuid: "drainer-a",
            })
            .unwrap(),
            DrainRequestResult::Installed(_, DrainHandoff::Ready)
        ));
        assert!(matches!(
            db.advance_runtime_generation_drain(AdvanceRuntimeGenerationDrain {
                fence,
                drain_request_id: &drain_request_id,
            })
            .unwrap(),
            DrainAdvanceResult::Advanced(ref row)
                if row.lifecycle_state == RuntimeLifecycleState::Draining
        ));
        assert!(matches!(
            db.finish_runtime_generation_drain(FinishRuntimeGenerationDrain {
                fence,
                drain_request_id: &drain_request_id,
                exit_code: Some(0),
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
            db.acquire_runtime_generation_delivery(AcquireRuntimeGenerationDelivery {
                fence,
                claim_id: &claim_id,
                seqs: &[row.seq],
                stale_after_seconds: 30,
            })
            .unwrap(),
            DeliveryClaimAcquireResult::Acquired(_)
        ));
        assert!(matches!(
            db.request_runtime_generation_drain(RequestRuntimeGenerationDrain {
                fence,
                drain_request_id: &drain_request_id,
                requested_by_invocation_uuid: "drainer-a",
            })
            .unwrap(),
            DrainRequestResult::Installed(_, DrainHandoff::ClaimOutstanding { .. })
        ));
        assert_eq!(
            db.advance_runtime_generation_drain(AdvanceRuntimeGenerationDrain {
                fence,
                drain_request_id: &drain_request_id,
            })
            .unwrap(),
            DrainAdvanceResult::WaitingOnClaim(claim_id.clone())
        );
        assert!(matches!(
            db.confirm_runtime_generation_delivery(ConfirmRuntimeGenerationDelivery {
                fence,
                claim_id: &claim_id,
                seqs: &[row.seq],
                delivered_by_invocation_uuid: "invocation-a",
            })
            .unwrap(),
            GenerationMutation::Applied(_)
        ));
        assert!(matches!(
            db.advance_runtime_generation_drain(AdvanceRuntimeGenerationDrain {
                fence,
                drain_request_id: &drain_request_id,
            })
            .unwrap(),
            DrainAdvanceResult::Advanced(_)
        ));
        assert!(db.list_pending("session-a").unwrap().is_empty());
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

        db.request_runtime_generation_drain(RequestRuntimeGenerationDrain {
            fence,
            drain_request_id: &drain_request_id,
            requested_by_invocation_uuid: "drainer-a",
        })
        .unwrap();
        assert_eq!(
            db.acquire_runtime_generation_delivery(AcquireRuntimeGenerationDelivery {
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
        db.acquire_runtime_generation_delivery(AcquireRuntimeGenerationDelivery {
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
        db.create_runtime_generation(CreateRuntimeGeneration {
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
        db.bind_runtime_generation_running(BindRuntimeGenerationRunning {
            fence: RuntimeGenerationFence {
                generation_id,
                spawn_invocation_uuid: invocation_uuid,
            },
            spawned_os_pid: 103,
            exact_process_identity: None,
            os_pgid: None,
        })
        .unwrap();
    }

    #[test]
    fn mailbox_operations_do_not_change_state_db_schema() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("state.db");
        let state = StateDb::open(&state_path).unwrap();
        let baseline_version = user_version(state.connection());
        let baseline_columns = invocation_columns(state.connection());
        drop(state);

        let mut mailbox = MailboxDb::open(&dir.path().join("pid-identity.db")).unwrap();
        let row =
            inserted_row(mailbox.enqueue_agent_bash_complete(&input("handle-a", "session-a")));
        mailbox
            .mark_delivered("session-a", &[row.seq], "resume-1")
            .unwrap();
        let identity = current_identity();
        mailbox
            .mark_session_running(SessionRuntimeRunningUpdate {
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
            .mark_session_idle(SessionRuntimeIdleUpdate {
                session_id: "session-a",
                invocation_uuid: "invocation-a",
                last_exit_code: Some(0),
            })
            .unwrap();
        assert!(matches!(
            mailbox
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
        assert_eq!(user_version(state.connection()), baseline_version);
        assert_eq!(invocation_columns(state.connection()), baseline_columns);
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
