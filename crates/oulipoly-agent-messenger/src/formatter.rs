//! ## Declared roles
//!
//! - formatter
//! - predicate

use crate::MessengerError;
use crate::model::ReturnedArtifact;
use crate::validator;
use uuid::Uuid;

pub(crate) const RECEIPT_SCHEMA_VERSION: u32 = 1;

pub(crate) fn return_channel_line(receipt: &ReturnedArtifact) -> Result<Vec<u8>, MessengerError> {
    let mut line = serde_json::to_vec(receipt)?;
    line.push(b'\n');
    Ok(line)
}

pub(crate) fn return_workflow(invocation_uuid: Uuid) -> String {
    format!("return:{invocation_uuid}")
}

pub(crate) fn version_id(invocation_uuid: Uuid, name: &str, version: u64) -> String {
    format!(
        "store://return/{}/{}/{}",
        invocation_uuid,
        percent_encode(name),
        version
    )
}

pub(crate) fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .fold(String::new(), append_percent_encoded_byte)
}

fn append_percent_encoded_byte(mut encoded: String, byte: u8) -> String {
    encoded.push_str(&percent_encoded_byte(byte));
    encoded
}

fn percent_encoded_byte(byte: u8) -> String {
    if validator::is_unreserved_percent_byte(byte) {
        return (byte as char).to_string();
    }
    format_percent_byte(byte)
}

fn format_percent_byte(byte: u8) -> String {
    format!("%{byte:02X}")
}

pub(crate) fn store_schema_version_message(version: String) -> String {
    format!("version {version}")
}

pub(crate) fn scratchpad_schema_message() -> String {
    "scratchpad schema is incompatible".to_string()
}
