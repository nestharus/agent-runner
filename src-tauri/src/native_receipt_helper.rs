//! Scoped inspection process: one namespace owner, retained registry, bounded
//! heartbeat watchdog and explicit kill/reap. No model/launch capability.
//! Roles: orchestration, accessor, validator.
use super::{CancellationToken, Duration, MailboxDb, ReceiptPollGuard};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub(crate) const ARG: &str = "--internal-native-receipt-helper";
const INTERVAL: Duration = Duration::from_secs(2);
const STALL_BOUND: Duration = Duration::from_secs(30);
// Scope containment also bounds descendants that close provider pipes but keep
// running. Registry/hash reuse lasts across ticks, not across helper generations.
const MAX_LIFETIME: Duration = Duration::from_secs(60);

fn admission_file(path: &Path, suffix: &str) -> Result<File, String> {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path.with_extension(suffix))
        .map_err(|e| e.to_string())
}

pub(crate) fn try_admit(path: &Path, suffix: &str) -> Result<Option<File>, String> {
    let file = admission_file(path, suffix)?;
    match <File as fs4::FileExt>::try_lock(&file) {
        Ok(()) => Ok(Some(file)),
        Err(fs4::TryLockError::WouldBlock) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

fn admission_delay(now: u128, last: u128) -> Duration {
    // A future wall-clock stamp must not permanently exclude one-shot sweeps.
    // The exclusive owner can safely reset the cadence after a clock rollback.
    match now.checked_sub(last) {
        Some(elapsed) => {
            INTERVAL.saturating_sub(Duration::from_millis(elapsed.min(u64::MAX as u128) as u64))
        }
        None => Duration::ZERO,
    }
}

#[cfg(test)]
#[test]
fn receipt_admission_cadence_recovers_from_clock_rollback() {
    assert_eq!(admission_delay(1500, 1000), Duration::from_millis(1500));
    assert_eq!(admission_delay(3000, 1000), Duration::ZERO);
    assert_eq!(admission_delay(1000, 1500), Duration::ZERO);
}

/// Called before CLI discovery, config loading, DB opening or any model path.
/// The parent's gate byte means process containment is installed before IO.
#[cfg(test)]
pub(crate) fn entry(once: bool) -> Result<(), String> {
    entry_target(once, None)
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct Target {
    pub attempt_id: String,
    pub anchor_identity: String,
    pub model_name: String,
    pub cwd: std::path::PathBuf,
    pub config_root: std::path::PathBuf,
}

// Bound the target argv independently of provider-owned opaque token length.
// This hashes already-read anchor bytes only, not executable/filesystem IO.
// Length framing plus Option presence preserves every original anchor field.
pub(crate) fn anchor_identity(
    anchor: &oulipoly_state::mailbox::MailboxDeliveryObservationAnchor,
) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    for field in [
        anchor.provider_name.as_str(),
        anchor.provider_instance_id.as_str(),
        anchor.settings_id.as_str(),
        anchor.provider_session_id.as_str(),
        anchor.resume_token.as_deref().unwrap_or_default(),
        anchor.expected_sha256.as_str(),
    ] {
        digest.update((field.len() as u64).to_be_bytes());
        digest.update(field.as_bytes());
    }
    digest.update([u8::from(anchor.resume_token.is_some())]);
    format!("{:x}", digest.finalize())
}

// Every returning/error/unwinding entry tears down the whole group, including
// itself. Do not leave cleanup to a timer thread that dies with the helper.
struct LocalTeardown;
impl Drop for LocalTeardown {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::kill(-libc::getpgrp(), libc::SIGKILL);
        }
    }
}

