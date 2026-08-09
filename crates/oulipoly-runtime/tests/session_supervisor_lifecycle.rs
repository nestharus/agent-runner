use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Barrier, Mutex};

use oulipoly_runtime::session_supervisor::{
    ChildFailure, ProcessObservation, ProcessObserver, SessionNotification, SessionSupervisor,
    SupervisorConfig, SupervisorError, SupervisorStartError, TerminalReason, TurnOutcome,
    TurnRequest, TurnResult,
};
use oulipoly_state::{
    AcknowledgementWrite, DeliveryAcknowledgement, DispositionWrite, EventDisposition,
    ExactProcessIdentity, ExternalIngress, LeaseAcquire, LeaseReplace, LifecycleEvent,
    NewLifecycleEvent, ProviderTurnGeneration, SessionLifecycleRepository, SessionLifecycleResult,
    SessionReconstruction, StateDb, SupervisorFence, SupervisorLease, TurnFence, TurnState,
};
use proc_macro2::{TokenStream, TokenTree};

type FakeTurn = TurnRequest<&'static str, &'static str>;

#[derive(Clone, Default)]
struct FakeProcesses {
    observations: Arc<Mutex<HashMap<i64, ProcessObservation>>>,
}

impl FakeProcesses {
    fn set(&self, pid: i64, observation: ProcessObservation) {
        self.observations.lock().unwrap().insert(pid, observation);
    }
}

impl ProcessObserver for FakeProcesses {
    fn observe(&self, expected: &ExactProcessIdentity) -> ProcessObservation {
        self.observations
            .lock()
            .unwrap()
            .get(&expected.pid)
            .cloned()
            .unwrap_or(ProcessObservation::ExactLive)
    }
}

#[derive(Clone, Default)]
struct RepositoryProbe {
    reconstructed_sessions: Arc<Mutex<Vec<String>>>,
    fail_next_append: Arc<AtomicBool>,
    fail_next_start: Arc<AtomicBool>,
    fail_next_transition: Arc<AtomicBool>,
    replacement_barrier: Option<Arc<Barrier>>,
}

struct FakeRepository {
    inner: StateDb,
    probe: RepositoryProbe,
}

impl FakeRepository {
    fn open(path: &Path, probe: RepositoryProbe) -> Self {
        Self {
            inner: StateDb::open(path).unwrap(),
            probe,
        }
    }
}

impl SessionLifecycleRepository for FakeRepository {
    fn acquire_supervisor_lease(
        &mut self,
        session_id: &str,
        fence: &SupervisorFence,
        acquired_at: i64,
    ) -> SessionLifecycleResult<LeaseAcquire> {
        self.inner
            .acquire_supervisor_lease(session_id, fence, acquired_at)
    }

    fn replace_supervisor_lease(
        &mut self,
        session_id: &str,
        expected: &SupervisorFence,
        replacement: &SupervisorFence,
        acquired_at: i64,
    ) -> SessionLifecycleResult<LeaseReplace> {
        if let Some(barrier) = &self.probe.replacement_barrier {
            barrier.wait();
        }
        self.inner
            .replace_supervisor_lease(session_id, expected, replacement, acquired_at)
    }

    fn release_supervisor_lease(
        &mut self,
        session_id: &str,
        fence: &SupervisorFence,
    ) -> SessionLifecycleResult<()> {
        self.inner.release_supervisor_lease(session_id, fence)
    }

    fn supervisor_lease(
        &self,
        session_id: &str,
    ) -> SessionLifecycleResult<Option<SupervisorLease>> {
        self.inner.supervisor_lease(session_id)
    }

    fn start_provider_turn(&mut self, turn: &ProviderTurnGeneration) -> SessionLifecycleResult<()> {
        if self.probe.fail_next_start.swap(false, Ordering::SeqCst) {
            return Err(oulipoly_state::SessionLifecycleError::Conflict(
                "forced fake start failure",
            ));
        }
        self.inner.start_provider_turn(turn)
    }

    fn attach_provider_turn_session(
        &mut self,
        generation_id: &str,
        spawn_invocation_id: &str,
        session_id: &str,
    ) -> SessionLifecycleResult<()> {
        self.inner
            .attach_provider_turn_session(generation_id, spawn_invocation_id, session_id)
    }

    fn provider_turn(
        &self,
        generation_id: &str,
    ) -> SessionLifecycleResult<Option<ProviderTurnGeneration>> {
        self.inner.provider_turn(generation_id)
    }

    fn append_lifecycle_event(
        &mut self,
        session_id: &str,
        event: &NewLifecycleEvent,
    ) -> SessionLifecycleResult<LifecycleEvent> {
        if self.probe.fail_next_append.swap(false, Ordering::SeqCst) {
            return Err(oulipoly_state::SessionLifecycleError::Conflict(
                "forced fake commit failure",
            ));
        }
        self.inner.append_lifecycle_event(session_id, event)
    }

