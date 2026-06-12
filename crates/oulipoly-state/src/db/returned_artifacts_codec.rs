//! ## Declared roles
//!
//! - validator
//! - predicate
//! - mapper
//! - formatter
//! - parser
//!
//! Role set: { validator, predicate, mapper, formatter, parser }
//!
//! Returned-artifact identity and version-id codec helpers.

use super::*;
use chrono::{DateTime, Utc};
use uuid::Uuid;

pub(super) struct InvocationIdentity {
    pub(super) row_id: i64,
    pub(super) uuid: Uuid,
}

pub(super) struct ReturnedArtifactRawRow {
    pub(super) version_id: String,
    pub(super) name: String,
    pub(super) workflow_run_id: String,
    pub(super) artifact_name: String,
    pub(super) version: i64,
    pub(super) sha256: String,
    pub(super) content_len: i64,
    pub(super) format_hint: Option<String>,
    pub(super) verdict_line: Option<String>,
    pub(super) source_json: String,
    pub(super) returned_at_text: String,
}

pub(super) struct ReturnedArtifactValidatedInputs {
    pub(super) version: i64,
    pub(super) content_len: i64,
}

pub(super) struct ReturnedArtifactPayloadFields {
    pub(super) source_json: String,
    pub(super) returned_at: String,
}

pub(super) struct ReturnedArtifactRowParams<'a> {
    pub(super) invocation_row_id: i64,
    pub(super) ordinal: i64,
    pub(super) version_id: &'a str,
    pub(super) name: &'a str,
    pub(super) workflow_run_id: &'a str,
    pub(super) artifact_name: &'a str,
    pub(super) version: i64,
    pub(super) sha256: &'a str,
    pub(super) content_len: i64,
    pub(super) format_hint: &'a Option<String>,
    pub(super) verdict_line: &'a Option<String>,
    pub(super) source_kind: &'static str,
    pub(super) source_json: &'a str,
    pub(super) returned_at: &'a str,
}

pub(super) struct ParsedReturnedArtifactFieldValues {
    pub(super) source: oulipoly_agent_messenger::ReturnedArtifactSource,
    pub(super) returned_at: DateTime<Utc>,
    pub(super) producer_invocation_uuid: Uuid,
    pub(super) version: i64,
    pub(super) content_len: i64,
}

pub(super) struct ValidatedReturnedArtifactFieldValues {
    pub(super) source: oulipoly_agent_messenger::ReturnedArtifactSource,
    pub(super) returned_at: DateTime<Utc>,
    pub(super) producer_invocation_uuid: Uuid,
    pub(super) version: u64,
    pub(super) content_len: u64,
}

pub(super) enum ReturnedArtifactFieldError {
    SourceJson(serde_json::Error),
    ReturnedAt {
        raw: String,
        err: chrono::ParseError,
    },
    ProducerUuid(sqlite::Error),
    NegativeInteger {
        field: &'static str,
    },
}

pub(super) fn returned_source_kind(
    source: &oulipoly_agent_messenger::ReturnedArtifactSource,
) -> &'static str {
    match source {
        oulipoly_agent_messenger::ReturnedArtifactSource::Scratchpad { .. } => "scratchpad",
        oulipoly_agent_messenger::ReturnedArtifactSource::InlineBytes => "inline_bytes",
    }
}

pub(super) fn returned_artifact_producer_uuid(workflow_run_id: &str) -> sqlite::Result<Uuid> {
    let uuid_text = returned_artifact_workflow_uuid_text(workflow_run_id)?;
    parse_returned_artifact_uuid(uuid_text)
}

pub(super) fn returned_artifact_workflow_uuid_text(workflow_run_id: &str) -> sqlite::Result<&str> {
    workflow_run_id
        .strip_prefix("return:")
        .ok_or_else(returned_artifact_workflow_namespace_error)
}

pub(super) fn returned_artifact_workflow_namespace_error() -> sqlite::Error {
    sqlite::Error::FromSqlConversionFailure(
        2,
        sqlite::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "returned artifact workflow_run_id is not in return namespace",
        )),
    )
}

pub(super) fn parse_returned_artifact_uuid(uuid_text: &str) -> sqlite::Result<Uuid> {
    Uuid::parse_str(uuid_text).map_err(|err| {
        sqlite::Error::FromSqlConversionFailure(2, sqlite::Type::Text, Box::new(err))
    })
}

pub(super) fn returned_artifact_version_id(
    invocation_uuid: Uuid,
    artifact_name: &str,
    version: u64,
) -> String {
    let encoded_name = returned_artifact_encoded_name(artifact_name);
    format_returned_artifact_version_id(invocation_uuid, &encoded_name, version)
}

pub(super) fn returned_artifact_encoded_name(artifact_name: &str) -> String {
    let mut encoded_name = String::new();
    for byte in artifact_name.bytes() {
        if returned_artifact_byte_is_unreserved(byte) {
            encoded_name.push(byte as char);
        } else {
            encoded_name.push_str(&format_returned_artifact_percent_byte(byte));
        }
    }
    encoded_name
}

pub(super) fn returned_artifact_byte_is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

pub(super) fn format_returned_artifact_percent_byte(byte: u8) -> String {
    format!("%{byte:02X}")
}

pub(super) fn format_returned_artifact_version_id(
    invocation_uuid: Uuid,
    encoded_name: &str,
    version: u64,
) -> String {
    format!("store://return/{invocation_uuid}/{encoded_name}/{version}")
}

pub(super) fn returned_artifact_sql_integer(value: u64, field: &str) -> Result<i64, DbError> {
    validate_returned_artifact_sql_integer(value, field)?;
    Ok(map_returned_artifact_sql_integer(value))
}

pub(super) fn validate_returned_artifact_sql_integer(
    value: u64,
    field: &str,
) -> Result<(), DbError> {
    if value > i64::MAX as u64 {
        Err(returned_artifact_sql_integer_overflow(field, value))
    } else {
        Ok(())
    }
}

pub(super) fn map_returned_artifact_sql_integer(value: u64) -> i64 {
    value as i64
}

pub(super) fn returned_artifact_sql_integer_overflow(field: &str, value: u64) -> DbError {
    format!("Returned artifact {field} exceeds SQLite INTEGER range: {value}")
}
