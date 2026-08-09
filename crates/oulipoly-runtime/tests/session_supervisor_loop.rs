use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use oulipoly_runtime::session_supervisor::{
    ProcessObservation, ProcessObserver, SessionNotification, SessionSupervisor, SupervisorConfig,
    SupervisorPhase, TurnOutcome, TurnRequest, TurnResult,
};
use oulipoly_state::{
    ExactProcessIdentity, ProviderTurnGeneration, StateDb, SupervisorFence, TurnState,
};

type FakeTurn = TurnRequest<&'static str, &'static str>;

#[derive(Clone)]
struct ExactLiveProcesses;

impl ProcessObserver for ExactLiveProcesses {
    fn observe(&self, _expected: &ExactProcessIdentity) -> ProcessObservation {
        ProcessObservation::ExactLive
    }
}

fn process(pid: i64, suffix: &str) -> ExactProcessIdentity {
    ExactProcessIdentity {
        pid,
        boot_id: format!("boot-{suffix}"),
        start_time_ticks: pid * 10,
    }
}

fn owner(suffix: &str) -> SupervisorFence {
    SupervisorFence {
        generation: 1,
        token: format!("owner-{suffix}"),
        process: process(100, suffix),
    }
}

fn turn(session: &str, generation: i64) -> ProviderTurnGeneration {
    ProviderTurnGeneration {
        generation_id: format!("{session}-generation-{generation}"),
        spawn_invocation_id: format!("{session}-invocation-{generation}"),
        session_id: Some(session.to_owned()),
        state: TurnState::Running,
        child: process(200 + generation, &format!("{session}-{generation}")),
    }
}

fn start(
    session: &str,
    turns: mpsc::Sender<FakeTurn>,
) -> (
    tempfile::TempDir,
    SessionSupervisor<&'static str, &'static str>,
    Receiver<TurnResult<&'static str>>,
) {
    let dir = tempfile::tempdir().unwrap();
    let db = StateDb::open(&dir.path().join("state.db")).unwrap();
    let (supervisor, results) = start_with_db(session, turns, db);
    (dir, supervisor, results)
}

fn start_with_db(
    session: &str,
    turns: mpsc::Sender<FakeTurn>,
    db: StateDb,
) -> (
    SessionSupervisor<&'static str, &'static str>,
    Receiver<TurnResult<&'static str>>,
) {
    static NEXT_OWNER: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1);
    let (event_tx, _event_rx) = mpsc::channel();
    let owner_suffix = format!(
        "{session}-{}",
        NEXT_OWNER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let (supervisor, results) = SessionSupervisor::start(
        session,
        owner(&owner_suffix),
        1,
        Box::new(db),
        Arc::new(ExactLiveProcesses),
        SupervisorConfig::default(),
        turns,
        event_tx,
    )
    .unwrap();
    (supervisor, results)
}

fn notification(
    session: &str,
    sequence: i64,
    input: &'static str,
) -> SessionNotification<&'static str> {
    SessionNotification::new(sequence, input, turn(session, sequence))
}

fn receive_turn(turns: &Receiver<FakeTurn>) -> FakeTurn {
    turns
        .recv_timeout(Duration::from_secs(5))
        .expect("supervisor should launch a fake turn")
}

