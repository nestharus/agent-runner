//! Declared roles: validator

use uuid::Uuid;

pub(super) const DEFAULT_PAUSE_HANDSHAKE_TTL_MS: u64 = 60_000;
pub(super) const MAX_PAUSE_HANDSHAKE_TTL_MS: u64 = 600_000;

pub(super) fn validate_pause_handshake_args(session_id: &str, ttl_ms: Option<u64>) -> Option<u64> {
    if Uuid::parse_str(session_id).is_err() {
        super::formatter::emit_invalid_session_id(session_id);
        return None;
    }
    let ttl_ms = ttl_ms.unwrap_or(DEFAULT_PAUSE_HANDSHAKE_TTL_MS);
    if ttl_ms > MAX_PAUSE_HANDSHAKE_TTL_MS {
        super::formatter::emit_invalid_ttl(MAX_PAUSE_HANDSHAKE_TTL_MS);
        return None;
    }
    Some(ttl_ms)
}

pub(super) fn validate_resume_handshake_session_id(session_id: &str) -> Option<i32> {
    if Uuid::parse_str(session_id).is_err() {
        super::formatter::emit_invalid_session_id(session_id);
        Some(2)
    } else {
        None
    }
}
