//! Declared roles: validator

use uuid::Uuid;

pub(super) fn validate_locate_session_id(
    session_id: &str,
) -> Option<super::mapper::LocateSessionIdRejection> {
    if Uuid::parse_str(session_id).is_err() {
        Some(super::mapper::invalid_locate_session_id_rejection(
            session_id,
        ))
    } else {
        None
    }
}

pub(super) fn validate_session_export_args(
    session_id: &str,
    format: &str,
) -> Option<super::mapper::SessionExportArgsRejection> {
    if format != "canonical-jsonl" {
        return Some(super::mapper::invalid_session_export_format_rejection(
            format,
        ));
    }
    if Uuid::parse_str(session_id).is_err() {
        return Some(super::mapper::invalid_session_export_id_rejection(
            session_id,
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_UUID: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";

    #[test]
    fn validate_locate_session_id_rejects_invalid_uuid_with_exit_2() {
        assert_eq!(
            validate_locate_session_id("not-a-uuid").map(|rejection| {
                super::super::mapper::locate_session_id_rejection_exit_code(&rejection)
            }),
            Some(2)
        );
    }

    #[test]
    fn validate_locate_session_id_accepts_valid_uuid() {
        assert_eq!(validate_locate_session_id(VALID_UUID), None);
    }

    #[test]
    fn validate_session_export_args_rejects_unsupported_format_with_exit_2() {
        assert_eq!(
            validate_session_export_args(VALID_UUID, "other").map(|rejection| {
                super::super::mapper::session_export_args_rejection_exit_code(&rejection)
            }),
            Some(2)
        );
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
            validate_session_export_args("not-a-uuid", "canonical-jsonl").map(|rejection| {
                super::super::mapper::session_export_args_rejection_exit_code(&rejection)
            }),
            Some(2)
        );
    }
}
