//! ## Declared roles
//! formatter, parser
//!
//! UUID parsing and canonical session/chain id formatting.

use super::MetadataError;
use super::errors::invalid_session_id_error;
use uuid::Uuid;

pub(super) fn parse_session_uuid(input: &str) -> Result<Uuid, MetadataError> {
    parse_uuid(input).map_err(|_| invalid_session_id_error(input))
}

pub(super) fn parse_optional_uuid(input: &str) -> Option<Uuid> {
    parse_uuid(input).ok()
}

pub(super) fn format_uuid(uuid: Uuid) -> String {
    uuid.to_string()
}

pub(super) fn format_optional_uuid(fallback: &str, parsed: Option<Uuid>) -> String {
    parsed
        .map(format_uuid)
        .unwrap_or_else(|| fallback.to_string())
}

fn parse_uuid(input: &str) -> Result<Uuid, uuid::Error> {
    Uuid::parse_str(input)
}