    fn transition_turn_and_append_event(
        &mut self,
        fence: &TurnFence,
        from: TurnState,
        to: TurnState,
        event: &NewLifecycleEvent,
    ) -> SessionLifecycleResult<LifecycleEvent> {
        if self
            .probe
            .fail_next_transition
            .swap(false, Ordering::SeqCst)
        {
            return Err(oulipoly_state::SessionLifecycleError::Conflict(
                "forced fake transition failure",
            ));
        }
        self.inner
            .transition_turn_and_append_event(fence, from, to, event)
    }

    fn record_event_disposition(
        &mut self,
        event_id: &str,
        consumer_id: &str,
        disposition: EventDisposition,
        disposed_at: i64,
    ) -> SessionLifecycleResult<DispositionWrite> {
        self.inner
            .record_event_disposition(event_id, consumer_id, disposition, disposed_at)
    }

    fn append_external_ingress(&mut self, ingress: &ExternalIngress) -> SessionLifecycleResult<()> {
        self.inner.append_external_ingress(ingress)
    }

    fn read_external_ingress(
        &mut self,
        session_id: &str,
        limit: usize,
    ) -> SessionLifecycleResult<Vec<ExternalIngress>> {
        self.inner.read_external_ingress(session_id, limit)
    }

    fn accept_pending(
        &mut self,
        delivery_id: &str,
        session_id: &str,
        turn_generation_id: &str,
        accepted_at: i64,
    ) -> SessionLifecycleResult<AcknowledgementWrite> {
        self.inner
            .accept_pending(delivery_id, session_id, turn_generation_id, accepted_at)
    }

    fn mark_submitted(
        &mut self,
        delivery_id: &str,
        session_id: &str,
        turn_generation_id: &str,
        evidence: &str,
        submitted_at: i64,
    ) -> SessionLifecycleResult<AcknowledgementWrite> {
        self.inner.mark_submitted(
            delivery_id,
            session_id,
            turn_generation_id,
            evidence,
            submitted_at,
        )
    }

    fn mark_confirmed(
        &mut self,
        delivery_id: &str,
        session_id: &str,
        turn_generation_id: &str,
        evidence: &str,
        confirmed_at: i64,
    ) -> SessionLifecycleResult<AcknowledgementWrite> {
        self.inner.mark_confirmed(
            delivery_id,
            session_id,
            turn_generation_id,
            evidence,
            confirmed_at,
        )
    }

    fn acknowledgement(
        &self,
        delivery_id: &str,
    ) -> SessionLifecycleResult<Option<DeliveryAcknowledgement>> {
        self.inner.acknowledgement(delivery_id)
    }

    fn reconstruct_session(
        &self,
        session_id: &str,
        consumer_id: &str,
        limit: usize,
    ) -> SessionLifecycleResult<SessionReconstruction> {
        self.probe
            .reconstructed_sessions
            .lock()
            .unwrap()
            .push(session_id.to_owned());
        self.inner
            .reconstruct_session(session_id, consumer_id, limit)
    }
}

fn process(pid: i64, suffix: &str) -> ExactProcessIdentity {
    ExactProcessIdentity {
        pid,
        boot_id: format!("boot-{suffix}"),
        start_time_ticks: pid * 10,
    }
}

fn owner(generation: i64, suffix: &str) -> SupervisorFence {
    SupervisorFence {
        generation,
        token: format!("token-{suffix}"),
        process: process(100 + generation, suffix),
    }
}

fn turn(session: &str, number: i64) -> ProviderTurnGeneration {
    ProviderTurnGeneration {
        generation_id: format!("generation-{session}-{number}"),
        spawn_invocation_id: format!("invocation-{session}-{number}"),
        session_id: Some(session.to_owned()),
        state: TurnState::Running,
        child: process(200 + number, &format!("child-{session}-{number}")),
    }
}

fn notification(
    session: &str,
    sequence: i64,
    turns: impl IntoIterator<Item = i64>,
) -> SessionNotification<&'static str> {
    SessionNotification::with_retry_turns(
        sequence,
        "work",
        turns.into_iter().map(|number| turn(session, number)),
    )
}

fn event(id: &str, event_type: &str) -> NewLifecycleEvent {
    NewLifecycleEvent {
        event_id: id.to_owned(),
        event_type: event_type.to_owned(),
        cause_event_id: None,
        correlation_id: format!("correlation-{id}"),
        payload: format!("payload-{id}"),
        created_at: 10,
    }
}

struct Started {
    _dir: tempfile::TempDir,
    path: PathBuf,
    probe: RepositoryProbe,
    supervisor: SessionSupervisor<&'static str, &'static str>,
    turns: Receiver<FakeTurn>,
    results: Receiver<TurnResult<&'static str>>,
    events: Receiver<LifecycleEvent>,
}

