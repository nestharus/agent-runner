//! Session ingest CLI marker and warning helpers.
//!
//! ## Declared roles
//!
//! `orchestration`, `mapper`, `formatter`, `accessor`, `predicate`

use std::path::Path;

use oulipoly_config::ProvidersConfig;
use oulipoly_runtime::services::{
    ServiceError, SessionLifecycleIngestMode, SessionLifecycleRequest,
};
use oulipoly_state::{InvocationRecord, StateDb};

use crate::wiring;

pub(crate) enum ResumeIngestMode<'a> {
    Unpinned { capture_method: &'a str },
    Pinned { resume_target: &'a str },
}

pub(crate) struct SessionIngestRequest<'a> {
    pub(crate) state: &'a StateDb,
    pub(crate) sessions_cfg: &'a oulipoly_config::SessionsConfig,
    pub(crate) providers_cfg: Option<&'a ProvidersConfig>,
    pub(crate) provider_name: &'a str,
    pub(crate) invocation_row_id: i64,
    pub(crate) invocation_uuid: &'a str,
    pub(crate) effective_cwd: Option<&'a Path>,
    pub(crate) mode: ResumeIngestMode<'a>,
}

pub(crate) fn ingest_and_emit_session_id_resume_aware(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    request: SessionIngestRequest<'_>,
) -> bool {
    let mut stderr = std::io::stderr();
    let SessionIngestRequest {
        state,
        sessions_cfg,
        providers_cfg,
        provider_name,
        invocation_row_id,
        invocation_uuid,
        effective_cwd,
        mode,
    } = request;
    let lifecycle_request = session_lifecycle_request(SessionLifecycleRequestInput {
        state,
        sessions_cfg,
        providers_cfg,
        provider_name,
        invocation_row_id,
        invocation_uuid,
        effective_cwd,
        mode: session_lifecycle_ingest_mode(mode),
        stderr: &mut stderr,
    });
    emit_session_lifecycle_ingest_result(
        provider_name,
        agent_runtime_services
            .session_lifecycle_service
            .ingest_session(lifecycle_request),
    )
}

fn emit_session_lifecycle_ingest_result(
    provider_name: &str,
    result: Result<oulipoly_runtime::services::SessionLifecycleOutput, ServiceError>,
) -> bool {
    match session_lifecycle_ingest_result(result) {
        SessionLifecycleIngestResult::Emitted(emitted) => emitted,
        SessionLifecycleIngestResult::Failed(message) => {
            emit_session_ingest_failure(provider_name, &message);
            session_lifecycle_ingest_failed_result()
        }
    }
}

enum SessionLifecycleIngestResult {
    Emitted(bool),
    Failed(String),
}

fn session_lifecycle_ingest_result(
    result: Result<oulipoly_runtime::services::SessionLifecycleOutput, ServiceError>,
) -> SessionLifecycleIngestResult {
    match result {
        Ok(output) => SessionLifecycleIngestResult::Emitted(output.emitted),
        Err(ServiceError::Dependency { message })
        | Err(ServiceError::InvalidRequest { message })
        | Err(ServiceError::Unavailable { message }) => {
            SessionLifecycleIngestResult::Failed(message)
        }
    }
}

fn session_lifecycle_ingest_failed_result() -> bool {
    false
}

struct SessionLifecycleRequestInput<'a> {
    state: &'a StateDb,
    sessions_cfg: &'a oulipoly_config::SessionsConfig,
    providers_cfg: Option<&'a ProvidersConfig>,
    provider_name: &'a str,
    invocation_row_id: i64,
    invocation_uuid: &'a str,
    effective_cwd: Option<&'a Path>,
    mode: SessionLifecycleIngestMode,
    stderr: &'a mut std::io::Stderr,
}

fn session_lifecycle_request(
    input: SessionLifecycleRequestInput<'_>,
) -> SessionLifecycleRequest<'_> {
    SessionLifecycleRequest {
        state: input.state,
        sessions_cfg: input.sessions_cfg,
        providers_cfg: input.providers_cfg,
        provider_name: input.provider_name,
        invocation_row_id: input.invocation_row_id,
        invocation_uuid: input.invocation_uuid,
        effective_cwd: input.effective_cwd,
        mode: input.mode,
        stderr: input.stderr,
    }
}

fn session_lifecycle_ingest_mode(mode: ResumeIngestMode<'_>) -> SessionLifecycleIngestMode {
    match mode {
        ResumeIngestMode::Unpinned { capture_method } => SessionLifecycleIngestMode::Unpinned {
            capture_method: capture_method.to_string(),
        },
        ResumeIngestMode::Pinned { resume_target } => SessionLifecycleIngestMode::Pinned {
            resume_target: resume_target.to_string(),
        },
    }
}

fn format_session_ingest_failure(provider_name: &str, message: &str) -> String {
    format!("Warning: Session ingest failed for {provider_name}: {message}")
}

fn emit_session_ingest_failure(provider_name: &str, message: &str) {
    eprintln!("{}", format_session_ingest_failure(provider_name, message));
}

pub(crate) fn emit_known_session_id(
    state: &StateDb,
    invocation_row_id: i64,
    invocation_uuid: &str,
    session_id: &str,
    capture_method: &str,
) -> bool {
    if !emit_known_session_capture_update(state, invocation_row_id, session_id, capture_method) {
        return false;
    }
    emit_known_session_marker_for_capture(state, invocation_row_id, invocation_uuid, session_id);
    true
}

fn emit_known_session_marker_for_capture(
    state: &StateDb,
    invocation_row_id: i64,
    invocation_uuid: &str,
    session_id: &str,
) {
    let record = lookup_invocation_record(state, invocation_uuid);
    mint_known_session_chain_if_needed(state, invocation_row_id, record.as_ref());
    emit_known_session_marker(known_session_marker_payload(
        invocation_uuid,
        session_id,
        record.as_ref(),
        known_session_marker_chain_id(state, session_id, record.as_ref()),
    ));
}

