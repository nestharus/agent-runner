use oulipoly_runtime::session_metadata::{SessionOwnership, resolve_session_ownership};

#[test]
fn ownership_probe_is_available_through_the_public_session_metadata_api() {
    assert!(matches!(
        resolve_session_ownership(None, "ses_public_api"),
        SessionOwnership::Indeterminate(reason) if reason == "session_storage_missing"
    ));
}
