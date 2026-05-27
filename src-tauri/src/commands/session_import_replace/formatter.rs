//! Declared roles: formatter

use oulipoly_runtime::session_replace::{self, ReplaceError};

pub(crate) fn render_import_replace_output(
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

pub(super) fn render_replace_error(err: ReplaceError) -> i32 {
    eprintln!("{}", err.to_json());
    err.exit_code()
}

fn format_replace_receipt_error(error: serde_json::Error) -> String {
    format!("Failed to serialize replace receipt: {error}")
}

pub(super) fn format_invalid_preimage_sha256() -> String {
    "preimage sha256 must be 64 hex characters".to_string()
}
