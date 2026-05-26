//! Declared roles: validator

use super::formatter::{emit_export_error, emit_export_json_error, emit_metadata_error};
use super::mapper::export_error_exit_code;
use oulipoly_runtime::session_export::ExportError;
use oulipoly_runtime::session_metadata::MetadataError;
use uuid::Uuid;

pub(super) fn validate_locate_session_id(session_id: &str) -> Option<i32> {
    if Uuid::parse_str(session_id).is_err() {
        emit_metadata_error(&MetadataError::InvalidSessionId {
            input: session_id.to_string(),
        });
        Some(2)
    } else {
        None
    }
}

pub(super) fn validate_session_export_args(session_id: &str, format: &str) -> Option<i32> {
    if format != "canonical-jsonl" {
        emit_export_json_error(
            "invalid-format",
            &format!("unsupported export format {format}; expected canonical-jsonl"),
        );
        return Some(2);
    }
    if Uuid::parse_str(session_id).is_err() {
        let err = ExportError::InvalidSessionId {
            input: session_id.to_string(),
        };
        emit_export_error(&err);
        return Some(export_error_exit_code(&err));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_UUID: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";

    #[test]
    fn validate_locate_session_id_rejects_invalid_uuid_with_exit_2() {
        assert_eq!(validate_locate_session_id("not-a-uuid"), Some(2));
    }

    #[test]
    fn validate_locate_session_id_accepts_valid_uuid() {
        assert_eq!(validate_locate_session_id(VALID_UUID), None);
    }

    #[test]
    fn validate_session_export_args_rejects_unsupported_format_with_exit_2() {
        assert_eq!(validate_session_export_args(VALID_UUID, "other"), Some(2));
    }

    #[test]
    fn validate_session_export_args_accepts_valid_uuid_and_canonical_jsonl() {
        assert_eq!(
            validate_session_export_args(VALID_UUID, "canonical-jsonl"),
            None
        );
    }

    #[test]
    fn validate_session_export_args_rejects_invalid_uuid_with_exit_2() {
        assert_eq!(
            validate_session_export_args("not-a-uuid", "canonical-jsonl"),
            Some(2)
        );
    }
}
