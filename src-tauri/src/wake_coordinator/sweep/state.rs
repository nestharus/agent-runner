//! ## Declared roles
//!
//! `accessor`

use oulipoly_state::StateDb;
use oulipoly_state::mailbox::MailboxDb;
use std::path::Path;
use std::time::{Duration, Instant};

pub(super) fn open_default_state_read_only_with_timeout(
    timeout: Duration,
) -> Result<Option<StateDb>, String> {
    let path = StateDb::default_path()?;
    open_state_read_only_at_with_timeout(&path, timeout)
}

#[cfg(test)]
pub(super) fn open_state_read_only_at(path: &Path) -> Result<Option<StateDb>, String> {
    open_state_read_only_at_with_cancel(path, &|| false)
}

fn open_state_read_only_at_with_timeout(
    path: &Path,
    timeout: Duration,
) -> Result<Option<StateDb>, String> {
    let started = Instant::now();
    open_state_read_only_at_with_cancel(path, &|| started.elapsed() >= timeout)
}

fn open_state_read_only_at_with_cancel(
    path: &Path,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<Option<StateDb>, String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Failed to inspect State path: {error}")),
    }
    StateDb::open_read_only_with_cancel(path, is_cancelled)
        .map(Some)
        .map_err(|error| format!("Failed to open State read-only for wake sweep: {error:?}"))
}

pub(super) fn pending_mailbox_provider_name(
    db: &MailboxDb,
    session_id: &str,
) -> Result<Option<String>, String> {
    db.wake_session_reader()
        .session_metadata(session_id)
        .map(|runtime| runtime.and_then(|runtime| runtime.provider_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_state_is_distinct_from_unavailable_state() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing.db");
        assert!(open_state_read_only_at(&missing).unwrap().is_none());

        let invalid = directory.path().join("invalid.db");
        std::fs::write(&invalid, "not a SQLite database").unwrap();
        let error = match open_state_read_only_at(&invalid) {
            Ok(_) => panic!("invalid State must remain an unavailable observation"),
            Err(error) => error,
        };
        assert!(error.contains("Failed to open State read-only for wake sweep"));
    }

    #[test]
    fn timed_out_state_snapshot_leaves_a_later_retry_available() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        drop(StateDb::open(&state_path).unwrap());
        let connection = rusqlite::Connection::open(&state_path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE wake_snapshot_capacity_fixture (payload BLOB NOT NULL);
                 INSERT INTO wake_snapshot_capacity_fixture VALUES (zeroblob(2097152));",
            )
            .unwrap();
        drop(connection);
        let cancellation_checks = std::cell::Cell::new(0);

        let error = match open_state_read_only_at_with_cancel(&state_path, &|| {
            let checks = cancellation_checks.get() + 1;
            cancellation_checks.set(checks);
            checks >= 4
        }) {
            Ok(_) => panic!("a cancelled large stable wake snapshot must not complete"),
            Err(error) => error,
        };
        assert!(
            error.contains("cancelled"),
            "unexpected timeout error: {error}"
        );
        assert!(cancellation_checks.get() >= 4);

        assert!(
            open_state_read_only_at_with_timeout(&state_path, Duration::from_secs(5))
                .unwrap()
                .is_some(),
            "a timed-out sweep must not consume later retry authority"
        );
    }

    #[cfg(unix)]
    #[test]
    fn dangling_state_symlink_is_unavailable_not_absent() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let missing_target = directory.path().join("temporarily-missing.db");
        let state_link = directory.path().join("state.db");
        symlink(&missing_target, &state_link).unwrap();

        let error = match open_state_read_only_at(&state_link) {
            Ok(_) => panic!("a configured dangling State identity must remain unavailable"),
            Err(error) => error,
        };

        assert!(error.contains("Failed to open State read-only for wake sweep"));
    }
}