pub(crate) fn entry_target(once: bool, target: Option<Target>) -> Result<(), String> {
    let mut gate = [0u8];
    std::io::stdin()
        .read_exact(&mut gate)
        .map_err(|e| e.to_string())?;
    if gate != [1] {
        return Err("receipt helper gate rejected".into());
    }
    #[cfg(unix)]
    oulipoly_provider::client::enter_receipt_inspection_group()?;
    let _teardown = LocalTeardown;
    // A parent crash must not leave a namespace owner alive indefinitely. The
    // timer lives inside the disposable process, not in the caller's scope.
    std::thread::Builder::new()
        .name("receipt-lifetime".into())
        .spawn(|| {
            std::thread::sleep(MAX_LIFETIME);
            #[cfg(unix)]
            unsafe {
                libc::kill(-libc::getpgrp(), libc::SIGKILL);
            }
            std::process::exit(1);
        })
        .map_err(|e| e.to_string())?;
    let result = inspect(once, target);
    if let Err(error) = &result {
        eprintln!("receipt helper: {error}");
    }
    if result.is_ok() {
        // Process completion only, never receipt evidence. Unix cleanup kills
        // this helper too, so communicate completion before group teardown.
        std::io::stdout()
            .write_all(b"!")
            .map_err(|e| e.to_string())?;
        std::io::stdout().flush().map_err(|e| e.to_string())?;
    }
    result
}

fn inspect(once: bool, target: Option<Target>) -> Result<(), String> {
    if let Some(target) = target {
        return inspect_target(target);
    }
    let Some(db) = MailboxDb::open_default_if_exists()? else {
        return Ok(());
    };
    let Some(mut owner) = try_admit(db.path(), "receipt-owner")? else {
        return Ok(());
    };
    // The independent owner inode survives rebuild (only SQLite members are
    // reset). Never retain a connection or namespace fence through idle sleep.
    drop(db);
    let mut registry = crate::wiring::ReceiptRegistryCache::default();
    loop {
        let mut bytes = Vec::new();
        use std::io::{Seek, SeekFrom};
        owner.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        (&mut owner)
            .take(32)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        let last = String::from_utf8_lossy(&bytes).parse::<u128>().unwrap_or(0);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis();
        let delay = admission_delay(now, last);
        if !delay.is_zero() {
            if once {
                return Ok(());
            }
            std::thread::sleep(delay);
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis();
        owner.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        owner.set_len(0).map_err(|e| e.to_string())?;
        write!(owner, "{now}").map_err(|e| e.to_string())?;
        // Reopen under real shared custody every visit; rebuild may have
        // replaced the DB while idle. No stale connection/anchor survives it.
        if let Some(mut db) = MailboxDb::open_default_if_exists()? {
            if let Err(error) =
                super::poll_headless_receipt_tick_with(&mut db, |dir| registry.registry(dir))
            {
                eprintln!("receipt inspection: {error}");
            }
        }
        std::io::stdout()
            .write_all(b".")
            .map_err(|e| e.to_string())?;
        std::io::stdout().flush().map_err(|e| e.to_string())?;
        if once {
            return Ok(());
        }
    }
}

fn try_scan_admission() -> Result<Option<File>, String> {
    let Some(db) = MailboxDb::open_default_if_exists()? else {
        return Ok(None);
    };
    // Bind the independent inode under real custody. Rebuild leaves it intact.
    // Opportunistic observation must not wait behind another namespace scan.
    try_admit(db.path(), "receipt-scan")
}

fn wait_for_scan_admission(
    mut acquire: impl FnMut() -> Result<Option<File>, String>,
    bound: Duration,
) -> Result<File, String> {
    let deadline = Instant::now() + bound;
    loop {
        if let Some(admission) = acquire()? {
            return Ok(admission);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("receipt target scan admission deadline; attempt remains pending".into());
        }
        std::thread::sleep(remaining.min(Duration::from_millis(10)));
    }
}

#[cfg(test)]
#[test]
fn targeted_scan_waits_for_admission_and_bounds_persistent_contention() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("mailbox.db");
    let held = try_admit(&path, "receipt-scan").unwrap().unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        drop(held);
    });
    let admitted = wait_for_scan_admission(
        || try_admit(&path, "receipt-scan"),
        Duration::from_secs(2),
    ).unwrap();
    release.join().unwrap();
    let error = wait_for_scan_admission(
        || try_admit(&path, "receipt-scan"),
        Duration::from_millis(50),
    ).unwrap_err();
    assert!(error.contains("admission deadline"));
    drop(admitted);
}

