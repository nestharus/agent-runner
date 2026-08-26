//! ## Declared roles
//!
//! `accessor`, `filter`, `formatter`, `mapper`, `predicate`

use oulipoly_state::StateDb;
use oulipoly_state::mailbox::{MailboxDb, MailboxRow};

use super::state::pending_mailbox_provider_name;
use crate::wake_coordinator::constants::CONSUMED_NOTIFICATION_MARKER;

pub(super) fn pending_mailbox_consumed_marker_present(
    db: &MailboxDb,
    state: Option<&StateDb>,
    session_id: &str,
) -> Result<bool, String> {
    let pending = pending_mailbox_rows(db, session_id)?;
    if pending.is_empty() {
        return Ok(true);
    }
    let Some(context) = consumed_marker_context(db, state, session_id)? else {
        return Ok(false);
    };
    pending_rows_have_consumed_markers(context.state, &context.provider_name, session_id, &pending)
}

struct ConsumedMarkerContext<'a> {
    provider_name: String,
    state: &'a StateDb,
}

fn consumed_marker_context<'a>(
    db: &MailboxDb,
    state: Option<&'a StateDb>,
    session_id: &str,
) -> Result<Option<ConsumedMarkerContext<'a>>, String> {
    let Some(state) = state else {
        return Ok(None);
    };
    let Some(provider_name) = pending_mailbox_provider_name(db, session_id)? else {
        return Ok(None);
    };
    Ok(Some(ConsumedMarkerContext {
        provider_name,
        state,
    }))
}

fn pending_rows_have_consumed_markers(
    state: &StateDb,
    provider_name: &str,
    session_id: &str,
    pending: &[MailboxRow],
) -> Result<bool, String> {
    if !session_has_consumed_notification_marker(state, provider_name, session_id)? {
        return Ok(false);
    }
    for row in pending {
        if !state.has_session_user_turn_containing(
            provider_name,
            session_id,
            &mailbox_handle_marker(row),
        )? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn mailbox_handle_marker(row: &MailboxRow) -> String {
    format!("handle: {}", row.handle)
}

fn session_has_consumed_notification_marker(
    state: &StateDb,
    provider_name: &str,
    session_id: &str,
) -> Result<bool, String> {
    state.has_session_user_turn_containing(provider_name, session_id, CONSUMED_NOTIFICATION_MARKER)
}

fn pending_mailbox_rows(db: &MailboxDb, session_id: &str) -> Result<Vec<MailboxRow>, String> {
    db.list_pending(session_id)
}
