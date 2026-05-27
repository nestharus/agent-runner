//! Declared roles: orchestration

use crate::wiring::AgentRuntimeServices;
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
    let request = super::import_replace_request(session_id, from_file, preimage_sha256);
    let output = agent_runtime_services
        .session_replace_service
        .replace_session(request)
        .map_err(|err| err.to_string())?;

    super::render_import_replace_output(output.result)
}
