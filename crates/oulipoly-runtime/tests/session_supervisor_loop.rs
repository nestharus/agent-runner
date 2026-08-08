use std::sync::mpsc::{self, Receiver, TryRecvError};

use oulipoly_runtime::session_supervisor::{
    SessionNotification, SessionSupervisor, SupervisorSnapshot, TurnRequest, TurnResult,
};

type FakeTurn = TurnRequest<&'static str, &'static str>;

fn receive_turn(turns: &Receiver<FakeTurn>) -> FakeTurn {
    turns.recv().expect("supervisor should launch a fake turn")
}

fn receive_result(results: &Receiver<TurnResult<&'static str>>) -> TurnResult<&'static str> {
    results
        .recv()
        .expect("supervisor should publish the caller-visible turn result")
}

#[test]
fn owner_queues_active_notifications_and_stays_alive_across_turns() {
    let (turn_tx, turn_rx) = mpsc::channel();
    let (supervisor, results) = SessionSupervisor::start("session-a", turn_tx);

    let first_acceptance = supervisor
        .notify(SessionNotification {
            sequence: 1,
            input: "first",
        })
        .unwrap();
    assert_eq!(first_acceptance.active_generation, Some(1));
    assert_eq!(first_acceptance.queued_notifications, 0);
    let first = receive_turn(&turn_rx);
    assert_eq!(
        (first.session_id.as_str(), first.generation),
        ("session-a", 1)
    );
    assert_eq!(
        (first.notification.sequence, first.notification.input),
        (1, "first")
    );

    let second_acceptance = supervisor
        .notify(SessionNotification {
            sequence: 2,
            input: "second",
        })
        .unwrap();
    assert_eq!(second_acceptance.active_generation, Some(1));
    assert_eq!(second_acceptance.queued_notifications, 1);
    assert!(matches!(turn_rx.try_recv(), Err(TryRecvError::Empty)));

    first.completion.complete("first-result").unwrap();
    assert_eq!(
        receive_result(&results),
        TurnResult {
            session_id: "session-a".to_owned(),
            generation: 1,
            notification_sequence: 1,
            output: "first-result",
        }
    );
    let second = receive_turn(&turn_rx);
    assert_eq!((second.generation, second.notification.sequence), (2, 2));
    assert!(matches!(turn_rx.try_recv(), Err(TryRecvError::Empty)));

    second.completion.complete("second-result").unwrap();
    assert_eq!(receive_result(&results).generation, 2);
    assert_eq!(
        supervisor.status().unwrap(),
        SupervisorSnapshot {
            session_id: "session-a".to_owned(),
            active_generation: None,
            active_sequence: None,
            queued_sequences: Vec::new(),
        }
    );

    supervisor
        .notify(SessionNotification {
            sequence: 3,
            input: "third",
        })
        .unwrap();
    let third = receive_turn(&turn_rx);
    assert_eq!(third.generation, 3);
    third.completion.complete("third-result").unwrap();
    assert_eq!(receive_result(&results).notification_sequence, 3);
    supervisor.shutdown().unwrap();
}

#[test]
fn notification_order_and_child_exits_are_isolated_per_session() {
    let (turn_tx, turn_rx) = mpsc::channel();
    let (supervisor_a, results_a) = SessionSupervisor::start("session-a", turn_tx.clone());
    let (supervisor_b, results_b) = SessionSupervisor::start("session-b", turn_tx);

    supervisor_a
        .notify(SessionNotification {
            sequence: 10,
            input: "a-first",
        })
        .unwrap();
    let a_first = receive_turn(&turn_rx);
    supervisor_a
        .notify(SessionNotification {
            sequence: 11,
            input: "a-second",
        })
        .unwrap();

    supervisor_b
        .notify(SessionNotification {
            sequence: 20,
            input: "b-first",
        })
        .unwrap();
    let b_first = receive_turn(&turn_rx);
    supervisor_b
        .notify(SessionNotification {
            sequence: 21,
            input: "b-second",
        })
        .unwrap();
    assert!(matches!(turn_rx.try_recv(), Err(TryRecvError::Empty)));

    b_first.completion.complete("b-first-result").unwrap();
    assert_eq!(receive_result(&results_b).notification_sequence, 20);
    let b_second = receive_turn(&turn_rx);
    assert_eq!(
        (b_second.session_id.as_str(), b_second.notification.sequence),
        ("session-b", 21)
    );
    assert_eq!(supervisor_a.status().unwrap().queued_sequences, vec![11]);
    assert!(matches!(results_a.try_recv(), Err(TryRecvError::Empty)));

    a_first.completion.complete("a-first-result").unwrap();
    assert_eq!(receive_result(&results_a).notification_sequence, 10);
    let a_second = receive_turn(&turn_rx);
    assert_eq!(
        (a_second.session_id.as_str(), a_second.notification.sequence),
        ("session-a", 11)
    );
    assert!(matches!(turn_rx.try_recv(), Err(TryRecvError::Empty)));

    b_second.completion.complete("b-second-result").unwrap();
    a_second.completion.complete("a-second-result").unwrap();
    assert_eq!(receive_result(&results_b).notification_sequence, 21);
    assert_eq!(receive_result(&results_a).notification_sequence, 11);
    supervisor_a.shutdown().unwrap();
    supervisor_b.shutdown().unwrap();
}