fn start(config: SupervisorConfig) -> Started {
    static NEXT_OWNER: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let probe = RepositoryProbe::default();
    let processes = FakeProcesses::default();
    let (turn_tx, turn_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    let owner_suffix = format!(
        "current-{}",
        NEXT_OWNER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let (supervisor, results) = SessionSupervisor::start(
        "session-a",
        owner(1, &owner_suffix),
        1,
        Box::new(FakeRepository::open(&path, probe.clone())),
        Arc::new(processes.clone()),
        config,
        turn_tx,
        event_tx,
    )
    .unwrap();
    Started {
        _dir: dir,
        path,
        probe,
        supervisor,
        turns: turn_rx,
        results,
        events: event_rx,
    }
}

#[test]
fn lease_replay_targeted_reconstruction_and_event_replay_do_not_launch_a_child() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let fence = owner(1, "replayed");
    let mut seed = StateDb::open(&path).unwrap();
    seed.acquire_supervisor_lease("session-a", &fence, 1)
        .unwrap();
    seed.append_lifecycle_event("session-a", &event("replay-me", "accepted"))
        .unwrap();
    seed.append_lifecycle_event("session-b", &event("other-session", "accepted"))
        .unwrap();
    drop(seed);

    let probe = RepositoryProbe::default();
    let (turn_tx, turn_rx) = mpsc::channel::<FakeTurn>();
    let (event_tx, event_rx) = mpsc::channel();
    let (supervisor, _results) = SessionSupervisor::start(
        "session-a",
        fence.clone(),
        2,
        Box::new(FakeRepository::open(&path, probe.clone())),
        Arc::new(FakeProcesses::default()),
        SupervisorConfig::default(),
        turn_tx,
        event_tx,
    )
    .unwrap();

    assert_eq!(event_rx.recv().unwrap().event_id, "replay-me");
    assert!(matches!(turn_rx.try_recv(), Err(TryRecvError::Empty)));
    assert_eq!(
        probe.reconstructed_sessions.lock().unwrap().as_slice(),
        ["session-a"]
    );
    supervisor.close(20).unwrap();

    let db = StateDb::open(&path).unwrap();
    assert_eq!(db.supervisor_lease("session-a").unwrap(), None);
    assert!(
        db.reconstruct_session("session-a", "resident-session-supervisor", 10)
            .unwrap()
            .undisposed_events
            .is_empty()
    );
    assert_eq!(
        db.reconstruct_session("session-b", "resident-session-supervisor", 10)
            .unwrap()
            .undisposed_events[0]
            .event_id,
        "other-session"
    );
}

#[test]
fn live_dead_reused_and_unknown_owner_process_observations_fail_closed_or_replace_exactly() {
    for (observation, succeeds) in [
        (ProcessObservation::ExactLive, false),
        (ProcessObservation::Dead, true),
        (
            ProcessObservation::Reused(process(999, "reused-observation")),
            true,
        ),
        (
            ProcessObservation::Unknown("unreadable proc".to_owned()),
            false,
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let old = owner(1, "old");
        let candidate = owner(2, "candidate");
        let mut seed = StateDb::open(&path).unwrap();
        seed.acquire_supervisor_lease("session-a", &old, 1).unwrap();
        drop(seed);
        let processes = FakeProcesses::default();
        processes.set(old.process.pid, observation.clone());
        let (turn_tx, _turn_rx) = mpsc::channel::<FakeTurn>();
        let (event_tx, _event_rx) = mpsc::channel();
        let result = SessionSupervisor::start(
            "session-a",
            candidate.clone(),
            2,
            Box::new(FakeRepository::open(&path, RepositoryProbe::default())),
            Arc::new(processes),
            SupervisorConfig::default(),
            turn_tx,
            event_tx,
        );
        assert_eq!(result.is_ok(), succeeds, "observation: {observation:?}");
        if let Ok((supervisor, _results)) = result {
            assert_eq!(supervisor.status().unwrap().fence, candidate);
            supervisor.close(3).unwrap();
        }
    }
}

#[test]
fn simultaneous_stale_owner_replacements_converge_on_one_resident_owner() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let old = owner(1, "old");
    let mut seed = StateDb::open(&path).unwrap();
    seed.acquire_supervisor_lease("session-a", &old, 1).unwrap();
    drop(seed);

    let barrier = Arc::new(Barrier::new(2));
    let probe = RepositoryProbe {
        replacement_barrier: Some(barrier),
        ..RepositoryProbe::default()
    };
    let processes = FakeProcesses::default();
    processes.set(old.process.pid, ProcessObservation::Dead);

    let spawn = |candidate: SupervisorFence| {
        let path = path.clone();
        let probe = probe.clone();
        let processes = processes.clone();
        std::thread::spawn(move || {
            let (turn_tx, _turn_rx) = mpsc::channel::<FakeTurn>();
            let (event_tx, _event_rx) = mpsc::channel();
            SessionSupervisor::start(
                "session-a",
                candidate,
                2,
                Box::new(FakeRepository::open(&path, probe)),
                Arc::new(processes),
                SupervisorConfig::default(),
                turn_tx,
                event_tx,
            )
        })
    };
    let candidate_a = SupervisorFence {
        process: process(301, "candidate-a"),
        ..owner(2, "candidate-a")
    };
    let candidate_b = SupervisorFence {
        process: process(302, "candidate-b"),
        ..owner(2, "candidate-b")
    };
    let first = spawn(candidate_a.clone());
    let second = spawn(candidate_b.clone());
    let results = [first.join().unwrap(), second.join().unwrap()];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);

    let stored = StateDb::open(&path)
        .unwrap()
        .supervisor_lease("session-a")
        .unwrap();
    let winner = stored.expect("winning owner must hold the lease").fence;
    assert!(winner == candidate_a || winner == candidate_b);
    for (supervisor, _results) in results.into_iter().flatten() {
        assert_eq!(supervisor.status().unwrap().fence, winner);
        supervisor.close(3).unwrap();
    }
}

