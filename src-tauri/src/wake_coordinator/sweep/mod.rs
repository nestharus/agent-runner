//! ## Declared roles
//!
//! `orchestration`

mod candidate;
mod consumed;
mod handoff;
mod lease;
mod live_pty_retry;
mod plan;
mod state;

use oulipoly_state::mailbox::MailboxDb;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use super::auto_wake_env::is_auto_wake_invocation;
use super::constants::{
    WAKE_RECLAIM_STATE_SNAPSHOT_TIMEOUT_SECONDS, WAKE_RECLAIM_SWEEP_INTERVAL_SECONDS,
    WAKE_RECLAIM_SWEEP_SCAN_LIMIT,
};
use super::wake_start::start_wake_chain;
use handoff::{
    WAKE_SWEEP_HANDOFF_RETRY_INTERVAL, ensure_wake_sweep_handoff,
    spawn_wake_reclaim_bootstrap_or_warn,
};
pub(crate) use handoff::{is_wake_reclaim_handoff_invocation, run_wake_reclaim_handoff_invocation};
pub(super) use lease::try_with_serialized_drain;
use lease::{WakeSweepAdmissionAttempt, try_acquire_wake_sweep_admission};
use live_pty_retry::retry_pending_live_pty_deliveries;
pub(crate) use live_pty_retry::{LivePtyRetryDriverGuard, start_live_pty_retry_driver_for_owner};
use plan::{plan_wake_sweep, trace_wake_sweep_candidate, wake_sweep_start_input};

#[cfg(test)]
use handoff::{register_wake_sweep_handoff, try_acquire_wake_sweep_bootstrap_admission};
#[cfg(test)]
use lease::{
    WakeSweepAdmission, acquire_wake_sweep_coordination, read_wake_sweep_lease,
    wake_sweep_lease_path, write_wake_sweep_lease,
};
#[cfg(test)]
use oulipoly_state::mailbox::WakeSweepCandidate;
#[cfg(test)]
use plan::select_startable_sweep_candidate;
#[cfg(test)]
use std::fs::File;
#[cfg(test)]
use std::path::Path;
#[cfg(test)]
use std::time::Instant;

pub(crate) fn run_startup_wake_reclaim_sweep() {
    if is_auto_wake_invocation() {
        return;
    }
    run_wake_reclaim_sweep_or_warn("process_start");
}

pub(crate) fn run_post_settlement_wake_reclaim_sweep() {
    run_wake_reclaim_sweep_or_warn("session_settlement");
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

enum WakeSweepRunOutcome {
    Completed,
    Contended(String),
    CoordinationBusy,
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
    super::admission::drain_one_owned(&mut db)?;
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
    if let Some(candidate) = start {
        if is_cancelled() {
            return Ok(WakeSweepRunOutcome::Completed);
        }
        let diagnostic = start_wake_chain(wake_sweep_start_input(&candidate, trigger));
        trace_wake_sweep_candidate(&candidate.session_id, &diagnostic);
    }
    Ok(WakeSweepRunOutcome::Completed)
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

    fn reacquire_wake_sweep_admission_after_release(mailbox_path: &Path) -> WakeSweepAdmission {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match try_acquire_wake_sweep_admission(mailbox_path).unwrap() {
                WakeSweepAdmissionAttempt::Acquired(admission) => return admission,
                WakeSweepAdmissionAttempt::CoordinationBusy if Instant::now() < deadline => {
                    // A parallel test can fork while the flock descriptor is open;
                    // the child retains it briefly until exec applies CLOEXEC.
                    std::thread::sleep(Duration::from_millis(1));
                }
                WakeSweepAdmissionAttempt::CoordinationBusy => {
                    panic!("admission remained busy after the owner exited")
                }
                WakeSweepAdmissionAttempt::Owned(owner_token) => {
                    panic!("stale admission owner remained after release: {owner_token}")
                }
            }
        }
    }

    fn reacquire_wake_sweep_bootstrap_after_release(mailbox_path: &Path) -> File {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match try_acquire_wake_sweep_bootstrap_admission(mailbox_path).unwrap() {
                Some(admission) => return admission,
                None if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                None => panic!("bootstrap admission remained busy after the owner exited"),
            }
        }
    }

    #[test]
    fn wake_sweep_selects_only_the_oldest_startable_session() {
        let selected = select_startable_sweep_candidate(vec![
            WakeSweepCandidate {
                session_id: "newer".to_string(),
                auto_wake_count: 0,
                min_pending_seq: 20,
                max_pending_seq: 20,
            },
            WakeSweepCandidate {
                session_id: "oldest".to_string(),
                auto_wake_count: 0,
                min_pending_seq: 10,
                max_pending_seq: 10,
            },
            WakeSweepCandidate {
                session_id: "newest".to_string(),
                auto_wake_count: 0,
                min_pending_seq: 30,
                max_pending_seq: 30,
            },
        ])
        .unwrap();

        assert_eq!(selected.session_id, "oldest");
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
        drop(reacquire_wake_sweep_admission_after_release(&mailbox_path));
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
        drop(reacquire_wake_sweep_bootstrap_after_release(&mailbox_path));
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

        let second = reacquire_wake_sweep_admission_after_release(&mailbox_path);
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
        assert!(start.is_none());
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
