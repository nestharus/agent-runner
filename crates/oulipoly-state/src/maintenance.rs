//! Durable coordination for detached, partition-bounded maintenance workers.
//!
//! The ledger is deliberately filesystem-backed and independent of State and
//! PID-mailbox SQLite.  An advisory lock is held for the complete worker run;
//! durable status is evidence and a restart cursor, never the ownership
//! mechanism.  Consequently an old heartbeat cannot evict a slow live worker,
//! while kernel release of the lock after process death permits exact recovery.

use crate::pid_identity::{ProcessIdentity, read_current_process_identity};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub const MAINTENANCE_FORMAT_VERSION: u32 = 1;
pub const MAINTENANCE_DIRECTORY: &str = "maintenance-v1";
pub const MAX_MAINTENANCE_PARTITION_BYTES: usize = 256;
pub const MAX_MAINTENANCE_CURSOR_BYTES: usize = 1_024;
pub const MAX_MAINTENANCE_SCHEDULE_BASIS_BYTES: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceJobKind {
    EventDiscovery,
    OrphanClassification,
    RotationFollowUp,
    CatalogInspection,
    HistoricalCompaction,
    IndexRebuild,
    EventRetirement,
    StateRetention,
    MailboxRetention,
}

impl MaintenanceJobKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EventDiscovery => "event_discovery",
            Self::OrphanClassification => "orphan_classification",
            Self::RotationFollowUp => "rotation_follow_up",
            Self::CatalogInspection => "catalog_inspection",
            Self::HistoricalCompaction => "historical_compaction",
            Self::IndexRebuild => "index_rebuild",
            Self::EventRetirement => "event_retirement",
            Self::StateRetention => "state_retention",
            Self::MailboxRetention => "mailbox_retention",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "event_discovery" => Self::EventDiscovery,
            "orphan_classification" => Self::OrphanClassification,
            "rotation_follow_up" => Self::RotationFollowUp,
            "catalog_inspection" => Self::CatalogInspection,
            "historical_compaction" => Self::HistoricalCompaction,
            "index_rebuild" => Self::IndexRebuild,
            "event_retirement" => Self::EventRetirement,
            "state_retention" => Self::StateRetention,
            "mailbox_retention" => Self::MailboxRetention,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceJobKey {
    pub kind: MaintenanceJobKind,
    /// Stable logical partition, not a filesystem path. Event work uses the
    /// exact lowercase `<writer-id>/<generation-id>` pair.
    pub partition: String,
}

impl MaintenanceJobKey {
    pub fn new(
        kind: MaintenanceJobKind,
        partition: impl Into<String>,
    ) -> Result<Self, MaintenanceError> {
        let key = Self {
            kind,
            partition: partition.into(),
        };
        key.validate()?;
        Ok(key)
    }

