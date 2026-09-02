use super::dispatch::read_turn_page;
use super::turns;
use super::types::{
    SessionProviderError, SessionProviderIdentity, SessionProviderPageCursor,
    SessionProviderReadPageRequest, SessionProviderTurnProjection, SessionTurnIngestQuantumRequest,
};
use oulipoly_state::{
    SessionTurnIngestStreamKey, SessionTurnPageApplyOutcome, SessionTurnStreamProjection,
};

pub fn canonical_stream_key(
    identity: &SessionProviderIdentity,
    session_id: &str,
) -> Result<SessionTurnIngestStreamKey, SessionProviderError> {
    let provider_instance_id = identity
        .provider_instance_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            SessionProviderError::new(
                "session_provider_instance_identity_missing",
                "canonical ingest requires an authenticated provider instance identity",
            )
        })?;
    Ok(SessionTurnIngestStreamKey {
        provider_name: identity.provider_name.clone(),
        provider_instance_id: provider_instance_id.to_string(),
        settings_id: identity.settings_id.clone(),
        session_id: session_id.to_string(),
        projection: SessionTurnStreamProjection::CanonicalIngest,
    })
}

pub fn ingest_one_canonical_turn_page(
    request: SessionTurnIngestQuantumRequest<'_>,
) -> Result<SessionTurnPageApplyOutcome, SessionProviderError> {
    let key = canonical_stream_key(&request.identity, request.session_id)?;
    let stream = request
        .state
        .session_turn_ingest_stream(&key)
        .map_err(state_error)?
        .ok_or_else(|| {
            SessionProviderError::new(
                "session_turn_stream_missing",
                "canonical session turn ingest stream is not queued",
            )
        })?;
    if stream.lease_owner.as_deref() != Some(request.lease_owner) {
        return Err(SessionProviderError::new(
            "session_turn_stream_lease_lost",
            "canonical session turn ingest stream is not leased by this worker",
        ));
    }
    let cursor = match (stream.snapshot_id, stream.next_page_token) {
        (Some(snapshot_id), Some(page_token)) => SessionProviderPageCursor::Continuation {
            snapshot_id,
            page_token,
        },
        (None, None) => SessionProviderPageCursor::Beginning {
            after_token: stream.after_token,
        },
        _ => {
            return Err(SessionProviderError::new(
                "session_turn_checkpoint_invalid",
                "active snapshot checkpoint is incomplete",
            ));
        }
    };
    let page = read_turn_page(SessionProviderReadPageRequest {
        registry: request.registry,
        identity: request.identity,
        session_id: request.session_id,
        effective_cwd: request.effective_cwd,
        projection: SessionProviderTurnProjection::CanonicalIngest,
        expected_delivery_nonce: None,
        cursor,
        expected_page_index: stream.expected_page_index,
        expected_turn_sequence: stream.expected_turn_sequence,
        max_turns: request.max_turns,
        max_response_bytes: request.max_response_bytes,
        max_source_bytes: request.max_source_bytes,
        max_inline_body_bytes: request.max_inline_body_bytes,
        cancellation: request.cancellation,
        timeout: request.timeout,
    })?;
    let apply = turns::page_apply(
        key,
        request.lease_owner.to_string(),
        stream.checkpoint_generation,
        page,
    )?;
    request
        .state
        .apply_session_turn_page(&apply)
        .map_err(state_error)
}

fn state_error(error: String) -> SessionProviderError {
    SessionProviderError::new("session_turn_page_state_failed", error)
}
