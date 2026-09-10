//! ## Declared roles
//!
//! `accessor`

use oulipoly_state::StateDb;
use std::path::Path;
use std::time::Duration;

pub(super) fn open_default_state_read_only_with_timeout_and_cancel(
    timeout: Duration,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<Option<StateDb>, String> {
    let path = StateDb::default_path()?;
    open_state_read_only_at_with_retry_and_work_timeout(&path, timeout, timeout, is_cancelled)
}

#[cfg(test)]
pub(super) fn open_state_read_only_at(path: &Path) -> Result<Option<StateDb>, String> {
    open_state_read_only_at_with_cancel(path, &|| false)
}

#[cfg(test)]
fn open_state_read_only_at_with_timeout(
    path: &Path,
    timeout: Duration,
) -> Result<Option<StateDb>, String> {
    open_state_read_only_at_with_retry_timeout(path, timeout, &|| false)
}

#[cfg(test)]
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

#[cfg(test)]
fn open_state_read_only_at_with_retry_timeout(
    path: &Path,
    retry_timeout: Duration,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<Option<StateDb>, String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Failed to inspect State path: {error}")),
    }
    StateDb::open_read_only_with_retry_timeout_and_cancel(path, retry_timeout, is_cancelled)
        .map(Some)
        .map_err(|error| format!("Failed to open State read-only for wake sweep: {error:?}"))
}

fn open_state_read_only_at_with_retry_and_work_timeout(
    path: &Path,
    retry_timeout: Duration,
    work_timeout: Duration,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<Option<StateDb>, String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Failed to inspect State path: {error}")),
    }
    StateDb::open_read_only_with_retry_and_work_timeout_and_cancel(
        path,
        retry_timeout,
        work_timeout,
        is_cancelled,
    )
    .map(Some)
    .map_err(|error| format!("Failed to open State read-only for wake sweep: {error:?}"))
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

    #[test]
    fn stable_first_state_snapshot_ignores_the_retry_timeout() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        drop(StateDb::open(&state_path).unwrap());

        assert!(
            open_state_read_only_at_with_timeout(&state_path, Duration::ZERO)
                .unwrap()
                .is_some(),
            "the retry budget must begin only after the first source mismatch"
        );
    }

    #[test]
    fn wake_snapshot_total_work_budget_is_an_unavailable_observation() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        drop(StateDb::open(&state_path).unwrap());
        let started = std::time::Instant::now();

        let error = match open_state_read_only_at_with_retry_and_work_timeout(
            &state_path,
            Duration::from_secs(5),
            Duration::ZERO,
            &|| false,
        ) {
            Ok(_) => panic!("a wake snapshot must honor its total work budget"),
            Err(error) => error,
        };

        assert!(error.contains("total work budget"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(1));
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
