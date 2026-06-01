//! Role: formatter.

use super::hash_formatter::sha256_hex;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

pub(crate) fn data_base64(bytes: &[u8]) -> String {
    BASE64.encode(bytes)
}

pub(crate) fn records_sha256(bytes: &[u8]) -> String {
    sha256_hex(bytes)
}
