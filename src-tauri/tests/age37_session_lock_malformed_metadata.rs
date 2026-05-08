#![cfg(unix)]

use chrono::{Duration, Utc};
use oulipoly_runtime::session_lock::{self, LockError};
use serde_json::json;
use std::fs;
use std::os::unix::fs::PermissionsExt;

const SESSION_ID: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
const OTHER_SESSION_ID: &str = "8f0a6a1f-9cd2-4c91-b6c6-1f0a0a8c9e22";

/// Risk: AGE-37 lock service seams could flatten malformed lock metadata into
/// false/no-active instead of preserving the current operational error.
/// Level: component characterization.
/// Observable: any_active_for_session returns LockError::Operational when the
/// on-disk lock filename matches the queried session but metadata names a
/// different session id.
#[test]
fn any_active_for_session_mismatched_lock_metadata_is_operational() {
    let dir = tempfile::tempdir().unwrap();
    let lock_dir = dir.path().join("locks");
    fs::create_dir_all(&lock_dir).unwrap();
    fs::set_permissions(&lock_dir, fs::Permissions::from_mode(0o700)).unwrap();
    let lock_path = lock_dir.join(format!("session-{SESSION_ID}.lock"));
    fs::write(
        &lock_path,
        serde_json::to_vec(&json!({
            "version": 1,
            "session_id": OTHER_SESSION_ID,
            "provider_name": "claude",
            "token_hash": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "created_at": Utc::now().to_rfc3339(),
            "expires_at": (Utc::now() + Duration::minutes(5)).to_rfc3339(),
            "owner_pid": std::process::id(),
        }))
        .unwrap(),
    )
    .unwrap();
    fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o600)).unwrap();

    match session_lock::any_active_for_session(&lock_dir, SESSION_ID) {
        Err(LockError::Operational { message }) => {
            assert_eq!(
                message,
                format!(
                    "lock metadata session mismatch: expected {SESSION_ID}, got {OTHER_SESSION_ID}"
                )
            );
        }
        other => panic!("expected mismatched metadata to be operational, got {other:?}"),
    }
}