    pub fn validate(&self) -> Result<(), MaintenanceError> {
        if self.partition.is_empty()
            || self.partition.len() > MAX_MAINTENANCE_PARTITION_BYTES
            || !self.partition.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-')
            })
            || self.partition.starts_with('/')
            || self.partition.ends_with('/')
            || self
                .partition
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(MaintenanceError::InvalidKey);
        }
        Ok(())
    }

    pub fn storage_id(&self) -> Result<String, MaintenanceError> {
        self.validate()?;
        let encoded = serde_json::to_vec(self)?;
        let mut digest = Sha256::new();
        digest.update(b"oulipoly.detached-maintenance-job.v1\0");
        digest.update(encoded);
        Ok(hex(&digest.finalize()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceWorkerIdentity {
    pub worker_instance_id: Uuid,
    pub os_pid: i64,
    pub os_boot_id: String,
    pub os_pid_starttime_ticks: i64,
    /// Durable causal binding to the launch admission that selected this work.
    pub schedule_basis: String,
    pub launch_id: Option<Uuid>,
}

impl MaintenanceWorkerIdentity {
    pub fn current() -> Result<Self, MaintenanceError> {
        let identity = read_current_process_identity().map_err(MaintenanceError::Identity)?;
        Ok(Self::from_process_identity(identity))
    }

    pub fn from_process_identity(identity: ProcessIdentity) -> Self {
        Self {
            worker_instance_id: Uuid::new_v4(),
            os_pid: identity.os_pid,
            os_boot_id: identity.os_boot_id,
            os_pid_starttime_ticks: identity.os_pid_starttime_ticks,
            schedule_basis: "direct_private_worker".to_string(),
            launch_id: None,
        }
    }

    pub fn with_schedule_basis(
        mut self,
        basis: impl Into<String>,
        launch_id: Option<Uuid>,
    ) -> Self {
        self.schedule_basis = basis.into();
        self.launch_id = launch_id;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceRunState {
    Running,
    /// The worker intentionally released ownership after a bounded slice.
    /// The same opportunity may resume this cursor without dead-owner recovery.
    Yielded,
    Succeeded,
    Preserved,
    Cancelled,
    Failed,
}

impl MaintenanceRunState {
    pub const fn terminal(self) -> bool {
        !matches!(self, Self::Running | Self::Yielded)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceDiagnosticDelivery {
    Appended,
    Gap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceEvidenceKind {
    Job,
    Request,
}

/// Latest direct evidence for one exact job. It is a separate two-slot sink,
/// readable by the offline diagnostics command, and never initializes the
/// normal per-process event writer. The job checkpoint remains work authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceEvidenceRecord {
    pub format_version: u32,
    pub sequence: u64,
    pub kind: MaintenanceEvidenceKind,
    pub key: MaintenanceJobKey,
    pub opportunity_epoch: i64,
    pub attempt: Uuid,
    pub actor: MaintenanceWorkerIdentity,
    pub state: MaintenanceRunState,
    pub phase: String,
    pub cursor: Option<String>,
    pub units_completed: u64,
    pub bytes_completed: u64,
    pub reason: Option<String>,
    pub observed_at_unix_micros: i64,
    pub checksum_sha256: String,
}

impl MaintenanceEvidenceRecord {
    fn validate(&self) -> Result<(), MaintenanceError> {
        self.key.validate()?;
        if self.format_version != MAINTENANCE_FORMAT_VERSION
            || self.opportunity_epoch < 0
            || self.observed_at_unix_micros < 0
            || self.phase.is_empty()
            || self.phase.len() > 128
            || self
                .cursor
                .as_ref()
                .is_some_and(|cursor| cursor.len() > MAX_MAINTENANCE_CURSOR_BYTES)
            || self
                .reason
                .as_ref()
                .is_some_and(|reason| reason.is_empty() || reason.len() > 1_024)
            || self.expected_checksum()? != self.checksum_sha256
        {
            return Err(MaintenanceError::InvalidEvidence);
        }
        Ok(())
    }

    fn expected_checksum(&self) -> Result<String, MaintenanceError> {
        #[derive(Serialize)]
        struct Body<'a> {
            format_version: u32,
            sequence: u64,
            kind: MaintenanceEvidenceKind,
            key: &'a MaintenanceJobKey,
            opportunity_epoch: i64,
            attempt: Uuid,
            actor: &'a MaintenanceWorkerIdentity,
            state: MaintenanceRunState,
            phase: &'a str,
            cursor: &'a Option<String>,
            units_completed: u64,
            bytes_completed: u64,
            reason: &'a Option<String>,
            observed_at_unix_micros: i64,
        }
        let bytes = serde_json::to_vec(&Body {
            format_version: self.format_version,
            sequence: self.sequence,
            kind: self.kind,
            key: &self.key,
            opportunity_epoch: self.opportunity_epoch,
            attempt: self.attempt,
            actor: &self.actor,
            state: self.state,
            phase: &self.phase,
            cursor: &self.cursor,
            units_completed: self.units_completed,
            bytes_completed: self.bytes_completed,
            reason: &self.reason,
            observed_at_unix_micros: self.observed_at_unix_micros,
        })?;
        let mut digest = Sha256::new();
        digest.update(b"oulipoly.detached-maintenance-evidence.v1\0");
        digest.update(bytes);
        Ok(hex(&digest.finalize()))
    }

    fn canonical_bytes(&self) -> Result<Vec<u8>, MaintenanceError> {
        self.validate()?;
        let mut bytes = serde_json::to_vec(self)?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}

/// Explicit durable notice that a duplicate/already-terminal request could
/// not be published to the independent evidence sink. This is observable
/// evidence of a diagnostic gap, never proof of the maintenance effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceRequestEvidenceGap {
    pub format_version: u32,
    pub opportunity_epoch: i64,
    pub attempt: Uuid,
    pub reason: String,
    pub checksum_sha256: String,
}

impl MaintenanceRequestEvidenceGap {
    fn expected_checksum(&self) -> Result<String, MaintenanceError> {
        #[derive(Serialize)]
        struct Body<'a> {
            format_version: u32,
            opportunity_epoch: i64,
            attempt: Uuid,
            reason: &'a str,
        }
        let bytes = serde_json::to_vec(&Body {
            format_version: self.format_version,
            opportunity_epoch: self.opportunity_epoch,
            attempt: self.attempt,
            reason: &self.reason,
        })?;
        let mut digest = Sha256::new();
        digest.update(b"oulipoly.detached-maintenance-request-gap.v1\0");
        digest.update(bytes);
        Ok(hex(&digest.finalize()))
    }

    fn validate(&self) -> Result<(), MaintenanceError> {
        if self.format_version != MAINTENANCE_FORMAT_VERSION
            || self.opportunity_epoch < 0
            || self.reason.is_empty()
            || self.reason.len() > 512
            || self.expected_checksum()? != self.checksum_sha256
        {
            return Err(MaintenanceError::InvalidEvidence);
        }
        Ok(())
    }

    fn canonical_bytes(&self) -> Result<Vec<u8>, MaintenanceError> {
        self.validate()?;
        let mut bytes = serde_json::to_vec(self)?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceCheckpoint {
    pub format_version: u32,
    pub sequence: u64,
    pub key: MaintenanceJobKey,
    /// Caller-defined monotonic opportunity (normally UTC day number).
    pub opportunity_epoch: i64,
    pub attempt: Uuid,
    pub owner: MaintenanceWorkerIdentity,
    pub recovered_dead_owner: bool,
    pub state: MaintenanceRunState,
    pub phase: String,
    pub cursor: Option<String>,
    pub units_completed: u64,
    pub bytes_completed: u64,
    pub started_at_unix_micros: i64,
    pub heartbeat_at_unix_micros: i64,
    pub completed_at_unix_micros: Option<i64>,
    pub terminal_reason: Option<String>,
    /// The immediately preceding terminal attempt remains available after a
    /// later opportunity acquires this exact singleton.
    pub previous_terminal: Option<MaintenanceTerminalEvidence>,
    pub diagnostic_delivery: Option<MaintenanceDiagnosticDelivery>,
    pub diagnostic_gap: bool,
    pub checksum_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceTerminalEvidence {
    pub opportunity_epoch: i64,
    pub attempt: Uuid,
    pub state: MaintenanceRunState,
    pub phase: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<MaintenanceWorkerIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at_unix_micros: Option<i64>,
}

impl MaintenanceCheckpoint {
    fn validate(&self) -> Result<(), MaintenanceError> {
        self.key.validate()?;
        if self.format_version != MAINTENANCE_FORMAT_VERSION
            || self.opportunity_epoch < 0
            || self.started_at_unix_micros < 0
            || self.heartbeat_at_unix_micros < self.started_at_unix_micros
            || self.phase.is_empty()
            || self.phase.len() > 128
            || self.owner.schedule_basis.is_empty()
            || self.owner.schedule_basis.len() > MAX_MAINTENANCE_SCHEDULE_BASIS_BYTES
            || self
                .cursor
                .as_ref()
                .is_some_and(|value| value.len() > MAX_MAINTENANCE_CURSOR_BYTES)
            || self
                .terminal_reason
                .as_ref()
                .is_some_and(|value| value.is_empty() || value.len() > 1_024)
            || (self.state.terminal() && self.terminal_reason.is_none())
            || self.previous_terminal.as_ref().is_some_and(|previous| {
                previous.opportunity_epoch < 0
                    || !previous.state.terminal()
                    || previous.phase.is_empty()
                    || previous.phase.len() > 128
                    || previous.reason.is_empty()
                    || previous.reason.len() > 1_024
                    || previous.owner.as_ref().is_some_and(|owner| {
                        owner.schedule_basis.is_empty()
                            || owner.schedule_basis.len() > MAX_MAINTENANCE_SCHEDULE_BASIS_BYTES
                    })
                    || previous
                        .completed_at_unix_micros
                        .is_some_and(|value| value < 0)
            })
            || (self.state.terminal() != self.completed_at_unix_micros.is_some())
            || self.expected_checksum()? != self.checksum_sha256
        {
            return Err(MaintenanceError::InvalidCheckpoint);
        }
        Ok(())
    }

    fn expected_checksum(&self) -> Result<String, MaintenanceError> {
        #[derive(Serialize)]
        struct Body<'a> {
            format_version: u32,
            sequence: u64,
            key: &'a MaintenanceJobKey,
            opportunity_epoch: i64,
            attempt: Uuid,
            owner: &'a MaintenanceWorkerIdentity,
            recovered_dead_owner: bool,
            state: MaintenanceRunState,
            phase: &'a str,
            cursor: &'a Option<String>,
            units_completed: u64,
            bytes_completed: u64,
            started_at_unix_micros: i64,
            heartbeat_at_unix_micros: i64,
            completed_at_unix_micros: Option<i64>,
            terminal_reason: &'a Option<String>,
            previous_terminal: &'a Option<MaintenanceTerminalEvidence>,
            diagnostic_delivery: Option<MaintenanceDiagnosticDelivery>,
            diagnostic_gap: bool,
        }
        let bytes = serde_json::to_vec(&Body {
            format_version: self.format_version,
            sequence: self.sequence,
            key: &self.key,
            opportunity_epoch: self.opportunity_epoch,
            attempt: self.attempt,
            owner: &self.owner,
            recovered_dead_owner: self.recovered_dead_owner,
            state: self.state,
            phase: &self.phase,
            cursor: &self.cursor,
            units_completed: self.units_completed,
            bytes_completed: self.bytes_completed,
            started_at_unix_micros: self.started_at_unix_micros,
            heartbeat_at_unix_micros: self.heartbeat_at_unix_micros,
            completed_at_unix_micros: self.completed_at_unix_micros,
            terminal_reason: &self.terminal_reason,
            previous_terminal: &self.previous_terminal,
            diagnostic_delivery: self.diagnostic_delivery,
            diagnostic_gap: self.diagnostic_gap,
        })?;
        let mut digest = Sha256::new();
        digest.update(b"oulipoly.detached-maintenance-checkpoint.v1\0");
        digest.update(bytes);
        Ok(hex(&digest.finalize()))
    }

    fn canonical_bytes(&self) -> Result<Vec<u8>, MaintenanceError> {
        self.validate()?;
        let mut bytes = serde_json::to_vec(self)?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CancellationRecord {
    format_version: u32,
    opportunity_epoch: i64,
    requested_at_unix_micros: i64,
}

#[derive(Debug)]
pub enum AcquireMaintenanceJob {
    Acquired(MaintenanceJobLease),
    DuplicateLive(MaintenanceCheckpoint),
    AlreadyTerminal(MaintenanceCheckpoint),
}

#[derive(Debug, Clone)]
pub struct MaintenanceJobStore {
    root: PathBuf,
}

impl MaintenanceJobStore {
    pub fn open(data_root: impl AsRef<Path>) -> Result<Self, MaintenanceError> {
        let root = data_root.as_ref().join(MAINTENANCE_DIRECTORY);
        fs::create_dir_all(&root).map_err(|source| MaintenanceError::Io {
            operation: "create maintenance root",
            source,
        })?;
        set_private_directory(&root)?;
        sync_dir(&root)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn acquire(
        &self,
        key: MaintenanceJobKey,
        opportunity_epoch: i64,
        owner: MaintenanceWorkerIdentity,
        now_unix_micros: i64,
    ) -> Result<AcquireMaintenanceJob, MaintenanceError> {
        key.validate()?;
        if opportunity_epoch < 0 || now_unix_micros < 0 {
            return Err(MaintenanceError::InvalidTime);
        }
        let directory = self.job_directory(&key)?;
        fs::create_dir_all(&directory).map_err(|source| MaintenanceError::Io {
            operation: "create maintenance job directory",
            source,
        })?;
        set_private_directory(&directory)?;
        let lock_path = directory.join("owner.lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|source| MaintenanceError::Io {
                operation: "open maintenance owner lock",
                source,
            })?;
        match <File as fs4::FileExt>::try_lock(&lock) {
            Ok(()) => {}
            Err(fs4::TryLockError::WouldBlock) => {
                let checkpoint = read_selected_checkpoint(&directory)?
                    .ok_or(MaintenanceError::LiveOwnerWithoutCheckpoint)?;
                emit_request_observation(
                    &directory,
                    &checkpoint,
                    opportunity_epoch,
                    &owner,
                    "duplicate_live",
                    "exact_owner_lock_live",
                )
                .or_else(|error| {
                    write_request_evidence_gap(&directory, &checkpoint, &error.to_string())
                })
                .ok();
                return Ok(AcquireMaintenanceJob::DuplicateLive(checkpoint));
            }
            Err(fs4::TryLockError::Error(source)) => {
                return Err(MaintenanceError::Io {
                    operation: "lock maintenance job owner",
                    source,
                });
            }
        }
        let previous = read_selected_checkpoint(&directory)?;
        if let Some(previous) = previous.as_ref()
            && previous.state.terminal()
            && previous.opportunity_epoch >= opportunity_epoch
        {
            let observation = emit_request_observation(
                &directory,
                previous,
                opportunity_epoch,
                &owner,
                "already_terminal",
                "opportunity_already_terminal",
            );
            let returned = if let Err(error) = observation {
                mark_terminal_request_evidence_gap(&directory, previous, &error.to_string())
                    .unwrap_or_else(|_| previous.clone())
            } else {
                previous.clone()
            };
            let _ = <File as fs4::FileExt>::unlock(&lock);
            return Ok(AcquireMaintenanceJob::AlreadyTerminal(returned));
        }
        let recovered_dead_owner = previous
            .as_ref()
            .is_some_and(|checkpoint| checkpoint.state == MaintenanceRunState::Running);
        let cursor = previous
            .as_ref()
            .filter(|checkpoint| checkpoint.opportunity_epoch == opportunity_epoch)
            .and_then(|checkpoint| checkpoint.cursor.clone());
        let units_completed = previous
            .as_ref()
            .filter(|checkpoint| checkpoint.opportunity_epoch == opportunity_epoch)
            .map_or(0, |checkpoint| checkpoint.units_completed);
        let bytes_completed = previous
            .as_ref()
            .filter(|checkpoint| checkpoint.opportunity_epoch == opportunity_epoch)
            .map_or(0, |checkpoint| checkpoint.bytes_completed);
        let sequence = previous
            .as_ref()
            .map_or(0, |checkpoint| checkpoint.sequence + 1);
        let mut checkpoint = MaintenanceCheckpoint {
            format_version: MAINTENANCE_FORMAT_VERSION,
            sequence,
            key,
            opportunity_epoch,
            attempt: Uuid::new_v4(),
            owner,
            recovered_dead_owner,
            state: MaintenanceRunState::Running,
            phase: "acquired".to_string(),
            cursor,
            units_completed,
            bytes_completed,
            started_at_unix_micros: now_unix_micros,
            heartbeat_at_unix_micros: now_unix_micros,
            completed_at_unix_micros: None,
            terminal_reason: None,
            previous_terminal: previous.as_ref().and_then(|checkpoint| {
                if checkpoint.state.terminal() {
                    Some(MaintenanceTerminalEvidence {
                        opportunity_epoch: checkpoint.opportunity_epoch,
                        attempt: checkpoint.attempt,
                        state: checkpoint.state,
                        phase: checkpoint.phase.clone(),
                        reason: checkpoint.terminal_reason.clone().unwrap_or_default(),
                        owner: Some(checkpoint.owner.clone()),
                        completed_at_unix_micros: checkpoint.completed_at_unix_micros,
                    })
                } else {
                    checkpoint.previous_terminal.clone()
                }
            }),
            diagnostic_delivery: None,
            diagnostic_gap: false,
            checksum_sha256: String::new(),
        };
        checkpoint.checksum_sha256 = checkpoint.expected_checksum()?;
        let active_slot = write_checkpoint(&directory, None, &checkpoint)?;
        let mut lease = MaintenanceJobLease {
            directory,
            lock,
            checkpoint,
            active_slot,
            finished: false,
        };
        lease.emit_observation("acquired");
        Ok(AcquireMaintenanceJob::Acquired(lease))
    }

    pub fn read_status(
        &self,
        key: &MaintenanceJobKey,
    ) -> Result<Option<MaintenanceCheckpoint>, MaintenanceError> {
        key.validate()?;
        read_selected_checkpoint(&self.job_directory(key)?)
    }

    pub fn read_evidence(
        &self,
        key: &MaintenanceJobKey,
    ) -> Result<Option<MaintenanceEvidenceRecord>, MaintenanceError> {
        key.validate()?;
        read_selected_evidence(&self.job_directory(key)?)
    }

    pub fn read_request_evidence_gap(
        &self,
        key: &MaintenanceJobKey,
    ) -> Result<Option<MaintenanceRequestEvidenceGap>, MaintenanceError> {
        key.validate()?;
        let path = self.job_directory(key)?.join("request-evidence-gap.json");
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(MaintenanceError::Io {
                    operation: "read maintenance request evidence gap",
                    source,
                });
            }
        };
        if bytes.len() > 2_048 {
            return Err(MaintenanceError::InvalidEvidence);
        }
        let gap: MaintenanceRequestEvidenceGap = serde_json::from_slice(&bytes)?;
        gap.validate()?;
        if gap.canonical_bytes()? != bytes {
            return Err(MaintenanceError::InvalidEvidence);
        }
        Ok(Some(gap))
    }

    pub fn request_cancellation(
        &self,
        key: &MaintenanceJobKey,
        opportunity_epoch: i64,
        now_unix_micros: i64,
    ) -> Result<(), MaintenanceError> {
        key.validate()?;
        if opportunity_epoch < 0 || now_unix_micros < 0 {
            return Err(MaintenanceError::InvalidTime);
        }
        let directory = self.job_directory(key)?;
        fs::create_dir_all(&directory).map_err(|source| MaintenanceError::Io {
            operation: "create cancelled maintenance job directory",
            source,
        })?;
        set_private_directory(&directory)?;
        // Cancellation and the destructive source-directory move share this
        // exact transition gate. Whichever actor acquires it first owns the
        // before/after-move ordering observed by the caller.
        let transition = open_transition_gate(&directory)?;
        <File as fs4::FileExt>::lock(&transition).map_err(|source| MaintenanceError::Io {
            operation: "lock maintenance transition gate for cancellation",
            source,
        })?;
        write_cancellation(&directory, opportunity_epoch, now_unix_micros)
    }

    /// Operator-facing cancellation accepts only the exact currently admitted
    /// nonterminal epoch. Status validation and publication share the same gate
    /// as the destructive move, so the command cannot race into broad or stale
    /// cancellation authority.
    pub fn request_admitted_cancellation(
        &self,
        key: &MaintenanceJobKey,
        opportunity_epoch: i64,
        now_unix_micros: i64,
    ) -> Result<(), MaintenanceError> {
        key.validate()?;
        if opportunity_epoch < 0 || now_unix_micros < 0 {
            return Err(MaintenanceError::InvalidTime);
        }
        let directory = self.job_directory(key)?;
        let transition = open_transition_gate(&directory)?;
        <File as fs4::FileExt>::lock(&transition).map_err(|source| MaintenanceError::Io {
            operation: "lock maintenance transition gate for admitted cancellation",
            source,
        })?;
        let checkpoint = read_selected_checkpoint(&directory)?
            .ok_or(MaintenanceError::CancellationNotAdmitted)?;
        if checkpoint.opportunity_epoch != opportunity_epoch {
            return Err(MaintenanceError::CancellationEpochConflict {
                requested: opportunity_epoch,
                admitted: checkpoint.opportunity_epoch,
            });
        }
        if checkpoint.state.terminal() {
            return Err(MaintenanceError::CancellationAlreadyTerminal);
        }
        write_cancellation(&directory, opportunity_epoch, now_unix_micros)
    }

    pub fn read_cancellation_epoch(
        &self,
        key: &MaintenanceJobKey,
    ) -> Result<Option<i64>, MaintenanceError> {
        key.validate()?;
        let path = self.job_directory(key)?.join("cancel.json");
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(MaintenanceError::Io {
                    operation: "read maintenance cancellation for inspection",
                    source,
                });
            }
        };
        let record: CancellationRecord = serde_json::from_slice(&bytes)?;
        if record.format_version != MAINTENANCE_FORMAT_VERSION
            || record.opportunity_epoch < 0
            || record.requested_at_unix_micros < 0
        {
            return Err(MaintenanceError::InvalidTransition);
        }
        Ok(Some(record.opportunity_epoch))
    }

    fn job_directory(&self, key: &MaintenanceJobKey) -> Result<PathBuf, MaintenanceError> {
        Ok(self.root.join(key.storage_id()?))
    }
}

fn write_cancellation(
    directory: &Path,
    opportunity_epoch: i64,
    now_unix_micros: i64,
) -> Result<(), MaintenanceError> {
    let record = CancellationRecord {
        format_version: MAINTENANCE_FORMAT_VERSION,
        opportunity_epoch,
        requested_at_unix_micros: now_unix_micros,
    };
    replace_synced_file(directory, "cancel.json", &serde_json::to_vec(&record)?)
}

fn emit_request_observation(
    directory: &Path,
    checkpoint: &MaintenanceCheckpoint,
    requested_epoch: i64,
    requester: &MaintenanceWorkerIdentity,
    lifecycle_phase: &str,
    reason: &str,
) -> Result<(), MaintenanceError> {
    let mut evidence = MaintenanceEvidenceRecord {
        format_version: MAINTENANCE_FORMAT_VERSION,
        sequence: checkpoint.sequence.saturating_add(1),
        kind: MaintenanceEvidenceKind::Request,
        key: checkpoint.key.clone(),
        opportunity_epoch: requested_epoch,
        attempt: checkpoint.attempt,
        actor: requester.clone(),
        state: checkpoint.state,
        phase: lifecycle_phase.to_string(),
        cursor: checkpoint.cursor.clone(),
        units_completed: checkpoint.units_completed,
        bytes_completed: checkpoint.bytes_completed,
        reason: Some(format!(
            "{reason}; active_epoch={}; active_attempt={}",
            checkpoint.opportunity_epoch, checkpoint.attempt
        )),
        observed_at_unix_micros: now_unix_micros(),
        checksum_sha256: String::new(),
    };
    evidence.checksum_sha256 = evidence.expected_checksum()?;
    write_evidence(directory, &evidence)
}

fn mark_terminal_request_evidence_gap(
    directory: &Path,
    checkpoint: &MaintenanceCheckpoint,
    reason: &str,
) -> Result<MaintenanceCheckpoint, MaintenanceError> {
    let _ = write_request_evidence_gap(directory, checkpoint, reason);
    let active_slot = selected_checkpoint_slot(directory, checkpoint)?;
    let mut updated = checkpoint.clone();
    updated.sequence = updated.sequence.saturating_add(1);
    updated.diagnostic_delivery = Some(MaintenanceDiagnosticDelivery::Gap);
    updated.diagnostic_gap = true;
    updated.checksum_sha256 = updated.expected_checksum()?;
    write_checkpoint(directory, Some(active_slot), &updated)?;
    Ok(updated)
}

fn write_request_evidence_gap(
    directory: &Path,
    checkpoint: &MaintenanceCheckpoint,
    reason: &str,
) -> Result<(), MaintenanceError> {
    let reason = truncate(reason, 512);
    let mut gap = MaintenanceRequestEvidenceGap {
        format_version: MAINTENANCE_FORMAT_VERSION,
        opportunity_epoch: checkpoint.opportunity_epoch,
        attempt: checkpoint.attempt,
        reason,
        checksum_sha256: String::new(),
    };
    gap.checksum_sha256 = gap.expected_checksum()?;
    replace_synced_file(
        directory,
        "request-evidence-gap.json",
        &gap.canonical_bytes()?,
    )
}

#[derive(Debug)]
pub struct MaintenanceJobLease {
    directory: PathBuf,
    lock: File,
    checkpoint: MaintenanceCheckpoint,
    active_slot: usize,
    finished: bool,
}

impl MaintenanceJobLease {
    pub fn checkpoint(&self) -> &MaintenanceCheckpoint {
        &self.checkpoint
    }

    pub fn is_cancelled(&self) -> Result<bool, MaintenanceError> {
        let path = self.directory.join("cancel.json");
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(source) => {
                return Err(MaintenanceError::Io {
                    operation: "read maintenance cancellation",
                    source,
                });
            }
        };
        let record: CancellationRecord = serde_json::from_slice(&bytes)?;
        Ok(record.format_version == MAINTENANCE_FORMAT_VERSION
            && record.opportunity_epoch == self.checkpoint.opportunity_epoch)
    }

    pub fn lock_transition(&self) -> Result<MaintenanceTransitionGuard, MaintenanceError> {
        let file = open_transition_gate(&self.directory)?;
        <File as fs4::FileExt>::lock(&file).map_err(|source| MaintenanceError::Io {
            operation: "lock maintenance destructive transition gate",
            source,
        })?;
        Ok(MaintenanceTransitionGuard { file })
    }

    pub fn record_progress(
        &mut self,
        phase: impl Into<String>,
        cursor: Option<String>,
        units_completed: u64,
        bytes_completed: u64,
        now_unix_micros: i64,
    ) -> Result<(), MaintenanceError> {
        if self.finished || now_unix_micros < self.checkpoint.heartbeat_at_unix_micros {
            return Err(MaintenanceError::InvalidTransition);
        }
        let phase = phase.into();
        if phase.is_empty()
            || phase.len() > 128
            || cursor
                .as_ref()
                .is_some_and(|value| value.len() > MAX_MAINTENANCE_CURSOR_BYTES)
            || units_completed < self.checkpoint.units_completed
            || bytes_completed < self.checkpoint.bytes_completed
        {
            return Err(MaintenanceError::InvalidTransition);
        }
        self.checkpoint.sequence += 1;
        self.checkpoint.phase = phase.clone();
        self.checkpoint.cursor = cursor;
        self.checkpoint.units_completed = units_completed;
        self.checkpoint.bytes_completed = bytes_completed;
        self.checkpoint.heartbeat_at_unix_micros = now_unix_micros;
        self.checkpoint.diagnostic_delivery = None;
        self.checkpoint.diagnostic_gap = false;
        self.rewrite()?;
        self.emit_observation(&phase);
        Ok(())
    }

    pub fn finish(
        &mut self,
        state: MaintenanceRunState,
        phase: impl Into<String>,
        reason: impl Into<String>,
        now_unix_micros: i64,
    ) -> Result<(), MaintenanceError> {
        if self.finished
            || !state.terminal()
            || now_unix_micros < self.checkpoint.heartbeat_at_unix_micros
        {
            return Err(MaintenanceError::InvalidTransition);
        }
        let phase = phase.into();
        let reason = reason.into();
        if phase.is_empty() || phase.len() > 128 || reason.is_empty() || reason.len() > 1_024 {
            return Err(MaintenanceError::InvalidTransition);
        }
        self.checkpoint.sequence += 1;
        self.checkpoint.state = state;
        self.checkpoint.phase = phase.clone();
        self.checkpoint.heartbeat_at_unix_micros = now_unix_micros;
        self.checkpoint.completed_at_unix_micros = Some(now_unix_micros);
        self.checkpoint.terminal_reason = Some(reason);
        self.checkpoint.diagnostic_delivery = None;
        self.checkpoint.diagnostic_gap = false;
        self.rewrite()?;
        self.emit_observation(&phase);
        self.finished = true;
        Ok(())
    }

    pub fn yield_run(
        &mut self,
        phase: impl Into<String>,
        reason: impl Into<String>,
        now_unix_micros: i64,
    ) -> Result<(), MaintenanceError> {
        if self.finished || now_unix_micros < self.checkpoint.heartbeat_at_unix_micros {
            return Err(MaintenanceError::InvalidTransition);
        }
        let phase = phase.into();
        let reason = reason.into();
        if phase.is_empty() || phase.len() > 128 || reason.is_empty() || reason.len() > 1_024 {
            return Err(MaintenanceError::InvalidTransition);
        }
        self.checkpoint.sequence += 1;
        self.checkpoint.state = MaintenanceRunState::Yielded;
        self.checkpoint.phase = phase.clone();
        self.checkpoint.heartbeat_at_unix_micros = now_unix_micros;
        self.checkpoint.terminal_reason = Some(reason);
        self.checkpoint.diagnostic_delivery = None;
        self.checkpoint.diagnostic_gap = false;
        self.rewrite()?;
        self.emit_observation(&phase);
        self.finished = true;
        Ok(())
    }

    fn rewrite(&mut self) -> Result<(), MaintenanceError> {
        self.checkpoint.checksum_sha256 = self.checkpoint.expected_checksum()?;
        self.active_slot =
            write_checkpoint(&self.directory, Some(self.active_slot), &self.checkpoint)?;
        Ok(())
    }

    fn emit_observation(&mut self, lifecycle_phase: &str) {
        let mut evidence = MaintenanceEvidenceRecord {
            format_version: MAINTENANCE_FORMAT_VERSION,
            sequence: self.checkpoint.sequence,
            kind: MaintenanceEvidenceKind::Job,
            key: self.checkpoint.key.clone(),
            opportunity_epoch: self.checkpoint.opportunity_epoch,
            attempt: self.checkpoint.attempt,
            actor: self.checkpoint.owner.clone(),
            state: self.checkpoint.state,
            phase: lifecycle_phase.to_string(),
            cursor: self.checkpoint.cursor.clone(),
            units_completed: self.checkpoint.units_completed,
            bytes_completed: self.checkpoint.bytes_completed,
            reason: self.checkpoint.terminal_reason.clone(),
            observed_at_unix_micros: self.checkpoint.heartbeat_at_unix_micros,
            checksum_sha256: String::new(),
        };
        let delivery = evidence.expected_checksum().and_then(|checksum| {
            evidence.checksum_sha256 = checksum;
            write_evidence(&self.directory, &evidence)
        });
        self.checkpoint.diagnostic_delivery = Some(if delivery.is_ok() {
            MaintenanceDiagnosticDelivery::Appended
        } else {
            self.checkpoint.diagnostic_gap = true;
            MaintenanceDiagnosticDelivery::Gap
        });
        // Diagnostics are evidence only. Persisting their delivery result is a
        // separate best-effort checkpoint and cannot change the work outcome.
        self.checkpoint.sequence += 1;
        let _ = self.rewrite();
    }
}

#[derive(Debug)]
pub struct MaintenanceTransitionGuard {
    file: File,
}

impl Drop for MaintenanceTransitionGuard {
    fn drop(&mut self) {
        let _ = <File as fs4::FileExt>::unlock(&self.file);
    }
}

impl Drop for MaintenanceJobLease {
    fn drop(&mut self) {
        let _ = <File as fs4::FileExt>::unlock(&self.lock);
    }
}

#[derive(Debug)]
pub enum MaintenanceError {
    Io {
        operation: &'static str,
        source: io::Error,
    },
    Json(serde_json::Error),
    Identity(String),
    InvalidKey,
    InvalidTime,
    InvalidCheckpoint,
    InvalidEvidence,
    InvalidTransition,
    CheckpointConflict,
    EvidenceConflict,
    CancellationNotAdmitted,
    CancellationEpochConflict {
        requested: i64,
        admitted: i64,
    },
    CancellationAlreadyTerminal,
    LiveOwnerWithoutCheckpoint,
}

impl fmt::Display for MaintenanceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { operation, source } => write!(formatter, "{operation}: {source}"),
            Self::Json(error) => write!(formatter, "maintenance JSON error: {error}"),
            Self::Identity(error) => write!(formatter, "maintenance owner identity error: {error}"),
            Self::InvalidKey => write!(formatter, "invalid maintenance job key"),
            Self::InvalidTime => write!(formatter, "invalid maintenance timestamp or opportunity"),
            Self::InvalidCheckpoint => write!(formatter, "invalid maintenance checkpoint"),
            Self::InvalidEvidence => write!(formatter, "invalid maintenance evidence"),
            Self::InvalidTransition => {
                write!(formatter, "invalid maintenance lifecycle transition")
            }
            Self::CheckpointConflict => write!(formatter, "maintenance checkpoint slots conflict"),
            Self::EvidenceConflict => write!(formatter, "maintenance evidence slots conflict"),
            Self::CancellationNotAdmitted => {
                write!(
                    formatter,
                    "exact maintenance job has no admitted checkpoint"
                )
            }
            Self::CancellationEpochConflict {
                requested,
                admitted,
            } => write!(
                formatter,
                "maintenance epoch conflict: requested {requested}, exact job is at {admitted}"
            ),
            Self::CancellationAlreadyTerminal => {
                write!(
                    formatter,
                    "exact maintenance opportunity is already terminal"
                )
            }
            Self::LiveOwnerWithoutCheckpoint => write!(
                formatter,
                "live maintenance owner lacks a durable checkpoint"
            ),
        }
    }
}

impl std::error::Error for MaintenanceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<serde_json::Error> for MaintenanceError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

pub fn opportunity_epoch_utc() -> i64 {
    Utc::now().timestamp().max(0) / (24 * 60 * 60)
}

pub fn now_unix_micros() -> i64 {
    Utc::now().timestamp_micros().max(0)
}

fn read_selected_checkpoint(
    directory: &Path,
) -> Result<Option<MaintenanceCheckpoint>, MaintenanceError> {
    let mut valid = Vec::new();
    for slot in 0..=1 {
        let path = directory.join(format!("status.{slot}"));
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(MaintenanceError::Io {
                    operation: "read maintenance checkpoint",
                    source,
                });
            }
        };
        let checkpoint: MaintenanceCheckpoint = match serde_json::from_slice(&bytes) {
            Ok(checkpoint) => checkpoint,
            Err(_) => continue,
        };
        if checkpoint.validate().is_ok()
            && checkpoint
                .canonical_bytes()
                .is_ok_and(|canonical| canonical == bytes)
        {
            valid.push(checkpoint);
        }
    }
    valid.sort_by_key(|checkpoint| checkpoint.sequence);
    if valid.len() == 2 && valid[0].sequence == valid[1].sequence && valid[0] != valid[1] {
        return Err(MaintenanceError::CheckpointConflict);
    }
    Ok(valid.pop())
}

