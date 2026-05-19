//! Session import-replace CLI validation and rendering helpers.
//!
//! ## Declared roles
//!
//! `validator`, `mapper`, `formatter`

use oulipoly_runtime::session_replace::{self, ReplaceError};
use uuid::Uuid;

pub(super) fn validate_import_replace_args(
    session_id: &str,
    preimage_sha256: Option<&str>,
) -> Option<i32> {
    if Uuid::try_parse(session_id).is_err() {
        return Some(render_replace_error(ReplaceError::InvalidSessionId {
            input: session_id.to_string(),
        }));
    }
    if preimage_sha256.is_some_and(invalid_sha256_hex) {
        return Some(render_replace_error(ReplaceError::InvalidArgument {
            message: "preimage sha256 must be 64 hex characters".to_string(),
        }));
    }
    None
}

pub(super) fn render_import_replace_output(
    result: Result<session_replace::ReplaceReceipt, ReplaceError>,
) -> Result<i32, String> {
    match result {
        Ok(receipt) => {
            let json = serde_json::to_string(&receipt).map_err(format_replace_receipt_error)?;
            println!("{json}");
            Ok(0)
        }
        Err(err) => Ok(render_replace_error(err)),
    }
}

fn invalid_sha256_hex(hash: &str) -> bool {
    hash.len() != 64 || !hash.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn render_replace_error(err: ReplaceError) -> i32 {
    eprintln!("{}", err.to_json());
    err.exit_code()
}

fn format_replace_receipt_error(error: serde_json::Error) -> String {
    format!("Failed to serialize replace receipt: {error}")
}
