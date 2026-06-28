//! validator

use oulipoly_state::{ResumeError, StateDb};

pub(crate) fn validate_resume_input(session_id: &str) -> Result<(), String> {
    StateDb::validate_resume_input_id(session_id).map_err(format_resume_input_error)
}

fn format_resume_input_error(error: ResumeError) -> String {
    match error {
        ResumeError::InvalidResumeInput { reason, .. } => reason,
        other => format!("invalid resume input: {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_bounded_opaque_resume_input() {
        assert!(validate_resume_input("5169694d-de0f-40d1-890c-6e28e55bab27").is_ok());
        assert!(validate_resume_input("ses_fixture").is_ok());
        assert!(validate_resume_input("external-abc123xyz").is_ok());
        assert!(validate_resume_input("ses_ab").is_ok());
        assert!(validate_resume_input("ses_fixture-1").is_ok());
    }

    #[test]
    fn rejects_malformed_resume_input() {
        assert_eq!(
            validate_resume_input("abc\n123").unwrap_err(),
            "session id contains control characters"
        );
        assert_eq!(
            validate_resume_input(&"x".repeat(oulipoly_state::RESUME_INPUT_MAX_LEN + 1))
                .unwrap_err(),
            "session id exceeds maximum length of 512 bytes"
        );
    }

    #[test]
    fn rejects_blank_resume_input() {
        assert_eq!(
            validate_resume_input(" ").unwrap_err(),
            "session id is required"
        );
    }
}