#[test]
fn exact_lease_replay_cannot_start_a_second_local_owner() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let fence = owner(1, "same-owner");
    let (first_turn_tx, _first_turn_rx) = mpsc::channel::<FakeTurn>();
    let (first_event_tx, _first_events) = mpsc::channel();
    let (first, _first_results) = SessionSupervisor::start(
        "session-a",
        fence.clone(),
        1,
        Box::new(FakeRepository::open(&path, RepositoryProbe::default())),
        Arc::new(FakeProcesses::default()),
        SupervisorConfig::default(),
        first_turn_tx,
        first_event_tx,
    )
    .unwrap();

    let (second_turn_tx, _second_turn_rx) = mpsc::channel::<FakeTurn>();
    let (second_event_tx, _second_events) = mpsc::channel();
    let second = SessionSupervisor::start(
        "session-a",
        fence,
        2,
        Box::new(FakeRepository::open(&path, RepositoryProbe::default())),
        Arc::new(FakeProcesses::default()),
        SupervisorConfig::default(),
        second_turn_tx,
        second_event_tx,
    );
    assert!(matches!(
        second,
        Err(SupervisorStartError::OwnerAlreadyRunning)
    ));
    first.close(3).unwrap();
}

#[test]
fn committed_events_publish_directly_and_failed_commits_never_publish() {
    let started = start(SupervisorConfig::default());
    let committed = started
        .supervisor
        .commit_event(event("committed", "test_event"))
        .unwrap();
    assert_eq!(committed.event_id, "committed");
    assert_eq!(started.events.recv().unwrap().event_id, "committed");

    started.probe.fail_next_append.store(true, Ordering::SeqCst);
    assert!(matches!(
        started
            .supervisor
            .commit_event(event("uncommitted", "test_event")),
        Err(SupervisorError::Repository(_))
    ));
    assert!(matches!(
        started.events.try_recv(),
        Err(TryRecvError::Empty)
    ));
    started.supervisor.close(20).unwrap();
}

#[test]
fn child_adapter_disconnect_preserves_fifo_work_until_a_new_exact_generation_is_handed_off() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let probe = RepositoryProbe::default();
    let (closed_turn_tx, closed_turn_rx) = mpsc::channel::<FakeTurn>();
    drop(closed_turn_rx);
    let (event_tx, _events) = mpsc::channel();
    let (supervisor, results) = SessionSupervisor::start(
        "session-a",
        owner(1, "adapter-reconnect"),
        1,
        Box::new(FakeRepository::open(&path, probe)),
        Arc::new(FakeProcesses::default()),
        SupervisorConfig::default(),
        closed_turn_tx,
        event_tx,
    )
    .unwrap();

    supervisor
        .notify(notification("session-a", 1, [1, 2]), 10)
        .unwrap();
    let snapshot = supervisor.status().unwrap();
    assert!(!snapshot.child_adapter_connected);
    assert_eq!(snapshot.queued_sequences, vec![1]);
    assert_eq!(snapshot.active_generation, None);

    let (replacement_tx, replacement_rx) = mpsc::channel();
    supervisor
        .reconnect_child_adapter(replacement_tx, 11)
        .unwrap();
    let request = replacement_rx.recv().unwrap();
    assert_eq!(request.notification.sequence, 1);
    // The disconnected send consumed turn 1, so reconnection receives the next generation.
    assert_eq!(request.turn.generation_id, "generation-session-a-2");
    request.completion.complete("result", 12).unwrap();
    assert!(matches!(
        results.recv().unwrap().outcome,
        TurnOutcome::Completed("result")
    ));
    supervisor.close(13).unwrap();
}

