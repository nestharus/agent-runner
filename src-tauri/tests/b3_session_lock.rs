#![cfg(unix)]

use agent_runner_lib::session_lock::{
    FilesystemSessionLockProvider, LockError, SessionLockProvider,
};
use std::time::Duration;

const SESSION_ID: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
const PROVIDER: &str = "claude";

/// Risk: T11 (session lock provider preserves acquire/active/release semantics)
/// Source: proposal §8 T11; B3 contract §2 SessionLockProvider
/// Level: component
/// Fixture source: tempfile lock directory
#[test]
fn session_lock_provider_acquire_active_release_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let lock_dir = dir.path().join("locks");
    let provider = FilesystemSessionLockProvider::default();

    let lease = provider
        .acquire(&lock_dir, SESSION_ID, PROVIDER, Duration::from_secs(60))
        .unwrap();

    assert!(
        provider
            .any_active_for_session(&lock_dir, SESSION_ID)
            .unwrap()
    );
    let receipt = provider
        .release(&lock_dir, SESSION_ID, &lease.token)
        .unwrap();
    assert_eq!(receipt.session_id, SESSION_ID);
    assert!(
        !provider
            .any_active_for_session(&lock_dir, SESSION_ID)
            .unwrap()
    );
}

/// Risk: T11/T10 (session lock provider exposes busy leases without changing lock metadata semantics)
/// Source: proposal §8 T10/T11; B3 contract §6 session replace lock-held edge
/// Level: component
/// Fixture source: tempfile lock directory
#[test]
fn session_lock_provider_reports_busy_for_live_second_acquire() {
    let dir = tempfile::tempdir().unwrap();
    let lock_dir = dir.path().join("locks");
    let provider = FilesystemSessionLockProvider::default();
    let _lease = provider
        .acquire(&lock_dir, SESSION_ID, PROVIDER, Duration::from_secs(60))
        .unwrap();

    let err = provider
        .acquire(&lock_dir, SESSION_ID, PROVIDER, Duration::from_secs(60))
        .unwrap_err();

    match err {
        LockError::Busy { token_hash, .. } => {
            assert_eq!(token_hash.as_deref().unwrap_or("").len(), 64)
        }
        other => panic!("expected busy lock error, got {other:?}"),
    }
}
