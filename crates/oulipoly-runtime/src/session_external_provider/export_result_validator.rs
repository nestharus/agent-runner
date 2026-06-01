//! Role: validator.

use super::export_result_parser::parse_canonical_count;
use super::hash_formatter::sha256_hex;
use super::provider_error::{
    ExternalSessionProviderError, map_hash_mismatch_error, map_invalid_canonical_format_error,
    map_turn_count_mismatch_error,
};
use super::request_builder::CANONICAL_FORMAT;
use oulipoly_provider::generated::SessionExportResult;

pub(crate) fn validate_export_result(
    result: &SessionExportResult,
    bytes: &[u8],
) -> Result<(), ExternalSessionProviderError> {
    validate_canonical_format(&result.canonical_format)?;
    validate_sha256(bytes, &result.sha256)?;
    validate_canonical_record_count(bytes, result.turn_count)
}

fn validate_canonical_format(format: &str) -> Result<(), ExternalSessionProviderError> {
    if format == CANONICAL_FORMAT {
        Ok(())
    } else {
        Err(map_invalid_canonical_format_error())
    }
}

fn validate_sha256(bytes: &[u8], expected: &str) -> Result<(), ExternalSessionProviderError> {
    if sha256_hex(bytes) == expected {
        Ok(())
    } else {
        Err(map_hash_mismatch_error())
    }
}

fn validate_canonical_record_count(
    bytes: &[u8],
    expected: u64,
) -> Result<(), ExternalSessionProviderError> {
    let count = parse_canonical_count(bytes)?;
    if count == expected {
        Ok(())
    } else {
        Err(map_turn_count_mismatch_error())
    }
}
