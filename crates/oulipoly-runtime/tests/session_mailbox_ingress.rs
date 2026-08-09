use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

#[cfg(unix)]
use oulipoly_runtime::executor::cli::pty_broker::PtyControlClientErrorKind;
#[cfg(unix)]
use oulipoly_runtime::session_ingress::send_headless_resume_poke;
use oulipoly_runtime::session_ingress::{
    HeadlessResumePoke, SessionIngressError, SessionMailboxIngress,
};
use oulipoly_runtime::session_supervisor::{
    ProcessObservation, ProcessObserver, SessionNotification, SessionSupervisor, SupervisorConfig,
    TurnRequest,
};
use oulipoly_state::mailbox::{
    AgentBashCompleteEnqueue, EnqueueResult, InboxTarget, InboxTargetKind,
    MAILBOX_INGRESS_EXPIRED_ERROR, MAILBOX_PAYLOAD_VERIFICATION_FAILED_ERROR, MailboxDb,
    MailboxRow, SubmittedInputEnqueue,
};
use oulipoly_state::{
    ExactProcessIdentity, ExternalIngress, ProviderTurnGeneration, SessionLifecycleRepository,
    StateDb, SupervisorFence, TurnState,
};

type FakeTurn = TurnRequest<MailboxRow, &'static str>;

#[derive(Clone)]
struct ExactLive;

impl ProcessObserver for ExactLive {
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

fn owner(generation: i64, suffix: &str) -> SupervisorFence {
    SupervisorFence {
        generation,
        token: format!("owner-{suffix}"),
        process: process(100 + generation, suffix),
    }
}

fn turn(session_id: &str, sequence: i64) -> ProviderTurnGeneration {
    ProviderTurnGeneration {
        generation_id: format!("generation-{session_id}-{sequence}"),
        spawn_invocation_id: format!("invocation-{session_id}-{sequence}"),
        session_id: Some(session_id.to_owned()),
        state: TurnState::Running,
        child: process(200 + sequence, &format!("child-{session_id}-{sequence}")),
    }
}

fn map_notification(
    session_id: &str,
    row: &MailboxRow,
) -> Result<SessionNotification<MailboxRow>, String> {
    Ok(SessionNotification::new(
        row.seq,
        row.clone(),
        turn(session_id, row.seq),
    ))
}

fn enqueue(mailbox: &mut MailboxDb, session_id: &str, handle: &str) -> MailboxRow {
    let owner_invocation_uuid = format!("nested-owner-{session_id}");
    let result = mailbox
        .enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
            session_id,
            handle,
            payload_json: &format!(r#"{{"handle":"{handle}"}}"#),
            owner_invocation_uuid: Some(&owner_invocation_uuid),
            matched_os_pid: Some(42),
            matched_os_boot_id: Some("boot-owner"),
            matched_os_pid_starttime_ticks: Some(420),
            matched_chain_index: Some(2),
            state_dir: "/tmp/state",
            meta_path: "/tmp/meta",
            log_path: "/tmp/log",
            rc_path: "/tmp/rc",
            rc: 0,
        })
        .unwrap();
    match result {
        EnqueueResult::Inserted(row) | EnqueueResult::AlreadyEnqueued(row) => row,
        EnqueueResult::Conflict { .. } => panic!("fixture enqueue conflicted"),
    }
}

fn enqueue_chain_input(mailbox: &mut MailboxDb, chain_id: &str, token: &str) -> MailboxRow {
    let result = mailbox
        .enqueue_submitted_input(&SubmittedInputEnqueue {
            submission_token: token,
            target: InboxTarget {
                kind: InboxTargetKind::Chain,
                id: chain_id,
            },
            input: b"chain input",
        })
        .unwrap();
    match result {
        EnqueueResult::Inserted(row) | EnqueueResult::AlreadyEnqueued(row) => row,
        EnqueueResult::Conflict { .. } => panic!("fixture enqueue conflicted"),
    }
}

fn start_owner(
    state_path: &std::path::Path,
    fence: SupervisorFence,
    queue_capacity: usize,
) -> (
    SessionSupervisor<MailboxRow, &'static str>,
    Receiver<FakeTurn>,
) {
    start_owner_with_retries(state_path, fence, queue_capacity, 0)
}