fn inspect_target(target: Target) -> Result<(), String> {
    // Unlike an opportunistic periodic tick, a terminal request owes one
    // bounded opportunity after a contending scanner releases admission. Do not
    // equate a skipped scan with absence of native evidence at turn completion.
    // This wait is inside the contained helper, below its 30s stall watchdog.
    let admission = wait_for_scan_admission(try_scan_admission, Duration::from_secs(5))?;
    let Some(db) = MailboxDb::open_default_if_exists()? else {
        return Ok(());
    };
    let Some(anchor) = db.delivery_observation_anchor(&target.attempt_id)? else {
        return Ok(());
    };
    if anchor_identity(&anchor) != target.anchor_identity {
        return Err("receipt target anchor changed".into());
    }
    super::ensure_observation_not_stopped(&db, &anchor.provider_session_id)?;
    let mut cache = crate::wiring::ReceiptRegistryCache::default();
    let registry = cache.registry_at(Some(&target.config_root))?;
    let identity = oulipoly_runtime::session_provider::SessionProviderIdentity {
        model_name: target.model_name,
        provider_name: anchor.provider_name.clone(),
        provider_instance_id: Some(anchor.provider_instance_id.clone()),
        settings_id: anchor.settings_id.clone(),
    };
    super::confirm_delivery_observation(
        &db,
        &target.attempt_id,
        registry.as_ref(),
        identity,
        &target.cwd,
        &anchor,
    )?;
    drop(db);
    drop(admission);
    Ok(())
}

pub(crate) fn observe_target(target: Target) -> Result<(), String> {
    let mut command = command(true)?;
    command.arg(serde_json::to_string(&target).map_err(|e| e.to_string())?);
    #[cfg(test)]
    let bound = TEST_BOUND.get().unwrap_or(STALL_BOUND);
    #[cfg(not(test))]
    let bound = STALL_BOUND;
    supervise(&mut command, &CancellationToken::new(), bound)
}

#[cfg(test)]
thread_local! {
    pub(crate) static TEST_COMMAND: std::cell::RefCell<Option<Box<dyn Fn(bool) -> Command>>> = Default::default();
    pub(crate) static TEST_BOUND: std::cell::Cell<Option<Duration>> = const { std::cell::Cell::new(None) };
}

fn command(once: bool) -> Result<Command, String> {
    #[cfg(test)]
    if let Some(command) = TEST_COMMAND.with_borrow(|factory| factory.as_ref().map(|f| f(once))) {
        return Ok(command);
    }
    let mut command = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    command.arg(ARG);
    if once {
        command.arg("once");
    }
    Ok(command)
}

pub(crate) fn start() -> Result<ReceiptPollGuard, String> {
    start_command(command(false)?)
}

