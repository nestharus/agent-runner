//! ## Declared roles
//!
//! `accessor`

use oulipoly_state::StateDb;
use oulipoly_state::mailbox::MailboxDb;
use std::path::Path;

pub(super) fn open_default_state_read_only() -> Result<Option<StateDb>, String> {
    let path = StateDb::default_path()?;
    open_state_read_only_at(&path)
}

pub(super) fn open_state_read_only_at(path: &Path) -> Result<Option<StateDb>, String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Failed to inspect State path: {error}")),
    }
    StateDb::open_read_only(path)
        .map(Some)
        .map_err(|error| format!("Failed to open State read-only for wake sweep: {error:?}"))
}

pub(super) fn pending_mailbox_provider_name(
    db: &MailboxDb,
    session_id: &str,
) -> Result<Option<String>, String> {
    db.session_runtime(session_id)
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
