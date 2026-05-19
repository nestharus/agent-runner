//! ## Declared roles
//! orchestration, mapper, predicate, accessor, formatter
//!
//! Session marker emission helper. It owns the update/mint/payload/write
//! sequence for OULIPOLY_SESSION marker emission.

use super::dtos::{SessionLifecycleIngestMode, SessionLifecycleOutput};
use super::error::ServiceError;
use super::session_warning::write_session_ingest_warning;
use oulipoly_state::{InvocationRecord, StateDb};
use std::io::Write;

pub(super) fn emit_pinned_session_id_for_service(
    state: &StateDb,
    stderr: &mut dyn Write,
    invocation_row_id: i64,
    invocation_uuid: &str,
    mode: SessionLifecycleIngestMode,
) -> Result<SessionLifecycleOutput, ServiceError> {
    match mode {
        SessionLifecycleIngestMode::Unpinned { .. } => Ok(SessionLifecycleOutput {
            emitted: false,
            session_id: None,
        }),
        SessionLifecycleIngestMode::Pinned { resume_target } => emit_known_session_id_for_service(
            state,
            stderr,
            invocation_row_id,
            invocation_uuid,
            &resume_target,
            "resumed",
        ),
    }
}

pub(super) fn emit_known_session_id_for_service(
    state: &StateDb,
    stderr: &mut dyn Write,
    invocation_row_id: i64,
    invocation_uuid: &str,
    session_id: &str,
    capture_method: &str,
) -> Result<SessionLifecycleOutput, ServiceError> {
    if let Err(err) =
        update_session_capture_for_marker(state, invocation_row_id, session_id, capture_method)
    {
        write_session_capture_update_warning(stderr, err)?;
        return Ok(not_emitted_session_lifecycle_output());
    }
    let record = invocation_record_for_marker(state, invocation_uuid);
    mint_chain_for_marker_if_needed(state, stderr, invocation_row_id, record.as_ref())?;
    let payload = session_marker_payload(
        state,
        invocation_uuid,
        session_id,
        capture_method,
        record.as_ref(),
    );
    write_session_marker(stderr, &payload)?;
    Ok(emitted_session_lifecycle_output(session_id))
}

fn update_session_capture_for_marker(
    state: &StateDb,
    invocation_row_id: i64,
    session_id: &str,
    capture_method: &str,
) -> Result<(), String> {
    state.update_session_capture(invocation_row_id, Some(session_id), capture_method)
}

fn write_session_capture_update_warning(
    stderr: &mut dyn Write,
    err: String,
) -> Result<(), ServiceError> {
    write_session_ingest_warning(
        stderr,
        &format!("Failed to update invocation session_id: {err}"),
    )
}

fn not_emitted_session_lifecycle_output() -> SessionLifecycleOutput {
    SessionLifecycleOutput {
        emitted: false,
        session_id: None,
    }
}

fn invocation_record_for_marker(
    state: &StateDb,
    invocation_uuid: &str,
) -> Option<InvocationRecord> {
    state.get_invocation_by_uuid(invocation_uuid).ok().flatten()
}

fn emitted_session_lifecycle_output(session_id: &str) -> SessionLifecycleOutput {
    SessionLifecycleOutput {
        emitted: true,
        session_id: Some(session_id.to_string()),
    }
}

fn mint_chain_for_marker_if_needed(
    state: &StateDb,
    stderr: &mut dyn Write,
    invocation_row_id: i64,
    record: Option<&InvocationRecord>,
) -> Result<(), ServiceError> {
    if !should_mint_chain_for_marker(record) {
        return Ok(());
    }
    write_marker_chain_mint_warning_on_error(
        stderr,
        state.mint_chain_for_invocation_session(invocation_row_id),
    )
}

fn write_marker_chain_mint_warning_on_error(
    stderr: &mut dyn Write,
    result: Result<(), String>,
) -> Result<(), ServiceError> {
    if let Err(err) = result {
        write_session_ingest_warning(stderr, &format!("Failed to mint session chain: {err}"))?;
    }
    Ok(())
}

fn should_mint_chain_for_marker(record: Option<&InvocationRecord>) -> bool {
    record.is_none_or(|row| row.resume_input_id.as_deref() != row.provider_session_id.as_deref())
}

fn session_marker_payload(
    state: &StateDb,
    invocation_uuid: &str,
    session_id: &str,
    capture_method: &str,
    record: Option<&InvocationRecord>,
) -> oulipoly_state::SessionMarkerPayload {
    let provider_name = marker_provider_name(record);
    let provider_session_id = marker_provider_session_id(record, capture_method, session_id);
    let agent_runner_chain_id = marker_agent_runner_chain_id(
        state,
        provider_name.as_deref(),
        provider_session_id.as_deref(),
    );

    oulipoly_state::SessionMarkerPayload {
        agent_runner_invocation_id: invocation_uuid.to_string(),
        provider_session_id: provider_session_id.clone(),
        provider_name,
        agent_runner_chain_id,
        resume_input_id: record.and_then(|row| row.resume_input_id.clone()),
        legacy_id: invocation_uuid.to_string(),
        legacy_session_id: Some(session_id.to_string()),
    }
}

fn marker_provider_name(record: Option<&InvocationRecord>) -> Option<String> {
    record.and_then(|row| row.provider_name.clone())
}

fn marker_provider_session_id(
    record: Option<&InvocationRecord>,
    capture_method: &str,
    session_id: &str,
) -> Option<String> {
    record
        .and_then(|row| row.provider_session_id.clone())
        .or_else(|| fallback_marker_provider_session_id(capture_method, session_id))
}

fn fallback_marker_provider_session_id(capture_method: &str, session_id: &str) -> Option<String> {
    if capture_method == "resumed" {
        None
    } else {
        Some(session_id.to_string())
    }
}

fn marker_agent_runner_chain_id(
    state: &StateDb,
    provider_name: Option<&str>,
    provider_session_id: Option<&str>,
) -> Option<String> {
    let provider_name = provider_name?;
    let provider_session_id = provider_session_id?;
    state
        .chain_id_for_segment(provider_name, provider_session_id)
        .ok()
        .flatten()
}

fn write_session_marker(
    stderr: &mut dyn Write,
    payload: &oulipoly_state::SessionMarkerPayload,
) -> Result<(), ServiceError> {
    write!(stderr, "{}", payload.stderr_line()).map_err(|err| ServiceError::Dependency {
        message: format!("Failed to write session marker: {err}"),
    })
}
