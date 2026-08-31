//! Detached custody for bounded canonical session-turn ingestion.

use oulipoly_provider::client::CancellationToken;
use oulipoly_runtime::provider_registry::ProviderRegistryHandle;
use oulipoly_runtime::session_provider::{
    SessionTurnIngestDriverRequest, SessionTurnIngestQuantumOutcome,
    run_one_session_turn_ingest_quantum,
};
use oulipoly_state::{SessionTurnStreamProjection, StateDb};
use rusqlite::{Connection, OpenFlags, params};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

const IDLE_POLL_INTERVAL: Duration = Duration::from_secs(1);
const ERROR_POLL_INTERVAL: Duration = Duration::from_secs(2);
static DRIVER_STARTED: AtomicBool = AtomicBool::new(false);

pub(crate) struct SessionTurnIngestDriver {
    cancellation: CancellationToken,
    join: Option<JoinHandle<()>>,
}

pub(crate) fn start_session_turn_ingest_driver(
    registry: ProviderRegistryHandle,
) -> Option<SessionTurnIngestDriver> {
    if DRIVER_STARTED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return None;
    }
    let state_path = match StateDb::default_path() {
        Ok(state_path) => state_path,
        Err(error) => {
            DRIVER_STARTED.store(false, Ordering::SeqCst);
            tracing::warn!("Session turn ingest driver could not resolve state DB: {error}");
            return None;
        }
    };
    let cancellation = CancellationToken::new();
    let worker_cancellation = cancellation.clone();
    match std::thread::Builder::new()
        .name("oulipoly-session-turn-ingest".to_string())
        .spawn(move || {
            session_turn_ingest_loop(state_path, registry, worker_cancellation);
            DRIVER_STARTED.store(false, Ordering::SeqCst);
        }) {
        Ok(join) => Some(SessionTurnIngestDriver {
            cancellation,
            join: Some(join),
        }),
        Err(error) => {
            DRIVER_STARTED.store(false, Ordering::SeqCst);
            tracing::warn!("Failed to start session turn ingest driver: {error}");
            None
        }
    }
}

impl Drop for SessionTurnIngestDriver {
    fn drop(&mut self) {
        self.cancellation.cancel();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn session_turn_ingest_loop(
    state_path: PathBuf,
    registry: ProviderRegistryHandle,
    cancellation: CancellationToken,
) {
    let worker_thread = std::thread::current();
    let _cancellation_registration = cancellation.register(move || worker_thread.unpark());
    let lease_owner = format!("session-turn-worker-{}", std::process::id());
    while !cancellation.is_cancelled() {
        let state = match open_current_state(&state_path) {
            Ok(Some(state)) => state,
            Ok(None) => {
                std::thread::park_timeout(IDLE_POLL_INTERVAL);
                continue;
            }
            Err(error) => {
                tracing::warn!("Session turn ingest driver could not open state DB: {error}");
                std::thread::park_timeout(ERROR_POLL_INTERVAL);
                continue;
            }
        };
        let current_registry = registry.current();
        let outcome = run_one_session_turn_ingest_quantum(SessionTurnIngestDriverRequest {
            state: &state,
            registry: current_registry.as_ref(),
            lease_owner: &lease_owner,
            effective_cwd: None,
            cancellation: &cancellation,
            now: chrono::Utc::now(),
        });
        drop(state);
        match outcome {
            Ok(SessionTurnIngestQuantumOutcome::Idle) => {
                std::thread::park_timeout(IDLE_POLL_INTERVAL);
            }
            Ok(SessionTurnIngestQuantumOutcome::Applied { .. })
            | Ok(SessionTurnIngestQuantumOutcome::RetryScheduled { .. })
            | Ok(SessionTurnIngestQuantumOutcome::Unsupported { .. })
            | Ok(SessionTurnIngestQuantumOutcome::Quarantined { .. }) => {}
            Err(error) => {
                tracing::warn!("Session turn ingest quantum failed: {error}");
                std::thread::park_timeout(ERROR_POLL_INTERVAL);
            }
        }
    }
}

fn open_current_state(path: &Path) -> Result<Option<StateDb>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let read_only = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("Failed to open state DB read-only: {error}"))?;
    let version = read_only
        .query_row("PRAGMA user_version", [], |row| row.get::<_, i32>(0))
        .map_err(|error| format!("Failed to read state schema version: {error}"))?;
    if version != oulipoly_state::CURRENT_SCHEMA_VERSION {
        return Ok(None);
    }
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
    let stream_ready = read_only
        .query_row(
            "SELECT EXISTS (
                SELECT 1
                FROM session_turn_ingest_streams
                WHERE projection = ?1
                  AND (lease_owner IS NULL OR lease_expires_at <= ?2)
                  AND (status = 'ready'
                       OR (status = 'retry_wait'
                           AND (next_attempt_at IS NULL OR next_attempt_at <= ?2))
                       OR (status = 'active' AND lease_expires_at <= ?2))
                LIMIT 1
            )",
            params![SessionTurnStreamProjection::CanonicalIngest.as_str(), now],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|error| format!("Failed to inspect ready session turn streams: {error}"))?;
    drop(read_only);
    if !stream_ready {
        return Ok(None);
    }
    StateDb::open(path).map(Some)
}
