//! Declared roles: orchestration

use crate::wiring::AgentRuntimeServices;

pub(crate) fn run_pause_handshake(
    session_id: &str,
    ttl_ms: Option<u64>,
    agent_runtime_services: &AgentRuntimeServices,
) -> Result<i32, String> {
    let Some(ttl_ms) = super::validator::validate_pause_handshake_args(session_id, ttl_ms) else {
        return Ok(2);
    };

    let output = agent_runtime_services
        .session_lock_service
        .lock_session(super::mapper::pause_handshake_request(session_id, ttl_ms))
        .map_err(|err| err.to_string())?;
    super::formatter::render_pause_handshake_output(output.result)
}

pub(crate) fn run_resume_handshake(
    session_id: &str,
    token: &str,
    agent_runtime_services: &AgentRuntimeServices,
) -> Result<i32, String> {
    if let Some(exit_code) = super::validator::validate_resume_handshake_session_id(session_id) {
        return Ok(exit_code);
    }

    let output = agent_runtime_services
        .session_lock_service
        .lock_session(super::mapper::resume_handshake_request(session_id, token))
        .map_err(|err| err.to_string())?;
    super::formatter::render_resume_handshake_output(output.result)
}
