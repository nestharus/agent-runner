#![cfg(unix)]

use agent_runner_session::{FilesystemSessionLockProvider, SessionLockProvider};
use std::time::Duration;

/// Risk: T11 - adding `SessionLockProvider` can drift from filesystem lock
/// token/active/release semantics if it wraps only part of `SessionLock`.
/// Level: component.
/// Source: proposal §8 T11; B3 contract §2 `SessionLockProvider`.
/// Observable: acquire marks the session active through the provider, release
/// clears it, and the release receipt uses the acquired token.
#[test]
fn filesystem_session_lock_provider_preserves_acquire_active_release_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let lock_dir = dir.path().join("locks");
    let provider = FilesystemSessionLockProvider;
    let provider: &dyn SessionLockProvider = &provider;

    let lease = provider
        .acquire(
            &lock_dir,
            "5169694d-de0f-40d1-890c-6e28e55bab27",
            "claude",
            Duration::from_secs(60),
        )
        .unwrap();

    assert!(
        provider
            .any_active_for_session(&lock_dir, "5169694d-de0f-40d1-890c-6e28e55bab27")
            .unwrap()
    );
    let receipt = provider
        .release(
            &lock_dir,
            "5169694d-de0f-40d1-890c-6e28e55bab27",
            &lease.token,
        )
        .unwrap();
    assert_eq!(receipt.token, lease.token);
    assert!(
        !provider
            .any_active_for_session(&lock_dir, "5169694d-de0f-40d1-890c-6e28e55bab27")
            .unwrap()
    );
}
