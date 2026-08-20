//! ## Declared roles
//!
//! `filter`, `formatter`, `mapper`, `orchestration`

mod candidate;
mod consumed;
mod state;

use oulipoly_state::mailbox::{
    MailboxDb, RuntimeGenerationRow, RuntimeLifecycleState, SessionGenerationProjection,
    SessionLiveness, WakeSweepCandidate,
};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::auto_wake_env::is_auto_wake_invocation;
use super::constants::{
    LIVE_PTY_RETRY_INTERVAL_SECONDS, WAKE_RECLAIM_HANDOFF_OWNER_ENV,
    WAKE_RECLAIM_HANDOFF_TOKEN_ENV, WAKE_RECLAIM_STATE_SNAPSHOT_TIMEOUT_SECONDS,
    WAKE_RECLAIM_SWEEP_INTERVAL_SECONDS, WAKE_RECLAIM_SWEEP_LIMIT, WAKE_RECLAIM_SWEEP_SCAN_LIMIT,
};
use super::diagnostics::WakeDiagnostic;
use super::wake_start::{StartWakeInput, start_wake_chain};
use crate::mailbox_delivery::PtyMailboxDeliveryDiagnostic;

const WAKE_SWEEP_LEASE_DURATION: Duration = Duration::from_secs(10);
const WAKE_SWEEP_HANDOFF_RETRY_INTERVAL: Duration = Duration::from_millis(25);
const WAKE_SWEEP_BOOTSTRAP_OWNER_GRACE: Duration = Duration::from_millis(250);
const WAKE_SWEEP_LEASE_SCHEMA_VERSION: u8 = 1;
const WAKE_SWEEP_BOOTSTRAP_OWNER_TOKEN: &str = "wake-reclaim-bootstrap";
const WAKE_SWEEP_BOOTSTRAP_HANDOFF_TOKEN: &str = "wake-reclaim-bootstrap";

pub(crate) fn run_startup_wake_reclaim_sweep() {
    if is_auto_wake_invocation() {
        return;
    }
    run_wake_reclaim_sweep_or_warn("process_start");
}

pub(crate) struct StartupWakeReclaimGuard {
    stop: Arc<AtomicBool>,
    owned_lease: Arc<Mutex<Option<String>>>,
    done: Receiver<()>,
    handle: Option<JoinHandle<()>>,
    schedule_handoff: Option<fn(Option<&str>)>,
}

impl StartupWakeReclaimGuard {
    fn schedule_handoff(&self) {
        let owner_token = self
            .owned_lease
            .try_lock()
            .ok()
            .and_then(|owner| owner.clone());
        if let Some(schedule_handoff) = self.schedule_handoff {
            schedule_handoff(owner_token.as_deref());
        }
    }
}

impl Drop for StartupWakeReclaimGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            match self.done.try_recv() {
                Ok(()) => {
                    if handle.is_finished()
                        && let Err(err) = handle.join()
                    {
                        tracing::warn!(?err, "Startup wake reclaim sweep failed after completion");
                    }
                }
                Err(TryRecvError::Empty) => {
                    self.schedule_handoff();
                    self.stop.store(true, Ordering::SeqCst);
                    handle.thread().unpark();
                }
                Err(TryRecvError::Disconnected) => {
                    self.schedule_handoff();
                    if handle.is_finished()
                        && let Err(err) = handle.join()
                    {
                        tracing::warn!(?err, "Startup wake reclaim sweep failed after disconnect");
                    }
                }
            }
        }
    }
}

pub(crate) fn start_startup_wake_reclaim_sweep() -> Option<StartupWakeReclaimGuard> {
    if is_auto_wake_invocation() {
        return None;
    }
    static STARTUP_SWEEP_STARTED: AtomicBool = AtomicBool::new(false);
    if STARTUP_SWEEP_STARTED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return None;
    }
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let owned_lease = Arc::new(Mutex::new(None));
    let thread_owned_lease = Arc::clone(&owned_lease);
    let (done_tx, done) = mpsc::channel();
    match spawn_wake_reclaim_thread(move || {
        run_wake_reclaim_sweep_or_warn_with_cancel_and_owner(
            "process_start",
            &|| thread_stop.load(Ordering::SeqCst),
            Some(&thread_owned_lease),
        );
        let _ = done_tx.send(());
    }) {
        Ok(handle) => Some(StartupWakeReclaimGuard {
            stop,
            owned_lease,
            done,
            handle: Some(handle),
            schedule_handoff: Some(spawn_wake_reclaim_bootstrap_or_warn),
        }),
        Err(err) => {
            STARTUP_SWEEP_STARTED.store(false, Ordering::SeqCst);
            warn_wake_reclaim_driver_start_failed(err);
            None
        }
    }
}

pub(crate) fn start_wake_reclaim_maintenance_driver() {
    if is_auto_wake_invocation() {
        return;
    }
    static MAINTENANCE_DRIVER_STARTED: AtomicBool = AtomicBool::new(false);
    if let Err(err) = try_start_wake_reclaim_driver(&MAINTENANCE_DRIVER_STARTED, || {
        spawn_wake_reclaim_thread(|| wake_reclaim_maintenance_loop("maintenance_start")).map(drop)
    }) {
        warn_wake_reclaim_driver_start_failed(err);
    }
}

struct WakeSweepAdmission {
    mailbox_path: PathBuf,
    token: String,
}

