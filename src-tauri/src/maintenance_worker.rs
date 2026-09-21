//! Detached AGE-377 process boundary and cheap durable startup admission.

use crate::usage::cli::{Cli, Subcommands};
use oulipoly_state::detached_maintenance::{
    CoordinationRetentionOutcome, OpportunityOutcome, OpportunityWorkLimits,
    run_coordination_retention_opportunity, run_event_store_opportunity,
};
use oulipoly_state::maintenance::{
    AcquireMaintenanceJob, MAINTENANCE_DIRECTORY, MAX_MAINTENANCE_SCHEDULE_BASIS_BYTES,
    MaintenanceJobKey, MaintenanceJobKind, MaintenanceJobStore, MaintenanceRunState,
    MaintenanceWorkerIdentity, now_unix_micros, opportunity_epoch_utc,
};
use oulipoly_state::pid_identity::{
    ProcessIdentity, ProcessIdentityObservation, observe_live_process_identity,
    read_live_process_identity,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use uuid::Uuid;

pub(crate) const WORKER_ARG: &str = "--oulipoly-detached-maintenance-worker-v1";
const LAUNCH_FORMAT_VERSION: u32 = 2;
const LAUNCH_FALLBACK_FORMAT_VERSION: u32 = 1;
const MIDNIGHT_ALLOWANCE_SECONDS: i64 = 15 * 60;
const LAUNCH_RETENTION_DAYS: i64 = 30;
const LAUNCH_CLEANUP_EPOCHS_PER_RUN: usize = 4;
const LAUNCH_CLEANUP_ENTRIES_PER_RUN: usize = 64;
const OLDEST_LAUNCH_EPOCH_FILE: &str = "oldest-epoch.json";
const ERROR_CARRIER_ENV: &str = "OULIPOLY_MAINTENANCE_ERROR_CARRIER";
const ERROR_EPOCH_ENV: &str = "OULIPOLY_MAINTENANCE_ERROR_EPOCH";
const ERROR_LAUNCH_ID_ENV: &str = "OULIPOLY_MAINTENANCE_ERROR_LAUNCH_ID";
const ERROR_BASIS_ENV: &str = "OULIPOLY_MAINTENANCE_ERROR_BASIS";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScheduleBasis {
    GuiStartup,
    ProviderStartup,
}

impl ScheduleBasis {
    fn as_str(self) -> &'static str {
        match self {
            Self::GuiStartup => "gui_startup",
            Self::ProviderStartup => "provider_startup",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScheduleOutcome {
    Spawned,
    SuppressedLive,
    SuppressedComplete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchProcessIdentity {
    os_pid: i64,
    os_boot_id: String,
    os_pid_starttime_ticks: i64,
}

impl From<ProcessIdentity> for LaunchProcessIdentity {
    fn from(value: ProcessIdentity) -> Self {
        Self {
            os_pid: value.os_pid,
            os_boot_id: value.os_boot_id,
            os_pid_starttime_ticks: value.os_pid_starttime_ticks,
        }
    }
}

impl LaunchProcessIdentity {
    fn matches(&self, value: &ProcessIdentity) -> bool {
        self.os_pid == value.os_pid
            && self.os_boot_id == value.os_boot_id
            && self.os_pid_starttime_ticks == value.os_pid_starttime_ticks
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchRecord {
    format_version: u32,
    opportunity_epoch: i64,
    launch_id: Uuid,
    schedule_basis: String,
    state: LaunchAdmissionState,
    child: Option<LaunchProcessIdentity>,
    outcome: Option<LaunchOutcome>,
    parent_error: Option<String>,
    child_error: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    parent_evidence_gap: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    child_evidence_gap: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    fallback_armed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous_launch: Option<LaunchTerminalEvidence>,
    checksum_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchTerminalEvidence {
    opportunity_epoch: i64,
    launch_id: Uuid,
    schedule_basis: String,
    child: Option<LaunchProcessIdentity>,
    outcome: LaunchOutcome,
    parent_error: Option<String>,
    child_error: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    parent_evidence_gap: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    child_evidence_gap: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LaunchAdmissionState {
    Intent,
    Admitted,
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LaunchOutcome {
    Completed,
    Yielded,
    Cancelled,
    PreservedGapped,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OldestLaunchEpoch {
    format_version: u32,
    epoch: i64,
    checksum_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchCleanupCursor {
    next_epoch: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    first_gap: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LaunchCleanupOutcome {
    Completed,
    Yielded,
    Duplicate,
    PreservedGap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LaunchFallbackState {
    Armed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchFallbackRecord {
    format_version: u32,
    opportunity_epoch: i64,
    launch_id: Uuid,
    schedule_basis: String,
    state: LaunchFallbackState,
    error: Option<String>,
    checksum_sha256: String,
}

impl LaunchFallbackRecord {
    fn expected_checksum(&self) -> Result<String, String> {
        #[derive(Serialize)]
        struct Body<'a> {
            format_version: u32,
            opportunity_epoch: i64,
            launch_id: Uuid,
            schedule_basis: &'a str,
            state: LaunchFallbackState,
            error: &'a Option<String>,
        }
        let bytes = serde_json::to_vec(&Body {
            format_version: self.format_version,
            opportunity_epoch: self.opportunity_epoch,
            launch_id: self.launch_id,
            schedule_basis: &self.schedule_basis,
            state: self.state,
            error: &self.error,
        })
        .map_err(|error| error.to_string())?;
        let mut digest = Sha256::new();
        digest.update(b"oulipoly.maintenance-launch-fallback.v1\0");
        digest.update(bytes);
        Ok(hex(&digest.finalize()))
    }

    fn validate(&self) -> Result<(), String> {
        if self.format_version != LAUNCH_FALLBACK_FORMAT_VERSION
            || self.opportunity_epoch < 0
            || self.schedule_basis.is_empty()
            || self.schedule_basis.len() > MAX_MAINTENANCE_SCHEDULE_BASIS_BYTES
            || self.error.as_ref().is_some_and(|error| error.len() > 1_024)
            || (self.state == LaunchFallbackState::Failed && self.error.is_none())
            || self.expected_checksum()? != self.checksum_sha256
        {
            return Err("invalid detached maintenance launch fallback".to_string());
        }
        Ok(())
    }
}

impl LaunchRecord {
    fn expected_checksum(&self) -> Result<String, String> {
        #[derive(Serialize)]
        struct Body<'a> {
            format_version: u32,
            opportunity_epoch: i64,
            launch_id: Uuid,
            schedule_basis: &'a str,
            state: LaunchAdmissionState,
            child: &'a Option<LaunchProcessIdentity>,
            outcome: &'a Option<LaunchOutcome>,
            parent_error: &'a Option<String>,
            child_error: &'a Option<String>,
            #[serde(skip_serializing_if = "std::ops::Not::not")]
            parent_evidence_gap: bool,
            #[serde(skip_serializing_if = "std::ops::Not::not")]
            child_evidence_gap: bool,
            #[serde(skip_serializing_if = "std::ops::Not::not")]
            fallback_armed: bool,
            #[serde(skip_serializing_if = "Option::is_none")]
            previous_launch: &'a Option<LaunchTerminalEvidence>,
        }
        let bytes = serde_json::to_vec(&Body {
            format_version: self.format_version,
            opportunity_epoch: self.opportunity_epoch,
            launch_id: self.launch_id,
            schedule_basis: &self.schedule_basis,
            state: self.state,
            child: &self.child,
            outcome: &self.outcome,
            parent_error: &self.parent_error,
            child_error: &self.child_error,
            parent_evidence_gap: self.parent_evidence_gap,
            child_evidence_gap: self.child_evidence_gap,
            fallback_armed: self.fallback_armed,
            previous_launch: &self.previous_launch,
        })
        .map_err(|error| error.to_string())?;
        let mut digest = Sha256::new();
        digest.update(b"oulipoly.maintenance-launch.v2\0");
        digest.update(bytes);
        Ok(hex(&digest.finalize()))
    }

    fn validate(&self) -> Result<(), String> {
        let terminal_shape = self.state == LaunchAdmissionState::Terminal && self.outcome.is_some();
        let nonterminal_shape = self.state != LaunchAdmissionState::Terminal
            && self.outcome.is_none()
            && self.parent_error.is_none()
            && self.child_error.is_none()
            && !self.parent_evidence_gap
            && !self.child_evidence_gap;
        let child_shape = match self.state {
            LaunchAdmissionState::Intent => self.child.is_none(),
            LaunchAdmissionState::Admitted => self.child.is_some(),
            LaunchAdmissionState::Terminal => true,
        };
        let errors_valid = [&self.parent_error, &self.child_error]
            .into_iter()
            .all(|value| value.as_ref().is_none_or(|error| error.len() <= 1_024));
        let previous_valid = self.previous_launch.as_ref().is_none_or(|previous| {
            previous.opportunity_epoch >= 0
                && !previous.schedule_basis.is_empty()
                && previous.schedule_basis.len() <= MAX_MAINTENANCE_SCHEDULE_BASIS_BYTES
                && [&previous.parent_error, &previous.child_error]
                    .into_iter()
                    .all(|value| value.as_ref().is_none_or(|error| error.len() <= 1_024))
        });
        if self.format_version != LAUNCH_FORMAT_VERSION
            || self.opportunity_epoch < 0
            || self.schedule_basis.is_empty()
            || self.schedule_basis.len() > MAX_MAINTENANCE_SCHEDULE_BASIS_BYTES
            || !(terminal_shape || nonterminal_shape)
            || !child_shape
            || !errors_valid
            || !previous_valid
            || self.expected_checksum()? != self.checksum_sha256
        {
            return Err("invalid detached maintenance launch record".to_string());
        }
        Ok(())
    }

    fn terminal_nonfailure(&self) -> bool {
        self.state == LaunchAdmissionState::Terminal
            && self
                .outcome
                .is_some_and(|outcome| outcome != LaunchOutcome::Failed)
    }
}

impl OldestLaunchEpoch {
    fn expected_checksum(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"oulipoly.maintenance-launch-oldest-epoch.v1\0");
        digest.update(self.format_version.to_le_bytes());
        digest.update(self.epoch.to_le_bytes());
        hex(&digest.finalize())
    }

    fn validate(&self) -> Result<(), String> {
        if self.format_version != 1
            || self.epoch < 0
            || self.expected_checksum() != self.checksum_sha256
        {
            return Err("invalid detached maintenance oldest launch epoch".to_string());
        }
        Ok(())
    }
}

pub(crate) fn is_worker_invocation() -> bool {
    std::env::args_os().nth(1).as_deref() == Some(OsStr::new(WORKER_ARG))
}

pub(crate) fn run_worker_invocation() -> Result<(), String> {
    let mut arguments = std::env::args();
    let _executable = arguments.next();
    let _private = arguments.next();
    let epoch = arguments
        .next()
        .ok_or_else(|| "detached maintenance opportunity epoch is missing".to_string())?
        .parse::<i64>()
        .map_err(|_| "detached maintenance opportunity epoch is invalid".to_string())?;
    validate_worker_epoch(epoch, chrono::Utc::now().timestamp())?;
    let launch_id = arguments
        .next()
        .map(|value| Uuid::parse_str(&value))
        .transpose()
        .map_err(|_| "detached maintenance launch id is invalid".to_string())?;
    let basis = arguments
        .next()
        .unwrap_or_else(|| "direct_private_worker".to_string());
    if arguments.next().is_some() {
        return Err("detached maintenance worker received excess arguments".to_string());
    }
    let data_root = oulipoly_state::paths::data_dir()?;
    let owner = MaintenanceWorkerIdentity::current()
        .map_err(|error| error.to_string())?
        .with_schedule_basis(&basis, launch_id);
    if let Some(launch_id) = launch_id {
        await_launch_release(&data_root, epoch, launch_id, &basis, &owner)?;
    }
    let event_result = run_event_store_opportunity(
        &data_root,
        epoch,
        owner.clone(),
        OpportunityWorkLimits::default(),
    );
    let coordination_result =
        run_coordination_retention_opportunity(&data_root, epoch, owner.clone(), 64);
    let cleanup_result = run_launch_evidence_cleanup(&data_root, epoch, owner);
    let (outcome, error) =
        summarize_launch_outcome(&event_result, &coordination_result, &cleanup_result);
    if let Some(launch_id) = launch_id {
        update_launch_result(&data_root, epoch, launch_id, outcome, error.as_deref())?;
    }
    error.map_or(Ok(()), Err)
}

fn summarize_launch_outcome(
    event: &Result<OpportunityOutcome, String>,
    coordination: &Result<CoordinationRetentionOutcome, String>,
    cleanup: &Result<LaunchCleanupOutcome, String>,
) -> (LaunchOutcome, Option<String>) {
    let mut errors = Vec::new();
    if let Err(error) = event {
        errors.push(format!("event={error}"));
    }
    if let Err(error) = coordination {
        errors.push(format!("coordination={error}"));
    }
    if let Err(error) = cleanup {
        errors.push(format!("launch_cleanup={error}"));
    }
    if !errors.is_empty() {
        return (LaunchOutcome::Failed, Some(errors.join(" ")));
    }
    let event = event.as_ref().expect("event result checked");
    let coordination = coordination.as_ref().expect("coordination result checked");
    let cleanup = cleanup.as_ref().expect("cleanup result checked");
    if *event == OpportunityOutcome::Preserved
        || !coordination.gaps.is_empty()
        || *cleanup == LaunchCleanupOutcome::PreservedGap
    {
        return (LaunchOutcome::PreservedGapped, None);
    }
    if *event == OpportunityOutcome::Cancelled {
        return (LaunchOutcome::Cancelled, None);
    }
    if *event == OpportunityOutcome::Yielded
        || coordination.jobs_yielded > 0
        || *cleanup == LaunchCleanupOutcome::Yielded
    {
        return (LaunchOutcome::Yielded, None);
    }
    (LaunchOutcome::Completed, None)
}

pub(crate) fn cli_requests_opportunity(cli: &Cli) -> bool {
    if cli.usage {
        return false;
    }
    matches!(
        &cli.command,
        None | Some(Subcommands::Repl { .. }) | Some(Subcommands::Resume { .. })
    )
}

pub(crate) fn schedule_daily_opportunity_fail_open(basis: ScheduleBasis) {
    if let Err(error) = schedule_daily_opportunity(basis) {
        eprintln!("OULIPOLY_MAINTENANCE_GAP=schedule_failed:{error}");
    }
}

fn schedule_daily_opportunity(basis: ScheduleBasis) -> Result<ScheduleOutcome, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("could not resolve maintenance worker executable: {error}"))?;
    let data_root = oulipoly_state::paths::data_dir()?;
    schedule_at(
        &data_root,
        &executable,
        opportunity_epoch_utc(),
        basis.as_str(),
    )
}

fn schedule_at(
    data_root: &Path,
    executable: &Path,
    epoch: i64,
    basis: &str,
) -> Result<ScheduleOutcome, String> {
    schedule_at_with_post_spawn(data_root, executable, epoch, basis, || Ok(()))
}

fn schedule_at_with_post_spawn(
    data_root: &Path,
    executable: &Path,
    epoch: i64,
    basis: &str,
    after_spawn: impl FnOnce() -> Result<(), String>,
) -> Result<ScheduleOutcome, String> {
    let directory = launch_directory(data_root);
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    set_private_directory(&directory)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join("admission.lock"))
        .map_err(|error| error.to_string())?;
    match <File as fs4::FileExt>::try_lock(&lock) {
        Ok(()) => {}
        Err(fs4::TryLockError::WouldBlock) => return Ok(ScheduleOutcome::SuppressedLive),
        Err(fs4::TryLockError::Error(error)) => return Err(error.to_string()),
    }

    let path = launch_record_path(&directory, epoch);
    let mut previous_launch = None;
    if let Some(existing) = read_launch_record(&path)? {
        if existing.terminal_nonfailure() {
            return Ok(ScheduleOutcome::SuppressedComplete);
        }
        if existing.state == LaunchAdmissionState::Admitted && launch_child_is_live(&existing)? {
            return Ok(ScheduleOutcome::SuppressedLive);
        }
        previous_launch = Some(reconcile_and_settle_launch(&path, existing)?);
    } else if epoch > 0 {
        let preceding_path = launch_record_path(&directory, epoch - 1);
        if let Some(existing) = read_launch_record(&preceding_path)?
            && existing.state != LaunchAdmissionState::Terminal
        {
            if existing.state == LaunchAdmissionState::Admitted && launch_child_is_live(&existing)?
            {
                return Ok(ScheduleOutcome::SuppressedLive);
            }
            previous_launch = Some(reconcile_and_settle_launch(&preceding_path, existing)?);
        }
    }

    ensure_oldest_launch_epoch(&directory, epoch)?;
    let epoch_directory = launch_epoch_directory(&directory, epoch);
    fs::create_dir_all(&epoch_directory).map_err(|error| error.to_string())?;
    set_private_directory(&epoch_directory)?;
    sync_directory(&directory)?;
    let launch_id = Uuid::new_v4();
    let fallback_path = launch_fallback_path(&directory, epoch, launch_id);
    write_launch_fallback(
        &epoch_directory,
        &fallback_path,
        launch_fallback(epoch, launch_id, basis, LaunchFallbackState::Armed, None)?,
    )?;
    let stderr_path = launch_stderr_path(&directory, epoch, launch_id);
    let stderr = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&stderr_path)
        .map_err(|error| format!("could not create maintenance stderr carrier: {error}"))?;
    stderr.sync_all().map_err(|error| error.to_string())?;
    sync_directory(&epoch_directory)?;

    let mut record = LaunchRecord {
        format_version: LAUNCH_FORMAT_VERSION,
        opportunity_epoch: epoch,
        launch_id,
        schedule_basis: basis.to_string(),
        state: LaunchAdmissionState::Intent,
        child: None,
        outcome: None,
        parent_error: None,
        child_error: None,
        parent_evidence_gap: false,
        child_evidence_gap: false,
        fallback_armed: true,
        previous_launch,
        checksum_sha256: String::new(),
    };
    refresh_launch_checksum(&mut record)?;
    write_launch_record(&path, &record)?;

    let arguments = worker_arguments(epoch, launch_id, basis);
    let mut command = Command::new(executable);
    command
        .args(&arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr))
        .env(ERROR_CARRIER_ENV, &fallback_path)
        .env(ERROR_EPOCH_ENV, epoch.to_string())
        .env(ERROR_LAUNCH_ID_ENV, launch_id.to_string())
        .env(ERROR_BASIS_ENV, basis);
    configure_detached(&mut command);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let message = format!("could not spawn detached maintenance worker: {error}");
            return Err(persist_parent_launch_failure(&path, &mut record, &message));
        }
    };
    if let Err(error) = after_spawn() {
        return Err(persist_parent_launch_failure(&path, &mut record, &error));
    }
    let child_identity = match read_live_process_identity(i64::from(child.id())) {
        Ok(Some(identity)) => identity,
        Ok(None) => {
            let message = "maintenance worker exited before launch admission".to_string();
            return Err(persist_parent_launch_failure(&path, &mut record, &message));
        }
        Err(error) => {
            let message = format!("could not identify maintenance worker: {error}");
            return Err(persist_parent_launch_failure(&path, &mut record, &message));
        }
    };
    record.state = LaunchAdmissionState::Admitted;
    record.child = Some(child_identity.into());
    refresh_launch_checksum(&mut record)?;
    if let Err(error) = write_launch_record(&path, &record) {
        let message = format!("could not publish full maintenance launch admission: {error}");
        return Err(persist_parent_launch_failure(&path, &mut record, &message));
    }
    let mut stdin = match child.stdin.take() {
        Some(stdin) => stdin,
        None => {
            let message = "maintenance worker admission pipe is absent".to_string();
            return Err(persist_parent_launch_failure(&path, &mut record, &message));
        }
    };
    if let Err(error) = writeln!(stdin, "{launch_id}").and_then(|()| stdin.flush()) {
        let message =
            format!("maintenance worker did not accept durable launch admission: {error}");
        return Err(persist_parent_launch_failure(&path, &mut record, &message));
    }
    drop(stdin);
    drop(child);
    Ok(ScheduleOutcome::Spawned)
}

fn refresh_launch_checksum(record: &mut LaunchRecord) -> Result<(), String> {
    record.checksum_sha256 = record.expected_checksum()?;
    record.validate()
}

fn persist_parent_launch_failure(path: &Path, record: &mut LaunchRecord, error: &str) -> String {
    record.state = LaunchAdmissionState::Terminal;
    record.outcome = Some(LaunchOutcome::Failed);
    record.parent_error = Some(truncate(error, 1_024));
    let persistence =
        refresh_launch_checksum(record).and_then(|()| write_launch_record(path, record));
    match persistence {
        Ok(()) => error.to_string(),
        Err(write_error) => format!("{error}; parent launch failure evidence gap: {write_error}"),
    }
}

fn launch_child_is_live(record: &LaunchRecord) -> Result<bool, String> {
    let Some(child) = record.child.as_ref() else {
        return Ok(false);
    };
    match observe_live_process_identity(child.os_pid) {
        ProcessIdentityObservation::ExactLive(live) => {
            Ok(child.matches(&live) && !process_has_exited(live.os_pid))
        }
        ProcessIdentityObservation::Dead => Ok(false),
        ProcessIdentityObservation::Unsupported => {
            Err("cannot prove prior maintenance launch dead on this platform".into())
        }
        ProcessIdentityObservation::ReadError(error) => {
            Err(format!("cannot inspect prior maintenance launch: {error}"))
        }
    }
}

fn reconcile_and_settle_launch(
    path: &Path,
    mut existing: LaunchRecord,
) -> Result<LaunchTerminalEvidence, String> {
    let evidence = reconcile_launch_terminal(
        path.parent()
            .ok_or_else(|| "maintenance launch index has no parent".to_string())?,
        existing.clone(),
    );
    if existing.state != LaunchAdmissionState::Terminal {
        existing.state = LaunchAdmissionState::Terminal;
        existing.outcome = Some(evidence.outcome);
        existing.parent_error = evidence.parent_error.clone();
        existing.child_error = evidence.child_error.clone();
        existing.parent_evidence_gap = evidence.parent_evidence_gap;
        existing.child_evidence_gap = evidence.child_evidence_gap;
        refresh_launch_checksum(&mut existing)?;
        write_launch_record(path, &existing)?;
    }
    Ok(evidence)
}

fn worker_arguments(epoch: i64, launch_id: Uuid, basis: &str) -> [OsString; 4] {
    [
        OsString::from(WORKER_ARG),
        OsString::from(epoch.to_string()),
        OsString::from(launch_id.to_string()),
        OsString::from(basis),
    ]
}

fn await_launch_release(
    data_root: &Path,
    epoch: i64,
    launch_id: Uuid,
    basis: &str,
    owner: &MaintenanceWorkerIdentity,
) -> Result<(), String> {
    let mut line = String::new();
    io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|error| format!("maintenance launch admission read failed: {error}"))?;
    if line.trim() != launch_id.to_string() {
        return Err("maintenance launch admission token conflict".to_string());
    }
    let record = read_launch_record(&launch_record_path(&launch_directory(data_root), epoch))?
        .ok_or_else(|| "maintenance launch record is absent".to_string())?;
    let child = record
        .child
        .as_ref()
        .ok_or_else(|| "maintenance launch child identity is absent".to_string())?;
    if record.launch_id != launch_id
        || record.schedule_basis != basis
        || record.state != LaunchAdmissionState::Admitted
        || child.os_pid != owner.os_pid
        || child.os_boot_id != owner.os_boot_id
        || child.os_pid_starttime_ticks != owner.os_pid_starttime_ticks
    {
        return Err("maintenance launch record does not match exact child".to_string());
    }
    Ok(())
}

fn update_launch_result(
    data_root: &Path,
    epoch: i64,
    launch_id: Uuid,
    outcome: LaunchOutcome,
    error: Option<&str>,
) -> Result<(), String> {
    let directory = launch_directory(data_root);
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join("admission.lock"))
        .map_err(|error| error.to_string())?;
    <File as fs4::FileExt>::lock(&lock).map_err(|error| error.to_string())?;
    let path = launch_record_path(&directory, epoch);
    let mut record = read_launch_record(&path)?
        .ok_or_else(|| "maintenance launch record disappeared".to_string())?;
    if record.launch_id != launch_id {
        return Err("maintenance launch result identity conflict".to_string());
    }
    if record.state != LaunchAdmissionState::Admitted {
        return Err("maintenance launch result has no admitted child".to_string());
    }
    record.state = LaunchAdmissionState::Terminal;
    record.outcome = Some(outcome);
    record.child_error = error.map(|value| truncate(value, 1_024));
    refresh_launch_checksum(&mut record)?;
    write_launch_record(&path, &record)
}

/// Last-resort carrier used by the private worker entry after any escaping
/// error, including failure of `update_launch_result` itself. The parent arms
/// this exact file before spawn, so a missing/unreadable carrier is reported as
/// an evidence gap rather than being mislabeled as an ordinary abrupt death.
pub(crate) fn record_worker_failure_fallback(error: &str) -> Result<(), String> {
    let path = std::env::var_os(ERROR_CARRIER_ENV)
        .map(PathBuf::from)
        .ok_or_else(|| "maintenance fallback carrier environment is absent".to_string())?;
    let epoch = std::env::var(ERROR_EPOCH_ENV)
        .map_err(|_| "maintenance fallback epoch is absent".to_string())?
        .parse::<i64>()
        .map_err(|_| "maintenance fallback epoch is invalid".to_string())?;
    let launch_id = Uuid::parse_str(
        &std::env::var(ERROR_LAUNCH_ID_ENV)
            .map_err(|_| "maintenance fallback launch id is absent".to_string())?,
    )
    .map_err(|_| "maintenance fallback launch id is invalid".to_string())?;
    let basis = std::env::var(ERROR_BASIS_ENV)
        .map_err(|_| "maintenance fallback basis is absent".to_string())?;
    let directory = path
        .parent()
        .ok_or_else(|| "maintenance fallback carrier has no parent".to_string())?;
    write_launch_fallback(
        directory,
        &path,
        launch_fallback(
            epoch,
            launch_id,
            &basis,
            LaunchFallbackState::Failed,
            Some(truncate(error, 1_024)),
        )?,
    )
}

fn run_launch_evidence_cleanup(
    data_root: &Path,
    opportunity_epoch: i64,
    owner: MaintenanceWorkerIdentity,
) -> Result<LaunchCleanupOutcome, String> {
    let directory = launch_directory(data_root);
    if !directory.exists() {
        return Ok(LaunchCleanupOutcome::Completed);
    }
    let cutoff = opportunity_epoch.saturating_sub(LAUNCH_RETENTION_DAYS);
    if cutoff <= 0 {
        return Ok(LaunchCleanupOutcome::Completed);
    }
    let store = MaintenanceJobStore::open(data_root).map_err(|error| error.to_string())?;
    let key = MaintenanceJobKey::new(MaintenanceJobKind::HistoricalCompaction, "launch/evidence")
        .map_err(|error| error.to_string())?;
    let effective_epoch = store
        .read_status(&key)
        .map_err(|error| error.to_string())?
        .filter(|status| !status.state.terminal())
        .map_or(opportunity_epoch, |status| status.opportunity_epoch);
    let mut job = match store
        .acquire(key, effective_epoch, owner, now_unix_micros())
        .map_err(|error| error.to_string())?
    {
        AcquireMaintenanceJob::DuplicateLive(_) => return Ok(LaunchCleanupOutcome::Duplicate),
        AcquireMaintenanceJob::AlreadyTerminal(_) => return Ok(LaunchCleanupOutcome::Completed),
        AcquireMaintenanceJob::Acquired(job) => job,
    };
    let marker = match read_oldest_launch_epoch(&oldest_launch_epoch_path(&directory)) {
        Ok(Some(marker)) => marker,
        Ok(None) => {
            return preserve_launch_cleanup_gap(
                &mut job,
                "launch evidence oldest-epoch marker is absent".to_string(),
            );
        }
        Err(error) => {
            return preserve_launch_cleanup_gap(
                &mut job,
                format!("launch evidence oldest-epoch marker is invalid: {error}"),
            );
        }
    };
    let mut cursor = job
        .checkpoint()
        .cursor
        .as_deref()
        .map(serde_json::from_str::<LaunchCleanupCursor>)
        .transpose()
        .map_err(|error| format!("invalid launch cleanup cursor: {error}"))?
        .unwrap_or(LaunchCleanupCursor {
            next_epoch: marker.epoch,
            first_gap: None,
        });
    if cursor.next_epoch < marker.epoch {
        cursor.next_epoch = marker.epoch;
    }
    if cursor.next_epoch > opportunity_epoch {
        return preserve_launch_cleanup_gap(
            &mut job,
            "launch cleanup cursor is ahead of the current opportunity".to_string(),
        );
    }

    let mut oldest_unresolved_epoch = marker.epoch;
    let mut epochs_examined = 0usize;
    let mut entries_removed = 0usize;
    while cursor.next_epoch < cutoff && epochs_examined < LAUNCH_CLEANUP_EPOCHS_PER_RUN {
        let epoch = cursor.next_epoch;
        let source = launch_epoch_directory(&directory, epoch);
        let pending = retiring_epoch_directory(&directory, epoch);
        if source.exists() && pending.exists() {
            cursor.first_gap.get_or_insert_with(|| {
                format!("launch epoch {epoch} has both active and retiring directories")
            });
            cursor.next_epoch = epoch.saturating_add(1);
            epochs_examined += 1;
            record_launch_cleanup_progress(&mut job, &cursor, 1, 0)?;
            continue;
        }
        if source.exists() {
            let record = match read_launch_record(&source.join("index.json")) {
                Ok(Some(record)) => record,
                Ok(None) => {
                    cursor.first_gap.get_or_insert_with(|| {
                        format!("launch epoch {epoch} has no durable index")
                    });
                    cursor.next_epoch = epoch.saturating_add(1);
                    epochs_examined += 1;
                    record_launch_cleanup_progress(&mut job, &cursor, 1, 0)?;
                    continue;
                }
                Err(error) => {
                    cursor.first_gap.get_or_insert_with(|| {
                        format!("launch epoch {epoch} index is invalid: {error}")
                    });
                    cursor.next_epoch = epoch.saturating_add(1);
                    epochs_examined += 1;
                    record_launch_cleanup_progress(&mut job, &cursor, 1, 0)?;
                    continue;
                }
            };
            if record.opportunity_epoch != epoch || record.state != LaunchAdmissionState::Terminal {
                cursor.first_gap.get_or_insert_with(|| {
                    format!("launch epoch {epoch} is not terminal and remains preserved")
                });
                cursor.next_epoch = epoch.saturating_add(1);
                epochs_examined += 1;
                record_launch_cleanup_progress(&mut job, &cursor, 1, 0)?;
                continue;
            }
            fs::rename(&source, &pending).map_err(|error| error.to_string())?;
            sync_directory(&directory)?;
        }
        if pending.exists() {
            let remaining = LAUNCH_CLEANUP_ENTRIES_PER_RUN.saturating_sub(entries_removed);
            if remaining == 0 {
                break;
            }
            let mut removed_now = 0usize;
            let mut pending_gap = None;
            for entry in fs::read_dir(&pending)
                .map_err(|error| error.to_string())?
                .take(remaining)
            {
                let entry = entry.map_err(|error| error.to_string())?;
                if entry
                    .file_type()
                    .map_err(|error| error.to_string())?
                    .is_dir()
                {
                    pending_gap = Some(format!(
                        "launch epoch {epoch} contains an unexpected directory"
                    ));
                    break;
                }
                fs::remove_file(entry.path()).map_err(|error| error.to_string())?;
                removed_now += 1;
            }
            entries_removed += removed_now;
            sync_directory(&pending)?;
            if let Some(reason) = pending_gap {
                cursor.first_gap.get_or_insert(reason);
                cursor.next_epoch = epoch.saturating_add(1);
                epochs_examined += 1;
                record_launch_cleanup_progress(&mut job, &cursor, 1, entries_removed)?;
                entries_removed = 0;
                continue;
            }
            if fs::read_dir(&pending)
                .map_err(|error| error.to_string())?
                .next()
                .is_some()
            {
                record_launch_cleanup_progress(
                    &mut job,
                    &cursor,
                    epochs_examined,
                    entries_removed,
                )?;
                job.yield_run(
                    "launch_cleanup_yielded",
                    "bounded launch evidence unlink has more entries",
                    now_unix_micros(),
                )
                .map_err(|error| error.to_string())?;
                return Ok(LaunchCleanupOutcome::Yielded);
            }
            fs::remove_dir(&pending).map_err(|error| error.to_string())?;
            sync_directory(&directory)?;
        }
        cursor.next_epoch = epoch.saturating_add(1);
        if epoch == oldest_unresolved_epoch {
            oldest_unresolved_epoch = cursor.next_epoch;
            write_oldest_launch_epoch(&directory, oldest_unresolved_epoch, false)?;
        }
        epochs_examined += 1;
        record_launch_cleanup_progress(&mut job, &cursor, 1, entries_removed)?;
        entries_removed = 0;
    }
    if cursor.next_epoch < cutoff {
        job.yield_run(
            "launch_cleanup_yielded",
            "bounded launch evidence epoch budget exhausted",
            now_unix_micros(),
        )
        .map_err(|error| error.to_string())?;
        Ok(LaunchCleanupOutcome::Yielded)
    } else if oldest_unresolved_epoch < cutoff {
        cursor.next_epoch = oldest_unresolved_epoch;
        record_launch_cleanup_progress(&mut job, &cursor, 0, entries_removed)?;
        job.yield_run(
            "launch_cleanup_gap_preserved",
            cursor.first_gap.clone().unwrap_or_else(|| {
                "older launch evidence remains unresolved; cleanup will revisit it while later epochs continue".to_string()
            }),
            now_unix_micros(),
        )
        .map_err(|error| error.to_string())?;
        Ok(LaunchCleanupOutcome::Yielded)
    } else {
        job.finish(
            MaintenanceRunState::Succeeded,
            "launch_cleanup_complete",
            "terminal launch evidence older than thirty days was retired",
            now_unix_micros(),
        )
        .map_err(|error| error.to_string())?;
        Ok(LaunchCleanupOutcome::Completed)
    }
}

fn record_launch_cleanup_progress(
    job: &mut oulipoly_state::maintenance::MaintenanceJobLease,
    cursor: &LaunchCleanupCursor,
    epochs_added: usize,
    entries_added: usize,
) -> Result<(), String> {
    job.record_progress(
        "launch_cleanup",
        Some(serde_json::to_string(cursor).map_err(|error| error.to_string())?),
        job.checkpoint()
            .units_completed
            .saturating_add(epochs_added as u64),
        job.checkpoint()
            .bytes_completed
            .saturating_add(entries_added as u64),
        now_unix_micros(),
    )
    .map_err(|error| error.to_string())
}

fn preserve_launch_cleanup_gap(
    job: &mut oulipoly_state::maintenance::MaintenanceJobLease,
    reason: String,
) -> Result<LaunchCleanupOutcome, String> {
    job.finish(
        MaintenanceRunState::Preserved,
        "launch_cleanup_preserved",
        reason,
        now_unix_micros(),
    )
    .map_err(|error| error.to_string())?;
    Ok(LaunchCleanupOutcome::PreservedGap)
}

fn validate_worker_epoch(epoch: i64, now_seconds: i64) -> Result<(), String> {
    if epoch < 0 || now_seconds < 0 {
        return Err("detached maintenance opportunity epoch is invalid".to_string());
    }
    let current = now_seconds / (24 * 60 * 60);
    let seconds_today = now_seconds % (24 * 60 * 60);
    if epoch == current || (epoch == current - 1 && seconds_today <= MIDNIGHT_ALLOWANCE_SECONDS) {
        Ok(())
    } else {
        Err("detached maintenance opportunity epoch is outside the current UTC window".into())
    }
}

fn launch_directory(data_root: &Path) -> PathBuf {
    data_root.join(MAINTENANCE_DIRECTORY).join("launch")
}

fn launch_epoch_directory(directory: &Path, epoch: i64) -> PathBuf {
    directory.join(format!("opportunity-{epoch}"))
}

fn launch_record_path(directory: &Path, epoch: i64) -> PathBuf {
    launch_epoch_directory(directory, epoch).join("index.json")
}

fn launch_fallback_path(directory: &Path, epoch: i64, launch_id: Uuid) -> PathBuf {
    launch_epoch_directory(directory, epoch).join(format!("{launch_id}.fallback.json"))
}

fn launch_stderr_path(directory: &Path, epoch: i64, launch_id: Uuid) -> PathBuf {
    launch_epoch_directory(directory, epoch).join(format!("{launch_id}.stderr"))
}

fn retiring_epoch_directory(directory: &Path, epoch: i64) -> PathBuf {
    directory.join(format!(".retiring-opportunity-{epoch}"))
}

fn oldest_launch_epoch_path(directory: &Path) -> PathBuf {
    directory.join(OLDEST_LAUNCH_EPOCH_FILE)
}

fn ensure_oldest_launch_epoch(directory: &Path, epoch: i64) -> Result<(), String> {
    let path = oldest_launch_epoch_path(directory);
    match read_oldest_launch_epoch(&path)? {
        Some(existing) if existing.epoch <= epoch => Ok(()),
        Some(_) => Err("maintenance oldest launch epoch is ahead of admission".to_string()),
        None => write_oldest_launch_epoch(directory, epoch, true),
    }
}

fn read_oldest_launch_epoch(path: &Path) -> Result<Option<OldestLaunchEpoch>, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let marker: OldestLaunchEpoch =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    marker.validate()?;
    let mut canonical = serde_json::to_vec(&marker).map_err(|error| error.to_string())?;
    canonical.push(b'\n');
    if canonical != bytes {
        return Err("maintenance oldest launch epoch bytes are not canonical".to_string());
    }
    Ok(Some(marker))
}

fn write_oldest_launch_epoch(directory: &Path, epoch: i64, create: bool) -> Result<(), String> {
    let mut marker = OldestLaunchEpoch {
        format_version: 1,
        epoch,
        checksum_sha256: String::new(),
    };
    marker.checksum_sha256 = marker.expected_checksum();
    marker.validate()?;
    let mut bytes = serde_json::to_vec(&marker).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    let path = oldest_launch_epoch_path(directory);
    let temp = directory.join(format!(".oldest.{}.tmp", Uuid::new_v4().simple()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)
        .map_err(|error| error.to_string())?;
    file.write_all(&bytes).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    drop(file);
    if create {
        match fs::hard_link(&temp, &path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let _ = fs::remove_file(&temp);
                return read_oldest_launch_epoch(&path).and_then(|existing| {
                    existing
                        .filter(|value| value.epoch <= epoch)
                        .map(|_| ())
                        .ok_or_else(|| "maintenance oldest launch epoch conflicts".to_string())
                });
            }
            Err(error) => {
                let _ = fs::remove_file(&temp);
                return Err(error.to_string());
            }
        }
        fs::remove_file(&temp).map_err(|error| error.to_string())?;
    } else {
        #[cfg(windows)]
        if path.exists() {
            fs::remove_file(&path).map_err(|error| error.to_string())?;
        }
        fs::rename(&temp, &path).map_err(|error| error.to_string())?;
    }
    sync_directory(directory)
}

fn launch_fallback(
    epoch: i64,
    launch_id: Uuid,
    basis: &str,
    state: LaunchFallbackState,
    error: Option<String>,
) -> Result<LaunchFallbackRecord, String> {
    let mut record = LaunchFallbackRecord {
        format_version: LAUNCH_FALLBACK_FORMAT_VERSION,
        opportunity_epoch: epoch,
        launch_id,
        schedule_basis: basis.to_string(),
        state,
        error,
        checksum_sha256: String::new(),
    };
    record.checksum_sha256 = record.expected_checksum()?;
    record.validate()?;
    Ok(record)
}

fn reconcile_launch_terminal(
    epoch_directory: &Path,
    existing: LaunchRecord,
) -> LaunchTerminalEvidence {
    let mut child_error = existing.child_error.clone();
    let mut child_evidence_gap = existing.child_evidence_gap;
    let parent_evidence_gap = existing.parent_evidence_gap
        || (existing.state == LaunchAdmissionState::Intent && existing.parent_error.is_none());
    if child_error.is_none() && existing.fallback_armed {
        let fallback_path = epoch_directory.join(format!("{}.fallback.json", existing.launch_id));
        match read_launch_fallback(&fallback_path) {
            Ok(Some(fallback))
                if fallback.opportunity_epoch == existing.opportunity_epoch
                    && fallback.launch_id == existing.launch_id
                    && fallback.schedule_basis == existing.schedule_basis =>
            {
                if fallback.state == LaunchFallbackState::Failed {
                    child_error = fallback.error;
                }
            }
            Ok(Some(_)) | Err(_) | Ok(None) => child_evidence_gap = true,
        }
        if child_error.is_none() {
            let stderr_path = epoch_directory.join(format!("{}.stderr", existing.launch_id));
            match fs::read(stderr_path) {
                Ok(bytes) if !bytes.is_empty() => {
                    child_error = Some(truncate(&String::from_utf8_lossy(&bytes), 1_024));
                    child_evidence_gap = false;
                }
                Ok(_) => {}
                Err(_) => child_evidence_gap = true,
            }
        }
    }
    LaunchTerminalEvidence {
        opportunity_epoch: existing.opportunity_epoch,
        launch_id: existing.launch_id,
        schedule_basis: existing.schedule_basis,
        child: existing.child,
        outcome: existing.outcome.unwrap_or(LaunchOutcome::Failed),
        parent_error: existing.parent_error,
        child_error,
        parent_evidence_gap,
        child_evidence_gap,
    }
}
fn read_launch_fallback(path: &Path) -> Result<Option<LaunchFallbackRecord>, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let record: LaunchFallbackRecord =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    record.validate()?;
    let mut canonical = serde_json::to_vec(&record).map_err(|error| error.to_string())?;
    canonical.push(b'\n');
    if canonical != bytes {
        return Err("maintenance launch fallback bytes are not canonical".to_string());
    }
    Ok(Some(record))
}