#[test]
fn closing_after_a_single_turn_adapter_disconnect_reports_the_consumed_generation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let (closed_turn_tx, closed_turn_rx) = mpsc::channel::<FakeTurn>();
    drop(closed_turn_rx);
    let (event_tx, _events) = mpsc::channel();
    let (supervisor, results) = SessionSupervisor::start(
        "session-a",
        owner(1, "single-disconnect"),
        1,
        Box::new(FakeRepository::open(&path, RepositoryProbe::default())),
        Arc::new(FakeProcesses::default()),
        SupervisorConfig::default(),
        closed_turn_tx,
        event_tx,
    )
    .unwrap();

    supervisor
        .notify(notification("session-a", 1, [1]), 10)
        .unwrap();
    supervisor.close(11).unwrap();

    let result = results.recv().unwrap();
    assert_eq!(result.notification_sequence, 1);
    assert_eq!(result.generation_id, "generation-session-a-1");
    assert!(matches!(
        result.outcome,
        TurnOutcome::Terminated(TerminalReason::ExplicitClose)
    ));
}

#[test]
fn scheduling_failure_does_not_reject_or_discard_accepted_work() {
    let started = start(SupervisorConfig::default());
    started.probe.fail_next_start.store(true, Ordering::SeqCst);

    let accepted = started
        .supervisor
        .notify(notification("session-a", 1, [1]), 10)
        .unwrap();
    assert_eq!(accepted.sequence, 1);
    assert_eq!(accepted.active_generation, None);
    assert_eq!(accepted.queued_notifications, 1);

    started.supervisor.pause(11).unwrap();
    started.supervisor.resume(12).unwrap();
    let request = started.turns.recv().unwrap();
    assert_eq!(request.notification.sequence, 1);
    request.completion.complete("result", 13).unwrap();
    assert_eq!(started.results.recv().unwrap().notification_sequence, 1);
    started.supervisor.close(14).unwrap();
}

#[test]
fn dropped_completion_child_panic_and_abnormal_exit_have_bounded_retry_outcomes() {
    let config = SupervisorConfig {
        max_retries: 2,
        ..SupervisorConfig::default()
    };
    let started = start(config);
    started
        .supervisor
        .notify(notification("session-a", 1, [1, 2, 3]), 10)
        .unwrap();

    let first = started.turns.recv().unwrap();
    drop(first.completion);
    assert!(matches!(
        started.results.recv().unwrap().outcome,
        TurnOutcome::RetryScheduled(reason) if reason == ChildFailure::CompletionDropped.to_string()
    ));
    assert_eq!(started.events.recv().unwrap().created_at, 10);
    let dropped_event = started.events.recv().unwrap();
    assert_eq!(dropped_event.event_type, "supervisor_turn_failed");
    assert_eq!(dropped_event.created_at, 10);

    let second = started.turns.recv().unwrap();
    second.completion.panicked(11).unwrap();
    assert!(matches!(
        started.results.recv().unwrap().outcome,
        TurnOutcome::RetryScheduled(reason) if reason == ChildFailure::Panic.to_string()
    ));

    let third = started.turns.recv().unwrap();
    third.completion.abnormal_exit("signal 9", 12).unwrap();
    assert!(matches!(
        started.results.recv().unwrap().outcome,
        TurnOutcome::RetryExhausted(reason) if reason.contains("signal 9")
    ));
    assert_eq!(started.supervisor.status().unwrap().active_generation, None);
    started.supervisor.close(13).unwrap();
}

#[test]
fn pause_queue_caps_and_expiry_are_command_driven_and_deterministic() {
    let config = SupervisorConfig {
        queue_capacity: 2,
        ..SupervisorConfig::default()
    };
    let started = start(config);
    started.supervisor.pause(9).unwrap();
    started
        .supervisor
        .notify(notification("session-a", 1, [1]).expiring_at(20), 10)
        .unwrap();
    started
        .supervisor
        .notify(notification("session-a", 2, [2]).expiring_at(21), 10)
        .unwrap();
    assert!(matches!(
        started
            .supervisor
            .notify(notification("session-a", 3, [3]), 11),
        Err(SupervisorError::QueueFull)
    ));
    assert!(matches!(started.turns.try_recv(), Err(TryRecvError::Empty)));

    started.supervisor.resume(20).unwrap();
    let surviving = started.turns.recv().unwrap();
    assert_eq!(surviving.notification.sequence, 2);
    let expired = started.results.recv().unwrap();
    assert_eq!(expired.notification_sequence, 1);
    assert_eq!(expired.generation_id, "generation-session-a-1");
    assert!(matches!(
        expired.outcome,
        TurnOutcome::Terminated(TerminalReason::Expired)
    ));
    let expiry_event = started
        .events
        .try_iter()
        .find(|event| event.event_type == "supervisor_work_expired")
        .unwrap();
    assert_eq!(expiry_event.payload, "sequence=1");
    surviving.completion.complete("second", 21).unwrap();
    assert_eq!(started.results.recv().unwrap().notification_sequence, 2);
    assert!(
        started
            .supervisor
            .status()
            .unwrap()
            .queued_sequences
            .is_empty()
    );

    started
        .supervisor
        .notify(notification("session-a", 4, [4]), 22)
        .unwrap();
    let fourth = started.turns.recv().unwrap();
    fourth.completion.complete("fourth", 23).unwrap();
    assert_eq!(started.results.recv().unwrap().notification_sequence, 4);
    started.supervisor.close(24).unwrap();
}