// The OS lock protects only lease-file mutation. The lease may expire while an
// old sweep is alive because per-session wake claims fence the valuable action.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WakeSweepLease {
    schema_version: u8,
    owner_token: String,
    expires_at_unix_ms: u64,
    handoff_token: Option<String>,
}

enum WakeSweepAdmissionAttempt {
    Acquired(WakeSweepAdmission),
    Owned(String),
    CoordinationBusy,
}

enum WakeSweepRunOutcome {
    Completed,
    Contended(String),
    CoordinationBusy,
}

impl Drop for WakeSweepAdmission {
    fn drop(&mut self) {
        if let Err(error) = clear_wake_sweep_lease(&self.mailbox_path, &self.token) {
            tracing::warn!("Failed to clear wake sweep lease: {error}");
        }
    }
}

pub(crate) struct LivePtyRetryDriverGuard {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Drop for LivePtyRetryDriverGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            handle.thread().unpark();
            if let Err(err) = handle.join() {
                tracing::warn!(?err, "Live PTY retry driver failed to join cleanly");
            }
        }
    }
}

pub(crate) fn start_live_pty_retry_driver_for_owner() -> Option<LivePtyRetryDriverGuard> {
    if is_auto_wake_invocation() {
        return None;
    }
    spawn_live_pty_retry_driver()
}

fn spawn_live_pty_retry_driver() -> Option<LivePtyRetryDriverGuard> {
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    match std::thread::Builder::new()
        .name("oulipoly-live-pty-retry".to_string())
        .spawn(move || live_pty_retry_loop(thread_stop))
    {
        Ok(handle) => Some(LivePtyRetryDriverGuard {
            stop,
            handle: Some(handle),
        }),
        Err(err) => {
            tracing::warn!("Failed to start live PTY retry driver: {err}");
            None
        }
    }
}

fn live_pty_retry_loop(stop: Arc<AtomicBool>) {
    retry_pending_live_pty_deliveries_or_warn("live_pty_retry_start");
    loop {
        std::thread::park_timeout(Duration::from_secs(LIVE_PTY_RETRY_INTERVAL_SECONDS));
        if stop.load(Ordering::SeqCst) {
            break;
        }
        retry_pending_live_pty_deliveries_or_warn("live_pty_retry_tick");
    }
}

fn try_start_wake_reclaim_driver(
    started: &AtomicBool,
    spawn: impl FnOnce() -> Result<(), std::io::Error>,
) -> Result<bool, std::io::Error> {
    if started
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Ok(false);
    }
    if let Err(error) = spawn() {
        started.store(false, Ordering::SeqCst);
        return Err(error);
    }
    Ok(true)
}

fn spawn_wake_reclaim_thread(
    worker: impl FnOnce() + Send + 'static,
) -> Result<JoinHandle<()>, std::io::Error> {
    std::thread::Builder::new()
        .name("oulipoly-wake-reclaim-sweep".to_string())
        .spawn(worker)
}

fn warn_wake_reclaim_driver_start_failed(err: std::io::Error) {
    tracing::warn!("Failed to start wake reclaim sweep driver: {err}");
}

fn wake_reclaim_maintenance_loop(initial_trigger: &'static str) {
    run_wake_reclaim_sweep_or_warn(initial_trigger);
    loop {
        std::thread::sleep(Duration::from_secs(WAKE_RECLAIM_SWEEP_INTERVAL_SECONDS));
        run_wake_reclaim_sweep_or_warn("maintenance_tick");
    }
}

fn run_wake_reclaim_sweep_or_warn(trigger: &str) {
    run_wake_reclaim_sweep_or_warn_with_cancel(trigger, &|| false);
}

fn run_wake_reclaim_sweep_or_warn_with_cancel(trigger: &str, is_cancelled: &dyn Fn() -> bool) {
    run_wake_reclaim_sweep_or_warn_with_cancel_and_owner(trigger, is_cancelled, None);
}

fn run_wake_reclaim_sweep_or_warn_with_cancel_and_owner(
    trigger: &str,
    is_cancelled: &dyn Fn() -> bool,
    owned_lease: Option<&Mutex<Option<String>>>,
) {
    run_wake_reclaim_sweep_or_warn_with_runner(trigger, is_cancelled, || {
        run_wake_reclaim_sweep_with_owner(trigger, is_cancelled, owned_lease)
    });
}

fn run_wake_reclaim_sweep_or_warn_with_runner(
    trigger: &str,
    is_cancelled: &dyn Fn() -> bool,
    mut run_sweep: impl FnMut() -> Result<WakeSweepRunOutcome, String>,
) {
    loop {
        match run_sweep() {
            Ok(WakeSweepRunOutcome::Contended(owner_token)) if trigger == "process_start" => {
                if let Ok(mailbox_path) = MailboxDb::default_path()
                    && !ensure_wake_sweep_handoff(&mailbox_path, &owner_token)
                {
                    spawn_wake_reclaim_bootstrap_or_warn(None);
                }
                return;
            }
            Ok(WakeSweepRunOutcome::CoordinationBusy)
                if trigger == "process_start" && !is_cancelled() =>
            {
                std::thread::park_timeout(WAKE_SWEEP_HANDOFF_RETRY_INTERVAL);
            }
            Ok(_) => return,
            Err(err) => {
                if trigger == "process_start" {
                    spawn_wake_reclaim_bootstrap_or_warn(None);
                }
                warn_wake_reclaim_sweep_failed(trigger, err);
                return;
            }
        }
    }
}