pub(crate) fn start_command(command: Command) -> Result<ReceiptPollGuard, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = stop.clone();
    let cancellation = CancellationToken::new();
    let cancelled = cancellation.clone();
    let worker = std::thread::Builder::new()
        .name("native-receipt-custodian".into())
        .spawn(move || {
            let mut command = command;
            while !stopping.load(Ordering::SeqCst) {
                if let Err(error) = supervise(&mut command, &cancelled, STALL_BOUND) {
                    tracing::warn!("receipt helper: {error}");
                }
                if stopping.load(Ordering::SeqCst) {
                    break;
                }
                std::thread::park_timeout(INTERVAL);
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(ReceiptPollGuard {
        stop,
        worker: Some(worker),
        cancellation: Some(cancellation),
    })
}

pub(crate) fn run_once() -> Result<(), String> {
    supervise(&mut command(true)?, &CancellationToken::new(), STALL_BOUND)
}

struct OwnedHelper {
    child: Child,
    reaped: bool,
    #[cfg(windows)]
    job: windows_job::Job,
}
impl OwnedHelper {
    fn spawn(command: &mut Command) -> Result<Self, String> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        for key in [
            "OULIPOLY_PARENT_INVOCATION",
            "OULIPOLY_RETURN_CHANNEL",
            "AGENT_BASH_OWNER_INVOCATION_UUID",
        ] {
            command.env_remove(key);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let child = command.spawn().map_err(|e| e.to_string())?;
        #[cfg(windows)]
        let job = match windows_job::Job::assign(&child) {
            Ok(job) => job,
            Err(e) => {
                let mut child = child;
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        };
        Ok(Self {
            child,
            reaped: false,
            #[cfg(windows)]
            job,
        })
    }
    fn finish(&mut self) -> Result<std::process::ExitStatus, String> {
        self.terminate();
        let status = self.child.wait().map_err(|e| e.to_string())?;
        self.reaped = true;
        Ok(status)
    }
    fn terminate(&mut self) {
        #[cfg(unix)]
        unsafe {
            // Direct Child has not been reaped: group ID cannot be reused.
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        #[cfg(windows)]
        self.job.terminate();
        let _ = self.child.kill();
    }
}
impl Drop for OwnedHelper {
    fn drop(&mut self) {
        if self.reaped {
            return;
        }
        self.terminate();
        // Do not detach an IO worker, release admission early, or discard the
        // direct process handle on cancellation. Reap before returning scope.
        let _ = self.child.wait();
    }
}

pub(crate) fn supervise(
    command: &mut Command,
    cancel: &CancellationToken,
    bound: Duration,
) -> Result<(), String> {
    if cancel.is_cancelled() {
        return Ok(());
    }
    let mut owned = OwnedHelper::spawn(command)?;
    let stdout = owned
        .child
        .stdout
        .take()
        .ok_or("receipt helper missing stdout")?;
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    let completed = Arc::new(AtomicBool::new(false));
    let completion = completed.clone();
    let reader = std::thread::Builder::new()
        .name("receipt-heartbeat".into())
        .spawn(move || {
            let mut stdout = stdout;
            let mut byte = [0u8];
            while stdout.read_exact(&mut byte).is_ok() {
                if byte == [b'!'] {
                    completion.store(true, Ordering::SeqCst);
                }
                let _ = send.try_send(());
            }
        })
        .map_err(|e| e.to_string())?;
    let result: Result<bool, String> = (|| {
        owned
            .child
            .stdin
            .take()
            .ok_or("receipt helper missing gate")?
            .write_all(&[1])
            .map_err(|e| e.to_string())?;
        let started = Instant::now();
        let mut heartbeat = started;
        loop {
            if cancel.is_cancelled() || started.elapsed() >= MAX_LIFETIME {
                return Ok(false);
            }
            if heartbeat.elapsed() >= bound {
                return Err("receipt helper stalled; terminated and reaped".into());
            }
            match receive.recv_timeout(Duration::from_millis(10)) {
                Ok(()) => heartbeat = Instant::now(),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(true),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    })();
    let status = owned.finish(); // kill/reap before joining the pipe reader
    reader
        .join()
        .map_err(|_| "receipt heartbeat reader panicked")?;
    let result = result?;
    let status = status?;
    if completed.load(Ordering::SeqCst) {
        return Ok(());
    }
    match (result, status) {
        (true, status) if !status.success() => Err(format!("receipt helper exited: {status}")),
        _ => Ok(()),
    }
}

#[cfg(windows)]
#[path = "native_receipt_windows_job.rs"]
mod windows_job;
