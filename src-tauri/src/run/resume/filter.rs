//! filter

pub(super) fn first_attempt_manual_migrate(
    attempts: usize,
    manual_migrate: Option<&str>,
) -> Option<&str> {
    if attempts == 1 { manual_migrate } else { None }
}

pub(super) fn resumed_session_target<'a>(
    _manual_migrate: Option<&str>,
    _session_id: &'a str,
    active_session_id: &'a str,
) -> &'a str {
    // The original resume input can identify a closed segment after rotation.
    active_session_id
}
