use super::enumerate;
use super::locate;
use super::provider_client::{
    invoke_session, provider_client, session_client, session_client_with_cancellation,
    session_enumerate_client, session_page_client,
};
use super::request::{
    base_request, capture_extra, enumerate_request, lifecycle_extra, live_capture_extra,
    locate_extra, page_request,
};
use super::turns;
use super::types::{
    SessionProviderCaptureRequest, SessionProviderCaptureResult, SessionProviderEnumerateRequest,
    SessionProviderEnumerateResult, SessionProviderError, SessionProviderIdentity,
    SessionProviderLifecycleContext, SessionProviderLiveCaptureRequest,
    SessionProviderLocateRequest, SessionProviderLocatedTranscript, SessionProviderReadPageRequest,
    SessionProviderReadPageResult,
};
use crate::provider_registry::DescribeHostOptions;
use crate::session_metadata::LocatedTranscript;
use oulipoly_provider::client::CancellationToken;
use oulipoly_provider::client::ProviderClient;
use oulipoly_provider::generated::{
    JsonObject, SessionCaptureResult as ProviderCaptureResult,
    SessionEnumerateResult as ProviderEnumerateResult, SessionLocateTranscriptResult,
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
    locate_transcript_with_client(&client, request)
}

pub(crate) fn locate_transcript_with_raw_metadata_with_cancellation(
    request: SessionProviderLocateRequest<'_>,
    cancellation: &CancellationToken,
) -> Result<SessionProviderLocatedTranscript, SessionProviderError> {
    let client =
        session_client_with_cancellation(request.registry, &request.identity, cancellation)?;
    locate_transcript_with_client(&client, request)
}

fn locate_transcript_with_client(
    client: &ProviderClient,
    request: SessionProviderLocateRequest<'_>,
) -> Result<SessionProviderLocatedTranscript, SessionProviderError> {
    let provider_result = invoke_session::<SessionLocateTranscriptResult>(
        client,
        "session.locate_transcript",
        base_request(
            &request.identity,
            Some(request.session_id),
            request.effective_cwd,
            request.registry.host_options(),
            locate_extra(
                request.lookup_mode,
                request.purpose,
                request.tail_bytes_hint,
            ),
            "locate",
        )?,
    )?;
    locate::map_locate_result_with_raw_metadata(provider_result, request.lookup_mode)
}

pub fn read_turn_page(
    request: SessionProviderReadPageRequest<'_>,
) -> Result<SessionProviderReadPageResult, SessionProviderError> {
    let client = session_page_client(
        request.registry,
        &request.identity,
        request.cancellation,
        request.timeout,
    )?;
    let built = page_request(&request)?;
    let result =
        invoke_session::<SessionReadTurnsResult>(&client, "session.read_turns", built.value)?;
    let captured_response_bytes = client.last_diagnostics().stdout.captured_len;
    turns::map_read_page_result(
        result,
        &request,
        captured_response_bytes,
        built.request_token_sha256,
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
        request.registry.host_options(),
        capture_extra(request.invocation_uuid),
    )
}

pub fn capture_live_report(
    request: SessionProviderLiveCaptureRequest<'_>,
) -> Result<SessionProviderCaptureResult, SessionProviderError> {
    let client = session_client(request.registry, &request.identity)?;
    capture_live_report_with_client(&client, request)
}

pub(crate) fn capture_live_report_with_client(
    client: &ProviderClient,
    request: SessionProviderLiveCaptureRequest<'_>,
) -> Result<SessionProviderCaptureResult, SessionProviderError> {
    capture_with_client(
        client,
        &request.identity,
        request.effective_cwd,
        request.registry.host_options(),
        live_capture_extra(request.invocation_uuid, request.provider_session_id),
    )
}

pub fn enumerate_sessions(
    request: SessionProviderEnumerateRequest<'_>,
) -> Result<SessionProviderEnumerateResult, SessionProviderError> {
    let client = session_enumerate_client(request.registry, &request.identity)?;
    let result = invoke_session::<ProviderEnumerateResult>(
        &client,
        "session.enumerate",
        enumerate_request(&request, request.registry.host_options())?,
    )?;
    enumerate::map_enumerate_result(result)
}

pub fn capture_for_lifecycle(
    context: &SessionProviderLifecycleContext<'_>,
) -> Result<SessionProviderCaptureResult, SessionProviderError> {
    let client = session_client(context.registry, &context.identity)?;
    capture_with_client(
        &client,
        &context.identity,
        context.effective_cwd,
        context.registry.host_options(),
        lifecycle_extra(context),
    )
}

fn capture_with_client(
    client: &ProviderClient,
    identity: &SessionProviderIdentity,
    effective_cwd: Option<&Path>,
    host_options: &DescribeHostOptions,
    extra: JsonObject,
) -> Result<SessionProviderCaptureResult, SessionProviderError> {
    let result = invoke_session::<ProviderCaptureResult>(
        client,
        "session.capture",
        base_request(
            identity,
            None,
            effective_cwd,
            host_options,
            extra,
            "capture",
        )?,
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
