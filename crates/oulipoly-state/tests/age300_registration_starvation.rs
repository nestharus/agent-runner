use fs4::FileExt;
use oulipoly_state::mailbox::{CompletionEventRegistrationInput, MailboxDb};
use rusqlite::Connection;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier, mpsc};
use std::time::{Duration, Instant};

const REGISTRATIONS: usize = 6;

fn registration<'a>(event_id: &'a str) -> CompletionEventRegistrationInput<'a> {
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

fn registration_lock_path(db_path: &Path) -> PathBuf {
    db_path.with_extension("registration.lock")
}

fn locked_registration_file(db_path: &Path) -> File {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(registration_lock_path(db_path))
        .unwrap();
    FileExt::lock(&file).unwrap();
    file
}

#[test]
fn registration_burst_waits_behind_the_current_registration_writer() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pid-identity.db");
    drop(MailboxDb::open(&path).unwrap());
    let lock = locked_registration_file(&path);
    let start = Arc::new(Barrier::new(REGISTRATIONS + 1));
    let (result_tx, result_rx) = mpsc::channel();
    let mut workers = Vec::new();
    for index in 0..REGISTRATIONS {
        let path = path.clone();
        let start = Arc::clone(&start);
        let result_tx = result_tx.clone();
        workers.push(std::thread::spawn(move || {
            let event_id = format!("ab_age_300_burst_{index}");
            let mut mailbox = MailboxDb::open(&path).unwrap();
            start.wait();
            let result = mailbox.register_completion_event(registration(&event_id));
            result_tx.send((event_id, result)).unwrap();
        }));
    }
    drop(result_tx);

    start.wait();
    match result_rx.recv_timeout(Duration::from_millis(250)) {
        Err(mpsc::RecvTimeoutError::Timeout) => {}
        Err(err) => panic!("registration result channel failed: {err}"),
        Ok((event_id, result)) => {
            panic!("{event_id} bypassed the active registration writer: {result:?}")
        }
    }
    FileExt::unlock(&lock).unwrap();
    drop(lock);

    for _ in 0..REGISTRATIONS {
        let (event_id, result) = result_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let result = result.unwrap_or_else(|err| panic!("{event_id} failed: {err}"));
        assert!(result.inserted);
        assert_eq!(result.listeners.len(), 1);
    }
    for worker in workers {
        worker.join().unwrap();
    }
}

#[test]
fn registration_uses_only_the_remaining_shared_wait_budget_for_sqlite() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pid-identity.db");
    let mut mailbox = MailboxDb::open(&path).unwrap();
    let lock = locked_registration_file(&path);
    let writer = Connection::open(&path).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE;").unwrap();
    let (result_tx, result_rx) = mpsc::channel();
    let registration = std::thread::spawn(move || {
        let started = Instant::now();
        let result = mailbox.register_completion_event(registration("ab_age_300_shared_budget"));
        result_tx.send((started.elapsed(), result)).unwrap();
    });

    std::thread::sleep(Duration::from_millis(4_500));
    FileExt::unlock(&lock).unwrap();
    drop(lock);
    let (elapsed, result) = result_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    registration.join().unwrap();
    writer.execute_batch("COMMIT;").unwrap();

    assert_eq!(
        result.unwrap_err(),
        "Failed to start completion event registration transaction: database is locked"
    );
    assert!(elapsed >= Duration::from_millis(4_500), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(7), "{elapsed:?}");
}

#[test]
fn registration_lock_timeout_fails_closed_and_replay_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pid-identity.db");
    let mut mailbox = MailboxDb::open(&path).unwrap();
    let lock_path = registration_lock_path(&path);
    let lock = locked_registration_file(&path);

    let started = Instant::now();
    let err = mailbox
        .register_completion_event(registration("ab_age_300_over_bound"))
        .unwrap_err();
    let elapsed = started.elapsed();

    assert_eq!(
        err,
        format!(
            "Failed to acquire completion event registration lock {}: timed out after 5000ms",
            lock_path.display()
        )
    );
    assert!(elapsed >= Duration::from_millis(4_500), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(15), "{elapsed:?}");
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

    FileExt::unlock(&lock).unwrap();
    drop(lock);
    let inserted = mailbox
        .register_completion_event(registration("ab_age_300_over_bound"))
        .unwrap();
    assert!(inserted.inserted);
    assert_eq!(inserted.listeners.len(), 1);
    let replay = mailbox
        .register_completion_event(registration("ab_age_300_over_bound"))
        .unwrap();
    assert!(!replay.inserted);
    assert_eq!(replay.listeners.len(), 1);
}
