//! Provider-neutral outbound turn observation for the interactive PTY relay.

use crate::provider_registry::ProviderRegistry;
use crate::session_provider::{
    self, SessionProviderIdentity, SessionProviderReadTurnsRequest, SessionProviderReadTurnsResult,
    SessionProviderTurn,
};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, TryLockError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const OBSERVATION_INTERVAL: Duration = Duration::from_millis(250);

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
    pub(super) body: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OutboundObservation {
    pub(super) identity: OutboundObservationIdentity,
    pub(super) generation: u64,
    pub(super) complete: bool,
    pub(super) turn_count: u64,
    pub(super) turn_ids: BTreeSet<String>,
    pub(super) user_turns: Vec<ObservedUserTurn>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum OutboundObservationResult {
    Available(Box<OutboundObservation>),
    Unavailable { generation: u64, detail: String },
    Failed { generation: u64, detail: String },
}

pub(super) struct ProviderSessionTurnSource {
    registry: Arc<ProviderRegistry>,
    identity: SessionProviderIdentity,
    provider_session_id: String,
    invocation_uuid: String,
    effective_cwd: Option<PathBuf>,
}

impl ProviderSessionTurnSource {
    pub(super) fn new(
        registry: Arc<ProviderRegistry>,
        identity: SessionProviderIdentity,
        provider_session_id: String,
        invocation_uuid: String,
        effective_cwd: Option<PathBuf>,
    ) -> Self {
        Self {
            registry,
            identity,
            provider_session_id,
            invocation_uuid,
            effective_cwd,
        }
    }

    fn read(&self, generation: u64) -> OutboundObservationResult {
        let result = session_provider::read_turns(SessionProviderReadTurnsRequest {
            registry: self.registry.as_ref(),
            identity: self.identity.clone(),
            session_id: &self.provider_session_id,
            effective_cwd: self.effective_cwd.as_deref(),
        });
        match result {
            Ok(result) => self.map_result(generation, result),
            Err(error) => map_source_error(generation, error),
        }
    }

    fn map_result(
        &self,
        generation: u64,
        result: SessionProviderReadTurnsResult,
    ) -> OutboundObservationResult {
        if result
            .turns
            .iter()
            .any(|turn| turn.session_id != self.provider_session_id)
        {
            return OutboundObservationResult::Unavailable {
                generation,
                detail: "observation_identity_mismatch".to_string(),
            };
        }
        let raw_turn_count = result.turns.len() as u64;
        let turn_ids = result
            .turns
            .iter()
            .map(|turn| turn.turn_id.clone())
            .collect();
        let user_turns = result
            .turns
            .iter()
            .filter(|turn| turn.role == "user")
            .map(observed_user_turn)
            .collect();
        OutboundObservationResult::Available(Box::new(OutboundObservation {
            identity: self.observation_identity(),
            generation,
            complete: result.complete && raw_turn_count == result.turn_count,
            turn_count: result.turn_count,
            turn_ids,
            user_turns,
        }))
    }

    fn observation_identity(&self) -> OutboundObservationIdentity {
        OutboundObservationIdentity {
            invocation_uuid: self.invocation_uuid.clone(),
            model_name: self.identity.model_name.clone(),
            provider_name: self.identity.provider_name.clone(),
            provider_instance_id: self.identity.provider_instance_id.clone(),
            settings_id: self.identity.settings_id.clone(),
            provider_session_id: self.provider_session_id.clone(),
            effective_cwd: self.effective_cwd.clone(),
        }
    }
}

fn observed_user_turn(turn: &SessionProviderTurn) -> ObservedUserTurn {
    ObservedUserTurn {
        turn_id: turn.turn_id.clone(),
        body: turn.body.as_ref().and_then(canonical_text_body),
    }
}

fn canonical_text_body(body: &serde_json::Value) -> Option<String> {
    let chunks = body.as_array()?;
    if chunks.is_empty() {
        return None;
    }
    let mut text = String::new();
    for chunk in chunks {
        let chunk = chunk.as_object()?;
        if chunk.get("type")?.as_str()? != "text" {
            return None;
        }
        text.push_str(chunk.get("text")?.as_str()?);
    }
    Some(text)
}

fn map_source_error(
    generation: u64,
    error: session_provider::SessionProviderError,
) -> OutboundObservationResult {
    let detail = error.token().to_string();
    if matches!(
        detail.as_str(),
        "session_capability_missing" | "session_provider_describe_unavailable"
    ) {
        OutboundObservationResult::Unavailable { generation, detail }
    } else {
        OutboundObservationResult::Failed { generation, detail }
    }
}

pub(super) enum OutboundObserverSource {
    Provider(ProviderSessionTurnSource),
    Unavailable(String),
}

impl OutboundObserverSource {
    fn read(&self, generation: u64) -> OutboundObservationResult {
        match self {
            Self::Provider(source) => source.read(generation),
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

    pub(super) fn set_demand(&self, active: bool) -> Option<u64> {
        self.shared.set_demand(active)
    }

    pub(super) fn request_fresh_generation(&self) -> u64 {
        self.shared.request_fresh_generation()
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
}

impl ObserverShared {
    fn new() -> Self {
        Self {
            latest: Mutex::new(None),
            state: Mutex::new(ObserverState::default()),
            wake: Condvar::new(),
            shutdown: AtomicBool::new(false),
        }
    }

    fn latest_result(&self) -> Option<Arc<OutboundObservationResult>> {
        match self.latest.try_lock() {
            Ok(guard) => guard.clone(),
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner().clone(),
            Err(TryLockError::WouldBlock) => None,
        }
    }

    fn publish(&self, result: OutboundObservationResult) {
        *lock_or_recover(&self.latest) = Some(Arc::new(result));
    }

    fn set_demand(&self, active: bool) -> Option<u64> {
        let mut state = lock_or_recover(&self.state);
        if state.demand == active {
            return None;
        }
        state.demand = active;
        if active {
            let floor = state.started_generation.saturating_add(1);
            state.refresh_requested = true;
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
        state.refresh_requested = true;
        self.wake.notify_one();
        floor
    }

    fn request_shutdown(&self) {
        let _state = lock_or_recover(&self.state);
        self.shutdown.store(true, Ordering::SeqCst);
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
}

fn run_observer(source: OutboundObserverSource, shared: Arc<ObserverShared>) {
    let mut next_scan = Instant::now();
    while let Some(generation) = wait_for_read(&shared, next_scan) {
        if shared.shutdown_requested() {
            return;
        }
        shared.publish(source.read(generation));
        next_scan = Instant::now() + OBSERVATION_INTERVAL;
    }
}

fn wait_for_read(shared: &ObserverShared, deadline: Instant) -> Option<u64> {
    let mut state = lock_or_recover(&shared.state);
    loop {
        if shared.shutdown_requested() {
            return None;
        }
        if state.demand && (state.refresh_requested || Instant::now() >= deadline) {
            state.refresh_requested = false;
            state.started_generation = state.started_generation.saturating_add(1);
            return Some(state.started_generation);
        }
        let wait = if state.demand {
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
    use crate::provider_registry::ProviderRegistryOptions;
    use oulipoly_config::provider_implementation_ref::ProviderImplementationRef;
    use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig};
    use oulipoly_provider::client::ProviderClientOptions;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    const MODEL: &str = "observer-model";
    const ACCOUNT: &str = "observer-account";
    const SESSION: &str = "observer-session";

    struct Fixture {
        _dir: tempfile::TempDir,
        mode: PathBuf,
        record: PathBuf,
        script: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let mode = dir.path().join("mode");
            let record = dir.path().join("record.jsonl");
            let script = dir.path().join("observer-provider.py");
            fs::write(&mode, "success").expect("mode");
            fs::write(&script, fixture_script(&mode, &record)).expect("script");
            let mut permissions = fs::metadata(&script).expect("metadata").permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&script, permissions).expect("chmod");
            Self {
                _dir: dir,
                mode,
                record,
                script,
            }
        }

        fn set_mode(&self, mode: &str) {
            fs::write(&self.mode, mode).expect("set mode");
        }

        fn source(&self) -> ProviderSessionTurnSource {
            self.source_with_options(ProviderRegistryOptions::default())
        }

        fn source_with_options(
            &self,
            options: ProviderRegistryOptions,
        ) -> ProviderSessionTurnSource {
            let model = fixture_model(&self.script);
            let registry =
                ProviderRegistry::from_model_configs(&[model], options).expect("provider registry");
            ProviderSessionTurnSource::new(
                Arc::new(registry),
                fixture_identity(),
                SESSION.to_string(),
                "11111111-1111-4111-8111-111111111111".to_string(),
                Some(PathBuf::from("/fixture/cwd")),
            )
        }
    }

    #[test]
    fn complete_provider_result_preserves_identity_ids_and_canonical_user_text() {
        let fixture = Fixture::new();
        let result = fixture.source().read(7);
        let OutboundObservationResult::Available(observation) = result else {
            panic!("expected available observation: {result:?}");
        };

        assert_eq!(observation.identity.model_name, MODEL);
        assert_eq!(observation.identity.provider_name, ACCOUNT);
        assert_eq!(observation.identity.provider_session_id, SESSION);
        assert_eq!(
            observation.identity.effective_cwd,
            Some(PathBuf::from("/fixture/cwd"))
        );
        assert_eq!(observation.generation, 7);
        assert!(observation.complete);
        assert_eq!(observation.turn_count, 5);
        assert_eq!(
            observation.turn_ids,
            ["assistant", "system", "tool", "user-old", "user-text"]
                .into_iter()
                .map(str::to_string)
                .collect()
        );
        assert_eq!(
            observation.user_turns,
            vec![
                ObservedUserTurn {
                    turn_id: "user-old".to_string(),
                    body: Some("same body".to_string()),
                },
                ObservedUserTurn {
                    turn_id: "user-text".to_string(),
                    body: Some("helloworld".to_string()),
                },
            ]
        );
        let request = fs::read_to_string(&fixture.record).expect("record");
        assert!(request.contains("session.read_turns"));
        assert!(request.contains(SESSION));
        assert!(request.contains("/fixture/cwd"));
    }

    #[test]
    fn wrong_session_and_incomplete_windows_are_ineligible() {
        let fixture = Fixture::new();
        fixture.set_mode("wrong-session");
        assert!(matches!(
            fixture.source().read(1),
            OutboundObservationResult::Unavailable { ref detail, .. }
                if detail == "observation_identity_mismatch"
        ));

        for mode in ["incomplete", "partial", "count-mismatch"] {
            fixture.set_mode(mode);
            let OutboundObservationResult::Available(observation) = fixture.source().read(2) else {
                panic!("{mode} should remain available for diagnostics");
            };
            assert!(
                !observation.complete,
                "{mode} must not establish a baseline"
            );
        }
    }

    #[test]
    fn non_user_roles_and_non_text_user_content_cannot_match() {
        let fixture = Fixture::new();
        fixture.set_mode("non-text");
        let OutboundObservationResult::Available(observation) = fixture.source().read(1) else {
            panic!("expected available observation");
        };
        assert_eq!(observation.user_turns.len(), 2);
        assert_eq!(observation.user_turns[0].body, None);
        assert_eq!(observation.user_turns[1].body, None);
        assert!(
            observation
                .user_turns
                .iter()
                .all(|turn| !matches!(turn.turn_id.as_str(), "assistant" | "tool" | "system"))
        );
    }

    #[test]
    fn malformed_transport_and_timeout_publish_failed_results() {
        let fixture = Fixture::new();
        fixture.set_mode("malformed");
        assert!(matches!(
            fixture.source().read(1),
            OutboundObservationResult::Failed { .. }
        ));

        fixture.set_mode("failure");
        assert!(matches!(
            fixture.source().read(2),
            OutboundObservationResult::Failed { .. }
        ));

        fixture.set_mode("slow");
        let options = ProviderRegistryOptions::default().with_client_options(
            ProviderClientOptions::default().with_timeout(Duration::from_millis(30)),
        );
        let started = Instant::now();
        assert!(matches!(
            fixture.source_with_options(options).read(3),
            OutboundObservationResult::Failed { .. }
        ));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn worker_read_is_nonblocking_and_fresh_barrier_excludes_in_flight_read() {
        let fixture = Fixture::new();
        fixture.set_mode("slow");
        let worker =
            OutboundObserverWorker::start(OutboundObserverSource::Provider(fixture.source()))
                .expect("worker");
        let _ = worker.set_demand(true);
        wait_for_record(&fixture.record);

        let before_latest = Instant::now();
        assert!(worker.latest_result().is_none());
        assert!(before_latest.elapsed() < Duration::from_millis(50));
        let floor = worker.request_fresh_generation();
        assert_eq!(floor, 2);

        let first = wait_for_generation(&worker, 1);
        assert_eq!(result_generation(&first), 1);
        let second = wait_for_generation(&worker, floor);
        assert!(result_generation(&second) >= floor);
        worker.shutdown_and_join().expect("shutdown");
    }

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
        assert_eq!(worker.set_demand(false), None);
        assert_eq!(worker.set_demand(true), Some(2));
        let second = wait_for_generation(&worker, 2);
        assert!(result_generation(&second) >= 2);
        worker.shutdown_and_join().expect("shutdown");
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

    fn fixture_model(script: &Path) -> ModelConfig {
        ModelConfig {
            name: MODEL.to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![ProviderConfig::model_provider(ACCOUNT, Vec::new())],
            inputs: Vec::new(),
            provider: Some(ProviderImplementationRef {
                path: Some(script.display().to_string()),
                crate_name: None,
                version: None,
                binary: None,
                script: None,
            }),
        }
    }

    fn fixture_identity() -> SessionProviderIdentity {
        SessionProviderIdentity {
            model_name: MODEL.to_string(),
            provider_name: ACCOUNT.to_string(),
            provider_instance_id: Some("observer-instance".to_string()),
            settings_id: "observer-settings".to_string(),
        }
    }

    fn result_generation(result: &OutboundObservationResult) -> u64 {
        match result {
            OutboundObservationResult::Available(observation) => observation.generation,
            OutboundObservationResult::Unavailable { generation, .. }
            | OutboundObservationResult::Failed { generation, .. } => *generation,
        }
    }

    fn wait_for_record(path: &Path) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if path.exists() {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("provider read did not start");
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

    fn fixture_script(mode: &Path, record: &Path) -> String {
        format!(
            r#"#!/usr/bin/env python3
import json
import pathlib
import sys
import time

CONTRACT = "oulipoly.provider/v1"
mode = pathlib.Path({mode:?}).read_text().strip()
request = json.loads(sys.stdin.read() or "{{}}")
subcommand = sys.argv[1] if len(sys.argv) > 1 else ""
with pathlib.Path({record:?}).open("a", encoding="utf-8") as handle:
    handle.write(json.dumps({{"subcommand": subcommand, "request": request}}, sort_keys=True) + "\n")

def turn(turn_id, role, body, session={session:?}):
    return {{
        "session_id": session,
        "turn_id": turn_id,
        "timestamp": "2026-05-01T00:00:01Z",
        "role": role,
        "body": body,
    }}

turns = [
    turn("user-old", "user", [{{"type": "text", "text": "same body"}}]),
    turn("assistant", "assistant", [{{"type": "text", "text": "same body"}}]),
    turn("tool", "tool", [{{"type": "text", "text": "same body"}}]),
    turn("system", "system", [{{"type": "text", "text": "same body"}}]),
    turn("user-text", "user", [{{"type": "text", "text": "hello"}}, {{"type": "text", "text": "world"}}]),
]

if mode == "slow":
    time.sleep(0.3)
if mode == "malformed":
    print("{{")
    raise SystemExit(0)
if mode == "failure":
    raise SystemExit(3)
if mode == "wrong-session":
    turns = [turn("wrong", "user", [{{"type": "text", "text": "hello"}}], "other-session")]
if mode == "non-text":
    turns = [
        turn("user-missing", "user", [{{"type": "text"}}]),
        turn("user-image", "user", [{{"type": "image", "uri": "file:///tmp/image"}}]),
        turn("assistant", "assistant", [{{"type": "text", "text": "hello"}}]),
        turn("tool", "tool", [{{"type": "text", "text": "hello"}}]),
        turn("system", "system", [{{"type": "text", "text": "hello"}}]),
    ]

turn_count = len(turns)
complete = mode != "incomplete"
if mode == "partial":
    turn_count += 1
if mode == "count-mismatch":
    turn_count = 99
response = {{
    "contract": request.get("contract", CONTRACT),
    "request_id": request.get("request_id", "observer-request"),
    "ok": True,
    "result": {{"turns": turns, "turn_count": turn_count, "complete": complete}},
}}
print(json.dumps(response))
"#,
            mode = mode.display().to_string(),
            record = record.display().to_string(),
            session = SESSION,
        )
    }
}