fn write_launch_fallback(
    directory: &Path,
    path: &Path,
    record: LaunchFallbackRecord,
) -> Result<(), String> {
    record.validate()?;
    let temp = directory.join(format!(".fallback.{}.tmp", Uuid::new_v4().simple()));
    let mut bytes = serde_json::to_vec(&record).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)
        .map_err(|error| error.to_string())?;
    file.write_all(&bytes).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    drop(file);
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(path).map_err(|error| error.to_string())?;
    }
    fs::rename(&temp, path).map_err(|error| error.to_string())?;
    sync_directory(directory)
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

fn read_launch_record(path: &Path) -> Result<Option<LaunchRecord>, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let record: LaunchRecord = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    record.validate()?;
    let mut canonical = serde_json::to_vec(&record).map_err(|error| error.to_string())?;
    canonical.push(b'\n');
    if canonical != bytes {
        return Err("maintenance launch record bytes are not canonical".to_string());
    }
    Ok(Some(record))
}

fn write_launch_record(path: &Path, record: &LaunchRecord) -> Result<(), String> {
    record.validate()?;
    let directory = path
        .parent()
        .ok_or_else(|| "maintenance launch index has no parent".to_string())?;
    let temp = directory.join(format!(".launch.{}.tmp", Uuid::new_v4().simple()));
    let mut bytes = serde_json::to_vec(record).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)
        .map_err(|error| error.to_string())?;
    file.write_all(&bytes).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    drop(file);
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(path).map_err(|error| error.to_string())?;
    }
    fs::rename(&temp, path).map_err(|error| error.to_string())?;
    sync_directory(directory)
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn set_private_directory(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn set_private_directory(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(value, "{byte:02x}");
    }
    value
}