#[test]
fn active_and_queued_cancellation_drain_close_and_expiry_have_exact_terminal_outcomes() {
    let started = start(SupervisorConfig::default());
    started
        .supervisor
        .notify(notification("session-a", 1, [1]), 10)
        .unwrap();
    let active = started.turns.recv().unwrap();
    started
        .supervisor
        .notify(notification("session-a", 2, [2]), 11)
        .unwrap();
    started
        .supervisor
        .notify(notification("session-a", 3, [3]), 12)
        .unwrap();
    started.supervisor.cancel(2, 13).unwrap();
    let queued_cancelled = started.results.recv().unwrap();
    assert_eq!(queued_cancelled.notification_sequence, 2);
    assert_eq!(queued_cancelled.generation_id, "generation-session-a-2");
    assert!(matches!(queued_cancelled.outcome, TurnOutcome::Cancelled));
    assert_eq!(
        started.supervisor.status().unwrap().queued_sequences,
        vec![3]
    );
    started.supervisor.cancel(1, 14).unwrap();
    assert!(matches!(
        started.results.recv().unwrap().outcome,
        TurnOutcome::Cancelled
    ));
    assert!(active.completion.complete("stale", 15).is_err());

    let third = started.turns.recv().unwrap();
    started.supervisor.drain(16).unwrap();
    assert!(matches!(
        started
            .supervisor
            .notify(notification("session-a", 4, [4]), 17),
        Err(SupervisorError::Draining)
    ));
    third.completion.complete("third", 18).unwrap();
    assert!(matches!(
        started.results.recv().unwrap().outcome,
        TurnOutcome::Completed("third")
    ));
    drop(started.supervisor);
    let db = StateDb::open(&started.path).unwrap();
    assert_eq!(db.supervisor_lease("session-a").unwrap(), None);

    let explicit_close = start(SupervisorConfig::default());
    explicit_close
        .supervisor
        .notify(notification("session-a", 1, [1]), 20)
        .unwrap();
    explicit_close
        .supervisor
        .notify(notification("session-a", 2, [2]), 20)
        .unwrap();
    explicit_close.supervisor.close(21).unwrap();
    assert!(matches!(
        explicit_close.results.recv().unwrap().outcome,
        TurnOutcome::Terminated(TerminalReason::ExplicitClose)
    ));
    let queued = explicit_close.results.recv().unwrap();
    assert_eq!(queued.notification_sequence, 2);
    assert_eq!(queued.generation_id, "generation-session-a-2");
    assert!(matches!(
        queued.outcome,
        TurnOutcome::Terminated(TerminalReason::ExplicitClose)
    ));

    let expiry = start(SupervisorConfig::default());
    expiry
        .supervisor
        .notify(notification("session-a", 1, [1]), 30)
        .unwrap();
    expiry.supervisor.expire(31).unwrap();
    assert!(matches!(
        expiry.results.recv().unwrap().outcome,
        TurnOutcome::Terminated(TerminalReason::Expired)
    ));
}

#[test]
fn notifications_without_a_provider_turn_are_rejected_before_acceptance() {
    let started = start(SupervisorConfig::default());

    assert!(matches!(
        started
            .supervisor
            .notify(notification("session-a", 1, []), 10),
        Err(SupervisorError::MissingTurn)
    ));
    assert!(matches!(
        started.results.try_recv(),
        Err(TryRecvError::Empty)
    ));
    started.supervisor.close(11).unwrap();
}

#[test]
fn cancelling_the_last_active_turn_while_draining_stops_the_released_owner() {
    let started = start(SupervisorConfig::default());
    started
        .supervisor
        .notify(notification("session-a", 1, [1]), 10)
        .unwrap();
    let active = started.turns.recv().unwrap();

    started.supervisor.drain(11).unwrap();
    started.supervisor.cancel(1, 12).unwrap();

    assert!(matches!(
        started.results.recv().unwrap().outcome,
        TurnOutcome::Cancelled
    ));
    assert!(matches!(
        started.supervisor.status(),
        Err(SupervisorError::Closed)
    ));
    assert!(active.completion.complete("stale", 13).is_err());
    assert_eq!(
        StateDb::open(&started.path)
            .unwrap()
            .supervisor_lease("session-a")
            .unwrap(),
        None
    );
}