fn selected_checkpoint_slot(
    directory: &Path,
    selected: &MaintenanceCheckpoint,
) -> Result<usize, MaintenanceError> {
    for slot in 0..=1 {
        let bytes = match fs::read(directory.join(format!("status.{slot}"))) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(MaintenanceError::Io {
                    operation: "read selected maintenance checkpoint slot",
                    source,
                });
            }
        };
        if serde_json::from_slice::<MaintenanceCheckpoint>(&bytes)
            .is_ok_and(|candidate| candidate == *selected)
        {
            return Ok(slot);
        }
    }
    Err(MaintenanceError::CheckpointConflict)
}

fn truncate(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

fn read_selected_evidence(
    directory: &Path,
) -> Result<Option<MaintenanceEvidenceRecord>, MaintenanceError> {
    let mut valid = Vec::new();
    for slot in 0..=1 {
        let path = directory.join(format!("evidence.{slot}"));
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(MaintenanceError::Io {
                    operation: "read maintenance evidence",
                    source,
                });
            }
        };
        let evidence: MaintenanceEvidenceRecord = match serde_json::from_slice(&bytes) {
            Ok(evidence) => evidence,
            Err(_) => continue,
        };
        if evidence.validate().is_ok()
            && evidence
                .canonical_bytes()
                .is_ok_and(|canonical| canonical == bytes)
        {
            valid.push(evidence);
        }
    }
    valid.sort_by_key(|evidence| evidence.sequence);
    if valid.len() == 2 && valid[0].sequence == valid[1].sequence && valid[0] != valid[1] {
        return Err(MaintenanceError::EvidenceConflict);
    }
    Ok(valid.pop())
}