#[cfg(target_os = "linux")]
fn process_has_exited(pid: i64) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| {
            stat.rfind(") ")
                .and_then(|close| stat[close + 2..].split_whitespace().next())
                .map(|state| matches!(state, "Z" | "X" | "x"))
        })
        .unwrap_or(false)
}

#[cfg(not(target_os = "linux"))]
fn process_has_exited(_pid: i64) -> bool {
    false
}

#[cfg(unix)]
fn configure_detached(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(windows)]
fn configure_detached(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
}

#[cfg(not(any(unix, windows)))]
fn configure_detached(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::time::{Duration, Instant};

    #[test]
    fn startup_policy_schedules_provider_work_but_not_offline_diagnostics() {
        let direct = Cli::try_parse_from(["runner", "--model", "test"]).unwrap();
        assert!(cli_requests_opportunity(&direct));
        let diagnostics =
            Cli::try_parse_from(["runner", "diagnostics", "recent", "--limit", "1"]).unwrap();
        assert!(!cli_requests_opportunity(&diagnostics));
    }

    #[test]
    fn epoch_accepts_current_and_brief_previous_day_but_never_future() {
        let day = 50;
        let day_seconds = 24 * 60 * 60;
        assert!(validate_worker_epoch(day, day * day_seconds + 1).is_ok());
        assert!(validate_worker_epoch(day - 1, day * day_seconds + 1).is_ok());
        assert!(validate_worker_epoch(day - 1, day * day_seconds + 3_600).is_err());
        assert!(validate_worker_epoch(day + 1, day * day_seconds + 1).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn real_scheduler_argv_is_nonblocking_deduplicated_and_recovers_dead_child() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let script = root.path().join("worker-fixture.sh");
        let observed = root.path().join("observed.txt");
        let body = format!(
            "#!/bin/sh\nread token\nprintf '%s\\n%s\\n%s\\n%s\\n%s\\n' \"$1\" \"$2\" \"$3\" \"$4\" \"$token\" > '{}'\nsleep 1\n",
            observed.display()
        );
        fs::write(&script, body).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        let launch = launch_directory(root.path());
        fs::create_dir_all(&launch).unwrap();
        let admission = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(launch.join("admission.lock"))
            .unwrap();
        <File as fs4::FileExt>::lock(&admission).unwrap();
        let contended = Instant::now();
        assert_eq!(
            schedule_at(root.path(), &script, 7, "test_startup").unwrap(),
            ScheduleOutcome::SuppressedLive
        );
        assert!(contended.elapsed() < Duration::from_millis(500));
        drop(admission);
        let started = Instant::now();
        assert_eq!(
            schedule_at(root.path(), &script, 7, "test_startup").unwrap(),
            ScheduleOutcome::Spawned
        );
        assert!(started.elapsed() < Duration::from_millis(500));
        assert!(
            !root
                .path()
                .join("diagnostics/event-store-v1/writers")
                .exists()
        );
        assert_eq!(
            schedule_at(root.path(), &script, 7, "test_startup").unwrap(),
            ScheduleOutcome::SuppressedLive
        );
        let first = read_launch_record(&launch_record_path(&launch, 7))
            .unwrap()
            .unwrap();
        update_launch_result(
            root.path(),
            7,
            first.launch_id,
            LaunchOutcome::Failed,
            Some("fixture storage failure"),
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(1_200));
        assert_eq!(
            schedule_at(root.path(), &script, 7, "test_startup").unwrap(),
            ScheduleOutcome::Spawned
        );
        let lines = fs::read_to_string(observed).unwrap();
        let lines = lines.lines().collect::<Vec<_>>();
        assert_eq!(lines[0], WORKER_ARG);
        assert_eq!(lines[1], "7");
        assert_eq!(lines[3], "test_startup");
        assert_eq!(lines[2], lines[4]);
        let retried = read_launch_record(&launch_record_path(&launch, 7))
            .unwrap()
            .unwrap();
        let previous = retried.previous_launch.unwrap();
        assert_eq!(previous.launch_id, first.launch_id);
        assert_eq!(previous.schedule_basis, "test_startup");
        assert_eq!(
            previous.child_error.as_deref(),
            Some("fixture storage failure")
        );
        assert_eq!(previous.outcome, LaunchOutcome::Failed);
        assert!(!previous.child_evidence_gap);
    }

    #[test]
    fn next_admission_reads_exact_failed_fallback_and_distinguishes_storage_gap() {
        let root = tempfile::tempdir().unwrap();
        let directory = launch_directory(root.path());
        fs::create_dir_all(&directory).unwrap();
        let epoch = 41;
        let epoch_directory = launch_epoch_directory(&directory, epoch);
        fs::create_dir_all(&epoch_directory).unwrap();
        let launch_id = Uuid::new_v4();
        let identity = MaintenanceWorkerIdentity::current().unwrap();
        let existing = LaunchRecord {
            format_version: LAUNCH_FORMAT_VERSION,
            opportunity_epoch: epoch,
            launch_id,
            schedule_basis: "provider_startup".to_string(),
            state: LaunchAdmissionState::Admitted,
            child: Some(LaunchProcessIdentity {
                os_pid: identity.os_pid,
                os_boot_id: identity.os_boot_id,
                os_pid_starttime_ticks: identity.os_pid_starttime_ticks,
            }),
            outcome: None,
            parent_error: None,
            child_error: None,
            parent_evidence_gap: false,
            child_evidence_gap: false,
            fallback_armed: true,
            previous_launch: None,
            checksum_sha256: String::new(),
        };
        let fallback_path = launch_fallback_path(&directory, epoch, launch_id);
        write_launch_fallback(
            &epoch_directory,
            &fallback_path,
            launch_fallback(
                epoch,
                launch_id,
                "provider_startup",
                LaunchFallbackState::Failed,
                Some("launch result ledger refused update".to_string()),
            )
            .unwrap(),
        )
        .unwrap();

        let recovered = reconcile_launch_terminal(&epoch_directory, existing.clone());
        assert_eq!(
            recovered.child_error.as_deref(),
            Some("launch result ledger refused update")
        );
        assert!(!recovered.child_evidence_gap);

        fs::write(&fallback_path, b"not canonical fallback\n").unwrap();
        let gap = reconcile_launch_terminal(&epoch_directory, existing);
        assert!(gap.child_error.is_none());
        assert!(gap.child_evidence_gap);
    }

    fn fixture_launch_record(
        epoch: i64,
        launch_id: Uuid,
        state: LaunchAdmissionState,
        outcome: Option<LaunchOutcome>,
    ) -> LaunchRecord {
        let mut record = LaunchRecord {
            format_version: LAUNCH_FORMAT_VERSION,
            opportunity_epoch: epoch,
            launch_id,
            schedule_basis: "test_startup".to_string(),
            state,
            child: None,
            outcome,
            parent_error: None,
            child_error: None,
            parent_evidence_gap: false,
            child_evidence_gap: false,
            fallback_armed: state != LaunchAdmissionState::Terminal,
            previous_launch: None,
            checksum_sha256: String::new(),
        };
        refresh_launch_checksum(&mut record).unwrap();
        record
    }

    #[cfg(unix)]
    #[test]
    fn pre_spawn_intent_records_parent_failure_before_child_release() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let script = root.path().join("worker.sh");
        fs::write(&script, "#!/bin/sh\nread token\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        let error = schedule_at_with_post_spawn(root.path(), &script, 12, "test_startup", || {
            Err("parent identity capture failed".to_string())
        })
        .unwrap_err();
        assert_eq!(error, "parent identity capture failed");
        let record = read_launch_record(&launch_record_path(&launch_directory(root.path()), 12))
            .unwrap()
            .unwrap();
        assert_eq!(record.state, LaunchAdmissionState::Terminal);
        assert_eq!(record.outcome, Some(LaunchOutcome::Failed));
        assert_eq!(
            record.parent_error.as_deref(),
            Some("parent identity capture failed")
        );
        assert!(record.child.is_none());
        assert!(record.child_error.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn actual_scheduler_reconciles_previous_epoch_fallback_only_failure() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let script = root.path().join("worker.sh");
        fs::write(&script, "#!/bin/sh\nread token\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        let directory = launch_directory(root.path());
        let prior_directory = launch_epoch_directory(&directory, 20);
        fs::create_dir_all(&prior_directory).unwrap();
        set_private_directory(&directory).unwrap();
        set_private_directory(&prior_directory).unwrap();
        let launch_id = Uuid::new_v4();
        let prior = fixture_launch_record(20, launch_id, LaunchAdmissionState::Intent, None);
        write_launch_record(&launch_record_path(&directory, 20), &prior).unwrap();
        write_launch_fallback(
            &prior_directory,
            &launch_fallback_path(&directory, 20, launch_id),
            launch_fallback(
                20,
                launch_id,
                "test_startup",
                LaunchFallbackState::Failed,
                Some("fallback-only child failure".to_string()),
            )
            .unwrap(),
        )
        .unwrap();
        write_oldest_launch_epoch(&directory, 20, true).unwrap();
        assert_eq!(
            schedule_at(root.path(), &script, 21, "test_startup").unwrap(),
            ScheduleOutcome::Spawned
        );
        let current = read_launch_record(&launch_record_path(&directory, 21))
            .unwrap()
            .unwrap();
        let previous = current.previous_launch.unwrap();
        assert_eq!(previous.opportunity_epoch, 20);
        assert_eq!(previous.launch_id, launch_id);
        assert_eq!(
            previous.child_error.as_deref(),
            Some("fallback-only child failure")
        );
        assert!(previous.parent_evidence_gap);
        let settled = read_launch_record(&launch_record_path(&directory, 20))
            .unwrap()
            .unwrap();
        assert_eq!(settled.state, LaunchAdmissionState::Terminal);
    }

    #[test]
    fn launch_cleanup_preserves_incomplete_without_pinning_later_terminal_epochs() {
        let root = tempfile::tempdir().unwrap();
        let directory = launch_directory(root.path());
        fs::create_dir_all(&directory).unwrap();
        set_private_directory(&directory).unwrap();
        write_oldest_launch_epoch(&directory, 1, true).unwrap();
        for epoch in [1, 5, 6] {
            let epoch_directory = launch_epoch_directory(&directory, epoch);
            fs::create_dir_all(&epoch_directory).unwrap();
            set_private_directory(&epoch_directory).unwrap();
        }
        let terminal = fixture_launch_record(
            1,
            Uuid::new_v4(),
            LaunchAdmissionState::Terminal,
            Some(LaunchOutcome::Completed),
        );
        write_launch_record(&launch_record_path(&directory, 1), &terminal).unwrap();
        let incomplete =
            fixture_launch_record(5, Uuid::new_v4(), LaunchAdmissionState::Intent, None);
        write_launch_record(&launch_record_path(&directory, 5), &incomplete).unwrap();
        let later_terminal = fixture_launch_record(
            6,
            Uuid::new_v4(),
            LaunchAdmissionState::Terminal,
            Some(LaunchOutcome::Completed),
        );
        write_launch_record(&launch_record_path(&directory, 6), &later_terminal).unwrap();
        let owner = MaintenanceWorkerIdentity::current().unwrap();
        assert_eq!(
            run_launch_evidence_cleanup(root.path(), 40, owner.clone()).unwrap(),
            LaunchCleanupOutcome::Yielded
        );
        assert!(!launch_epoch_directory(&directory, 1).exists());
        assert!(launch_epoch_directory(&directory, 5).exists());
        assert_eq!(
            run_launch_evidence_cleanup(root.path(), 40, owner).unwrap(),
            LaunchCleanupOutcome::Yielded
        );
        assert!(launch_epoch_directory(&directory, 5).exists());
        assert!(!launch_epoch_directory(&directory, 6).exists());
        assert_eq!(
            read_oldest_launch_epoch(&oldest_launch_epoch_path(&directory))
                .unwrap()
                .unwrap()
                .epoch,
            5
        );
    }

    #[test]
    fn launch_outcome_summary_keeps_terminal_classes_distinct() {
        let coordination = CoordinationRetentionOutcome {
            jobs_completed: 1,
            jobs_yielded: 0,
            duplicate_jobs: 0,
            gaps: Vec::new(),
        };
        let summary = |event| {
            summarize_launch_outcome(
                &Ok(event),
                &Ok(coordination.clone()),
                &Ok(LaunchCleanupOutcome::Completed),
            )
            .0
        };
        assert_eq!(
            summary(OpportunityOutcome::Completed),
            LaunchOutcome::Completed
        );
        assert_eq!(summary(OpportunityOutcome::Yielded), LaunchOutcome::Yielded);
        assert_eq!(
            summary(OpportunityOutcome::Cancelled),
            LaunchOutcome::Cancelled
        );
        assert_eq!(
            summary(OpportunityOutcome::Preserved),
            LaunchOutcome::PreservedGapped
        );
        let failed = summarize_launch_outcome(
            &Err("event failure".to_string()),
            &Ok(coordination),
            &Ok(LaunchCleanupOutcome::Completed),
        );
        assert_eq!(failed.0, LaunchOutcome::Failed);
        assert_eq!(failed.1.as_deref(), Some("event=event failure"));
    }
}
