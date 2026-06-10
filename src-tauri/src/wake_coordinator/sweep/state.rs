//! ## Declared roles
//!
//! `accessor`

use oulipoly_state::StateDb;
use oulipoly_state::mailbox::MailboxDb;

pub(super) fn open_default_state_read_only() -> Option<StateDb> {
    let path = StateDb::default_path().ok()?;
    if !path.exists() {
        return None;
    }
    StateDb::open_read_only(&path).ok()
}

pub(super) fn pending_mailbox_provider_name(db: &MailboxDb, session_id: &str) -> Option<String> {
    db.session_runtime(session_id)
        .ok()
        .flatten()
        .and_then(|runtime| runtime.provider_name)
}
