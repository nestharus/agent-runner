//! Declared roles: mapper
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: src-tauri/src/commands/session_import_replace/mapper.rs
//!     role: intrinsic-surface
//!     Domain: session import-replace command DTO mapping
//!     Owns:
//!       - import-replace service request construction
//!       - replace source mapping
//!       - provider identity DTO carriage into the replace service boundary
//! ```

use oulipoly_runtime::services::{
    SessionReplaceServiceRequest, SessionServiceExternalProviderIdentity,
};
use oulipoly_runtime::session_replace::{ReplaceError, ReplaceSource};
use std::path::Path;

pub(crate) fn import_replace_request(
    session_id: &str,
    from_file: Option<&Path>,
    preimage_sha256: Option<&str>,
    external_provider: Option<SessionServiceExternalProviderIdentity>,
) -> SessionReplaceServiceRequest {
    SessionReplaceServiceRequest {
        session_id: session_id.to_string(),
        source: replace_source(from_file),
        preimage_sha256: preimage_sha256.map(str::to_string),
        external_provider,
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
