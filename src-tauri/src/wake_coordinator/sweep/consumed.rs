//! ## Declared roles
//!
//! `accessor`, `filter`, `formatter`, `mapper`, `predicate`

use oulipoly_state::StateDb;
use oulipoly_state::mailbox::{MailboxDb, MailboxRow};

use super::state::{open_default_state_read_only, pending_mailbox_provider_name};
use crate::wake_coordinator::constants::CONSUMED_NOTIFICATION_MARKER;

pub(super) fn pending_mailbox_consumed_marker_present(db: &MailboxDb, session_id: &str) -> bool {
    let Ok(pending) = pending_mailbox_rows(db, session_id) else {
        return false;
    };
    if pending.is_empty() {
        return true;
    }
    let Some(context) = consumed_marker_context(db, session_id) else {
        return false;
    };
    pending_rows_have_consumed_markers(&context.state, &context.provider_name, session_id, &pending)
}

struct ConsumedMarkerContext {
    provider_name: String,
    state: StateDb,
}

fn consumed_marker_context(db: &MailboxDb, session_id: &str) -> Option<ConsumedMarkerContext> {
    Some(ConsumedMarkerContext {
        provider_name: pending_mailbox_provider_name(db, session_id)?,
        state: open_default_state_read_only()?,
    })
}

fn pending_rows_have_consumed_markers(
    state: &StateDb,
    provider_name: &str,
    session_id: &str,
    pending: &[MailboxRow],
) -> bool {
    session_has_consumed_notification_marker(state, provider_name, session_id)
        && pending.iter().all(|row| {
            state
                .has_session_user_turn_containing(
                    provider_name,
                    session_id,
                    &mailbox_handle_marker(row),
                )
                .unwrap_or(false)
        })
}

fn mailbox_handle_marker(row: &MailboxRow) -> String {
    format!("handle: {}", row.handle)
}

fn session_has_consumed_notification_marker(
    state: &StateDb,
    provider_name: &str,
    session_id: &str,
) -> bool {
    state
        .has_session_user_turn_containing(provider_name, session_id, CONSUMED_NOTIFICATION_MARKER)
        .unwrap_or(false)
}

fn pending_mailbox_rows(db: &MailboxDb, session_id: &str) -> Result<Vec<MailboxRow>, String> {
    db.list_pending(session_id)
}
