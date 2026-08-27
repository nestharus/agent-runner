//! Detached wake-sweep bootstrap and owner-handoff protocol.
//!
//! ## Declared roles
//!
//! `orchestration`, `parser`

use oulipoly_state::mailbox::MailboxDb;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::lease::{
    WAKE_SWEEP_LEASE_DURATION, acquire_wake_sweep_coordination, read_wake_sweep_lease,
    try_acquire_wake_sweep_coordination, unix_time_ms, write_wake_sweep_lease,
};
use super::{WakeSweepRunOutcome, run_wake_reclaim_sweep};
use crate::wake_coordinator::constants::{
    WAKE_RECLAIM_HANDOFF_OWNER_ENV, WAKE_RECLAIM_HANDOFF_TOKEN_ENV,
};

pub(super) const WAKE_SWEEP_HANDOFF_RETRY_INTERVAL: Duration = Duration::from_millis(25);
const WAKE_SWEEP_BOOTSTRAP_OWNER_GRACE: Duration = Duration::from_millis(250);
const WAKE_SWEEP_BOOTSTRAP_OWNER_TOKEN: &str = "wake-reclaim-bootstrap";
const WAKE_SWEEP_BOOTSTRAP_HANDOFF_TOKEN: &str = "wake-reclaim-bootstrap";

pub(super) fn spawn_wake_reclaim_bootstrap_or_warn(owner_token: Option<&str>) {
    if let Err(error) = crate::wake_coordinator::spawn::spawn_detached_wake_reclaim_handoff(
        owner_token.unwrap_or(WAKE_SWEEP_BOOTSTRAP_OWNER_TOKEN),
        WAKE_SWEEP_BOOTSTRAP_HANDOFF_TOKEN,
    ) {
        tracing::warn!("{error}");
    }
}

pub(super) fn ensure_wake_sweep_handoff(mailbox_path: &Path, owner_token: &str) -> bool {
    ensure_wake_sweep_handoff_inner(mailbox_path, owner_token, false)
}

fn ensure_wake_sweep_handoff_inner(
    mailbox_path: &Path,
    owner_token: &str,
    expire_owner: bool,
) -> bool {
    let handoff_token = match register_wake_sweep_handoff(mailbox_path, owner_token, expire_owner) {
        Ok(Some(token)) => token,
        Ok(None) => return true,
        Err(error) => {
            tracing::warn!("Failed to register wake sweep handoff: {error}");
            return false;
        }
    };
    if let Err(error) = crate::wake_coordinator::spawn::spawn_detached_wake_reclaim_handoff(
        owner_token,
        &handoff_token,
    ) {
        let _ = clear_wake_sweep_handoff(mailbox_path, owner_token, &handoff_token);
        tracing::warn!("{error}");
        return false;
    }
    true
}

fn current_wake_sweep_lease(
    mailbox_path: &Path,
) -> Result<Option<super::lease::WakeSweepLease>, String> {
    let _coordination = acquire_wake_sweep_coordination(mailbox_path)?;
    read_wake_sweep_lease(mailbox_path)
}

pub(super) fn register_wake_sweep_handoff(
    mailbox_path: &Path,
    owner_token: &str,
    expire_owner: bool,
) -> Result<Option<String>, String> {
    let _coordination = acquire_wake_sweep_coordination(mailbox_path)?;
    let Some(mut lease) = read_wake_sweep_lease(mailbox_path)? else {
        return Ok(None);
    };
    if lease.owner_token != owner_token {
        return Ok(None);
    }
    if expire_owner {
        lease.expires_at_unix_ms = 0;
    }
    if lease.handoff_token.is_some() {
        write_wake_sweep_lease(mailbox_path, &lease)?;
        return Ok(None);
    }
    let handoff_token = uuid::Uuid::new_v4().to_string();
    lease.handoff_token = Some(handoff_token.clone());
    write_wake_sweep_lease(mailbox_path, &lease)?;
    Ok(Some(handoff_token))
}

fn clear_wake_sweep_handoff(
    mailbox_path: &Path,
    owner_token: &str,
    handoff_token: &str,
) -> Result<(), String> {
    let _coordination = acquire_wake_sweep_coordination(mailbox_path)?;
    let Some(mut lease) = read_wake_sweep_lease(mailbox_path)? else {
        return Ok(());
    };
    if lease.owner_token != owner_token || lease.handoff_token.as_deref() != Some(handoff_token) {
        return Ok(());
    }
    lease.handoff_token = None;
    write_wake_sweep_lease(mailbox_path, &lease)
}

pub(crate) fn is_wake_reclaim_handoff_invocation() -> bool {
    std::env::var_os(WAKE_RECLAIM_HANDOFF_OWNER_ENV).is_some()
        || std::env::var_os(WAKE_RECLAIM_HANDOFF_TOKEN_ENV).is_some()
}

