//! Declared roles: orchestration, formatter

use std::io::{self, Write as _};

pub(crate) fn write_json_error(code: &str, message: &str) -> Result<(), String> {
    let value = json_error_payload(code, message);
    let json = serialize_json_error_payload(&value)?;
    emit_json_error_line(&json);
    Ok(())
}

fn serialize_json_error_payload(value: &serde_json::Value) -> Result<String, String> {
    serde_json::to_string(value).map_err(format_json_error_serialize_error)
}

pub(crate) fn format_json_error_serialize_error(error: serde_json::Error) -> String {
    format!("Failed to serialize schema probe error: {error}")
}

fn emit_json_error_line(json: &str) {
    eprintln!("{json}");
}

pub(crate) fn json_error_payload(code: &str, message: impl Into<String>) -> serde_json::Value {
    serde_json::json!({
        "error": {
            "code": code,
            "message": message.into(),
        }
    })
}

pub(crate) fn emit_json_error(error_code: &str, message: impl Into<String>) {
    let _ = writeln!(io::stderr(), "{}", json_error_payload(error_code, message));
}
