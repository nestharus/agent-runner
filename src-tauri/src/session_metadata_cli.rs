//! Session metadata CLI output mapping and rendering helpers.
//!
//! ## Declared roles
//!
//! `orchestration`, `mapper`, `formatter`

use oulipoly_runtime::session_metadata::MetadataError;

use crate::commands::session_locate_export::{emit_metadata_error, metadata_error_exit_code};

pub(super) fn render_session_metadata(
    result: Result<oulipoly_runtime::session_metadata::SessionMetadata, MetadataError>,
) -> Result<i32, String> {
    let outcome = session_metadata_render_outcome(result);
    emit_session_metadata_render_outcome(&outcome);
    Ok(session_metadata_render_exit_code(&outcome))
}

enum SessionMetadataRenderOutcome {
    Json(String),
    MetadataError { err: MetadataError, code: i32 },
}

fn session_metadata_render_outcome(
    result: Result<oulipoly_runtime::session_metadata::SessionMetadata, MetadataError>,
) -> SessionMetadataRenderOutcome {
    match result {
        Ok(metadata) => serialized_session_metadata_outcome(metadata),
        Err(err) => {
            let code = metadata_error_exit_code(&err);
            SessionMetadataRenderOutcome::MetadataError { err, code }
        }
    }
}

fn serialized_session_metadata_outcome(
    metadata: oulipoly_runtime::session_metadata::SessionMetadata,
) -> SessionMetadataRenderOutcome {
    serde_json::to_string(&metadata).map_or_else(
        serialization_error_outcome,
        SessionMetadataRenderOutcome::Json,
    )
}

fn serialization_error_outcome(err: serde_json::Error) -> SessionMetadataRenderOutcome {
    SessionMetadataRenderOutcome::MetadataError {
        err: MetadataError::Operational {
            message: format!("failed to serialize session metadata: {err}"),
        },
        code: 1,
    }
}

fn emit_session_metadata_render_outcome(outcome: &SessionMetadataRenderOutcome) {
    match outcome {
        SessionMetadataRenderOutcome::Json(json) => println!("{json}"),
        SessionMetadataRenderOutcome::MetadataError { err, .. } => emit_metadata_error(err),
    }
}

fn session_metadata_render_exit_code(outcome: &SessionMetadataRenderOutcome) -> i32 {
    match outcome {
        SessionMetadataRenderOutcome::Json(_) => 0,
        SessionMetadataRenderOutcome::MetadataError { code, .. } => *code,
    }
}
