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

pub(crate) fn try_admit(path: &Path, suffix: &str) -> Result<Option<File>, String> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path.with_extension(suffix))
        .map_err(|e| e.to_string())?;
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
pub(crate) fn entry(once: bool) -> Result<(), String> {
    let mut gate = [0u8];
    std::io::stdin()
        .read_exact(&mut gate)
        .map_err(|e| e.to_string())?;
    if gate != [1] {
        return Err("receipt helper gate rejected".into());
    }
    #[cfg(unix)]
    oulipoly_provider::client::enter_receipt_inspection_group()?;
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
    let Some(mut db) = MailboxDb::open_default_if_exists()? else {
        return Ok(());
    };
    let Some(mut owner) = try_admit(db.path(), "receipt-owner")? else {
        return Ok(());
    };
    let mut registry = crate::wiring::ReceiptRegistryCache::default();
    loop {
        // Durable admission cadence survives owner handoff. Serializing each
        // observer and then running its identical tick is intentionally avoided.
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
        if let Err(error) =
            super::poll_headless_receipt_tick_with(&mut db, |dir| registry.registry(dir))
        {
            // A failed preparation cannot
            // poison the cache into reusing stale endpoints on a later visit.
            eprintln!("receipt inspection: {error}");
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

fn command(once: bool) -> Result<Command, String> {
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
    let reader = std::thread::Builder::new()
        .name("receipt-heartbeat".into())
        .spawn(move || {
            let mut stdout = stdout;
            let mut byte = [0u8];
            while stdout.read_exact(&mut byte).is_ok() {
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
    match (result?, status?) {
        (true, status) if !status.success() => Err(format!("receipt helper exited: {status}")),
        _ => Ok(()),
    }
}

#[cfg(windows)]
#[path = "native_receipt_windows_job.rs"]
mod windows_job;
