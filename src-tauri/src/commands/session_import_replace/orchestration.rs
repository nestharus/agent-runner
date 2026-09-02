//! Declared roles: orchestration
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: src-tauri/src/commands/session_import_replace/orchestration.rs
//!     role: intrinsic-surface
//!     Domain: session import-replace command orchestration
//!     Owns:
//!       - import-replace command validation and service dispatch
//!       - external provider identity resolver sequencing for replace requests
//! ```

use crate::wiring::AgentRuntimeServices;
use oulipoly_runtime::session_replace::ReplaceError;
use std::path::Path;

pub(crate) fn run_session_import_replace(
    session_id: &str,
    from_file: Option<&Path>,
    preimage_sha256: Option<&str>,
    agent_runtime_services: &AgentRuntimeServices,
) -> Result<i32, String> {
    if let Some(exit_code) = super::validate_import_replace_args(session_id, preimage_sha256) {
        return Ok(exit_code);
    }
    let external_provider =
        match crate::commands::session_external_provider_identity::resolve_session_external_provider_identity(
            session_id,
        ) {
            Ok(identity) => identity,
            Err(crate::commands::session_external_provider_identity::SessionExternalProviderIdentityError::AmbiguousSession { input }) => {
                return super::render_import_replace_output(Err(ReplaceError::AmbiguousSession { input }));
            }
            Err(crate::commands::session_external_provider_identity::SessionExternalProviderIdentityError::Operational { message }) => {
                return super::render_import_replace_output(Err(replace_error_from_identity_error(
                    message,
                )));
            }
        };
    let request =
        super::import_replace_request(session_id, from_file, preimage_sha256, external_provider);
    let output = agent_runtime_services
        .session_replace_service
        .replace_session(request)
        .map_err(|err| err.to_string())?;

    super::render_import_replace_output(output.result)
}

fn replace_error_from_identity_error(message: String) -> ReplaceError {
    if is_state_schema_incompatible_message(&message) {
        return ReplaceError::SchemaIncompatible { reason: message };
    }
    ReplaceError::OperationalError { message }
}

fn is_state_schema_incompatible_message(message: &str) -> bool {
    message.contains("schema is incompatible") || message.contains("unrecognized schema shape")
}
