//! Background observability snapshot worker for the interactive TUI.

use crate::observability::{
    MonitorSnapshot, ObservabilityRoot, ObservabilitySnapshotPort, SnapshotLimits,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, TryLockError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub(super) type MonitorSnapshotProvider = Box<dyn ObservabilitySnapshotPort + Send>;

pub(super) struct MonitorSnapshotWorker {
    shared: Arc<SnapshotWorkerShared>,
    join: Option<JoinHandle<()>>,
}

impl MonitorSnapshotWorker {
    pub(super) fn start(
        provider: MonitorSnapshotProvider,
        root: ObservabilityRoot,
        interval: Duration,
    ) -> Result<Self, String> {
        let shared = Arc::new(SnapshotWorkerShared::new(interval));
        let thread_shared = Arc::clone(&shared);
        let join = thread::Builder::new()
            .name("pty-broker-snapshot".to_string())
            .spawn(move || run_snapshot_worker(provider, root, thread_shared))
            .map_err(format_snapshot_worker_spawn_error)?;
        Ok(Self {
            shared,
            join: Some(join),
        })
    }

    pub(super) fn latest_snapshot(&self) -> Option<Arc<MonitorSnapshot>> {
        self.shared.latest_snapshot()
    }

    pub(super) fn set_interval(&self, interval: Duration) {
        self.shared.set_interval(interval);
    }

    pub(super) fn request_refresh(&self) {
        self.shared.request_refresh();
    }

    pub(super) fn shutdown_and_join(mut self) -> Result<(), String> {
        self.shared.request_shutdown();
        if let Some(join) = self.join.take() {
            join.join().map_err(|_| snapshot_worker_panic_error())?;
        }
        Ok(())
    }
}

impl Drop for MonitorSnapshotWorker {
    fn drop(&mut self) {
        self.shared.request_shutdown();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

struct SnapshotWorkerShared {
    latest: Mutex<Option<Arc<MonitorSnapshot>>>,
    state: Mutex<SnapshotWorkerState>,
    wake: Condvar,
    shutdown: AtomicBool,
}

impl SnapshotWorkerShared {
    fn new(interval: Duration) -> Self {
        Self {
            latest: Mutex::new(None),
            state: Mutex::new(SnapshotWorkerState::new(interval)),
            wake: Condvar::new(),
            shutdown: AtomicBool::new(false),
        }
    }

    fn latest_snapshot(&self) -> Option<Arc<MonitorSnapshot>> {
        match self.latest.try_lock() {
            Ok(guard) => guard.clone(),
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner().clone(),
            Err(TryLockError::WouldBlock) => None,
        }
    }

    fn publish(&self, snapshot: MonitorSnapshot) {
        *lock_or_recover(&self.latest) = Some(Arc::new(snapshot));
    }

    fn set_interval(&self, interval: Duration) {
        let mut state = lock_or_recover(&self.state);
        if state.interval == interval {
            return;
        }
        state.interval = interval;
        state.refresh_requested = true;
        self.wake.notify_one();
    }

    fn interval(&self) -> Duration {
        lock_or_recover(&self.state).interval
    }

    fn request_refresh(&self) {
        lock_or_recover(&self.state).refresh_requested = true;
        self.wake.notify_one();
    }

    fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        self.wake.notify_all();
    }

    fn shutdown_requested(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }
}

struct SnapshotWorkerState {
    interval: Duration,
    refresh_requested: bool,
}

impl SnapshotWorkerState {
    fn new(interval: Duration) -> Self {
        Self {
            interval,
            refresh_requested: true,
        }
    }
}

fn run_snapshot_worker(
    provider: MonitorSnapshotProvider,
    root: ObservabilityRoot,
    shared: Arc<SnapshotWorkerShared>,
) {
    let mut next_scan = Instant::now();
    while wait_for_scan(&shared, next_scan) {
        if shared.shutdown_requested() {
            return;
        }
        let snapshot = read_monitor_snapshot(provider.as_ref(), &root, &shared);
        if shared.shutdown_requested() {
            return;
        }
        shared.publish(snapshot);
        next_scan = Instant::now() + shared.interval();
    }
}

fn wait_for_scan(shared: &SnapshotWorkerShared, deadline: Instant) -> bool {
    let mut state = lock_or_recover(&shared.state);
    loop {
        if shared.shutdown_requested() {
            return false;
        }
        if state.refresh_requested {
            state.refresh_requested = false;
            return true;
        }
        let now = Instant::now();
        if now >= deadline {
            return true;
        }
        state = match shared.wake.wait_timeout(state, deadline - now) {
            Ok((guard, _)) => guard,
            Err(poisoned) => poisoned.into_inner().0,
        };
    }
}

fn read_monitor_snapshot(
    provider: &dyn ObservabilitySnapshotPort,
    root: &ObservabilityRoot,
    shared: &SnapshotWorkerShared,
) -> MonitorSnapshot {
    provider.snapshot_with_cancel(root, SnapshotLimits::default(), &|| {
        shared.shutdown_requested()
    })
}

fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn format_snapshot_worker_spawn_error(err: std::io::Error) -> String {
    format!("Failed to spawn TUI snapshot worker: {err}")
}

fn snapshot_worker_panic_error() -> String {
    "TUI snapshot worker panicked".to_string()
}