fn start_owner_with_retries(
    state_path: &std::path::Path,
    fence: SupervisorFence,
    queue_capacity: usize,
    max_retries: usize,
) -> (
    SessionSupervisor<MailboxRow, &'static str>,
    Receiver<FakeTurn>,
) {
    let (turn_tx, turn_rx) = mpsc::channel();
    let (event_tx, _event_rx) = mpsc::channel();
    let (owner, _results) = SessionSupervisor::start(
        "session-a",
        fence,
        1,
        Box::new(StateDb::open(state_path).unwrap()),
        Arc::new(ExactLive),
        SupervisorConfig {
            queue_capacity,
            max_retries,
            ..SupervisorConfig::default()
        },
        turn_tx,
        event_tx,
    )
    .unwrap();
    (owner, turn_rx)
}

fn map_retry_notification(
    session_id: &str,
    row: &MailboxRow,
) -> Result<SessionNotification<MailboxRow>, String> {
    let mut notification = SessionNotification::with_retry_turns(
        row.seq,
        row.clone(),
        [
            turn(session_id, row.seq * 10),
            turn(session_id, row.seq * 10 + 1),
        ],
    );
    if row.handle == "expired" {
        notification = notification.expiring_at(10);
    }
    Ok(notification)
}

fn receive_turn(turns: &Receiver<FakeTurn>) -> FakeTurn {
    turns
        .recv_timeout(Duration::from_secs(5))
        .expect("resident owner should emit a bounded fake turn")
}

#[cfg(unix)]
#[test]
fn headless_resume_poke_preserves_control_transport_error_kind() {
    let dir = tempfile::tempdir().unwrap();
    let error = send_headless_resume_poke(
        dir.path().join("missing.sock"),
        &HeadlessResumePoke {
            session_id: "session-a".to_owned(),
            supervisor_generation: 1,
            lease_token: "owner-current".to_owned(),
        },
    )
    .unwrap_err();

    match error {
        SessionIngressError::ControlTransport(error) => {
            assert_eq!(error.kind, PtyControlClientErrorKind::Connect);
        }
        other => panic!("expected control transport error, got {other}"),
    }
}

#[test]
fn lost_duplicate_delayed_and_stale_pokes_converge_with_bounded_session_local_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let state_path = dir.path().join("state.db");
    let mailbox_path = dir.path().join("pid-identity.db");
    let mut mailbox = MailboxDb::open(&mailbox_path).unwrap();
    let first = enqueue(&mut mailbox, "session-a", "a-first");
    let _other = enqueue(&mut mailbox, "session-b", "b-only");
    let second = enqueue(&mut mailbox, "session-a", "a-second");
    let fence = owner(1, "current");
    let (owner, turns) = start_owner(&state_path, fence.clone(), 8);
    let mut ingress = SessionMailboxIngress::new(
        "session-a",
        None,
        fence.clone(),
        2,
        MailboxDb::open(&mailbox_path).unwrap(),
        StateDb::open(&state_path).unwrap(),
        map_notification,
    )
    .unwrap();

    let fallback = ingress.fallback_read(&owner, 10).unwrap();
    assert_eq!(fallback.accepted_sequences, vec![first.seq, second.seq]);
    let active = receive_turn(&turns);
    assert_eq!(active.notification.input.handle, "a-first");
    assert_eq!(
        active.notification.input.owner_invocation_uuid.as_deref(),
        Some("nested-owner-session-a")
    );
    assert!(matches!(turns.try_recv(), Err(TryRecvError::Empty)));

    let poke = HeadlessResumePoke {
        session_id: "session-a".to_owned(),
        supervisor_generation: fence.generation,
        lease_token: fence.token.clone(),
    };
    let control_payload = serde_json::to_string(&poke).unwrap();
    assert!(
        ingress
            .handle_control_payload(&control_payload, &owner, 11)
            .unwrap()
            .accepted_sequences
            .is_empty()
    );
    active.completion.complete("done", 12).unwrap();
    let queued = receive_turn(&turns);
    assert_eq!(queued.notification.input.handle, "a-second");
    queued.completion.complete("done", 13).unwrap();

    let delayed = enqueue(&mut mailbox, "session-a", "a-delayed");
    assert_eq!(
        ingress
            .fallback_read(&owner, 14)
            .unwrap()
            .accepted_sequences,
        vec![delayed.seq]
    );
    receive_turn(&turns)
        .completion
        .complete("done", 15)
        .unwrap();
    assert!(
        ingress
            .handle_poke(&poke, &owner, 16)
            .unwrap()
            .accepted_sequences
            .is_empty()
    );

    assert!(matches!(
        ingress.handle_poke(
            &HeadlessResumePoke {
                supervisor_generation: fence.generation - 1,
                ..poke.clone()
            },
            &owner,
            17,
        ),
        Err(SessionIngressError::StalePoke)
    ));
    assert!(matches!(
        ingress.handle_poke(
            &HeadlessResumePoke {
                lease_token: "wrong-owner".to_owned(),
                ..poke.clone()
            },
            &owner,
            17,
        ),
        Err(SessionIngressError::StalePoke)
    ));
    assert!(matches!(
        ingress.handle_poke(
            &HeadlessResumePoke {
                session_id: "session-b".to_owned(),
                ..poke
            },
            &owner,
            17,
        ),
        Err(SessionIngressError::StalePoke)
    ));
    let state = StateDb::open(&state_path).unwrap();
    assert_eq!(
        state.external_ingress_cursor("session-a").unwrap(),
        delayed.seq
    );
    assert_eq!(state.external_ingress_cursor("session-b").unwrap(), 0);
    owner.close(18).unwrap();
}

