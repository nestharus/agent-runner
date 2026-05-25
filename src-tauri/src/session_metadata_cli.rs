//! Session metadata CLI output mapping and rendering helpers.
//!
//! ## Declared roles
//!
//! `mapper`, `formatter`

use oulipoly_runtime::session_metadata::MetadataError;

use crate::commands::session_locate_export::{emit_metadata_error, metadata_error_exit_code};

pub(super) fn render_session_metadata(
    result: Result<oulipoly_runtime::session_metadata::SessionMetadata, MetadataError>,
) -> Result<i32, String> {
    match result {
        Ok(metadata) => match serde_json::to_string(&metadata) {
            Ok(json) => {
                println!("{json}");
                Ok(0)
            }
            Err(err) => {
                emit_metadata_error(&MetadataError::Operational {
                    message: format!("failed to serialize session metadata: {err}"),
                });
                Ok(1)
            }
        },
        Err(err) => {
            let code = metadata_error_exit_code(&err);
            emit_metadata_error(&err);
            Ok(code)
        }
    }
}