pub(crate) fn run_wake_reclaim_handoff_invocation() -> Result<(), String> {
    let owner_token = required_handoff_env(WAKE_RECLAIM_HANDOFF_OWNER_ENV)?;
    let handoff_token = required_handoff_env(WAKE_RECLAIM_HANDOFF_TOKEN_ENV)?;
    if handoff_token == WAKE_SWEEP_BOOTSTRAP_HANDOFF_TOKEN {
        let expected_owner =
            (owner_token != WAKE_SWEEP_BOOTSTRAP_OWNER_TOKEN).then_some(owner_token.as_str());
        return run_wake_reclaim_bootstrap_handoff(expected_owner);
    }
    let mailbox_path = MailboxDb::default_path()?;
    // A monotonic deadline prevents a wall-clock rollback from retaining a
    // detached waiter forever.
    let wait_deadline = Instant::now() + WAKE_SWEEP_LEASE_DURATION;
    loop {
        let Some(lease) = current_wake_sweep_lease(&mailbox_path)? else {
            return Ok(());
        };
        if lease.owner_token != owner_token
            || lease.handoff_token.as_deref() != Some(handoff_token.as_str())
        {
            return Ok(());
        }
        if lease.expires_at_unix_ms > unix_time_ms()? {
            if Instant::now() >= wait_deadline {
                expire_wake_sweep_handoff_owner(&mailbox_path, &owner_token, &handoff_token)?;
                continue;
            }
            std::thread::sleep(WAKE_SWEEP_HANDOFF_RETRY_INTERVAL);
            continue;
        }
        match run_wake_reclaim_sweep("process_start_handoff", &|| false)? {
            WakeSweepRunOutcome::Completed | WakeSweepRunOutcome::Contended(_) => return Ok(()),
            WakeSweepRunOutcome::CoordinationBusy => {
                std::thread::sleep(WAKE_SWEEP_HANDOFF_RETRY_INTERVAL);
            }
        }
    }
}

fn run_wake_reclaim_bootstrap_handoff(expected_owner: Option<&str>) -> Result<(), String> {
    let mailbox_path = MailboxDb::default_path()?;
    if !mailbox_path.exists() {
        return Ok(());
    }
    let Some(_bootstrap_admission) = try_acquire_wake_sweep_bootstrap_admission(&mailbox_path)?
    else {
        return Ok(());
    };
    let mut waiting_owner: Option<(String, Instant)> = None;
    loop {
        match run_wake_reclaim_sweep("process_start_handoff", &|| false)? {
            WakeSweepRunOutcome::Completed => return Ok(()),
            WakeSweepRunOutcome::Contended(owner_token) => {
                if let Some(expected_owner) = expected_owner {
                    if owner_token != expected_owner {
                        return Ok(());
                    }
                    try_expire_wake_sweep_owner(&mailbox_path, &owner_token)?;
                } else {
                    let waiting_since = match waiting_owner.as_ref() {
                        Some((waiting_token, waiting_since)) if waiting_token == &owner_token => {
                            *waiting_since
                        }
                        _ => {
                            let waiting_since = Instant::now();
                            waiting_owner = Some((owner_token.clone(), waiting_since));
                            waiting_since
                        }
                    };
                    if waiting_since.elapsed() >= WAKE_SWEEP_BOOTSTRAP_OWNER_GRACE {
                        try_expire_wake_sweep_owner(&mailbox_path, &owner_token)?;
                    }
                }
            }
            WakeSweepRunOutcome::CoordinationBusy => {}
        }
        std::thread::sleep(WAKE_SWEEP_HANDOFF_RETRY_INTERVAL);
    }
}

fn try_expire_wake_sweep_owner(mailbox_path: &Path, owner_token: &str) -> Result<(), String> {
    let Some(_coordination) = try_acquire_wake_sweep_coordination(mailbox_path)? else {
        return Ok(());
    };
    let Some(mut lease) = read_wake_sweep_lease(mailbox_path)? else {
        return Ok(());
    };
    if lease.owner_token == owner_token {
        lease.expires_at_unix_ms = 0;
        write_wake_sweep_lease(mailbox_path, &lease)?;
    }
    Ok(())
}

fn expire_wake_sweep_handoff_owner(
    mailbox_path: &Path,
    owner_token: &str,
    handoff_token: &str,
) -> Result<(), String> {
    let _coordination = acquire_wake_sweep_coordination(mailbox_path)?;
    let Some(mut lease) = read_wake_sweep_lease(mailbox_path)? else {
        return Ok(());
    };
    if lease.owner_token == owner_token && lease.handoff_token.as_deref() == Some(handoff_token) {
        lease.expires_at_unix_ms = 0;
        write_wake_sweep_lease(mailbox_path, &lease)?;
    }
    Ok(())
}

fn required_handoff_env(name: &str) -> Result<String, String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("Missing wake reclaim handoff environment: {name}"))
}

fn wake_sweep_bootstrap_admission_path(mailbox_path: &Path) -> Result<PathBuf, String> {
    let file_name = mailbox_path
        .file_name()
        .ok_or_else(|| "PID mailbox path has no file name".to_string())?;
    let mut admission_name = file_name.to_os_string();
    admission_name.push(".wake-reclaim-bootstrap.lock");
    Ok(mailbox_path.with_file_name(admission_name))
}

pub(super) fn try_acquire_wake_sweep_bootstrap_admission(
    mailbox_path: &Path,
) -> Result<Option<File>, String> {
    let path = wake_sweep_bootstrap_admission_path(mailbox_path)?;
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&path).map_err(|error| {
        format!(
            "Failed to open wake sweep bootstrap admission {}: {error}",
            path.display()
        )
    })?;
    match <File as fs4::FileExt>::try_lock(&file) {
        Ok(()) => Ok(Some(file)),
        Err(fs4::TryLockError::WouldBlock) => Ok(None),
        Err(fs4::TryLockError::Error(error)) => Err(format!(
            "Failed to acquire wake sweep bootstrap admission {}: {error}",
            path.display()
        )),
    }
}
