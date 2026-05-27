//! Declared roles: formatter

use crate::error_emit::{emit_lock_error, emit_resume_resolution_error};
use oulipoly_runtime::services::{SessionLockFailure, SessionLockSuccess};
use oulipoly_runtime::session_lock::Lease;

pub(super) fn render_pause_handshake_output(
    result: Result<SessionLockSuccess, SessionLockFailure>,
) -> Result<i32, String> {
    match result {
        Ok(SessionLockSuccess::Acquired {
            chain_id, lease, ..
        }) => {
            println!("{}", pause_handshake_receipt_json(&chain_id, &lease)?);
            Ok(0)
        }
        Ok(SessionLockSuccess::Released { .. }) => unreachable!("acquire cannot release a lock"),
        Err(SessionLockFailure::Resume(err)) => Ok(emit_resume_resolution_error(err)),
        Err(SessionLockFailure::Lock(err)) => Ok(emit_lock_error(err)),
    }
}

pub(super) fn pause_handshake_receipt_json(
    chain_id: &str,
    lease: &Lease,
) -> Result<String, String> {
    let payload = serde_json::json!({
        "session_id": lease.session_id,
        "chain_id": chain_id,
        "provider_name": lease.provider_name,
        "token": lease.token,
        "expires_at": lease.expires_at,
        "lock_path": lease.lock_path,
    });
    serde_json::to_string(&payload).map_err(|err| format!("failed to encode pause receipt: {err}"))
}

pub(super) fn render_resume_handshake_output(
    result: Result<SessionLockSuccess, SessionLockFailure>,
) -> Result<i32, String> {
    match result {
        Ok(SessionLockSuccess::Released { receipt }) => {
            println!(
                "{}",
                serde_json::to_string(&receipt)
                    .map_err(|err| format!("failed to encode resume receipt: {err}"))?
            );
            Ok(0)
        }
        Ok(SessionLockSuccess::Acquired { .. }) => unreachable!("release cannot acquire a lock"),
        Err(SessionLockFailure::Lock(err)) => Ok(emit_lock_error(err)),
        Err(SessionLockFailure::Resume(_)) => unreachable!("release does not resolve resume"),
    }
}

pub(super) fn emit_invalid_session_id(session_id: &str) {
    crate::json_error::emit_json_error(
        "invalid-session-id",
        format_invalid_session_uuid(session_id),
    );
}

pub(super) fn emit_invalid_ttl(max_ttl_ms: u64) {
    crate::json_error::emit_json_error("invalid-ttl", format_invalid_ttl(max_ttl_ms));
}

fn format_invalid_session_uuid(session_id: &str) -> String {
    format!("invalid session UUID: {session_id}")
}

fn format_invalid_ttl(max_ttl_ms: u64) -> String {
    format!("ttl-ms must be at most {max_ttl_ms}")
}
