use super::locate;
use super::provider_client::{invoke_session, provider_client, session_client};
use super::request::{
    base_request, capture_extra, lifecycle_extra, locate_extra, read_session_id_for_lifecycle,
};
use super::turns;
use super::types::{
    SessionProviderCaptureRequest, SessionProviderCaptureResult, SessionProviderError,
    SessionProviderIdentity, SessionProviderLifecycleContext, SessionProviderLocateRequest,
    SessionProviderLocatedTranscript, SessionProviderReadTurnsRequest, SessionProviderReadTurnsResult,
};
use crate::session_metadata::LocatedTranscript;
use oulipoly_provider::client::ProviderClient;
use oulipoly_provider::generated::{
    JsonObject, SessionCaptureResult as ProviderCaptureResult, SessionLocateTranscriptResult,
    SessionReadTurnsResult,
};
use std::path::Path;

pub fn locate_transcript(
    request: SessionProviderLocateRequest<'_>,
) -> Result<LocatedTranscript, SessionProviderError> {
    locate_transcript_with_raw_metadata(request).map(|located| LocatedTranscript {
        path: located.path,
        storage_classification: located.storage_classification,
        require_existing_observed: located.require_existing_observed,
    })
}

pub fn locate_transcript_with_raw_metadata(
    request: SessionProviderLocateRequest<'_>,
) -> Result<SessionProviderLocatedTranscript, SessionProviderError> {
    let client = session_client(request.registry, &request.identity)?;
    let provider_result = invoke_session::<SessionLocateTranscriptResult>(
        &client,
        "session.locate_transcript",
        base_request(
            &request.identity,
            Some(request.session_id),
            request.effective_cwd,
            locate_extra(request.lookup_mode, request.purpose, request.tail_bytes_hint),
            "locate",
        )?,
    )?;
    locate::map_locate_result_with_raw_metadata(provider_result, request.lookup_mode)
}

pub fn read_turns(
    request: SessionProviderReadTurnsRequest<'_>,
) -> Result<SessionProviderReadTurnsResult, SessionProviderError> {
    let client = provider_client(request.registry, &request.identity)?;
    read_turns_with_client(
        &client,
        &request.identity,
        Some(request.session_id),
        request.effective_cwd,
        JsonObject::new(),
    )
}

pub fn capture(
    request: SessionProviderCaptureRequest<'_>,
) -> Result<SessionProviderCaptureResult, SessionProviderError> {
    let client = provider_client(request.registry, &request.identity)?;
    capture_with_client(
        &client,
        &request.identity,
        request.effective_cwd,
        capture_extra(request.invocation_uuid),
    )
}

pub fn read_turns_for_lifecycle(
    context: &SessionProviderLifecycleContext<'_>,
) -> Result<SessionProviderReadTurnsResult, SessionProviderError> {
    let client = session_client(context.registry, &context.identity)?;
    read_turns_with_client(
        &client,
        &context.identity,
        read_session_id_for_lifecycle(context),
        context.effective_cwd,
        lifecycle_extra(context),
    )
}

pub fn capture_for_lifecycle(
    context: &SessionProviderLifecycleContext<'_>,
) -> Result<SessionProviderCaptureResult, SessionProviderError> {
    let client = session_client(context.registry, &context.identity)?;
    capture_with_client(
        &client,
        &context.identity,
        context.effective_cwd,
        lifecycle_extra(context),
    )
}

fn read_turns_with_client(
    client: &ProviderClient,
    identity: &SessionProviderIdentity,
    session_id: Option<&str>,
    effective_cwd: Option<&Path>,
    extra: JsonObject,
) -> Result<SessionProviderReadTurnsResult, SessionProviderError> {
    let result = invoke_session::<SessionReadTurnsResult>(
        client,
        "session.read_turns",
        base_request(identity, session_id, effective_cwd, extra, "read-turns")?,
    )?;
    turns::map_read_turns_result(result)
}

fn capture_with_client(
    client: &ProviderClient,
    identity: &SessionProviderIdentity,
    effective_cwd: Option<&Path>,
    extra: JsonObject,
) -> Result<SessionProviderCaptureResult, SessionProviderError> {
    let result = invoke_session::<ProviderCaptureResult>(
        client,
        "session.capture",
        base_request(identity, None, effective_cwd, extra, "capture")?,
    )?;
    Ok(map_capture_result(result))
}

fn map_capture_result(result: ProviderCaptureResult) -> SessionProviderCaptureResult {
    SessionProviderCaptureResult {
        provider_session_id: non_empty_optional(result.provider_session_id),
        state: result.state,
        artifacts: result.artifacts,
    }
}

fn non_empty_optional(input: Option<String>) -> Option<String> {
    input.filter(|value| !value.is_empty())
}
