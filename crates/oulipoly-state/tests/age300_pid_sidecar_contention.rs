use oulipoly_state::mailbox::{CompletionEventRegistrationInput, MailboxDb};
use oulipoly_state::pid_identity::PidIdentityDb;
use rusqlite::Connection;
use std::sync::{Arc, Barrier, mpsc};
use std::time::{Duration, Instant};

const EXPECTED_BUSY_TIMEOUT_MS: i64 = 5_000;

fn busy_timeout_ms(conn: &Connection) -> i64 {
    conn.query_row("PRAGMA busy_timeout", [], |row| row.get(0))
        .unwrap()
}

fn completion_registration<'a>(event_id: &'a str) -> CompletionEventRegistrationInput<'a> {
    CompletionEventRegistrationInput {
        event_id,
        delivery_mode: "async",
        owner_session_id: Some("session-age-300"),
        owner_invocation_uuid: Some("11111111-1111-4111-8111-111111111111"),
        state_dir: "/tmp/age-300",
        meta_path: "/tmp/age-300/meta.json",
        log_path: "/tmp/age-300/log",
        rc_path: "/tmp/age-300/rc",
    }
}

#[test]
fn age_300_writable_pid_sidecar_opens_share_the_bounded_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pid-identity.db");

    let pid_db = PidIdentityDb::open(&path).unwrap();
    assert_eq!(
        busy_timeout_ms(pid_db.connection()),
        EXPECTED_BUSY_TIMEOUT_MS
    );
    drop(pid_db);

    let mailbox = MailboxDb::open(&path).unwrap();
    assert_eq!(
        busy_timeout_ms(mailbox.connection()),
        EXPECTED_BUSY_TIMEOUT_MS
    );
}

#[test]
fn age_300_mailbox_open_waits_for_pre_schema_writer_contention() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pid-identity.db");
    let writer = Connection::open(&path).unwrap();
    writer.execute_batch("BEGIN EXCLUSIVE;").unwrap();
    let start = Arc::new(Barrier::new(2));
    let worker_start = Arc::clone(&start);
    let (result_tx, result_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        worker_start.wait();
        result_tx.send(MailboxDb::open(&path)).unwrap();
    });

    start.wait();
    let early_result = match result_rx.recv_timeout(Duration::from_millis(250)) {
        Err(mpsc::RecvTimeoutError::Timeout) => None,
        Err(err) => panic!("mailbox open result channel failed: {err}"),
        Ok(result) => Some(result),
    };
    writer.execute_batch("COMMIT;").unwrap();
    if let Some(result) = early_result {
        let outcome = result
            .map(|_| "success".to_string())
            .unwrap_or_else(|err| err);
        panic!("mailbox open completed before the writer released its lock: {outcome}");
    }
    result_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    worker.join().unwrap();
}

#[test]
fn age_300_completion_registration_waits_for_normal_writer_contention() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pid-identity.db");
    let mut mailbox = MailboxDb::open(&path).unwrap();
    let writer = Connection::open(&path).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE;").unwrap();
    let start = Arc::new(Barrier::new(2));
    let worker_start = Arc::clone(&start);
    let (result_tx, result_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        worker_start.wait();
        result_tx
            .send(mailbox.register_completion_event(completion_registration("ab_age_300_bounded")))
            .unwrap();
    });

    start.wait();
    let early_result = match result_rx.recv_timeout(Duration::from_millis(250)) {
        Err(mpsc::RecvTimeoutError::Timeout) => None,
        Err(err) => panic!("completion registration result channel failed: {err}"),
        Ok(result) => Some(result),
    };
    writer.execute_batch("COMMIT;").unwrap();
    if let Some(result) = early_result {
        panic!("completion registration finished before lock release: {result:?}");
    }
    let result = result_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert!(result.inserted);
    assert_eq!(result.listeners.len(), 1);
    worker.join().unwrap();
}

#[test]
fn age_300_over_bound_registration_fails_closed_without_completion_state() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pid-identity.db");
    let mut mailbox = MailboxDb::open(&path).unwrap();
    let writer = Connection::open(&path).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE;").unwrap();

    let started = Instant::now();
    let err = mailbox
        .register_completion_event(completion_registration("ab_age_300_over_bound"))
        .unwrap_err();
    let elapsed = started.elapsed();

    assert_eq!(
        err,
        "Failed to start completion event registration transaction: database is locked"
    );
    assert!(
        elapsed >= Duration::from_millis(4_500),
        "registration failed before the configured contention wait: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(15),
        "registration exceeded its bounded contention wait: {elapsed:?}"
    );
    assert!(
        mailbox
            .completion_event("ab_age_300_over_bound")
            .unwrap()
            .is_none()
    );
    assert!(
        mailbox
            .completion_event_listeners("ab_age_300_over_bound")
            .unwrap()
            .is_empty()
    );

    writer.execute_batch("COMMIT;").unwrap();
    let inserted = mailbox
        .register_completion_event(completion_registration("ab_age_300_over_bound"))
        .unwrap();
    assert!(inserted.inserted);
    assert_eq!(inserted.listeners.len(), 1);
    let replay = mailbox
        .register_completion_event(completion_registration("ab_age_300_over_bound"))
        .unwrap();
    assert!(!replay.inserted);
    assert_eq!(replay.listeners.len(), 1);
}