fn warn_wake_reclaim_sweep_failed(trigger: &str, err: String) {
    tracing::warn!(trigger, "Wake reclaim sweep failed: {err}");
}

fn run_wake_reclaim_sweep(
    trigger: &str,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<WakeSweepRunOutcome, String> {
    run_wake_reclaim_sweep_with_owner(trigger, is_cancelled, None)
}

fn run_wake_reclaim_sweep_with_owner(
    trigger: &str,
    is_cancelled: &dyn Fn() -> bool,
    owned_lease: Option<&Mutex<Option<String>>>,
) -> Result<WakeSweepRunOutcome, String> {
    if is_cancelled() {
        return Ok(WakeSweepRunOutcome::Completed);
    }
    let mailbox_path = MailboxDb::default_path()?;
    if !mailbox_path.exists() {
        return Ok(WakeSweepRunOutcome::Completed);
    }
    let _admission = match try_acquire_wake_sweep_admission(&mailbox_path)? {
        WakeSweepAdmissionAttempt::Acquired(admission) => {
            if let Some(owned_lease) = owned_lease {
                *owned_lease
                    .lock()
                    .map_err(|_| "Wake sweep owner token lock was poisoned".to_string())? =
                    Some(admission.token.clone());
            }
            admission
        }
        WakeSweepAdmissionAttempt::Owned(owner_token) => {
            tracing::debug!(
                trigger,
                "Wake reclaim sweep already owned by another process"
            );
            return Ok(WakeSweepRunOutcome::Contended(owner_token));
        }
        WakeSweepAdmissionAttempt::CoordinationBusy => {
            tracing::debug!(trigger, "Wake reclaim sweep coordination is busy");
            return Ok(WakeSweepRunOutcome::CoordinationBusy);
        }
    };
    let Some(mut db) = MailboxDb::open_default_if_exists()? else {
        return Ok(WakeSweepRunOutcome::Completed);
    };
    retry_pending_live_pty_deliveries(&mut db, trigger, is_cancelled)?;
    if is_cancelled() {
        return Ok(WakeSweepRunOutcome::Completed);
    }
    let candidates = db.wake_sessions().wake_sweep_candidates(
        super::constants::WAKE_CLAIM_STALE_AFTER_SECONDS,
        WAKE_RECLAIM_SWEEP_SCAN_LIMIT,
    )?;
    if candidates.is_empty() {
        return Ok(WakeSweepRunOutcome::Completed);
    }
    let start = plan_wake_sweep(
        &mut db,
        candidates,
        state::open_default_state_read_only_with_timeout_and_cancel(
            Duration::from_secs(WAKE_RECLAIM_STATE_SNAPSHOT_TIMEOUT_SECONDS),
            is_cancelled,
        ),
    )?;
    drop(db);
    for candidate in start {
        if is_cancelled() {
            return Ok(WakeSweepRunOutcome::Completed);
        }
        let diagnostic = start_wake_chain(wake_sweep_start_input(&candidate, trigger));
        trace_wake_sweep_candidate(&candidate.session_id, &diagnostic);
    }
    Ok(WakeSweepRunOutcome::Completed)
}

fn try_acquire_wake_sweep_admission(
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

fn try_acquire_wake_sweep_coordination(mailbox_path: &Path) -> Result<Option<File>, String> {
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

fn acquire_wake_sweep_coordination(mailbox_path: &Path) -> Result<File, String> {
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

fn wake_sweep_lease_path(mailbox_path: &Path) -> Result<PathBuf, String> {
    let file_name = mailbox_path
        .file_name()
        .ok_or_else(|| "PID mailbox path has no file name".to_string())?;
    let mut lease_name = file_name.to_os_string();
    lease_name.push(".wake-reclaim-owner.json");
    Ok(mailbox_path.with_file_name(lease_name))
}

fn wake_sweep_bootstrap_admission_path(mailbox_path: &Path) -> Result<PathBuf, String> {
    let file_name = mailbox_path
        .file_name()
        .ok_or_else(|| "PID mailbox path has no file name".to_string())?;
    let mut admission_name = file_name.to_os_string();
    admission_name.push(".wake-reclaim-bootstrap.lock");
    Ok(mailbox_path.with_file_name(admission_name))
}

fn try_acquire_wake_sweep_bootstrap_admission(mailbox_path: &Path) -> Result<Option<File>, String> {
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

fn read_wake_sweep_lease(mailbox_path: &Path) -> Result<Option<WakeSweepLease>, String> {
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

fn write_wake_sweep_lease(mailbox_path: &Path, lease: &WakeSweepLease) -> Result<(), String> {
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

fn unix_time_ms() -> Result<u64, String> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("System time precedes Unix epoch: {error}"))?;
    Ok(u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
}

fn wake_sweep_lease_duration_ms() -> u64 {
    u64::try_from(WAKE_SWEEP_LEASE_DURATION.as_millis()).unwrap_or(u64::MAX)
}

fn spawn_wake_reclaim_bootstrap_or_warn(owner_token: Option<&str>) {
    if let Err(error) = super::spawn::spawn_detached_wake_reclaim_handoff(
        owner_token.unwrap_or(WAKE_SWEEP_BOOTSTRAP_OWNER_TOKEN),
        WAKE_SWEEP_BOOTSTRAP_HANDOFF_TOKEN,
    ) {
        tracing::warn!("{error}");
    }
}

fn ensure_wake_sweep_handoff(mailbox_path: &Path, owner_token: &str) -> bool {
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
    if let Err(error) =
        super::spawn::spawn_detached_wake_reclaim_handoff(owner_token, &handoff_token)
    {
        let _ = clear_wake_sweep_handoff(mailbox_path, owner_token, &handoff_token);
        tracing::warn!("{error}");
        return false;
    }
    true
}

fn current_wake_sweep_lease(mailbox_path: &Path) -> Result<Option<WakeSweepLease>, String> {
    let _coordination = acquire_wake_sweep_coordination(mailbox_path)?;
    read_wake_sweep_lease(mailbox_path)
}

fn register_wake_sweep_handoff(
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

fn retry_pending_live_pty_deliveries_or_warn(trigger: &str) {
    if let Err(err) = retry_pending_live_pty_deliveries_from_default_db(trigger) {
        warn_wake_reclaim_sweep_failed(trigger, err);
    }
}

fn retry_pending_live_pty_deliveries_from_default_db(trigger: &str) -> Result<(), String> {
    let Some(mut db) = MailboxDb::open_default_if_exists()? else {
        return Ok(());
    };
    retry_pending_live_pty_deliveries(&mut db, trigger, &|| false)
}

fn retry_pending_live_pty_deliveries(
    db: &mut MailboxDb,
    trigger: &str,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    let session_ids = db
        .wake_sessions()
        .pending_delivery_session_ids(WAKE_RECLAIM_SWEEP_SCAN_LIMIT)?;
    for session_id in session_ids {
        if is_cancelled() {
            return Ok(());
        }
        match live_pty_retry_applicability(db, &session_id)? {
            LivePtyRetryApplicability::Applicable => {
                let diagnostic = crate::mailbox_delivery::attempt_pty_mailbox_delivery_with_trigger(
                    db,
                    &session_id,
                    trigger,
                );
                trace_live_pty_retry(trigger, &session_id, &diagnostic);
            }
            LivePtyRetryApplicability::Skip { reason, liveness } => {
                trace_live_pty_retry_skip(trigger, &session_id, reason, liveness.as_deref());
            }
        }
    }
    Ok(())
}

enum LivePtyRetryApplicability {
    Applicable,
    Skip {
        reason: &'static str,
        liveness: Option<String>,
    },
}

fn live_pty_retry_applicability(
    db: &mut MailboxDb,
    session_id: &str,
) -> Result<LivePtyRetryApplicability, String> {
    let runtime = match db
        .runtime_lifecycle_reader()
        .session_generation_projection(session_id)
        .map_err(|error| error.to_string())?
    {
        SessionGenerationProjection::One(runtime) => runtime,
        SessionGenerationProjection::None => {
            return Ok(LivePtyRetryApplicability::Skip {
                reason: "no_runtime",
                liveness: None,
            });
        }
        SessionGenerationProjection::Multiple(_) => {
            return Ok(LivePtyRetryApplicability::Skip {
                reason: "ambiguous_runtime",
                liveness: None,
            });
        }
    };
    if !runtime_is_running_pty_with_socket(&runtime) {
        return Ok(LivePtyRetryApplicability::Skip {
            reason: runtime_skip_reason(&runtime),
            liveness: None,
        });
    }
    let liveness = db
        .runtime_lifecycle()
        .reconcile_session_liveness(session_id)?;
    if liveness == SessionLiveness::Busy {
        Ok(LivePtyRetryApplicability::Applicable)
    } else {
        Ok(LivePtyRetryApplicability::Skip {
            reason: "not_busy",
            liveness: Some(format!("{liveness:?}")),
        })
    }
}

fn runtime_skip_reason(runtime: &RuntimeGenerationRow) -> &'static str {
    if runtime.runtime_mode != "pty_interactive" {
        "not_pty"
    } else if runtime.lifecycle_state != RuntimeLifecycleState::Running {
        "not_running"
    } else {
        "no_socket"
    }
}

fn runtime_is_running_pty_with_socket(runtime: &RuntimeGenerationRow) -> bool {
    runtime.runtime_mode == "pty_interactive"
        && runtime.lifecycle_state == RuntimeLifecycleState::Running
        && runtime
            .pty_control_path
            .as_deref()
            .is_some_and(|path| !path.is_empty())
}

fn wake_sweep_start_input<'a>(
    candidate: &'a WakeSweepCandidate,
    trigger: &'a str,
) -> StartWakeInput<'a> {
    StartWakeInput {
        session_id: &candidate.session_id,
        reason: trigger,
        auto_wake_count: candidate.auto_wake_count,
        renew_token: None,
    }
}

struct WakeSweepPlan {
    start: Vec<WakeSweepCandidate>,
}

fn wake_sweep_plan(
    db: &mut MailboxDb,
    candidates: Vec<WakeSweepCandidate>,
    state: Option<&oulipoly_state::StateDb>,
) -> Result<WakeSweepPlan, String> {
    let recoverable = partition_wake_sweep_candidates(db, candidates, state)?;
    let start = select_recoverable_sweep_candidates(recoverable);
    Ok(wake_sweep_plan_from_selected(start))
}

fn partition_wake_sweep_candidates(
    db: &mut MailboxDb,
    candidates: Vec<WakeSweepCandidate>,
    state: Option<&oulipoly_state::StateDb>,
) -> Result<Vec<WakeSweepCandidate>, String> {
    let mut recoverable = Vec::new();
    for candidate in candidates {
        match candidate::wake_sweep_candidate_disposition(db, state, &candidate)? {
            WakeSweepDisposition::Recoverable => {
                recoverable.push(candidate);
            }
            WakeSweepDisposition::Abandoned => {
                trace_abandoned_candidate_retained(&candidate.session_id);
            }
            WakeSweepDisposition::Skip => {}
        }
    }
    Ok(recoverable)
}

fn plan_wake_sweep(
    db: &mut MailboxDb,
    candidates: Vec<WakeSweepCandidate>,
    state: Result<Option<oulipoly_state::StateDb>, String>,
) -> Result<Vec<WakeSweepCandidate>, String> {
    let state = state?;
    let plan = wake_sweep_plan(db, candidates, state.as_ref())?;
    Ok(plan.start)
}

fn wake_sweep_plan_from_selected(start: Vec<WakeSweepCandidate>) -> WakeSweepPlan {
    WakeSweepPlan { start }
}

fn select_recoverable_sweep_candidates(
    mut candidates: Vec<WakeSweepCandidate>,
) -> Vec<WakeSweepCandidate> {
    if candidates.len() <= WAKE_RECLAIM_SWEEP_LIMIT {
        return candidates;
    }
    candidates.sort_by_key(|candidate| candidate.min_pending_seq);
    let oldest_slots = WAKE_RECLAIM_SWEEP_LIMIT.div_ceil(2);
    let newest_slots = WAKE_RECLAIM_SWEEP_LIMIT.saturating_sub(oldest_slots);
    let mut selected = candidates
        .iter()
        .take(oldest_slots)
        .cloned()
        .collect::<Vec<_>>();
    for candidate in candidates.iter().rev().take(newest_slots) {
        if !selected
            .iter()
            .any(|existing| existing.session_id == candidate.session_id)
        {
            selected.push(candidate.clone());
        }
    }
    selected
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WakeSweepDisposition {
    Recoverable,
    Abandoned,
    Skip,
}

fn trace_abandoned_candidate_retained(session_id: &str) {
    tracing::warn!(
        session_id,
        "Wake reclaim sweep retained abandoned candidate because terminal reap lacks cross-store authority"
    );
}

fn trace_wake_sweep_candidate(session_id: &str, diagnostic: &WakeDiagnostic) {
    tracing::debug!(
        session_id,
        status = diagnostic.status.as_str(),
        "Wake reclaim sweep candidate evaluated"
    );
}

fn trace_live_pty_retry(
    trigger: &str,
    session_id: &str,
    diagnostic: &PtyMailboxDeliveryDiagnostic,
) {
    tracing::debug!(
        session_id,
        status = diagnostic.status.as_str(),
        submitted = diagnostic.submitted,
        delivered = diagnostic.delivered_seqs.len(),
        "Wake reclaim sweep retried live PTY mailbox delivery"
    );
    if crate::mailbox_delivery::trace_notify_enabled() {
        eprintln!(
            concat!(
                "oulipoly_notify_trace trigger={} session_id={} liveness=Busy ",
                "attempted={} decision={} inject_status={} submitted={} ",
                "remaining_pending={:?} message={:?}"
            ),
            trigger,
            session_id,
            diagnostic.attempted,
            if diagnostic.submitted {
                "inject"
            } else {
                "skip"
            },
            diagnostic.status,
            diagnostic.submitted,
            diagnostic.remaining_pending,
            diagnostic.message,
        );
    }
}

fn trace_live_pty_retry_skip(
    trigger: &str,
    session_id: &str,
    reason: &str,
    liveness: Option<&str>,
) {
    if !crate::mailbox_delivery::trace_notify_enabled() {
        return;
    }
    eprintln!(
        concat!(
            "oulipoly_notify_trace trigger={} session_id={} liveness={} ",
            "attempted=false decision=skip inject_status={} submitted=false"
        ),
        trigger,
        session_id,
        liveness.unwrap_or("unknown"),
        reason,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_state::mailbox::{
        AgentBashCompleteEnqueue, EnqueueResult, WakeClaimAcquireResult, WakeClaimRequest,
    };

    static TEST_HANDOFF_SCHEDULED: AtomicBool = AtomicBool::new(false);
    static TEST_HANDOFF_OWNER_MATCHED: AtomicBool = AtomicBool::new(false);
    static TEST_HANDOFF_ASSERTION_LOCK: Mutex<()> = Mutex::new(());

    fn record_test_handoff(owner_token: Option<&str>) {
        TEST_HANDOFF_SCHEDULED.store(true, Ordering::SeqCst);
        TEST_HANDOFF_OWNER_MATCHED.store(owner_token == Some("test-owner"), Ordering::SeqCst);
    }

    #[test]
    fn startup_wake_reclaim_thread_returns_while_the_sweep_is_blocked() {
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let worker_release = Arc::clone(&release);

        let handle = spawn_wake_reclaim_thread(move || {
            started_tx.send(()).unwrap();
            let (lock, condition) = &*worker_release;
            let released = lock.lock().unwrap();
            drop(
                condition
                    .wait_while(released, |released| !*released)
                    .unwrap(),
            );
        })
        .unwrap();

        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(!handle.is_finished());
        let (lock, condition) = &*release;
        *lock.lock().unwrap() = true;
        condition.notify_one();
        handle.join().unwrap();
    }

    #[test]
    fn startup_guard_joins_an_already_finished_sweep() {
        let stop = Arc::new(AtomicBool::new(false));
        let completed = Arc::new(AtomicBool::new(false));
        let worker_completed = Arc::clone(&completed);
        let (done_tx, done) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            worker_completed.store(true, Ordering::SeqCst);
            done_tx.send(()).unwrap();
            finished_tx.send(()).unwrap();
        });
        finished_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let guard = StartupWakeReclaimGuard {
            stop: Arc::clone(&stop),
            owned_lease: Arc::new(Mutex::new(None)),
            done,
            handle: Some(handle),
            schedule_handoff: None,
        };

        drop(guard);

        assert!(completed.load(Ordering::SeqCst));
        assert!(!stop.load(Ordering::SeqCst));
    }

    #[test]
    fn startup_guard_hands_off_and_cancels_without_waiting() {
        let _handoff_assertion = TEST_HANDOFF_ASSERTION_LOCK.lock().unwrap();
        TEST_HANDOFF_SCHEDULED.store(false, Ordering::SeqCst);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let (done_tx, done) = mpsc::channel();
        let (started_tx, started_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            while !worker_stop.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(1));
            }
            let _ = done_tx.send(());
            finished_tx.send(()).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let guard = StartupWakeReclaimGuard {
            stop: Arc::clone(&stop),
            owned_lease: Arc::new(Mutex::new(None)),
            done,
            handle: Some(handle),
            schedule_handoff: Some(record_test_handoff),
        };
        let started = std::time::Instant::now();

        drop(guard);

        assert!(stop.load(Ordering::SeqCst));
        assert!(TEST_HANDOFF_SCHEDULED.load(Ordering::SeqCst));
        assert!(started.elapsed() < Duration::from_millis(100));
        finished_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn startup_guard_does_not_join_an_uncooperative_worker_without_a_bound() {
        let _handoff_assertion = TEST_HANDOFF_ASSERTION_LOCK.lock().unwrap();
        TEST_HANDOFF_SCHEDULED.store(false, Ordering::SeqCst);
        TEST_HANDOFF_OWNER_MATCHED.store(false, Ordering::SeqCst);
        let stop = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let worker_release = Arc::clone(&release);
        let (done_tx, done) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            while !worker_release.load(Ordering::SeqCst) {
                std::thread::park_timeout(Duration::from_millis(5));
            }
            let _ = done_tx.send(());
            finished_tx.send(()).unwrap();
        });
        let worker_thread = handle.thread().clone();
        let guard = StartupWakeReclaimGuard {
            stop: Arc::clone(&stop),
            owned_lease: Arc::new(Mutex::new(Some("test-owner".to_string()))),
            done,
            handle: Some(handle),
            schedule_handoff: Some(record_test_handoff),
        };
        let started = std::time::Instant::now();

        drop(guard);

        assert!(stop.load(Ordering::SeqCst));
        assert!(TEST_HANDOFF_SCHEDULED.load(Ordering::SeqCst));
        assert!(TEST_HANDOFF_OWNER_MATCHED.load(Ordering::SeqCst));
        assert!(started.elapsed() < Duration::from_millis(100));
        release.store(true, Ordering::SeqCst);
        worker_thread.unpark();
        finished_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn process_start_coordination_contention_retries_after_holder_releases() {
        let directory = tempfile::tempdir().unwrap();
        let mailbox_path = directory.path().join("pid-identity.db");
        drop(MailboxDb::open(&mailbox_path).unwrap());
        let holder = acquire_wake_sweep_coordination(&mailbox_path).unwrap();
        let worker_mailbox_path = mailbox_path.clone();
        let (attempted_tx, attempted_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            let mut first_attempt = true;
            run_wake_reclaim_sweep_or_warn_with_runner("process_start", &|| false, || {
                let attempt = try_acquire_wake_sweep_admission(&worker_mailbox_path)?;
                if first_attempt {
                    first_attempt = false;
                    attempted_tx.send(()).unwrap();
                }
                Ok(match attempt {
                    WakeSweepAdmissionAttempt::Acquired(_) => WakeSweepRunOutcome::Completed,
                    WakeSweepAdmissionAttempt::Owned(owner_token) => {
                        WakeSweepRunOutcome::Contended(owner_token)
                    }
                    WakeSweepAdmissionAttempt::CoordinationBusy => {
                        WakeSweepRunOutcome::CoordinationBusy
                    }
                })
            });
            finished_tx.send(()).unwrap();
        });

        attempted_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(finished_rx.try_recv().is_err());
        drop(holder);
        finished_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn failed_wake_reclaim_driver_spawn_leaves_admission_retryable() {
        let started = AtomicBool::new(false);
        let attempts = std::cell::Cell::new(0);

        let first = try_start_wake_reclaim_driver(&started, || {
            attempts.set(attempts.get() + 1);
            Err(std::io::Error::other("synthetic spawn failure"))
        });
        assert!(first.is_err());
        assert!(!started.load(Ordering::SeqCst));

        let second = try_start_wake_reclaim_driver(&started, || {
            attempts.set(attempts.get() + 1);
            Ok(())
        });
        assert_eq!(second.unwrap(), true);
        assert!(started.load(Ordering::SeqCst));

        let third = try_start_wake_reclaim_driver(&started, || {
            attempts.set(attempts.get() + 1);
            Ok(())
        });
        assert_eq!(third.unwrap(), false);
        assert_eq!(attempts.get(), 2);
    }

    #[test]
    fn wake_sweep_admission_is_cross_handle_single_flight_and_reusable() {
        let directory = tempfile::tempdir().unwrap();
        let mailbox_path = directory.path().join("pid-identity.db");
        drop(MailboxDb::open(&mailbox_path).unwrap());

        let first = match try_acquire_wake_sweep_admission(&mailbox_path).unwrap() {
            WakeSweepAdmissionAttempt::Acquired(admission) => admission,
            _ => panic!("first sweep must acquire admission"),
        };
        assert!(
            matches!(
                try_acquire_wake_sweep_admission(&mailbox_path).unwrap(),
                WakeSweepAdmissionAttempt::Owned(_) | WakeSweepAdmissionAttempt::CoordinationBusy
            ),
            "a second handle must not enter the same sweep"
        );
        drop(first);
        assert!(
            matches!(
                try_acquire_wake_sweep_admission(&mailbox_path).unwrap(),
                WakeSweepAdmissionAttempt::Acquired(_)
            ),
            "admission must return after the owner exits"
        );
    }

    #[test]
    fn wake_sweep_bootstrap_is_cross_handle_single_flight_and_reusable() {
        let directory = tempfile::tempdir().unwrap();
        let mailbox_path = directory.path().join("pid-identity.db");
        drop(MailboxDb::open(&mailbox_path).unwrap());

        let first = try_acquire_wake_sweep_bootstrap_admission(&mailbox_path)
            .unwrap()
            .expect("first bootstrap must acquire admission");
        assert!(
            try_acquire_wake_sweep_bootstrap_admission(&mailbox_path)
                .unwrap()
                .is_none(),
            "a second bootstrap must not run concurrently"
        );
        drop(first);
        assert!(
            try_acquire_wake_sweep_bootstrap_admission(&mailbox_path)
                .unwrap()
                .is_some(),
            "bootstrap admission must return after the owner exits"
        );
    }

    #[test]
    fn expired_wake_sweep_lease_is_replaceable_while_old_owner_remains_alive() {
        let directory = tempfile::tempdir().unwrap();
        let mailbox_path = directory.path().join("pid-identity.db");
        drop(MailboxDb::open(&mailbox_path).unwrap());
        let first = match try_acquire_wake_sweep_admission(&mailbox_path).unwrap() {
            WakeSweepAdmissionAttempt::Acquired(admission) => admission,
            _ => panic!("first sweep must acquire admission"),
        };
        let first_token = first.token.clone();
        {
            let _coordination = acquire_wake_sweep_coordination(&mailbox_path).unwrap();
            let mut lease = read_wake_sweep_lease(&mailbox_path).unwrap().unwrap();
            lease.expires_at_unix_ms = 0;
            write_wake_sweep_lease(&mailbox_path, &lease).unwrap();
        }

        let second = match try_acquire_wake_sweep_admission(&mailbox_path).unwrap() {
            WakeSweepAdmissionAttempt::Acquired(admission) => admission,
            _ => panic!("an expired owner must not suppress a later sweep"),
        };
        assert_ne!(second.token, first_token);
        drop(first);
        assert_eq!(
            read_wake_sweep_lease(&mailbox_path)
                .unwrap()
                .unwrap()
                .owner_token,
            second.token
        );
        drop(second);
        assert!(read_wake_sweep_lease(&mailbox_path).unwrap().is_none());
    }

    #[test]
    fn wake_sweep_handoff_registration_is_single_and_can_expire_the_owner() {
        let directory = tempfile::tempdir().unwrap();
        let mailbox_path = directory.path().join("pid-identity.db");
        drop(MailboxDb::open(&mailbox_path).unwrap());
        let owner = match try_acquire_wake_sweep_admission(&mailbox_path).unwrap() {
            WakeSweepAdmissionAttempt::Acquired(admission) => admission,
            _ => panic!("sweep must acquire admission"),
        };

        let handoff = register_wake_sweep_handoff(&mailbox_path, &owner.token, false)
            .unwrap()
            .unwrap();
        assert!(
            register_wake_sweep_handoff(&mailbox_path, &owner.token, false)
                .unwrap()
                .is_none()
        );
        assert!(
            register_wake_sweep_handoff(&mailbox_path, &owner.token, true)
                .unwrap()
                .is_none()
        );
        let lease = read_wake_sweep_lease(&mailbox_path).unwrap().unwrap();
        assert_eq!(lease.handoff_token.as_deref(), Some(handoff.as_str()));
        assert_eq!(lease.expires_at_unix_ms, 0);
        drop(owner);
        assert!(read_wake_sweep_lease(&mailbox_path).unwrap().is_some());
        let successor = match try_acquire_wake_sweep_admission(&mailbox_path).unwrap() {
            WakeSweepAdmissionAttempt::Acquired(admission) => admission,
            _ => panic!("the handoff must replace the expired owner"),
        };
        drop(successor);
        assert!(read_wake_sweep_lease(&mailbox_path).unwrap().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn wake_sweep_lease_replacement_does_not_follow_a_leaf_symlink() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let mailbox_path = directory.path().join("pid-identity.db");
        drop(MailboxDb::open(&mailbox_path).unwrap());
        let protected = directory.path().join("protected.txt");
        std::fs::write(&protected, "do not mutate").unwrap();
        let lease_path = wake_sweep_lease_path(&mailbox_path).unwrap();
        symlink(&protected, &lease_path).unwrap();

        let admission = match try_acquire_wake_sweep_admission(&mailbox_path).unwrap() {
            WakeSweepAdmissionAttempt::Acquired(admission) => admission,
            _ => panic!("malformed aliased lease must be replaceable"),
        };

        assert_eq!(
            std::fs::read_to_string(&protected).unwrap(),
            "do not mutate"
        );
        assert!(
            !std::fs::symlink_metadata(&lease_path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        drop(admission);
    }

    #[test]
    fn unavailable_state_prevents_terminal_reap_mutation() {
        let directory = tempfile::tempdir().unwrap();
        let state_dir = directory.path().join("agent-bash").join("blocked-state");
        std::fs::create_dir_all(&state_dir).unwrap();
        let meta = state_dir.join("meta.json");
        let log = state_dir.join("log");
        let rc = state_dir.join("rc");
        std::fs::write(&meta, r#"{"caller_chain":[]}"#).unwrap();
        std::fs::write(&log, "pending\n").unwrap();
        std::fs::write(&rc, "0\n").unwrap();
        let state_dir = state_dir.to_string_lossy().into_owned();
        let meta = meta.to_string_lossy().into_owned();
        let log = log.to_string_lossy().into_owned();
        let rc = rc.to_string_lossy().into_owned();
        let mut db = MailboxDb::open(&directory.path().join("pid-identity.db")).unwrap();
        let row = match db
            .enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
                session_id: "state-unavailable-session",
                handle: "state-unavailable-handle",
                payload_json: r#"{"schema_version":1,"kind":"agent_bash_complete"}"#,
                owner_invocation_uuid: None,
                matched_os_pid: None,
                matched_os_boot_id: None,
                matched_os_pid_starttime_ticks: None,
                matched_chain_index: None,
                state_dir: &state_dir,
                meta_path: &meta,
                log_path: &log,
                rc_path: &rc,
                rc: 0,
            })
            .unwrap()
        {
            EnqueueResult::Inserted(row) => row,
            other => panic!("unexpected enqueue result: {other:?}"),
        };
        let claim_token = "state-unavailable-claim";
        let acquired = db
            .wake_sessions()
            .try_acquire_wake_claim(WakeClaimRequest {
                session_id: &row.session_id,
                claim_token,
                reason: "maintenance_tick",
                auto_wake_count: 1,
                wake_invocation_uuid: None,
                stale_after_seconds: 600,
            })
            .unwrap();
        assert!(matches!(acquired, WakeClaimAcquireResult::Acquired(_)));
        let claim_before = db
            .wake_session_reader()
            .wake_claim(&row.session_id)
            .unwrap()
            .unwrap();
        let candidate = WakeSweepCandidate {
            session_id: row.session_id.clone(),
            auto_wake_count: 0,
            min_pending_seq: row.seq,
            max_pending_seq: row.seq,
        };

        let invalid_state = directory.path().join("invalid-state.db");
        std::fs::write(&invalid_state, "not sqlite").unwrap();
        let result = plan_wake_sweep(
            &mut db,
            vec![candidate.clone()],
            state::open_state_read_only_at(&invalid_state),
        );

        assert!(
            result
                .unwrap_err()
                .contains("Failed to open State read-only for wake sweep")
        );
        let rows = db.list_mailbox("state-unavailable-session", true).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].delivered_at.is_none());
        assert!(rows[0].delivery_error.is_none());
        assert_eq!(rows[0].delivery_attempts, 0);
        let claim_after = db
            .wake_session_reader()
            .wake_claim(&row.session_id)
            .unwrap()
            .unwrap();
        assert_eq!(claim_after.claim_token, claim_before.claim_token);
        assert_eq!(claim_after.auto_wake_count, claim_before.auto_wake_count);

        let second = db
            .enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
                session_id: "state-unavailable-session",
                handle: "newer-state-unavailable-handle",
                payload_json: r#"{"schema_version":1,"kind":"agent_bash_complete"}"#,
                owner_invocation_uuid: None,
                matched_os_pid: None,
                matched_os_boot_id: None,
                matched_os_pid_starttime_ticks: None,
                matched_chain_index: None,
                state_dir: &state_dir,
                meta_path: &meta,
                log_path: &log,
                rc_path: &rc,
                rc: 0,
            })
            .unwrap();
        assert!(matches!(second, EnqueueResult::Inserted(_)));

        let start = plan_wake_sweep(&mut db, vec![candidate], Ok(None)).unwrap();
        assert!(start.is_empty());
        let rows = db.list_mailbox("state-unavailable-session", true).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.delivery_error.is_none()));
        assert_eq!(
            db.wake_session_reader()
                .wake_claim("state-unavailable-session")
                .unwrap()
                .unwrap()
                .claim_token,
            claim_token
        );
    }
}
