//! Declared roles: mapper

use oulipoly_runtime::services::SessionReplaceServiceRequest;
use oulipoly_runtime::session_replace::{ReplaceError, ReplaceSource};
use std::path::Path;

pub(crate) fn import_replace_request(
    session_id: &str,
    from_file: Option<&Path>,
    preimage_sha256: Option<&str>,
) -> SessionReplaceServiceRequest {
    SessionReplaceServiceRequest {
        session_id: session_id.to_string(),
        source: replace_source(from_file),
        preimage_sha256: preimage_sha256.map(str::to_string),
    }
}

fn replace_source(from_file: Option<&Path>) -> ReplaceSource {
    from_file
        .map(|path| ReplaceSource::File(path.to_path_buf()))
        .unwrap_or(ReplaceSource::Stdin)
}

pub(super) fn invalid_session_id_error(session_id: &str) -> ReplaceError {
    ReplaceError::InvalidSessionId {
        input: session_id.to_string(),
    }
}

pub(super) fn invalid_preimage_sha256_error() -> ReplaceError {
    ReplaceError::InvalidArgument {
        message: super::formatter::format_invalid_preimage_sha256(),
    }
}
