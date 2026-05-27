//! filter

pub(super) fn first_attempt_manual_migrate(
    attempts: usize,
    manual_migrate: Option<&str>,
) -> Option<&str> {
    if attempts == 1 { manual_migrate } else { None }
}

pub(super) fn resumed_session_target<'a>(
    manual_migrate: Option<&str>,
    session_id: &'a str,
    active_session_id: &'a str,
) -> &'a str {
    if manual_migrate.is_some() {
        session_id
    } else {
        active_session_id
    }
}
