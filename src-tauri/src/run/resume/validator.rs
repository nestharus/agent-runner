//! validator

use uuid::Uuid;

const OPENCODE_SESSION_PREFIX: &str = "ses_";
const OPENCODE_SESSION_MIN_SUFFIX_LEN: usize = 3;

pub(crate) fn validate_resume_input(session_id: &str) -> Result<(), String> {
    if session_id.trim().is_empty() {
        return Err("session id is required".to_string());
    }
    if Uuid::parse_str(session_id).is_ok() || is_opencode_provider_session_id(session_id) {
        return Ok(());
    }
    Err(format!("invalid session id: {session_id}"))
}

fn is_opencode_provider_session_id(session_id: &str) -> bool {
    let Some(suffix) = session_id.strip_prefix(OPENCODE_SESSION_PREFIX) else {
        return false;
    };

    suffix.len() >= OPENCODE_SESSION_MIN_SUFFIX_LEN
        && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_uuid_or_opencode_provider_session_id() {
        assert!(validate_resume_input("5169694d-de0f-40d1-890c-6e28e55bab27").is_ok());
        assert!(validate_resume_input("ses_fixture").is_ok());
    }

    #[test]
    fn rejects_malformed_resume_input() {
        assert_eq!(
            validate_resume_input("not-a-uuid").unwrap_err(),
            "invalid session id: not-a-uuid"
        );
        assert_eq!(
            validate_resume_input("ses_ab").unwrap_err(),
            "invalid session id: ses_ab"
        );
        assert_eq!(
            validate_resume_input("ses_fixture-1").unwrap_err(),
            "invalid session id: ses_fixture-1"
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
