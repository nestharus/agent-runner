#[path = "../src/commands/session_import_replace/formatter.rs"]
mod formatter;
#[path = "../src/json_error.rs"]
mod json_error;
#[path = "../src/commands/session_import_replace/mapper.rs"]
mod mapper;
#[path = "../src/commands/session_import_replace/validator.rs"]
mod validator;

use formatter::*;
use json_error::*;
use mapper::*;
use oulipoly_runtime::services::SessionReplaceServiceRequest;
use oulipoly_runtime::session_replace::{ReplaceError, ReplaceReceipt, ReplaceSource};
use std::path::{Path, PathBuf};
use validator::*;

const VALID_SESSION_ID: &str = "11111111-1111-4111-8111-111111111111";
const VALID_PREIMAGE_SHA256: &str =
    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const INVALID_PREIMAGE_MESSAGE: &str = "preimage sha256 must be 64 hex characters";

#[test]
fn validate_import_replace_args_rejects_invalid_uuid_with_replace_error_exit_code() {
    let invalid = "not-a-uuid";
    let expected = ReplaceError::InvalidSessionId {
        input: invalid.to_string(),
    }
    .exit_code();

    assert_eq!(validate_import_replace_args(invalid, None), Some(expected));
}

#[test]
fn validate_import_replace_args_accepts_valid_uuid_without_preimage() {
    assert_eq!(validate_import_replace_args(VALID_SESSION_ID, None), None);
}

#[test]
fn validate_import_replace_args_rejects_malformed_preimage_with_replace_error_exit_code() {
    let expected = ReplaceError::InvalidArgument {
        message: INVALID_PREIMAGE_MESSAGE.to_string(),
    }
    .exit_code();

    for invalid in [
        "0123456789abcdef",
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef00",
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdeg",
    ] {
        assert_eq!(
            validate_import_replace_args(VALID_SESSION_ID, Some(invalid)),
            Some(expected),
            "invalid preimage should be rejected: {invalid}"
        );
    }
}

#[test]
fn validate_import_replace_args_accepts_valid_uuid_and_valid_preimage() {
    assert_eq!(
        validate_import_replace_args(VALID_SESSION_ID, Some(VALID_PREIMAGE_SHA256)),
        None
    );
}

#[test]
fn import_replace_request_maps_absent_file_to_stdin_source() {
    let request = import_replace_request(VALID_SESSION_ID, None, None, None);

    assert_eq!(request.source, ReplaceSource::Stdin);
}

#[test]
fn import_replace_request_maps_present_file_to_file_source() {
    let path = Path::new("/tmp/oulipoly-import-replace.jsonl");
    let request = import_replace_request(VALID_SESSION_ID, Some(path), None, None);

    assert_eq!(request.source, ReplaceSource::File(path.to_path_buf()));
}

#[test]
fn import_replace_request_preserves_session_id_and_absent_preimage() {
    let request = import_replace_request(VALID_SESSION_ID, None, None, None);

    assert_eq!(
        request,
        SessionReplaceServiceRequest {
            session_id: VALID_SESSION_ID.to_string(),
            source: ReplaceSource::Stdin,
            preimage_sha256: None,
            external_provider: None,
        }
    );
}

#[test]
fn import_replace_request_preserves_session_id_and_present_preimage() {
    let path = Path::new("/tmp/oulipoly-import-replace.jsonl");
    let request = import_replace_request(
        VALID_SESSION_ID,
        Some(path),
        Some(VALID_PREIMAGE_SHA256),
        None,
    );

    assert_eq!(
        request,
        SessionReplaceServiceRequest {
            session_id: VALID_SESSION_ID.to_string(),
            source: ReplaceSource::File(path.to_path_buf()),
            preimage_sha256: Some(VALID_PREIMAGE_SHA256.to_string()),
            external_provider: None,
        }
    );
}

#[test]
fn render_import_replace_output_returns_zero_for_success_receipt() {
    let result = render_import_replace_output(Ok(replace_receipt()));

    assert_eq!(result, Ok(0));
}

#[test]
fn render_import_replace_output_returns_replace_error_exit_code_for_invalid_session_id() {
    let err = ReplaceError::InvalidSessionId {
        input: "bad-session-id".to_string(),
    };
    let expected = err.exit_code();

    let result = render_import_replace_output(Err(err));

    assert_eq!(result, Ok(expected));
}

#[test]
fn json_error_payload_returns_expected_error_envelope() {
    assert_eq!(
        json_error_payload("schema-incompatible", "msg"),
        serde_json::json!({"error": {"code": "schema-incompatible", "message": "msg"}})
    );
}

#[test]
fn json_error_payload_embeds_converted_string_message() {
    let message = String::from("converted message");

    assert_eq!(
        json_error_payload("code", message),
        serde_json::json!({"error": {"code": "code", "message": "converted message"}})
    );
}

#[test]
fn write_json_error_returns_ok_on_happy_path() {
    assert_eq!(write_json_error("code", "msg"), Ok(()));
}

#[test]
fn emit_json_error_accepts_borrowed_message() {
    emit_json_error("code", "msg");
}

#[test]
fn emit_json_error_accepts_converted_string_message() {
    emit_json_error("code", String::from("converted message"));
}

#[test]
fn format_json_error_serialize_error_preserves_schema_probe_prefix() {
    let err = serde_json::Error::io(std::io::Error::other("synthetic serializer failure"));

    assert_eq!(
        format_json_error_serialize_error(err),
        "Failed to serialize schema probe error: synthetic serializer failure"
    );
}

fn replace_receipt() -> ReplaceReceipt {
    ReplaceReceipt {
        session_id: VALID_SESSION_ID.to_string(),
        provider_name: "fixture-provider".to_string(),
        storage_type: "fixture-storage".to_string(),
        operation: "replace".to_string(),
        preimage_sha256: VALID_PREIMAGE_SHA256.to_string(),
        postimage_sha256: "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210"
            .to_string(),
        jsonl_path: PathBuf::from("/tmp/oulipoly-replaced.jsonl"),
        state_updated: true,
        committed_at: "2026-05-26T00:00:00Z".to_string(),
    }
}
