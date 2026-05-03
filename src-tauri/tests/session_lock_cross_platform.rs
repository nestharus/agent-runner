use agent_runner_lib::session_lock::{LockError, SessionLock};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};

const SESSION_ID: &str = "11111111-1111-4111-8111-111111111111";
const PROVIDER_NAME: &str = "codex";
const HELPER_TEST_NAME: &str = "session_lock_cross_process_helper";
const WAIT_TIMEOUT: Duration = Duration::from_secs(30);

#[test]
fn test_single_process_busy() {
    let dir = tempfile::tempdir().unwrap();
    let lock = SessionLock::new(dir.path()).unwrap();

    let first = lock
        .acquire(SESSION_ID, PROVIDER_NAME, Duration::from_secs(60))
        .unwrap();

    match lock.acquire(SESSION_ID, PROVIDER_NAME, Duration::from_secs(60)) {
        Err(LockError::Busy {
            expires_at,
            token_hash,
        }) => {
            assert!(!expires_at.is_empty(), "busy error must expose expires_at");
            assert!(
                token_hash.is_some(),
                "busy error must expose a token hash for live leases"
            );
        }
        other => panic!("second acquire must return Busy, got {other:?}"),
    }

    let released = lock.release(SESSION_ID, &first.token).unwrap();
    assert!(
        !released.already_released,
        "first release must not be marked as replay"
    );

    let third = lock
        .acquire(SESSION_ID, PROVIDER_NAME, Duration::from_secs(60))
        .unwrap();
    assert_eq!(third.session_id, SESSION_ID);
    lock.release(SESSION_ID, &third.token).unwrap();
}

#[test]
fn test_release_idempotency() {
    let dir = tempfile::tempdir().unwrap();
    let lock = SessionLock::new(dir.path()).unwrap();
    let lease = lock
        .acquire(SESSION_ID, PROVIDER_NAME, Duration::from_secs(60))
        .unwrap();

    let first = lock.release(SESSION_ID, &lease.token).unwrap();
    assert!(
        !first.already_released,
        "first release must not be marked as replay"
    );

    let second = lock.release(SESSION_ID, &lease.token).unwrap();
    assert!(
        second.already_released,
        "second release with the same token must be idempotent"
    );
    assert_eq!(second.session_id, SESSION_ID);
    assert_eq!(second.token, lease.token);

    let bad_token = different_valid_token(&lease.token);
    match lock.release(SESSION_ID, &bad_token) {
        Err(LockError::TokenInvalid) => {}
        other => panic!("release with a bad token must return TokenInvalid, got {other:?}"),
    }
}

#[test]
fn test_cross_process_exclusivity() {
    let dir = tempfile::tempdir().unwrap();
    let ready_path = dir.path().join("helper.ready");
    let release_path = dir.path().join("helper.release");

    let mut child = spawn_helper(dir.path(), &ready_path, &release_path);
    wait_for_path(&ready_path, WAIT_TIMEOUT, "helper ready sentinel");

    let parent_lock = SessionLock::new(dir.path()).unwrap();
    match parent_lock.acquire(SESSION_ID, PROVIDER_NAME, Duration::from_secs(60)) {
        Err(LockError::Busy {
            expires_at,
            token_hash,
        }) => {
            assert!(
                !expires_at.is_empty(),
                "cross-process Busy must expose expires_at"
            );
            assert!(
                token_hash.is_some(),
                "cross-process Busy must preserve the live token hash"
            );
        }
        other => panic!("parent acquire while helper holds lease must return Busy, got {other:?}"),
    }

    fs::write(&release_path, b"release").unwrap();
    let status = wait_for_child(&mut child, WAIT_TIMEOUT);
    assert!(
        status.success(),
        "helper process must exit successfully, got {status:?}"
    );

    let lease = parent_lock
        .acquire(SESSION_ID, PROVIDER_NAME, Duration::from_secs(60))
        .unwrap();
    assert_eq!(lease.session_id, SESSION_ID);
    parent_lock.release(SESSION_ID, &lease.token).unwrap();
}

#[test]
fn test_acquire_after_release_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let lock = SessionLock::new(dir.path()).unwrap();

    let first = lock
        .acquire(SESSION_ID, PROVIDER_NAME, Duration::from_secs(60))
        .unwrap();
    let released = lock.release(SESSION_ID, &first.token).unwrap();
    assert!(
        !released.already_released,
        "first release must remove the live lease"
    );

    let second = lock
        .acquire(SESSION_ID, PROVIDER_NAME, Duration::from_secs(60))
        .unwrap();
    assert_eq!(second.session_id, SESSION_ID);
    assert_eq!(second.provider_name, PROVIDER_NAME);
    lock.release(SESSION_ID, &second.token).unwrap();
}

#[test]
#[ignore = "spawned by test_cross_process_exclusivity"]
fn session_lock_cross_process_helper() {
    if env::var_os("SESSION_LOCK_HELPER").is_none() {
        return;
    }

    let lock_dir = env_path("SESSION_LOCK_HELPER_LOCK_DIR");
    let ready_path = env_path("SESSION_LOCK_HELPER_READY_PATH");
    let release_path = env_path("SESSION_LOCK_HELPER_RELEASE_PATH");
    let session_id = env::var("SESSION_LOCK_HELPER_SESSION_ID").unwrap();
    let provider_name = env::var("SESSION_LOCK_HELPER_PROVIDER_NAME").unwrap();

    let lock = SessionLock::new(&lock_dir).unwrap();
    let lease = lock
        .acquire(&session_id, &provider_name, Duration::from_secs(60))
        .unwrap();
    fs::write(&ready_path, b"ready").unwrap();
    wait_for_path(&release_path, WAIT_TIMEOUT, "helper release sentinel");
    let receipt = lock.release(&session_id, &lease.token).unwrap();
    assert!(
        !receipt.already_released,
        "helper owns the first release for its lease"
    );
}

fn spawn_helper(lock_dir: &Path, ready_path: &Path, release_path: &Path) -> Child {
    Command::new(env::current_exe().unwrap())
        .arg(HELPER_TEST_NAME)
        .arg("--exact")
        .arg("--ignored")
        .arg("--nocapture")
        .env("SESSION_LOCK_HELPER", "1")
        .env("SESSION_LOCK_HELPER_LOCK_DIR", lock_dir)
        .env("SESSION_LOCK_HELPER_READY_PATH", ready_path)
        .env("SESSION_LOCK_HELPER_RELEASE_PATH", release_path)
        .env("SESSION_LOCK_HELPER_SESSION_ID", SESSION_ID)
        .env("SESSION_LOCK_HELPER_PROVIDER_NAME", PROVIDER_NAME)
        .spawn()
        .unwrap()
}

fn wait_for_path(path: &Path, timeout: Duration, label: &str) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
    panic!("timed out waiting for {label}: {}", path.display());
}

fn wait_for_child(child: &mut Child, timeout: Duration) -> ExitStatus {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        thread::sleep(Duration::from_millis(25));
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("timed out waiting for helper process");
}

fn env_path(name: &str) -> PathBuf {
    PathBuf::from(env::var_os(name).unwrap())
}

fn different_valid_token(token: &str) -> String {
    let mut bytes = token.as_bytes().to_vec();
    let first_hex = "pause_".len();
    bytes[first_hex] = if bytes[first_hex] == b'0' { b'1' } else { b'0' };
    String::from_utf8(bytes).unwrap()
}
