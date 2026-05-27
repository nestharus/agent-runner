//! Declared roles: validator, predicate

use uuid::Uuid;

pub(crate) fn validate_import_replace_args(
    session_id: &str,
    preimage_sha256: Option<&str>,
) -> Option<i32> {
    if Uuid::try_parse(session_id).is_err() {
        return Some(super::formatter::render_replace_error(
            super::mapper::invalid_session_id_error(session_id),
        ));
    }
    if preimage_sha256.is_some_and(invalid_sha256_hex) {
        return Some(super::formatter::render_replace_error(
            super::mapper::invalid_preimage_sha256_error(),
        ));
    }
    None
}

// Predicate helper used only by the validator entry point above.
fn invalid_sha256_hex(hash: &str) -> bool {
    hash.len() != 64 || !hash.chars().all(|ch| ch.is_ascii_hexdigit())
}