fn write_evidence(
    directory: &Path,
    evidence: &MaintenanceEvidenceRecord,
) -> Result<(), MaintenanceError> {
    evidence.validate()?;
    let mut evidence = evidence.clone();
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join("evidence.lock"))
        .map_err(|source| MaintenanceError::Io {
            operation: "open maintenance evidence lock",
            source,
        })?;
    <File as fs4::FileExt>::lock(&lock).map_err(|source| MaintenanceError::Io {
        operation: "lock maintenance evidence sink",
        source,
    })?;
    let selected = read_selected_evidence(directory)?;
    if selected
        .as_ref()
        .is_some_and(|selected| evidence.sequence <= selected.sequence)
    {
        evidence.sequence = selected
            .as_ref()
            .expect("selected evidence exists")
            .sequence
            .saturating_add(1);
        evidence.checksum_sha256 = evidence.expected_checksum()?;
    }
    let active_slot = selected.as_ref().and_then(|selected| {
        (0..=1).find(|slot| {
            fs::read(directory.join(format!("evidence.{slot}")))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<MaintenanceEvidenceRecord>(&bytes).ok())
                .is_some_and(|candidate| candidate == *selected)
        })
    });
    let slot = active_slot.map_or(0, |active| 1 - active);
    replace_synced_file(
        directory,
        &format!("evidence.{slot}"),
        &evidence.canonical_bytes()?,
    )
}