#[test]
fn chain_targeted_input_is_bound_to_the_exact_recipient_session() {
    let dir = tempfile::tempdir().unwrap();
    let state_path = dir.path().join("state.db");
    let mailbox_path = dir.path().join("pid-identity.db");
    let mut mailbox = MailboxDb::open(&mailbox_path).unwrap();
    let row = enqueue_chain_input(&mut mailbox, "chain-a", "chain-token");
    assert_eq!(row.session_id, "chain-a");

    let fence = owner(1, "chain-target");
    let (owner, turns) = start_owner(&state_path, fence.clone(), 4);
    let mut ingress = SessionMailboxIngress::new(
        "session-a",
        Some("chain-a".to_owned()),
        fence,
        4,
        mailbox,
        StateDb::open(&state_path).unwrap(),
        map_notification,
    )
    .unwrap();

    assert_eq!(
        ingress
            .fallback_read(&owner, 10)
            .unwrap()
            .accepted_sequences,
        vec![row.seq]
    );
    let accepted = receive_turn(&turns);
    assert_eq!(accepted.session_id, "session-a");
    assert_eq!(accepted.turn.session_id.as_deref(), Some("session-a"));
    assert_eq!(
        accepted.notification.input.target_id.as_deref(),
        Some("chain-a")
    );
    accepted.completion.complete("done", 11).unwrap();
    assert!(
        StateDb::open(&state_path)
            .unwrap()
            .acknowledgement(&format!("mailbox:session-a:{}", row.seq))
            .unwrap()
            .is_some()
    );
    owner.close(12).unwrap();
}

