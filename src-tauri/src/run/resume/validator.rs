//! validator

pub(super) fn validate_resume_input(session_id: &str) -> Result<(), String> {
    if session_id.trim().is_empty() {
        return Err("session id is required".to_string());
    }
    Ok(())
}