fn open_transition_gate(directory: &Path) -> Result<File, MaintenanceError> {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join("transition.lock"))
        .map_err(|source| MaintenanceError::Io {
            operation: "open maintenance transition gate",
            source,
        })
}

fn write_checkpoint(
    directory: &Path,
    active_slot: Option<usize>,
    checkpoint: &MaintenanceCheckpoint,
) -> Result<usize, MaintenanceError> {
    let slot = active_slot.map_or(0, |active| 1 - active);
    replace_synced_file(
        directory,
        &format!("status.{slot}"),
        &checkpoint.canonical_bytes()?,
    )?;
    Ok(slot)
}

fn replace_synced_file(directory: &Path, name: &str, bytes: &[u8]) -> Result<(), MaintenanceError> {
    let temp = directory.join(format!(".{name}.{}.tmp", Uuid::new_v4().simple()));
    let target = directory.join(name);
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)
        .map_err(|source| MaintenanceError::Io {
            operation: "create maintenance checkpoint temp",
            source,
        })?;
    file.write_all(bytes)
        .map_err(|source| MaintenanceError::Io {
            operation: "write maintenance checkpoint temp",
            source,
        })?;
    file.sync_all().map_err(|source| MaintenanceError::Io {
        operation: "sync maintenance checkpoint temp",
        source,
    })?;
    drop(file);
    #[cfg(windows)]
    if target.exists() {
        fs::remove_file(&target).map_err(|source| MaintenanceError::Io {
            operation: "replace maintenance checkpoint",
            source,
        })?;
    }
    fs::rename(&temp, &target).map_err(|source| MaintenanceError::Io {
        operation: "publish maintenance checkpoint",
        source,
    })?;
    sync_dir(directory)
}

