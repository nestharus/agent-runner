//! ## Declared roles
//!
//! `accessor`, `filter`, `formatter`, `mapper`, `predicate`

use oulipoly_state::StateDb;
use oulipoly_state::mailbox::{MailboxDb, MailboxRow};

pub(super) fn pending_mailbox_consumed_marker_present(
    db: &MailboxDb,
    state: Option<&StateDb>,
    session_id: &str,
) -> Result<bool, String> {
    let pending = pending_mailbox_rows(db, session_id)?;
    if pending.is_empty() {
        return Ok(true);
    }
    let _ = state;
    // Pre-anchor transcript markers can predate these rows, so they cannot
    // settle pending work without bounded evidence captured after delivery.
    Ok(false)
}

fn pending_mailbox_rows(db: &MailboxDb, session_id: &str) -> Result<Vec<MailboxRow>, String> {
    db.list_pending(session_id)
}
