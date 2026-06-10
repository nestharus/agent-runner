//! ## Declared roles
//!
//! `mapper`, `orchestration`

use oulipoly_state::mailbox::{MailboxDb, SessionRuntimeIdleUpdate};

pub(crate) fn mark_session_idle_after_turn(
    session_id: &str,
    invocation_uuid: &str,
    exit_code: Option<i32>,
) -> Result<(), String> {
    let Some(mut db) = MailboxDb::open_default_if_exists()? else {
        return Ok(());
    };
    db.mark_session_idle(session_runtime_idle_update(
        session_id,
        invocation_uuid,
        exit_code,
    ))?;
    Ok(())
}

fn session_runtime_idle_update<'a>(
    session_id: &'a str,
    invocation_uuid: &'a str,
    exit_code: Option<i32>,
) -> SessionRuntimeIdleUpdate<'a> {
    SessionRuntimeIdleUpdate {
        session_id,
        invocation_uuid,
        last_exit_code: exit_code,
    }
}