#[test]
fn crash_windows_leave_unpersisted_persisted_and_accepted_work_recoverable() {
    let dir = tempfile::tempdir().unwrap();
    let state_path = dir.path().join("state.db");
    let mailbox_path = dir.path().join("pid-identity.db");
    let first_fence = owner(1, "before-persistence");
    let (first_owner, _turns) = start_owner(&state_path, first_fence.clone(), 8);
    let mut empty_ingress = SessionMailboxIngress::new(
        "session-a",
        None,
        first_fence,
        4,
        MailboxDb::open(&mailbox_path).unwrap(),
        StateDb::open(&state_path).unwrap(),
        map_notification,
    )
    .unwrap();
    assert!(
        empty_ingress
            .fallback_read(&first_owner, 1)
            .unwrap()
            .accepted_sequences
            .is_empty()
    );
    assert_eq!(
        StateDb::open(&state_path)
            .unwrap()
            .external_ingress_cursor("session-a")
            .unwrap(),
        0
    );
    first_owner.close(2).unwrap();

    let mut mailbox = MailboxDb::open(&mailbox_path).unwrap();
    let row = enqueue(&mut mailbox, "session-a", "persisted-before-acceptance");
    assert_eq!(
        StateDb::open(&state_path)
            .unwrap()
            .external_ingress_cursor("session-a")
            .unwrap(),
        0
    );
    assert!(
        StateDb::open(&state_path)
            .unwrap()
            .acknowledgement(&format!("mailbox:session-a:{}", row.seq))
            .unwrap()
            .is_none()
    );

    let stale_fence = owner(1, "accepted-before-memory");
    let mut state = StateDb::open(&state_path).unwrap();
    state
        .acquire_supervisor_lease("session-a", &stale_fence, 3)
        .unwrap();
    let durable = ExternalIngress {
        session_id: "session-a".to_owned(),
        sequence: row.seq,
        ingress_id: format!("mailbox:session-a:{}", row.seq),
        payload: serde_json::to_string(&row).unwrap(),
    };
    state
        .start_provider_turn(&ProviderTurnGeneration {
            session_id: None,
            ..turn("session-a", row.seq)
        })
        .unwrap();
    state
        .accept_external_ingress(
            &durable,
            &stale_fence,
            &turn("session-a", row.seq).generation_id,
            4,
        )
        .unwrap();
    state
        .release_supervisor_lease("session-a", &stale_fence)
        .unwrap();
    drop(state);

    let recovery_fence = owner(1, "recovered");
    let (recovered_owner, recovered_turns) = start_owner(&state_path, recovery_fence.clone(), 8);
    let mut recovered_ingress = SessionMailboxIngress::new(
        "session-a",
        None,
        recovery_fence,
        4,
        mailbox,
        StateDb::open(&state_path).unwrap(),
        map_notification,
    )
    .unwrap();
    let drain = recovered_ingress
        .fallback_read(&recovered_owner, 5)
        .unwrap();
    assert_eq!(drain.recovered_sequences, vec![row.seq]);
    assert!(drain.accepted_sequences.is_empty());
    let recovered = receive_turn(&recovered_turns);
    assert_eq!(
        recovered.notification.input.handle,
        "persisted-before-acceptance"
    );
    recovered.completion.complete("done", 6).unwrap();
    recovered_owner.close(7).unwrap();
}

#[test]
fn lifecycle_append_failure_rearms_accepted_pending_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let state_path = dir.path().join("state.db");
    let mailbox_path = dir.path().join("pid-identity.db");
    let mut mailbox = MailboxDb::open(&mailbox_path).unwrap();
    let row = enqueue(&mut mailbox, "session-a", "recover-after-append-failure");
    let fence = owner(1, "append-failure");
    let (owner, turns) = start_owner(&state_path, fence.clone(), 8);
    let mut ingress = SessionMailboxIngress::new(
        "session-a",
        None,
        fence,
        4,
        mailbox,
        StateDb::open(&state_path).unwrap(),
        map_notification,
    )
    .unwrap();
    let connection = rusqlite::Connection::open(&state_path).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER fail_lifecycle_append
             BEFORE INSERT ON session_lifecycle_events
             BEGIN
                 SELECT RAISE(FAIL, 'forced lifecycle append failure');
             END;",
        )
        .unwrap();

    assert!(matches!(
        ingress.fallback_read(&owner, 10),
        Err(SessionIngressError::Supervisor(_))
    ));
    assert_eq!(
        StateDb::open(&state_path)
            .unwrap()
            .external_ingress_cursor("session-a")
            .unwrap(),
        row.seq
    );
    connection
        .execute_batch("DROP TRIGGER fail_lifecycle_append;")
        .unwrap();

    let recovered = ingress.fallback_read(&owner, 11).unwrap();
    assert_eq!(recovered.recovered_sequences, vec![row.seq]);
    assert!(recovered.accepted_sequences.is_empty());
    receive_turn(&turns)
        .completion
        .complete("done", 12)
        .unwrap();
    owner.close(13).unwrap();
}

