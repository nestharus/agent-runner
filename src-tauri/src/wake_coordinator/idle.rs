//! ## Declared roles
//!
//! `mapper`, `orchestration`

use oulipoly_state::mailbox::{
    LegacyRuntimeProjectionSettlement, MailboxDb, TerminalCompatibilityReconciliation,
};

pub(crate) fn mark_session_idle_after_turn(
    session_id: &str,
    invocation_uuid: &str,
    exit_code: Option<i32>,
) -> Result<(), String> {
    let Some(mut db) = MailboxDb::open_default_if_exists()? else {
        return Ok(());
    };
    match db
        .runtime_lifecycle()
        .reconcile_terminal_compatibility_projection(session_id, invocation_uuid, exit_code)
        .map_err(|error| error.to_string())?
    {
        TerminalCompatibilityReconciliation::Reconciled => return Ok(()),
        TerminalCompatibilityReconciliation::NoGeneration => {}
    }
    db.wake_sessions()
        .settle_legacy_runtime_projection(legacy_runtime_projection_settlement(
            session_id,
            invocation_uuid,
            exit_code,
        ))?;
    Ok(())
}

fn legacy_runtime_projection_settlement<'a>(
    session_id: &'a str,
    invocation_uuid: &'a str,
    exit_code: Option<i32>,
) -> LegacyRuntimeProjectionSettlement<'a> {
    LegacyRuntimeProjectionSettlement {
        session_id,
        invocation_uuid,
        last_exit_code: exit_code,
    }
}
