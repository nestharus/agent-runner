//! Declared roles: validator

use uuid::Uuid;

pub(super) fn validate_resume_list_uuid(uuid: &str) -> Result<(), String> {
    match Uuid::parse_str(uuid) {
        Ok(_) => Ok(()),
        Err(error) => Err(super::formatter::format_invalid_session_uuid_error(
            uuid, error,
        )),
    }
}