#[test]
fn retry_and_generation_exhaustion_are_distinct_and_stale_commands_cannot_finalize_a_turn() {
    let retry_exhausted = start(SupervisorConfig::default());
    retry_exhausted
        .supervisor
        .notify(notification("session-a", 1, [1]), 10)
        .unwrap();
    retry_exhausted
        .turns
        .recv()
        .unwrap()
        .completion
        .abnormal_exit("failed", 11)
        .unwrap();
    assert!(matches!(
        retry_exhausted.results.recv().unwrap().outcome,
        TurnOutcome::RetryExhausted(_)
    ));
    retry_exhausted.supervisor.close(12).unwrap();

    let generation_exhausted = start(SupervisorConfig {
        max_retries: 1,
        ..SupervisorConfig::default()
    });
    generation_exhausted
        .supervisor
        .notify(notification("session-a", 1, [1]), 20)
        .unwrap();
    generation_exhausted
        .turns
        .recv()
        .unwrap()
        .completion
        .abnormal_exit("failed", 21)
        .unwrap();
    assert!(matches!(
        generation_exhausted.results.recv().unwrap().outcome,
        TurnOutcome::GenerationExhausted(_)
    ));
    generation_exhausted.supervisor.close(22).unwrap();
}

#[test]
fn reconstructed_live_turn_is_not_relaunched_and_only_its_exact_exit_is_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let owner_fence = owner(1, "reconstructed-current");
    let active = turn("session-a", 1);
    let mut seed = StateDb::open(&path).unwrap();
    seed.acquire_supervisor_lease("session-a", &owner_fence, 1)
        .unwrap();
    seed.start_provider_turn(&active).unwrap();
    drop(seed);
    let processes = FakeProcesses::default();
    processes.set(active.child.pid, ProcessObservation::ExactLive);
    let (turn_tx, turn_rx) = mpsc::channel::<FakeTurn>();
    let (event_tx, _events) = mpsc::channel();
    let (supervisor, _results) = SessionSupervisor::start(
        "session-a",
        owner_fence,
        2,
        Box::new(FakeRepository::open(&path, RepositoryProbe::default())),
        Arc::new(processes),
        SupervisorConfig::default(),
        turn_tx,
        event_tx,
    )
    .unwrap();
    assert!(matches!(turn_rx.try_recv(), Err(TryRecvError::Empty)));
    assert_eq!(
        supervisor.status().unwrap().active_generation.as_deref(),
        Some("generation-session-a-1")
    );

    let stale = TurnFence {
        session_id: "session-a".to_owned(),
        generation_id: "stale".to_owned(),
        spawn_invocation_id: "stale".to_owned(),
    };
    assert!(matches!(
        supervisor.reconcile_reconstructed_exit(stale, 3, "stale"),
        Err(SupervisorError::StaleCommand)
    ));
    supervisor
        .reconcile_reconstructed_exit(
            TurnFence {
                session_id: "session-a".to_owned(),
                generation_id: active.generation_id,
                spawn_invocation_id: active.spawn_invocation_id,
            },
            4,
            "observed exit",
        )
        .unwrap();
    assert_eq!(supervisor.status().unwrap().active_generation, None);
    supervisor.close(5).unwrap();
}

#[test]
fn stale_reconstructed_turn_is_durably_exited_during_startup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let owner_fence = owner(1, "stale-reconstruction");
    let active = turn("session-a", 1);
    let mut seed = StateDb::open(&path).unwrap();
    seed.acquire_supervisor_lease("session-a", &owner_fence, 1)
        .unwrap();
    seed.start_provider_turn(&active).unwrap();
    drop(seed);

    let processes = FakeProcesses::default();
    processes.set(active.child.pid, ProcessObservation::Dead);
    let (turn_tx, turn_rx) = mpsc::channel::<FakeTurn>();
    let (event_tx, events) = mpsc::channel();
    let (supervisor, _results) = SessionSupervisor::start(
        "session-a",
        owner_fence,
        2,
        Box::new(FakeRepository::open(&path, RepositoryProbe::default())),
        Arc::new(processes),
        SupervisorConfig::default(),
        turn_tx,
        event_tx,
    )
    .unwrap();

    assert!(matches!(turn_rx.try_recv(), Err(TryRecvError::Empty)));
    assert_eq!(supervisor.status().unwrap().active_generation, None);
    assert_eq!(
        events.recv().unwrap().event_type,
        "supervisor_reconstructed_child_stale"
    );
    assert_eq!(
        StateDb::open(&path)
            .unwrap()
            .provider_turn(&active.generation_id)
            .unwrap()
            .unwrap()
            .state,
        TurnState::Exited
    );
    supervisor.close(3).unwrap();
}