#[test]
fn payload_verification_failure_is_retired_without_starving_later_rows() {
    let dir = tempfile::tempdir().unwrap();
    let state_path = dir.path().join("state.db");
    let mailbox_path = dir.path().join("pid-identity.db");
    let mut mailbox = MailboxDb::open(&mailbox_path).unwrap();
    let invalid = enqueue(&mut mailbox, "session-a", "invalid");
    let valid = enqueue(&mut mailbox, "session-a", "valid");
    drop(mailbox);
    let connection = rusqlite::Connection::open(&mailbox_path).unwrap();
    connection
        .execute(
            "UPDATE mailbox SET payload_file_path = 'incomplete' WHERE seq = ?1",
            [invalid.seq],
        )
        .unwrap();
    drop(connection);
    let fence = owner(1, "payload-verification");
    let (owner, turns) = start_owner(&state_path, fence.clone(), 1);
    let mut ingress = SessionMailboxIngress::new(
        "session-a",
        None,
        fence,
        1,
        MailboxDb::open(&mailbox_path).unwrap(),
        StateDb::open(&state_path).unwrap(),
        map_notification,
    )
    .unwrap();

    assert!(
        ingress
            .fallback_read(&owner, 10)
            .unwrap()
            .accepted_sequences
            .is_empty()
    );
    assert_eq!(
        ingress
            .fallback_read(&owner, 11)
            .unwrap()
            .accepted_sequences,
        vec![valid.seq]
    );
    let failed = MailboxDb::open(&mailbox_path)
        .unwrap()
        .list_mailbox("session-a", true)
        .unwrap()
        .into_iter()
        .find(|row| row.seq == invalid.seq)
        .unwrap();
    assert_eq!(
        failed.delivery_error.as_deref(),
        Some(MAILBOX_PAYLOAD_VERIFICATION_FAILED_ERROR)
    );
    receive_turn(&turns)
        .completion
        .complete("done", 12)
        .unwrap();
    owner.close(13).unwrap();
}

#[test]
fn expired_ingress_is_retired_without_starving_a_later_row_at_batch_size_one() {
    let dir = tempfile::tempdir().unwrap();
    let state_path = dir.path().join("state.db");
    let mailbox_path = dir.path().join("pid-identity.db");
    let mut mailbox = MailboxDb::open(&mailbox_path).unwrap();
    let expired = enqueue(&mut mailbox, "session-a", "expired");
    let valid = enqueue(&mut mailbox, "session-a", "valid");
    let fence = owner(1, "expiry");
    let (owner, turns) = start_owner(&state_path, fence.clone(), 1);
    let mut ingress = SessionMailboxIngress::new(
        "session-a",
        None,
        fence,
        1,
        mailbox,
        StateDb::open(&state_path).unwrap(),
        map_retry_notification,
    )
    .unwrap();

    assert!(
        ingress
            .fallback_read(&owner, 10)
            .unwrap()
            .accepted_sequences
            .is_empty()
    );
    assert_eq!(
        ingress
            .fallback_read(&owner, 11)
            .unwrap()
            .accepted_sequences,
        vec![valid.seq]
    );
    let retired = MailboxDb::open(&mailbox_path)
        .unwrap()
        .list_mailbox("session-a", true)
        .unwrap()
        .into_iter()
        .find(|row| row.seq == expired.seq)
        .unwrap();
    assert_eq!(
        retired.delivery_error.as_deref(),
        Some(MAILBOX_INGRESS_EXPIRED_ERROR)
    );
    assert!(retired.delivered_at.is_none());
    assert_eq!(receive_turn(&turns).notification.sequence, valid.seq);
    owner.close(12).unwrap();
}

