//! Role: parser.

use super::provider_error::{
    ExternalSessionProviderError, map_canonical_parse_count_mismatch_error,
    map_invalid_base64_error,
};
use crate::session_export::CanonicalRecord;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

pub(crate) fn decode_base64(input: &str) -> Result<Vec<u8>, ExternalSessionProviderError> {
    BASE64.decode(input).map_err(|_| map_invalid_base64_error())
}

pub(crate) fn parse_canonical_count(bytes: &[u8]) -> Result<u64, ExternalSessionProviderError> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| map_canonical_parse_count_mismatch_error())?;
    let mut count = 0_u64;
    for line in text.lines() {
        parse_canonical_line(line)?;
        count += 1;
    }
    Ok(count)
}

fn parse_canonical_line(line: &str) -> Result<CanonicalRecord, ExternalSessionProviderError> {
    if line.trim().is_empty() {
        return Err(map_canonical_parse_count_mismatch_error());
    }
    serde_json::from_str::<CanonicalRecord>(line)
        .map_err(|_| map_canonical_parse_count_mismatch_error())
}