#[test]
fn unknown_reconstructed_child_observation_releases_the_supervisor_lease() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let owner_fence = owner(1, "unknown-reconstruction");
    let active = turn("session-a", 1);
    let mut seed = StateDb::open(&path).unwrap();
    seed.acquire_supervisor_lease("session-a", &owner_fence, 1)
        .unwrap();
    seed.start_provider_turn(&active).unwrap();
    drop(seed);

    let processes = FakeProcesses::default();
    processes.set(
        active.child.pid,
        ProcessObservation::Unknown("unreadable proc".to_owned()),
    );
    let (turn_tx, _turn_rx) = mpsc::channel::<FakeTurn>();
    let (event_tx, _events) = mpsc::channel();
    let result = SessionSupervisor::start(
        "session-a",
        owner_fence,
        2,
        Box::new(FakeRepository::open(&path, RepositoryProbe::default())),
        Arc::new(processes),
        SupervisorConfig::default(),
        turn_tx,
        event_tx,
    );

    assert!(matches!(
        result,
        Err(SupervisorStartError::ProcessObservationUnknown(error))
            if error == "unreadable proc"
    ));
    let db = StateDb::open(&path).unwrap();
    assert_eq!(db.supervisor_lease("session-a").unwrap(), None);
    assert_eq!(
        db.provider_turn(&active.generation_id)
            .unwrap()
            .unwrap()
            .state,
        TurnState::Running
    );
}

#[test]
fn failed_reconstructed_turn_reconciliation_releases_the_supervisor_lease() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let owner_fence = owner(1, "failed-reconciliation");
    let active = turn("session-a", 1);
    let mut seed = StateDb::open(&path).unwrap();
    seed.acquire_supervisor_lease("session-a", &owner_fence, 1)
        .unwrap();
    seed.start_provider_turn(&active).unwrap();
    drop(seed);

    let probe = RepositoryProbe::default();
    probe.fail_next_transition.store(true, Ordering::SeqCst);
    let processes = FakeProcesses::default();
    processes.set(active.child.pid, ProcessObservation::Dead);
    let (turn_tx, _turn_rx) = mpsc::channel::<FakeTurn>();
    let (event_tx, events) = mpsc::channel();
    let result = SessionSupervisor::start(
        "session-a",
        owner_fence,
        2,
        Box::new(FakeRepository::open(&path, probe)),
        Arc::new(processes),
        SupervisorConfig::default(),
        turn_tx,
        event_tx,
    );

    assert!(matches!(result, Err(SupervisorStartError::Repository(_))));
    assert!(events.try_iter().next().is_none());
    let db = StateDb::open(&path).unwrap();
    assert_eq!(db.supervisor_lease("session-a").unwrap(), None);
}

#[test]
fn dropped_public_handle_terminates_accepted_work_and_invalidates_completion_lifetime() {
    let started = start(SupervisorConfig::default());
    started
        .supervisor
        .notify(notification("session-a", 1, [1]), 10)
        .unwrap();
    let request = started.turns.recv().unwrap();
    drop(started.supervisor);
    assert!(matches!(
        started.results.recv().unwrap().outcome,
        TurnOutcome::Terminated(TerminalReason::HandleDropped)
    ));
    assert!(request.completion.complete("too-late", 11).is_err());
    let db = StateDb::open(&started.path).unwrap();
    assert_eq!(db.supervisor_lease("session-a").unwrap(), None);
}

#[test]
fn replacement_generation_exhaustion_fails_before_starting_an_owner() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let old = SupervisorFence {
        generation: i64::MAX,
        token: "token-old".to_owned(),
        process: process(100, "old"),
    };
    let mut seed = StateDb::open(&path).unwrap();
    seed.acquire_supervisor_lease("session-a", &old, 1).unwrap();
    drop(seed);
    let processes = FakeProcesses::default();
    processes.set(old.process.pid, ProcessObservation::Dead);
    let (turn_tx, _turn_rx) = mpsc::channel::<FakeTurn>();
    let (event_tx, _events) = mpsc::channel();
    let result = SessionSupervisor::start(
        "session-a",
        owner(1, "candidate"),
        2,
        Box::new(FakeRepository::open(&path, RepositoryProbe::default())),
        Arc::new(processes),
        SupervisorConfig::default(),
        turn_tx,
        event_tx,
    );
    assert!(matches!(
        result,
        Err(SupervisorStartError::ReplacementGenerationExhausted)
    ));
}

#[test]
fn resident_owner_source_has_no_poll_scan_sweep_sleep_or_timer_coordination() {
    fn assert_allowed_identifiers(tokens: TokenStream) {
        for token in tokens {
            match token {
                TokenTree::Group(group) => assert_allowed_identifiers(group.stream()),
                TokenTree::Ident(ident) => {
                    let ident = ident.to_string();
                    let forbidden = ident == "recv_deadline"
                        || ident.starts_with("recv_timeout")
                        || matches!(
                            ident.as_str(),
                            "try_recv"
                                | "sleep"
                                | "sleep_until"
                                | "park_timeout"
                                | "Instant"
                                | "Duration"
                                | "scan_sessions"
                                | "maintenance_driver"
                        )
                        || ident.contains("outbox")
                        || ident.starts_with("sweep");
                    assert!(!forbidden, "resident coordination must not contain {ident}");
                }
                _ => {}
            }
        }
    }

    let source = include_str!("../src/session_supervisor.rs");
    assert_allowed_identifiers(source.parse().expect("supervisor source must tokenize"));
}
