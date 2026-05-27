//! validator

use uuid::Uuid;

pub(super) fn validate_resume_uuid(session_id: &str) -> Result<(), String> {
    Uuid::parse_str(session_id)
        .map(|_| ())
        .map_err(|_| format!("invalid session UUID: {session_id}"))
}