fn sync_dir(path: &Path) -> Result<(), MaintenanceError> {
    let directory = File::open(path).map_err(|source| MaintenanceError::Io {
        operation: "open maintenance directory for sync",
        source,
    })?;
    directory.sync_all().map_err(|source| MaintenanceError::Io {
        operation: "sync maintenance directory",
        source,
    })
}

#[cfg(unix)]
fn set_private_directory(path: &Path) -> Result<(), MaintenanceError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|source| {
        MaintenanceError::Io {
            operation: "set maintenance directory permissions",
            source,
        }
    })
}

#[cfg(not(unix))]
fn set_private_directory(_path: &Path) -> Result<(), MaintenanceError> {
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use fmt::Write as _;
        let _ = write!(value, "{byte:02x}");
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn owner(pid: i64) -> MaintenanceWorkerIdentity {
        MaintenanceWorkerIdentity {
            worker_instance_id: Uuid::new_v4(),
            os_pid: pid,
            os_boot_id: "boot".to_string(),
            os_pid_starttime_ticks: pid * 10,
            schedule_basis: "test".into(),
            launch_id: None,
        }
    }

    fn acquire(
        store: &MaintenanceJobStore,
        key: MaintenanceJobKey,
        epoch: i64,
        owner: MaintenanceWorkerIdentity,
        now: i64,
    ) -> AcquireMaintenanceJob {
        store.acquire(key, epoch, owner, now).unwrap()
    }

    #[test]
    fn duplicate_live_worker_cannot_be_evicted_by_stale_heartbeat() {
        let root = tempfile::tempdir().unwrap();
        let store = MaintenanceJobStore::open(root.path()).unwrap();
        let key = MaintenanceJobKey::new(MaintenanceJobKind::EventRetirement, "aa/bb").unwrap();
        let first = acquire(&store, key.clone(), 7, owner(10), 100);
        let AcquireMaintenanceJob::Acquired(_lease) = first else {
            panic!("first worker did not acquire");
        };
        let duplicate = acquire(&store, key, 7, owner(11), 9_000_000);
        assert!(matches!(duplicate, AcquireMaintenanceJob::DuplicateLive(_)));
    }

    #[test]
    fn released_live_lock_recovers_running_checkpoint_as_dead_owner() {
        let root = tempfile::tempdir().unwrap();
        let store = MaintenanceJobStore::open(root.path()).unwrap();
        let key = MaintenanceJobKey::new(MaintenanceJobKind::EventDiscovery, "aa/bb").unwrap();
        let AcquireMaintenanceJob::Acquired(mut first) =
            acquire(&store, key.clone(), 4, owner(1), 10)
        else {
            panic!();
        };
        first
            .record_progress("scan", Some("cursor-4".into()), 4, 40, 20)
            .unwrap();
        drop(first);
        let AcquireMaintenanceJob::Acquired(recovered) = acquire(&store, key, 4, owner(2), 30)
        else {
            panic!();
        };
        assert!(recovered.checkpoint().recovered_dead_owner);
        assert_eq!(recovered.checkpoint().cursor.as_deref(), Some("cursor-4"));
        assert_eq!(recovered.checkpoint().units_completed, 4);
    }

    #[test]
    fn terminal_epoch_deduplicates_daily_and_new_epoch_restarts_without_old_cursor() {
        let root = tempfile::tempdir().unwrap();
        let store = MaintenanceJobStore::open(root.path()).unwrap();
        let key =
            MaintenanceJobKey::new(MaintenanceJobKind::EventDiscovery, "event-store-v1").unwrap();
        let AcquireMaintenanceJob::Acquired(mut first) =
            acquire(&store, key.clone(), 12, owner(1), 100)
        else {
            panic!();
        };
        first
            .record_progress("scan", Some("writer-a".into()), 1, 0, 110)
            .unwrap();
        first
            .finish(
                MaintenanceRunState::Succeeded,
                "complete",
                "bounded scan complete",
                120,
            )
            .unwrap();
        drop(first);
        assert!(matches!(
            acquire(&store, key.clone(), 12, owner(2), 130),
            AcquireMaintenanceJob::AlreadyTerminal(_)
        ));
        let AcquireMaintenanceJob::Acquired(next) = acquire(&store, key, 13, owner(3), 140) else {
            panic!();
        };
        assert_eq!(next.checkpoint().cursor, None);
        assert_eq!(next.checkpoint().units_completed, 0);
        let previous = next.checkpoint().previous_terminal.as_ref().unwrap();
        assert_eq!(previous.opportunity_epoch, 12);
        assert_eq!(previous.state, MaintenanceRunState::Succeeded);
        assert_eq!(previous.phase, "complete");
        assert_eq!(previous.reason, "bounded scan complete");
        assert_eq!(previous.owner.as_ref().unwrap().schedule_basis, "test");
        assert_eq!(previous.owner.as_ref().unwrap().launch_id, None);
        assert_eq!(previous.completed_at_unix_micros, Some(120));
    }

    #[test]
    fn rejected_terminal_request_evidence_failure_marks_incumbent_gap() {
        let root = tempfile::tempdir().unwrap();
        let store = MaintenanceJobStore::open(root.path()).unwrap();
        let key =
            MaintenanceJobKey::new(MaintenanceJobKind::EventDiscovery, "event-store-v1").unwrap();
        let AcquireMaintenanceJob::Acquired(mut first) =
            acquire(&store, key.clone(), 9, owner(1), 10)
        else {
            panic!();
        };
        first
            .finish(MaintenanceRunState::Succeeded, "complete", "complete", 20)
            .unwrap();
        drop(first);
        let directory = store.job_directory(&key).unwrap();
        fs::remove_file(directory.join("evidence.lock")).unwrap();
        fs::create_dir(directory.join("evidence.lock")).unwrap();

        let AcquireMaintenanceJob::AlreadyTerminal(rejected) =
            acquire(&store, key.clone(), 9, owner(2), 30)
        else {
            panic!();
        };
        assert!(rejected.diagnostic_gap);
        assert_eq!(
            rejected.diagnostic_delivery,
            Some(MaintenanceDiagnosticDelivery::Gap)
        );
        assert!(directory.join("request-evidence-gap.json").is_file());
        assert!(store.read_status(&key).unwrap().unwrap().diagnostic_gap);
    }

    #[test]
    fn cancellation_is_epoch_fenced_and_terminal_evidence_is_offline_readable() {
        let root = tempfile::tempdir().unwrap();
        let store = MaintenanceJobStore::open(root.path()).unwrap();
        let key =
            MaintenanceJobKey::new(MaintenanceJobKind::HistoricalCompaction, "aa/bb").unwrap();
        store.request_cancellation(&key, 5, 10).unwrap();
        let AcquireMaintenanceJob::Acquired(mut lease) =
            acquire(&store, key.clone(), 5, owner(1), 20)
        else {
            panic!();
        };
        assert!(lease.is_cancelled().unwrap());
        lease
            .finish(
                MaintenanceRunState::Cancelled,
                "cancelled",
                "request observed",
                30,
            )
            .unwrap();
        drop(lease);
        let status = store.read_status(&key).unwrap().unwrap();
        assert_eq!(status.state, MaintenanceRunState::Cancelled);
        assert_eq!(
            status.diagnostic_delivery,
            Some(MaintenanceDiagnosticDelivery::Appended)
        );
        assert!(!status.diagnostic_gap);
        let evidence = store.read_evidence(&key).unwrap().unwrap();
        assert_eq!(evidence.state, MaintenanceRunState::Cancelled);
        assert_eq!(evidence.reason.as_deref(), Some("request observed"));

        let AcquireMaintenanceJob::Acquired(next) = acquire(&store, key, 6, owner(2), 40) else {
            panic!();
        };
        assert!(!next.is_cancelled().unwrap());
    }

    #[test]
    fn unavailable_direct_evidence_is_an_explicit_gap_without_event_writer_fallback() {
        let root = tempfile::tempdir().unwrap();
        let store = MaintenanceJobStore::open(root.path()).unwrap();
        let key =
            MaintenanceJobKey::new(MaintenanceJobKind::EventDiscovery, "event-store-v1").unwrap();
        let job_directory = store.job_directory(&key).unwrap();
        fs::create_dir_all(job_directory.join("evidence.0")).unwrap();

        let AcquireMaintenanceJob::Acquired(mut lease) =
            acquire(&store, key.clone(), 5, owner(1), 20)
        else {
            panic!();
        };
        assert_eq!(
            lease.checkpoint().diagnostic_delivery,
            Some(MaintenanceDiagnosticDelivery::Gap)
        );
        assert!(lease.checkpoint().diagnostic_gap);
        lease
            .finish(
                MaintenanceRunState::Preserved,
                "evidence_unavailable",
                "work outcome remains authoritative",
                30,
            )
            .unwrap();
        drop(lease);
        let status = store.read_status(&key).unwrap().unwrap();
        assert!(status.diagnostic_gap);
        assert_eq!(
            status.diagnostic_delivery,
            Some(MaintenanceDiagnosticDelivery::Gap)
        );
        assert!(
            !root
                .path()
                .join("diagnostics/event-store-v1/writers")
                .exists()
        );
    }

    #[test]
    fn subprocess_live_duplicate_and_abrupt_death_recovery_use_kernel_ownership() {
        const MODE: &str = "OULIPOLY_MAINTENANCE_LEASE_FIXTURE";
        if std::env::var_os(MODE).is_some() {
            let root = PathBuf::from(std::env::var_os("OULIPOLY_MAINTENANCE_TEST_ROOT").unwrap());
            let ready = PathBuf::from(std::env::var_os("OULIPOLY_MAINTENANCE_TEST_READY").unwrap());
            let store = MaintenanceJobStore::open(&root).unwrap();
            let key =
                MaintenanceJobKey::new(MaintenanceJobKind::EventDiscovery, "subprocess").unwrap();
            let AcquireMaintenanceJob::Acquired(_lease) = store
                .acquire(key, 4, owner(i64::from(std::process::id())), 10)
                .unwrap()
            else {
                panic!("fixture did not acquire lease");
            };
            fs::write(ready, b"ready").unwrap();
            std::thread::sleep(Duration::from_secs(30));
            return;
        }

        let root = tempfile::tempdir().unwrap();
        let ready = root.path().join("ready");
        let exact = "maintenance::tests::subprocess_live_duplicate_and_abrupt_death_recovery_use_kernel_ownership";
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", exact, "--nocapture"])
            .env(MODE, "child")
            .env("OULIPOLY_MAINTENANCE_TEST_ROOT", root.path())
            .env("OULIPOLY_MAINTENANCE_TEST_READY", &ready)
            .spawn()
            .unwrap();
        let started = Instant::now();
        while !ready.is_file() {
            assert!(started.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(10));
        }
        let store = MaintenanceJobStore::open(root.path()).unwrap();
        let key = MaintenanceJobKey::new(MaintenanceJobKind::EventDiscovery, "subprocess").unwrap();
        assert!(matches!(
            store.acquire(key.clone(), 4, owner(2), 20).unwrap(),
            AcquireMaintenanceJob::DuplicateLive(_)
        ));
        child.kill().unwrap();
        let status = child.wait().unwrap();
        assert!(!status.success());
        let AcquireMaintenanceJob::Acquired(recovered) =
            store.acquire(key, 4, owner(3), 30).unwrap()
        else {
            panic!("dead subprocess lock was not recovered");
        };
        assert!(recovered.checkpoint().recovered_dead_owner);
    }
}