#[test]
fn pause_caps_and_no_overlap_are_preserved_at_the_adapter_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let state_path = dir.path().join("state.db");
    let mailbox_path = dir.path().join("pid-identity.db");
    let mut mailbox = MailboxDb::open(&mailbox_path).unwrap();
    let first = enqueue(&mut mailbox, "session-a", "first");
    let second = enqueue(&mut mailbox, "session-a", "second");
    let third = enqueue(&mut mailbox, "session-a", "third");
    mailbox.set_notifications_paused("session-a", true).unwrap();
    let fence = owner(1, "caps");
    let (owner, turns) = start_owner(&state_path, fence.clone(), 1);
    let mut ingress = SessionMailboxIngress::new(
        "session-a",
        None,
        fence,
        3,
        MailboxDb::open(&mailbox_path).unwrap(),
        StateDb::open(&state_path).unwrap(),
        map_notification,
    )
    .unwrap();
    assert!(ingress.fallback_read(&owner, 10).unwrap().paused);
    assert!(matches!(turns.try_recv(), Err(TryRecvError::Empty)));

    mailbox
        .set_notifications_paused("session-a", false)
        .unwrap();
    let drain = ingress.fallback_read(&owner, 11).unwrap();
    assert_eq!(drain.accepted_sequences, vec![first.seq, second.seq]);
    assert!(drain.queue_saturated);
    let active = receive_turn(&turns);
    assert_eq!(active.notification.sequence, first.seq);
    assert!(matches!(turns.try_recv(), Err(TryRecvError::Empty)));
    active.completion.complete("done", 12).unwrap();
    let queued = receive_turn(&turns);
    assert_eq!(queued.notification.sequence, second.seq);
    queued.completion.complete("done", 13).unwrap();

    let final_drain = ingress.fallback_read(&owner, 14).unwrap();
    assert_eq!(final_drain.accepted_sequences, vec![third.seq]);
    let final_turn = receive_turn(&turns);
    final_turn.completion.complete("done", 15).unwrap();
    owner.close(16).unwrap();
}

#[test]
fn cancellation_retry_close_and_expiry_remain_owner_policy_without_blocking_ingress() {
    let dir = tempfile::tempdir().unwrap();
    let state_path = dir.path().join("state.db");
    let mailbox_path = dir.path().join("pid-identity.db");
    let mut mailbox = MailboxDb::open(&mailbox_path).unwrap();
    let retry = enqueue(&mut mailbox, "session-a", "retry");
    let cancelled = enqueue(&mut mailbox, "session-a", "cancelled");
    let expired = enqueue(&mut mailbox, "session-a", "expired");
    let later = enqueue(&mut mailbox, "session-a", "later");
    let fence = owner(1, "policy");
    let (owner, turns) = start_owner_with_retries(&state_path, fence.clone(), 4, 1);
    let mut ingress = SessionMailboxIngress::new(
        "session-a",
        None,
        fence,
        2,
        mailbox,
        StateDb::open(&state_path).unwrap(),
        map_retry_notification,
    )
    .unwrap();

    assert_eq!(
        ingress.fallback_read(&owner, 5).unwrap().accepted_sequences,
        vec![retry.seq, cancelled.seq]
    );
    let first_attempt = receive_turn(&turns);
    owner.cancel(cancelled.seq, 6).unwrap();
    assert!(owner.status().unwrap().queued_sequences.is_empty());
    first_attempt.completion.abnormal_exit("retry", 7).unwrap();
    let retry_attempt = receive_turn(&turns);
    assert_eq!(retry_attempt.notification.sequence, retry.seq);
    assert!(matches!(turns.try_recv(), Err(TryRecvError::Empty)));
    retry_attempt.completion.complete("done", 8).unwrap();

    assert_eq!(
        ingress
            .fallback_read(&owner, 10)
            .unwrap()
            .accepted_sequences,
        vec![later.seq]
    );
    let later_turn = receive_turn(&turns);
    assert_eq!(later_turn.notification.sequence, later.seq);
    later_turn.completion.complete("done", 11).unwrap();
    assert_eq!(
        StateDb::open(&state_path)
            .unwrap()
            .external_ingress_cursor("session-a")
            .unwrap(),
        later.seq,
        "expired work does not block later durable ingress"
    );
    assert!(
        StateDb::open(&state_path)
            .unwrap()
            .acknowledgement(&format!("mailbox:session-a:{}", expired.seq))
            .unwrap()
            .is_none()
    );
    owner.close(12).unwrap();
}
