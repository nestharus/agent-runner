//! Cross-process wake-sweep admission and lease persistence.
//!
//! ## Declared roles
//!
//! `orchestration`, `serializer`

use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub(super) const WAKE_SWEEP_LEASE_DURATION: Duration = Duration::from_secs(10);
const WAKE_SWEEP_LEASE_SCHEMA_VERSION: u8 = 1;

pub(super) struct WakeSweepAdmission {
    mailbox_path: PathBuf,
    pub(super) token: String,
}

// The OS lock protects only lease-file mutation. The lease may expire while an
// old sweep is alive because per-session wake claims fence the valuable action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct WakeSweepLease {
    pub(super) schema_version: u8,
    pub(super) owner_token: String,
    pub(super) expires_at_unix_ms: u64,
    pub(super) handoff_token: Option<String>,
}

pub(super) enum WakeSweepAdmissionAttempt {
    Acquired(WakeSweepAdmission),
    Owned(String),
    CoordinationBusy,
}

impl Drop for WakeSweepAdmission {
    fn drop(&mut self) {
        if let Err(error) = clear_wake_sweep_lease(&self.mailbox_path, &self.token) {
            tracing::warn!("Failed to clear wake sweep lease: {error}");
        }
    }
}

pub(super) fn try_acquire_wake_sweep_admission(
    mailbox_path: &Path,
) -> Result<WakeSweepAdmissionAttempt, String> {
    let Some(_coordination) = try_acquire_wake_sweep_coordination(mailbox_path)? else {
        return Ok(WakeSweepAdmissionAttempt::CoordinationBusy);
    };
    let now = unix_time_ms()?;
    if let Some(lease) = read_wake_sweep_lease(mailbox_path)?
        && lease.expires_at_unix_ms > now
    {
        return Ok(WakeSweepAdmissionAttempt::Owned(lease.owner_token));
    }
    let token = uuid::Uuid::new_v4().to_string();
    write_wake_sweep_lease(
        mailbox_path,
        &WakeSweepLease {
            schema_version: WAKE_SWEEP_LEASE_SCHEMA_VERSION,
            owner_token: token.clone(),
            expires_at_unix_ms: now.saturating_add(wake_sweep_lease_duration_ms()),
            handoff_token: None,
        },
    )?;
    Ok(WakeSweepAdmissionAttempt::Acquired(WakeSweepAdmission {
        mailbox_path: mailbox_path.to_path_buf(),
        token,
    }))
}

pub(in crate::wake_coordinator) fn try_with_serialized_drain<T>(
    mailbox_path: &Path,
    drain: impl FnOnce() -> Result<T, String>,
) -> Result<Option<T>, String> {
    match try_acquire_wake_sweep_admission(mailbox_path)? {
        WakeSweepAdmissionAttempt::Acquired(_admission) => drain().map(Some),
        WakeSweepAdmissionAttempt::Owned(_) | WakeSweepAdmissionAttempt::CoordinationBusy => {
            Ok(None)
        }
    }
}

pub(super) fn try_acquire_wake_sweep_coordination(
    mailbox_path: &Path,
) -> Result<Option<File>, String> {
    let path = wake_sweep_admission_path(mailbox_path)?;
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&path).map_err(|error| {
        format!(
            "Failed to open wake sweep admission {}: {error}",
            path.display()
        )
    })?;
    match <File as fs4::FileExt>::try_lock(&file) {
        Ok(()) => Ok(Some(file)),
        Err(fs4::TryLockError::WouldBlock) => Ok(None),
        Err(fs4::TryLockError::Error(error)) => Err(format!(
            "Failed to acquire wake sweep admission {}: {error}",
            path.display()
        )),
    }
}

pub(super) fn acquire_wake_sweep_coordination(mailbox_path: &Path) -> Result<File, String> {
    let path = wake_sweep_admission_path(mailbox_path)?;
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&path).map_err(|error| {
        format!(
            "Failed to open wake sweep coordination {}: {error}",
            path.display()
        )
    })?;
    <File as fs4::FileExt>::lock(&file).map_err(|error| {
        format!(
            "Failed to lock wake sweep coordination {}: {error}",
            path.display()
        )
    })?;
    Ok(file)
}

