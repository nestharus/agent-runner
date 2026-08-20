//! Background observability snapshot worker for the interactive TUI.

use crate::observability::{
    MonitorSnapshot, ObservabilityRoot, ObservabilitySnapshotPort, SnapshotLimits,
};
use oulipoly_core::CancellationToken;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, TryLockError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

// Four idle units per work unit cap the snapshot worker at a 20% duty cycle.
const SNAPSHOT_IDLE_MULTIPLIER: u32 = 4;

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
    shutdown: CancellationToken,
}

impl SnapshotWorkerShared {
    fn new(interval: Duration) -> Self {
        Self {
            latest: Mutex::new(None),
            state: Mutex::new(SnapshotWorkerState::new(interval)),
            wake: Condvar::new(),
            shutdown: CancellationToken::new(),
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
        self.shutdown.cancel();
        self.wake.notify_all();
    }

    fn shutdown_requested(&self) -> bool {
        self.shutdown.is_cancelled()
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
    let mut earliest_forced_scan = next_scan;
    while wait_for_scan(&shared, next_scan, earliest_forced_scan) {
        if shared.shutdown_requested() {
            return;
        }
        let snapshot_started = Instant::now();
        let snapshot = read_monitor_snapshot(provider.as_ref(), &root, &shared);
        let snapshot_elapsed = snapshot_started.elapsed();
        if shared.shutdown_requested() {
            return;
        }
        shared.publish(snapshot);
        let now = Instant::now();
        earliest_forced_scan = now + snapshot_idle_duration(snapshot_elapsed);
        next_scan = now + refresh_delay(shared.interval(), snapshot_elapsed);
    }
}

fn snapshot_idle_duration(snapshot_elapsed: Duration) -> Duration {
    snapshot_elapsed.saturating_mul(SNAPSHOT_IDLE_MULTIPLIER)
}

fn refresh_delay(configured_interval: Duration, snapshot_elapsed: Duration) -> Duration {
    configured_interval.max(snapshot_idle_duration(snapshot_elapsed))
}

fn scan_is_due(
    refresh_requested: bool,
    now: Instant,
    scheduled_deadline: Instant,
    earliest_forced_scan: Instant,
) -> bool {
    (refresh_requested && now >= earliest_forced_scan) || now >= scheduled_deadline
}

fn wait_for_scan(
    shared: &SnapshotWorkerShared,
    scheduled_deadline: Instant,
    earliest_forced_scan: Instant,
) -> bool {
    let mut state = lock_or_recover(&shared.state);
    loop {
        if shared.shutdown_requested() {
            return false;
        }
        let now = Instant::now();
        if scan_is_due(
            state.refresh_requested,
            now,
            scheduled_deadline,
            earliest_forced_scan,
        ) {
            state.refresh_requested = false;
            return true;
        }
        let deadline = if state.refresh_requested {
            scheduled_deadline.min(earliest_forced_scan)
        } else {
            scheduled_deadline
        };
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
    provider.snapshot_with_cancel(root, SnapshotLimits::default(), &shared.shutdown)
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

#[cfg(test)]
mod cadence_tests {
    use super::*;

    #[test]
    fn fast_snapshot_keeps_configured_interval() {
        assert_eq!(
            refresh_delay(Duration::from_millis(500), Duration::from_millis(100)),
            Duration::from_millis(500)
        );
    }

    #[test]
    fn slow_snapshot_caps_worker_duty_cycle() {
        assert_eq!(
            refresh_delay(Duration::from_millis(500), Duration::from_secs(2)),
            Duration::from_secs(8)
        );
    }

    #[test]
    fn forced_refresh_cannot_bypass_snapshot_idle_window() {
        let now = Instant::now();
        let earliest_forced_scan = now + Duration::from_secs(8);
        let scheduled_deadline = now + Duration::from_secs(10);

        assert!(!scan_is_due(
            true,
            now + Duration::from_secs(7),
            scheduled_deadline,
            earliest_forced_scan
        ));
        assert!(scan_is_due(
            true,
            earliest_forced_scan,
            scheduled_deadline,
            earliest_forced_scan
        ));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::observability::{MonitorStatus, MonitorSummary};
    use crate::provider_registry::{ProviderRegistry, ProviderRegistryOptions};
    use crate::session_metadata::TranscriptLookupMode;
    use crate::session_provider::{self, SessionProviderIdentity, SessionProviderLocateRequest};
    use oulipoly_config::{
        ModelConfig, PromptMode, ProviderConfig,
        provider_implementation_ref::ProviderImplementationRef,
    };
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::time::SystemTime;

    struct BlockingProviderSnapshot {
        registry: ProviderRegistry,
        identity: SessionProviderIdentity,
    }

    impl ObservabilitySnapshotPort for BlockingProviderSnapshot {
        fn snapshot_with_cancel(
            &self,
            _root: &ObservabilityRoot,
            _limits: SnapshotLimits,
            cancellation: &CancellationToken,
        ) -> MonitorSnapshot {
            let _ = session_provider::locate_transcript_with_raw_metadata_with_cancellation(
                SessionProviderLocateRequest {
                    registry: &self.registry,
                    identity: self.identity.clone(),
                    session_id: "blocked-provider-session",
                    lookup_mode: TranscriptLookupMode::RequireExisting,
                    effective_cwd: None,
                    purpose: Some("inspect"),
                    tail_bytes_hint: Some(1024),
                },
                cancellation,
            );
            empty_snapshot()
        }
    }

    #[test]
    fn worker_shutdown_terminates_and_reaps_blocked_provider_process() {
        let directory = tempfile::tempdir().unwrap();
        let pid_path = directory.path().join("provider.pid");
        let provider_path = write_blocking_provider(directory.path(), &pid_path);
        let worker = MonitorSnapshotWorker::start(
            Box::new(BlockingProviderSnapshot {
                registry: blocking_provider_registry(&provider_path),
                identity: blocking_provider_identity(),
            }),
            ObservabilityRoot::default(),
            Duration::from_secs(60),
        )
        .unwrap();
        wait_for_path(&pid_path);
        let pid = std::fs::read_to_string(&pid_path)
            .unwrap()
            .trim()
            .parse::<i32>()
            .unwrap();
        assert!(process_exists(pid));

        worker.shutdown_and_join().unwrap();

        wait_for_process_exit(pid);
        assert!(!process_exists(pid));
    }

    fn empty_snapshot() -> MonitorSnapshot {
        MonitorSnapshot {
            generated_at: SystemTime::now(),
            root_invocation_uuid: None,
            active_session_id: None,
            summary: MonitorSummary {
                status: MonitorStatus::Unknown,
                total_nodes: 0,
                invocation_nodes: 0,
                running_nodes: 0,
                pending_mailbox_count: 0,
                running_agent_bash_count: 0,
                diagnostics_count: 0,
            },
            nodes: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn write_blocking_provider(directory: &Path, pid_path: &Path) -> PathBuf {
        let provider_path = directory.join("blocking-provider.py");
        let script = format!(
            r#"#!/usr/bin/env python3
import json
import os
import pathlib
import sys
import time

request = json.loads(sys.stdin.read() or "{{}}")
contract = request.get("contract", "oulipoly.provider/v1")
request_id = request.get("request_id", "request-blocked")
subcommand = sys.argv[1] if len(sys.argv) > 1 else ""

if subcommand == "describe":
    response = {{
        "contract": contract,
        "request_id": request_id,
        "ok": True,
        "result": {{
            "provider_id": "provider-a",
            "display_name": "Provider A",
            "contract_versions": ["oulipoly.provider/v1"],
            "preferred_contract": "oulipoly.provider/v1",
            "capabilities": {{
                "launch": False,
                "policy": False,
                "quota": False,
                "session": True,
                "terminal": False,
                "rotation": False,
                "discovery": False,
                "settings": False,
                "setup_brain": False,
                "setup": False,
                "migration": False
            }},
            "settings_schema_id": "provider-a-test-settings"
        }}
    }}
    print(json.dumps(response))
elif subcommand == "session.locate_transcript":
    pathlib.Path({pid_path}).write_text(str(os.getpid()), encoding="utf-8")
    while True:
        time.sleep(1)
else:
    raise SystemExit(2)
"#,
            pid_path = serde_json::to_string(&pid_path.display().to_string()).unwrap(),
        );
        std::fs::write(&provider_path, script).unwrap();
        let mut permissions = std::fs::metadata(&provider_path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&provider_path, permissions).unwrap();
        provider_path
    }

    fn blocking_provider_registry(provider_path: &Path) -> ProviderRegistry {
        ProviderRegistry::from_model_configs(
            &[ModelConfig {
                name: "model-a".to_string(),
                prompt_mode: PromptMode::Arg,
                providers: vec![ProviderConfig::model_provider("provider-a", Vec::new())],
                inputs: Vec::new(),
                provider: Some(ProviderImplementationRef {
                    path: Some(provider_path.display().to_string()),
                    crate_name: None,
                    version: None,
                    binary: None,
                    script: None,
                }),
            }],
            ProviderRegistryOptions::default(),
        )
        .unwrap()
    }

    fn blocking_provider_identity() -> SessionProviderIdentity {
        SessionProviderIdentity {
            model_name: "model-a".to_string(),
            provider_name: "provider-a".to_string(),
            provider_instance_id: Some("provider-a-instance".to_string()),
            settings_id: "provider-a-settings".to_string(),
        }
    }

    fn wait_for_path(path: &Path) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !path.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(path.exists(), "blocked provider process did not start");
    }

    fn wait_for_process_exit(pid: i32) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while process_exists(pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn process_exists(pid: i32) -> bool {
        let result = unsafe { libc::kill(pid, 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}
