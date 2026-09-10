//! Provider-neutral outbound turn observation for the interactive PTY relay.

use crate::provider_registry::ProviderRegistry;
use crate::session_provider::{
    SessionProviderIdentity, SessionProviderPageCursor, SessionProviderReadPageRequest,
    SessionProviderTurnProjection, read_turn_page,
};
use chrono::{DateTime, Utc};
use oulipoly_provider::client::CancellationToken;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, TryLockError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const OBSERVATION_INTERVAL: Duration = Duration::from_millis(250);
const OBSERVATION_TIMEOUT: Duration = Duration::from_secs(30);
const OBSERVATION_MAX_TURNS: u64 = 64;
const OBSERVATION_MAX_RESPONSE_BYTES: u64 = 128 * 1024;
const OBSERVATION_MAX_SOURCE_BYTES: u64 = 512 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OutboundObservationIdentity {
    pub(super) invocation_uuid: String,
    pub(super) model_name: String,
    pub(super) provider_name: String,
    pub(super) provider_instance_id: Option<String>,
    pub(super) settings_id: String,
    pub(super) provider_session_id: String,
    pub(super) effective_cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ObservedUserTurn {
    pub(super) turn_id: String,
    pub(super) timestamp: DateTime<Utc>,
    pub(super) canonical_text_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum OutboundObservationPhase {
    TailAnchor { resume_token: String },
    PostAnchorPage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OutboundObservation {
    pub(super) identity: OutboundObservationIdentity,
    pub(super) generation: u64,
    pub(super) phase: OutboundObservationPhase,
    pub(super) snapshot_complete: bool,
    pub(super) user_turns: Vec<ObservedUserTurn>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub(super) enum OutboundObservationResult {
    Available(Box<OutboundObservation>),
    Unavailable {
        generation: u64,
        detail: String,
    },
    Failed {
        generation: u64,
        detail: String,
    },
    Stopped {
        generation: u64,
        reason: &'static str,
    },
}

impl OutboundObservationResult {
    fn generation(&self) -> u64 {
        match self {
            Self::Available(observation) => observation.generation,
            Self::Unavailable { generation, .. }
            | Self::Failed { generation, .. }
            | Self::Stopped { generation, .. } => *generation,
        }
    }
}

pub(super) struct ProviderSessionTurnSource {
    registry: Arc<ProviderRegistry>,
    identity: SessionProviderIdentity,
    observation_identity: OutboundObservationIdentity,
    expected_delivery_nonce: String,
    cursor: Mutex<ObservationCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ObservationCursor {
    Tail,
    After {
        token: String,
    },
    Continuation {
        snapshot_id: String,
        page_token: String,
        expected_page_index: u64,
        expected_turn_sequence: u64,
    },
}

impl ProviderSessionTurnSource {
    pub(super) fn new(
        registry: Arc<ProviderRegistry>,
        identity: SessionProviderIdentity,
        provider_session_id: String,
        invocation_uuid: String,
        effective_cwd: Option<PathBuf>,
    ) -> Self {
        let expected_delivery_nonce = format!("{:x}", Sha256::digest(invocation_uuid.as_bytes()));
        let observation_identity = OutboundObservationIdentity {
            invocation_uuid,
            model_name: identity.model_name.clone(),
            provider_name: identity.provider_name.clone(),
            provider_instance_id: identity.provider_instance_id.clone(),
            settings_id: identity.settings_id.clone(),
            provider_session_id,
            effective_cwd,
        };
        Self {
            registry,
            identity,
            observation_identity,
            expected_delivery_nonce,
            cursor: Mutex::new(ObservationCursor::Tail),
        }
    }

    fn read(
        &self,
        generation: u64,
        reset_anchor: bool,
        cancellation: &CancellationToken,
    ) -> OutboundObservationResult {
        let mut cursor = lock_or_recover(&self.cursor);
        // A refused reset must not destroy a retained continuation or anchor.
        let requested = if reset_anchor {
            ObservationCursor::Tail
        } else {
            cursor.clone()
        };
        match self.read_page(&requested, cancellation) {
            Ok(page) => {
                *cursor = requested;
                self.map_page(generation, &mut cursor, page)
            }
            Err(error)
                if crate::session_provider::fixed_paging_stop_reason(error.token()).is_some() =>
            {
                OutboundObservationResult::Stopped {
                    generation,
                    reason: crate::session_provider::fixed_paging_stop_reason(error.token())
                        .unwrap(),
                }
            }
            Err(error) if observation_unavailable(&error) => {
                OutboundObservationResult::Unavailable {
                    generation,
                    detail: error.to_string(),
                }
            }
            Err(error) => OutboundObservationResult::Failed {
                generation,
                detail: error.to_string(),
            },
        }
    }

    fn read_page(
        &self,
        cursor: &ObservationCursor,
        cancellation: &CancellationToken,
    ) -> Result<
        crate::session_provider::SessionProviderReadPageResult,
        crate::session_provider::SessionProviderError,
    > {
        let (cursor, expected_page_index, expected_turn_sequence) = match cursor {
            ObservationCursor::Tail => (SessionProviderPageCursor::Tail, 0, 0),
            ObservationCursor::After { token } => (
                SessionProviderPageCursor::Beginning {
                    after_token: Some(token.clone()),
                },
                0,
                0,
            ),
            ObservationCursor::Continuation {
                snapshot_id,
                page_token,
                expected_page_index,
                expected_turn_sequence,
            } => (
                SessionProviderPageCursor::Continuation {
                    snapshot_id: snapshot_id.clone(),
                    page_token: page_token.clone(),
                },
                *expected_page_index,
                *expected_turn_sequence,
            ),
        };
        read_turn_page(SessionProviderReadPageRequest {
            registry: &self.registry,
            identity: self.identity.clone(),
            session_id: &self.observation_identity.provider_session_id,
            effective_cwd: self.observation_identity.effective_cwd.as_deref(),
            projection: SessionProviderTurnProjection::UserObservation,
            expected_delivery_nonce: Some(&self.expected_delivery_nonce),
            cursor,
            expected_page_index,
            expected_turn_sequence,
            max_turns: OBSERVATION_MAX_TURNS,
            max_response_bytes: OBSERVATION_MAX_RESPONSE_BYTES,
            max_source_bytes: OBSERVATION_MAX_SOURCE_BYTES,
            max_inline_body_bytes: 0,
            cancellation,
            timeout: OBSERVATION_TIMEOUT,
        })
    }

    fn map_page(
        &self,
        generation: u64,
        cursor: &mut ObservationCursor,
        page: crate::session_provider::SessionProviderReadPageResult,
    ) -> OutboundObservationResult {
        let phase = if matches!(cursor, ObservationCursor::Tail) {
            let resume_token = page
                .resume_token
                .clone()
                .expect("validated tail anchors have resume tokens");
            OutboundObservationPhase::TailAnchor { resume_token }
        } else {
            OutboundObservationPhase::PostAnchorPage
        };
        let user_turns = page
            .turns
            .iter()
            .filter(|turn| turn.role == "user")
            .map(|turn| ObservedUserTurn {
                turn_id: turn.turn_id.clone(),
                timestamp: turn.timestamp,
                canonical_text_sha256: turn.canonical_text_sha256.clone(),
            })
            .collect();
        *cursor = if page.snapshot_complete {
            ObservationCursor::After {
                token: page
                    .resume_token
                    .clone()
                    .expect("validated completed pages have resume tokens"),
            }
        } else {
            ObservationCursor::Continuation {
                snapshot_id: page.snapshot_id.clone(),
                page_token: page
                    .next_page_token
                    .clone()
                    .expect("validated continuation pages have page tokens"),
                expected_page_index: page.page_index.saturating_add(1),
                expected_turn_sequence: page
                    .page_start_sequence
                    .saturating_add(page.page_turn_count),
            }
        };
        OutboundObservationResult::Available(Box::new(OutboundObservation {
            identity: self.observation_identity.clone(),
            generation,
            phase,
            snapshot_complete: page.snapshot_complete,
            user_turns,
        }))
    }
}

fn observation_unavailable(error: &crate::session_provider::SessionProviderError) -> bool {
    matches!(
        error.token(),
        "session_capability_missing" | "session_turn_pages_capability_missing"
    )
}

pub(super) enum OutboundObserverSource {
    Provider(Box<ProviderSessionTurnSource>),
    Unavailable(String),
}

impl OutboundObserverSource {
    fn read(
        &self,
        generation: u64,
        reset_anchor: bool,
        cancellation: &CancellationToken,
    ) -> OutboundObservationResult {
        match self {
            Self::Provider(source) => source.read(generation, reset_anchor, cancellation),
            Self::Unavailable(detail) => OutboundObservationResult::Unavailable {
                generation,
                detail: detail.clone(),
            },
        }
    }
}

pub(super) struct OutboundObserverWorker {
    shared: Arc<ObserverShared>,
    join: Option<JoinHandle<()>>,
}

impl OutboundObserverWorker {
    pub(super) fn start(source: OutboundObserverSource) -> Result<Self, String> {
        let shared = Arc::new(ObserverShared::new());
        let thread_shared = Arc::clone(&shared);
        let join = thread::Builder::new()
            .name("pty-broker-outbound-observer".to_string())
            .spawn(move || run_observer(source, thread_shared))
            .map_err(|error| format!("Failed to spawn outbound observer worker: {error}"))?;
        Ok(Self {
            shared,
            join: Some(join),
        })
    }

    pub(super) fn latest_result(&self) -> Option<Arc<OutboundObservationResult>> {
        self.shared.latest_result()
    }

    /// Acknowledge only after the queue has incorporated this exact result.
    /// Reading/cloning the latest result does not authorize another page read.
    pub(super) fn acknowledge(&self, result: &OutboundObservationResult) {
        self.shared.acknowledge(result.generation());
    }

    pub(super) fn set_demand(&self, active: bool) -> Option<u64> {
        self.shared.set_demand(active)
    }

    pub(super) fn request_fresh_generation(&self) -> u64 {
        self.shared.request_fresh_generation()
    }

    /// Recovery entry. The confirmed TUI caller must own this invocation's observer
    /// and have explicit authority after resolving the fixed cause. This is not
    /// message retry/discard authority and does not reset cursors or confirmations.
    pub(super) fn rearm_after_resolution(
        &self,
        expected_generation: u64,
        reason: &'static str,
        resolution: ObservationResolution,
    ) -> Result<u64, &'static str> {
        self.shared
            .rearm_after_resolution(expected_generation, reason, resolution)
    }

    pub(super) fn observe_after_anchor(&self) {
        self.shared.release_anchor();
    }

    pub(super) fn shutdown_and_join(mut self) -> Result<(), String> {
        self.shared.request_shutdown();
        if let Some(join) = self.join.take() {
            join.join()
                .map_err(|_| "Outbound observer worker panicked".to_string())?;
        }
        Ok(())
    }
}

impl Drop for OutboundObserverWorker {
    fn drop(&mut self) {
        self.shared.request_shutdown();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

struct ObserverShared {
    latest: Mutex<Option<Arc<OutboundObservationResult>>>,
    state: Mutex<ObserverState>,
    wake: Condvar,
    shutdown: AtomicBool,
    cancellation: CancellationToken,
}

impl ObserverShared {
    fn new() -> Self {
        Self {
            latest: Mutex::new(None),
            state: Mutex::new(ObserverState::default()),
            wake: Condvar::new(),
            shutdown: AtomicBool::new(false),
            cancellation: CancellationToken::new(),
        }
    }

    fn latest_result(&self) -> Option<Arc<OutboundObservationResult>> {
        match self.latest.try_lock() {
            Ok(guard) => guard.clone(),
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner().clone(),
            Err(TryLockError::WouldBlock) => None,
        }
    }

    fn acknowledge(&self, generation: u64) {
        let mut state = lock_or_recover(&self.state);
        if state.awaiting_incorporation != Some(generation) {
            return;
        }
        // The reservation begins before reading. An old result cannot release
        // an in-flight or subsequently published generation.
        state.awaiting_incorporation = None;
        self.wake.notify_one();
    }

    fn publish(&self, result: OutboundObservationResult) {
        let mut state = lock_or_recover(&self.state);
        if let OutboundObservationResult::Stopped { generation, reason } = &result {
            // Even if demand refreshed during this read, a fixed refusal remains
            // fixed for this identity. A refresh is not recovery authority.
            state.stopped = Some((*generation, *reason));
            state.refresh_requested = false;
            state.anchor_reset_requested = false;
        } else if matches!(&result, OutboundObservationResult::Available(observation)
            if matches!(observation.phase, OutboundObservationPhase::TailAnchor { .. }))
            && !state.anchor_reset_requested
        {
            state.anchor_held = true;
        }
        *lock_or_recover(&self.latest) = Some(Arc::new(result));
    }

    fn rearm_after_resolution(
        &self,
        expected_generation: u64,
        reason: &'static str,
        resolution: ObservationResolution,
    ) -> Result<u64, &'static str> {
        let mut state = lock_or_recover(&self.state);
        if state.stopped != Some((expected_generation, reason)) {
            return Err("observation_stop_identity_changed");
        }
        if !resolution.permits(reason) {
            return Err("observation_resolution_mismatch");
        }
        let floor = state
            .started_generation
            .checked_add(1)
            .ok_or("observation_generation_exhausted")?;
        state.stopped = None;
        state.awaiting_incorporation = None;
        state.refresh_requested = true;
        // Retain cursor and anchor; confirmation resumes, never resends input.
        state.anchor_reset_requested = false;
        *lock_or_recover(&self.latest) = None;
        self.wake.notify_one();
        Ok(floor)
    }

    fn set_demand(&self, active: bool) -> Option<u64> {
        let mut state = lock_or_recover(&self.state);
        if state.demand == active {
            return None;
        }
        state.demand = active;
        if active && state.stopped.is_none() {
            let floor = state.started_generation.saturating_add(1);
            state.refresh_requested = true;
            state.anchor_reset_requested = true;
            state.anchor_held = false;
            self.wake.notify_one();
            Some(floor)
        } else {
            None
        }
    }

    fn request_fresh_generation(&self) -> u64 {
        let mut state = lock_or_recover(&self.state);
        let floor = state.started_generation.saturating_add(1);
        state.demand = true;
        if state.stopped.is_some() {
            return floor;
        }
        state.refresh_requested = true;
        state.anchor_reset_requested = true;
        state.anchor_held = false;
        self.wake.notify_one();
        floor
    }

    fn release_anchor(&self) {
        let mut state = lock_or_recover(&self.state);
        if state.stopped.is_some() || !state.anchor_held {
            return;
        }
        state.anchor_held = false;
        state.refresh_requested = true;
        self.wake.notify_one();
    }

    fn request_shutdown(&self) {
        let _state = lock_or_recover(&self.state);
        self.shutdown.store(true, Ordering::SeqCst);
        self.cancellation.cancel();
        self.wake.notify_all();
    }

    fn shutdown_requested(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }
}

#[derive(Default)]
struct ObserverState {
    demand: bool,
    refresh_requested: bool,
    started_generation: u64,
    anchor_reset_requested: bool,
    anchor_held: bool,
    stopped: Option<(u64, &'static str)>,
    // One bounded result (or its in-flight read) until consumer acknowledgement.
    awaiting_incorporation: Option<u64>,
}

/// The recovery owner attests the relevant cause is resolved before calling rearm.
/// The TUI requires a distinct resolution/authority confirmation; routine demand
/// and message retry/discard never supply it.
#[derive(Clone, Copy)]
pub(super) enum ObservationResolution {
    PagingRestored,
    CapacityResolved,
}
impl ObservationResolution {
    fn permits(self, reason: &str) -> bool {
        match self {
            Self::PagingRestored => reason == "session_turn_paging_paused",
            Self::CapacityResolved => {
                crate::session_provider::fixed_paging_stop_reason(reason).is_some()
                    && reason != "session_turn_paging_paused"
            }
        }
    }
}

impl ObserverState {
    fn take_read(&mut self, now: Instant, deadline: Instant) -> Option<(u64, bool)> {
        if !self.demand
            || self.stopped.is_some()
            || self.anchor_held
            || self.awaiting_incorporation.is_some()
            || (!self.refresh_requested && now < deadline)
        {
            return None;
        }
        self.started_generation = self.started_generation.checked_add(1)?;
        self.awaiting_incorporation = Some(self.started_generation);
        self.refresh_requested = false;
        let reset_anchor = self.anchor_reset_requested;
        self.anchor_reset_requested = false;
        Some((self.started_generation, reset_anchor))
    }
}

fn run_observer(source: OutboundObserverSource, shared: Arc<ObserverShared>) {
    let mut next_scan = Instant::now();
    while let Some((generation, reset_anchor)) = wait_for_read(&shared, next_scan) {
        if shared.shutdown_requested() {
            return;
        }
        execute_read(&source, &shared, generation, reset_anchor);
        next_scan = Instant::now() + OBSERVATION_INTERVAL;
    }
}

fn wait_for_read(shared: &ObserverShared, deadline: Instant) -> Option<(u64, bool)> {
    let mut state = lock_or_recover(&shared.state);
    loop {
        if shared.shutdown_requested() {
            return None;
        }
        if let Some(read) = state.take_read(Instant::now(), deadline) {
            return Some(read);
        }
        let wait = if state.demand
            && state.stopped.is_none()
            && !state.anchor_held
            && state.awaiting_incorporation.is_none()
        {
            deadline.saturating_duration_since(Instant::now())
        } else {
            Duration::from_secs(60)
        };
        state = match shared.wake.wait_timeout(state, wait) {
            Ok((guard, _)) => guard,
            Err(poisoned) => poisoned.into_inner().0,
        };
    }
}

fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicitly_unavailable_worker_publishes_without_a_provider_call() {
        let worker = OutboundObserverWorker::start(OutboundObserverSource::Unavailable(
            "session_turn_source_unavailable".to_string(),
        ))
        .expect("worker");
        let _ = worker.set_demand(true);
        let result = wait_for_generation(&worker, 1);
        assert!(matches!(
            result.as_ref(),
            OutboundObservationResult::Unavailable { detail, .. }
                if detail == "session_turn_source_unavailable"
        ));
        worker.shutdown_and_join().expect("shutdown");
    }

    #[test]
    fn reactivating_demand_requires_a_generation_started_after_idle() {
        let worker = OutboundObserverWorker::start(OutboundObserverSource::Unavailable(
            "session_turn_source_unavailable".to_string(),
        ))
        .expect("worker");
        assert_eq!(worker.set_demand(true), Some(1));
        let first = wait_for_generation(&worker, 1);
        assert_eq!(result_generation(&first), 1);
        worker.acknowledge(&first);
        assert_eq!(worker.set_demand(false), None);
        assert_eq!(worker.set_demand(true), Some(2));
        let second = wait_for_generation(&worker, 2);
        assert!(result_generation(&second) >= 2);
        worker.shutdown_and_join().expect("shutdown");
    }

    #[test]
    fn acknowledgement_releases_continuous_demand_but_old_results_do_not() {
        let worker = OutboundObserverWorker::start(OutboundObserverSource::Unavailable(
            "synthetic unavailable source".to_string(),
        ))
        .expect("worker");
        worker.set_demand(true);
        let first = wait_for_generation(&worker, 1);
        let later = Instant::now() + Duration::from_secs(1);
        assert!(
            lock_or_recover(&worker.shared.state)
                .take_read(later, later)
                .is_none()
        );
        worker.acknowledge(&first);
        let second = wait_for_generation(&worker, 2);
        assert_eq!(result_generation(&second), 2);
        worker.acknowledge(&first);
        assert!(
            lock_or_recover(&worker.shared.state)
                .take_read(later, later)
                .is_none()
        );
        worker
            .shutdown_and_join()
            .expect("shutdown with outstanding result");
    }

    #[test]
    fn idle_worker_shutdown_joins_without_waiting_for_idle_timeout() {
        let worker = OutboundObserverWorker::start(OutboundObserverSource::Unavailable(
            "session_turn_source_unavailable".to_string(),
        ))
        .expect("worker");
        let started = Instant::now();
        worker.shutdown_and_join().expect("shutdown");
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    fn result_generation(result: &OutboundObservationResult) -> u64 {
        match result {
            OutboundObservationResult::Available(observation) => observation.generation,
            OutboundObservationResult::Unavailable { generation, .. }
            | OutboundObservationResult::Failed { generation, .. }
            | OutboundObservationResult::Stopped { generation, .. } => *generation,
        }
    }

    fn wait_for_generation(
        worker: &OutboundObserverWorker,
        minimum: u64,
    ) -> Arc<OutboundObservationResult> {
        let deadline = Instant::now() + Duration::from_secs(4);
        while Instant::now() < deadline {
            if let Some(result) = worker.latest_result()
                && result_generation(&result) >= minimum
            {
                return result;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("observer did not publish generation {minimum}");
    }
}

fn execute_read(
    source: &OutboundObserverSource,
    shared: &ObserverShared,
    generation: u64,
    reset_anchor: bool,
) {
    let result = source.read(generation, reset_anchor, &shared.cancellation);
    shared.publish(result);
}

#[cfg(all(test, unix))]
#[path = "outbound_observer_fixtures.rs"]
pub(super) mod fixtures;