fn wake_sweep_admission_path(mailbox_path: &Path) -> Result<PathBuf, String> {
    let file_name = mailbox_path
        .file_name()
        .ok_or_else(|| "PID mailbox path has no file name".to_string())?;
    let mut admission_name = file_name.to_os_string();
    admission_name.push(".wake-reclaim.lock");
    Ok(mailbox_path.with_file_name(admission_name))
}

pub(super) fn wake_sweep_lease_path(mailbox_path: &Path) -> Result<PathBuf, String> {
    let file_name = mailbox_path
        .file_name()
        .ok_or_else(|| "PID mailbox path has no file name".to_string())?;
    let mut lease_name = file_name.to_os_string();
    lease_name.push(".wake-reclaim-owner.json");
    Ok(mailbox_path.with_file_name(lease_name))
}

pub(super) fn read_wake_sweep_lease(mailbox_path: &Path) -> Result<Option<WakeSweepLease>, String> {
    let path = wake_sweep_lease_path(mailbox_path)?;
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "Failed to read wake sweep lease {}: {error}",
                path.display()
            ));
        }
    };
    match serde_json::from_str::<WakeSweepLease>(&text) {
        Ok(lease) if lease.schema_version == WAKE_SWEEP_LEASE_SCHEMA_VERSION => Ok(Some(lease)),
        Ok(lease) => {
            tracing::warn!(
                path = %path.display(),
                schema_version = lease.schema_version,
                "Replacing unsupported wake sweep lease"
            );
            Ok(None)
        }
        Err(error) => {
            tracing::warn!(path = %path.display(), "Replacing malformed wake sweep lease: {error}");
            Ok(None)
        }
    }
}

pub(super) fn write_wake_sweep_lease(
    mailbox_path: &Path,
    lease: &WakeSweepLease,
) -> Result<(), String> {
    let path = wake_sweep_lease_path(mailbox_path)?;
    let text = serde_json::to_vec(lease)
        .map_err(|error| format!("Failed to encode wake sweep lease: {error}"))?;
    let temporary = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&temporary).map_err(|error| {
            format!(
                "Failed to create wake sweep lease temporary {}: {error}",
                temporary.display()
            )
        })?;
        file.write_all(&text).map_err(|error| {
            format!(
                "Failed to write wake sweep lease temporary {}: {error}",
                temporary.display()
            )
        })?;
        file.sync_all().map_err(|error| {
            format!(
                "Failed to sync wake sweep lease temporary {}: {error}",
                temporary.display()
            )
        })?;
        drop(file);
        replace_wake_sweep_lease_file(&temporary, &path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn replace_wake_sweep_lease_file(temporary: &Path, destination: &Path) -> Result<(), String> {
    match std::fs::rename(temporary, destination) {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::PermissionDenied
            ) =>
        {
            std::fs::remove_file(destination).map_err(|remove_error| {
                format!(
                    "Failed to replace wake sweep lease {} after {error}: {remove_error}",
                    destination.display()
                )
            })?;
            std::fs::rename(temporary, destination).map_err(|rename_error| {
                format!(
                    "Failed to install wake sweep lease {}: {rename_error}",
                    destination.display()
                )
            })
        }
        Err(error) => Err(format!(
            "Failed to install wake sweep lease {}: {error}",
            destination.display()
        )),
    }
}

fn clear_wake_sweep_lease(mailbox_path: &Path, owner_token: &str) -> Result<(), String> {
    let _coordination = acquire_wake_sweep_coordination(mailbox_path)?;
    let Some(lease) = read_wake_sweep_lease(mailbox_path)? else {
        return Ok(());
    };
    if lease.owner_token != owner_token {
        return Ok(());
    }
    if lease.expires_at_unix_ms == 0 && lease.handoff_token.is_some() {
        return Ok(());
    }
    let path = wake_sweep_lease_path(mailbox_path)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "Failed to clear wake sweep lease {}: {error}",
            path.display()
        )),
    }
}

pub(super) fn unix_time_ms() -> Result<u64, String> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("System time precedes Unix epoch: {error}"))?;
    Ok(u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
}

fn wake_sweep_lease_duration_ms() -> u64 {
    u64::try_from(WAKE_SWEEP_LEASE_DURATION.as_millis()).unwrap_or(u64::MAX)
}
