//! Declared roles: mapper

use oulipoly_runtime::services::SessionLockServiceRequest;

pub(super) fn pause_handshake_request(session_id: &str, ttl_ms: u64) -> SessionLockServiceRequest {
    SessionLockServiceRequest::Acquire {
        session_id: session_id.to_string(),
        ttl_ms,
    }
}

pub(super) fn resume_handshake_request(session_id: &str, token: &str) -> SessionLockServiceRequest {
    SessionLockServiceRequest::Release {
        session_id: session_id.to_string(),
        token: token.to_string(),
    }
}