fn receive_result(results: &Receiver<TurnResult<&'static str>>) -> TurnResult<&'static str> {
    results
        .recv_timeout(Duration::from_secs(5))
        .expect("supervisor should publish the exact turn result")
}

#[test]
fn owner_queues_active_notifications_and_stays_alive_across_turns() {
    let (turn_tx, turn_rx) = mpsc::channel();
    let (_dir, supervisor, results) = start("session-a", turn_tx);

    let first_acceptance = supervisor
        .notify(notification("session-a", 1, "first"), 10)
        .unwrap();
    assert_eq!(
        first_acceptance.active_generation.as_deref(),
        Some("session-a-generation-1")
    );
    assert_eq!(first_acceptance.queued_notifications, 0);
    let first = receive_turn(&turn_rx);
    assert_eq!(first.session_id, "session-a");
    assert_eq!(first.turn.generation_id, "session-a-generation-1");
    assert_eq!(
        (first.notification.sequence, first.notification.input),
        (1, "first")
    );

    let second_acceptance = supervisor
        .notify(notification("session-a", 2, "second"), 11)
        .unwrap();
    assert_eq!(
        second_acceptance.active_generation.as_deref(),
        Some("session-a-generation-1")
    );
    assert_eq!(second_acceptance.queued_notifications, 1);
    assert!(matches!(turn_rx.try_recv(), Err(TryRecvError::Empty)));

    first.completion.complete("first-result", 12).unwrap();
    assert_eq!(
        receive_result(&results),
        TurnResult {
            session_id: "session-a".to_owned(),
            generation_id: "session-a-generation-1".to_owned(),
            spawn_invocation_id: "session-a-invocation-1".to_owned(),
            notification_sequence: 1,
            outcome: TurnOutcome::Completed("first-result"),
        }
    );
    let second = receive_turn(&turn_rx);
    assert_eq!(
        (
            second.turn.generation_id.as_str(),
            second.notification.sequence
        ),
        ("session-a-generation-2", 2)
    );
    assert!(matches!(turn_rx.try_recv(), Err(TryRecvError::Empty)));

    second.completion.complete("second-result", 13).unwrap();
    assert_eq!(receive_result(&results).notification_sequence, 2);
    let snapshot = supervisor.status().unwrap();
    assert_eq!(snapshot.phase, SupervisorPhase::Running);
    assert_eq!(snapshot.active_generation, None);
    assert_eq!(snapshot.active_sequence, None);
    assert!(snapshot.queued_sequences.is_empty());

    supervisor
        .notify(notification("session-a", 3, "third"), 14)
        .unwrap();
    let third = receive_turn(&turn_rx);
    assert_eq!(third.turn.generation_id, "session-a-generation-3");
    third.completion.complete("third-result", 15).unwrap();
    assert_eq!(receive_result(&results).notification_sequence, 3);
    supervisor.close(16).unwrap();
}

#[test]
fn notification_order_and_child_exits_are_isolated_per_session() {
    let (turn_tx, turn_rx) = mpsc::channel();
    let dir = tempfile::tempdir().unwrap();
    let state_path = dir.path().join("state.db");
    let (supervisor_a, results_a) = start_with_db(
        "session-a",
        turn_tx.clone(),
        StateDb::open(&state_path).unwrap(),
    );
    let (supervisor_b, results_b) =
        start_with_db("session-b", turn_tx, StateDb::open(&state_path).unwrap());

    supervisor_a
        .notify(notification("session-a", 10, "a-first"), 10)
        .unwrap();
    let a_first = receive_turn(&turn_rx);
    supervisor_a
        .notify(notification("session-a", 11, "a-second"), 11)
        .unwrap();

    supervisor_b
        .notify(notification("session-b", 20, "b-first"), 12)
        .unwrap();
    let b_first = receive_turn(&turn_rx);
    supervisor_b
        .notify(notification("session-b", 21, "b-second"), 13)
        .unwrap();
    assert!(matches!(turn_rx.try_recv(), Err(TryRecvError::Empty)));

    b_first.completion.complete("b-first-result", 14).unwrap();
    assert_eq!(receive_result(&results_b).notification_sequence, 20);
    let b_second = receive_turn(&turn_rx);
    assert_eq!(
        (b_second.session_id.as_str(), b_second.notification.sequence),
        ("session-b", 21)
    );
    assert_eq!(supervisor_a.status().unwrap().queued_sequences, vec![11]);
    assert!(matches!(results_a.try_recv(), Err(TryRecvError::Empty)));

    a_first.completion.complete("a-first-result", 15).unwrap();
    assert_eq!(receive_result(&results_a).notification_sequence, 10);
    let a_second = receive_turn(&turn_rx);
    assert_eq!(
        (a_second.session_id.as_str(), a_second.notification.sequence),
        ("session-a", 11)
    );
    assert!(matches!(turn_rx.try_recv(), Err(TryRecvError::Empty)));

    b_second.completion.complete("b-second-result", 16).unwrap();
    a_second.completion.complete("a-second-result", 17).unwrap();
    assert_eq!(receive_result(&results_b).notification_sequence, 21);
    assert_eq!(receive_result(&results_a).notification_sequence, 11);
    supervisor_a.close(18).unwrap();
    supervisor_b.close(18).unwrap();
}