fn emit_known_session_capture_update(
    state: &StateDb,
    invocation_row_id: i64,
    session_id: &str,
    capture_method: &str,
) -> bool {
    match update_known_session_capture(state, invocation_row_id, Some(session_id), capture_method) {
        Ok(()) => true,
        Err(err) => {
            emit_known_session_capture_warning(&err);
            false
        }
    }
}

fn emit_known_session_capture_warning(err: &str) {
    eprintln!("Warning: Failed to update invocation session_id: {err}");
}

fn lookup_invocation_record(state: &StateDb, invocation_uuid: &str) -> Option<InvocationRecord> {
    state.get_invocation_by_uuid(invocation_uuid).ok().flatten()
}

fn mint_known_session_chain_if_needed(
    state: &StateDb,
    invocation_row_id: i64,
    record: Option<&InvocationRecord>,
) {
    if should_mint_known_session_chain(record) {
        mint_known_session_chain(state, invocation_row_id)
            .unwrap_or_else(|err| emit_known_session_chain_warning(&err));
    }
}

fn mint_known_session_chain(state: &StateDb, invocation_row_id: i64) -> Result<(), String> {
    state.mint_chain_for_invocation_session(invocation_row_id)
}

fn emit_known_session_chain_warning(err: &str) {
    eprintln!("Warning: Failed to mint session chain: {err}");
}

fn emit_known_session_marker(payload: oulipoly_state::SessionMarkerPayload) {
    eprint!("{}", payload.stderr_line());
}

fn update_known_session_capture(
    state: &StateDb,
    invocation_row_id: i64,
    session_id: Option<&str>,
    capture_method: &str,
) -> Result<(), String> {
    state.update_session_capture(invocation_row_id, session_id, capture_method)
}

fn should_mint_known_session_chain(record: Option<&InvocationRecord>) -> bool {
    record.is_none_or(|row| row.resume_input_id.as_deref() != row.provider_session_id.as_deref())
}

fn known_session_marker_chain_id(
    state: &StateDb,
    session_id: &str,
    record: Option<&InvocationRecord>,
) -> Option<String> {
    let fields = known_session_marker_fields(record, session_id);
    lookup_marker_chain_id(state, marker_chain_lookup_key(&fields))
}

struct MarkerChainLookupKey<'a> {
    provider_name: Option<&'a str>,
    provider_session_id: Option<&'a str>,
}

fn marker_chain_lookup_key(fields: &KnownSessionMarkerFields) -> MarkerChainLookupKey<'_> {
    MarkerChainLookupKey {
        provider_name: fields.provider_name.as_deref(),
        provider_session_id: fields.provider_session_id.as_deref(),
    }
}

fn known_session_marker_payload(
    invocation_uuid: &str,
    session_id: &str,
    record: Option<&InvocationRecord>,
    agent_runner_chain_id: Option<String>,
) -> oulipoly_state::SessionMarkerPayload {
    let fields = known_session_marker_fields(record, session_id);
    session_marker_payload_from_parts(marker_payload_parts(
        invocation_uuid,
        session_id,
        fields,
        agent_runner_chain_id,
    ))
}

struct KnownSessionMarkerFields {
    provider_name: Option<String>,
    provider_session_id: Option<String>,
    resume_input_id: Option<String>,
}

fn known_session_marker_fields(
    record: Option<&InvocationRecord>,
    session_id: &str,
) -> KnownSessionMarkerFields {
    let provider_name = record.and_then(|row| row.provider_name.clone());
    let provider_session_id = record
        .and_then(|row| row.provider_session_id.clone())
        .or_else(|| Some(session_id.to_string()));
    KnownSessionMarkerFields {
        provider_name,
        provider_session_id,
        resume_input_id: record.and_then(|row| row.resume_input_id.clone()),
    }
}

fn marker_payload_parts<'a>(
    invocation_uuid: &'a str,
    session_id: &'a str,
    fields: KnownSessionMarkerFields,
    agent_runner_chain_id: Option<String>,
) -> SessionMarkerPayloadParts<'a> {
    SessionMarkerPayloadParts {
        invocation_uuid,
        session_id,
        provider_name: fields.provider_name,
        provider_session_id: fields.provider_session_id,
        agent_runner_chain_id,
        resume_input_id: fields.resume_input_id,
    }
}

struct SessionMarkerPayloadParts<'a> {
    invocation_uuid: &'a str,
    session_id: &'a str,
    provider_name: Option<String>,
    provider_session_id: Option<String>,
    agent_runner_chain_id: Option<String>,
    resume_input_id: Option<String>,
}

fn lookup_marker_chain_id(state: &StateDb, key: MarkerChainLookupKey<'_>) -> Option<String> {
    key.provider_name.and_then(|provider_name| {
        key.provider_session_id.and_then(|provider_session_id| {
            state
                .chain_id_for_segment(provider_name, provider_session_id)
                .ok()
                .flatten()
        })
    })
}

fn session_marker_payload_from_parts(
    parts: SessionMarkerPayloadParts<'_>,
) -> oulipoly_state::SessionMarkerPayload {
    oulipoly_state::SessionMarkerPayload {
        agent_runner_invocation_id: parts.invocation_uuid.to_string(),
        provider_session_id: parts.provider_session_id,
        provider_name: parts.provider_name,
        agent_runner_chain_id: parts.agent_runner_chain_id,
        resume_input_id: parts.resume_input_id,
        legacy_id: parts.invocation_uuid.to_string(),
        legacy_session_id: Some(parts.session_id.to_string()),
    }
}
