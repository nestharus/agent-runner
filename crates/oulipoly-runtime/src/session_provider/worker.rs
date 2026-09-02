use super::ingest::ingest_one_canonical_turn_page;
use super::types::{
    SessionProviderError, SessionProviderIdentity, SessionTurnIngestQuantumRequest,
};
use crate::provider_registry::ProviderRegistry;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use oulipoly_provider::client::CancellationToken;
use oulipoly_state::{
    SessionTurnIngestStream, SessionTurnIngestStreamKey, SessionTurnStreamProjection, StateDb,
};
use std::path::Path;
use std::time::Duration;

const DEFAULT_MAX_TURNS: u64 = 128;
const DEFAULT_MAX_RESPONSE_BYTES: u64 = 256 * 1024;
const DEFAULT_MAX_SOURCE_BYTES: u64 = 1024 * 1024;
const DEFAULT_MAX_INLINE_BODY_BYTES: u64 = 64 * 1024;
const DEFAULT_PAGE_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_LEASE_DURATION: ChronoDuration = ChronoDuration::seconds(75);
const MAX_RETRY_BACKOFF_SECS: i64 = 300;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionTurnIngestQuantumOutcome {
    Idle,
    Applied {
        key: SessionTurnIngestStreamKey,
        inserted_turns: u64,
        duplicate_turns: u64,
        checkpoint_generation: u64,
    },
    RetryScheduled {
        key: SessionTurnIngestStreamKey,
        error: String,
    },
    Unsupported {
        key: SessionTurnIngestStreamKey,
        error: String,
    },
    Quarantined {
        key: SessionTurnIngestStreamKey,
        error: String,
    },
}

pub struct SessionTurnIngestDriverRequest<'a> {
    pub state: &'a StateDb,
    pub registry: &'a ProviderRegistry,
    pub lease_owner: &'a str,
    pub effective_cwd: Option<&'a Path>,
    pub cancellation: &'a CancellationToken,
    pub now: DateTime<Utc>,
}

pub fn run_one_session_turn_ingest_quantum(
    request: SessionTurnIngestDriverRequest<'_>,
) -> Result<SessionTurnIngestQuantumOutcome, SessionProviderError> {
    let lease_expires_at = request.now + DEFAULT_LEASE_DURATION;
    let Some(stream) = request
        .state
        .lease_ready_session_turn_ingest_stream(
            SessionTurnStreamProjection::CanonicalIngest,
            request.lease_owner,
            request.now,
            lease_expires_at,
        )
        .map_err(state_error)?
    else {
        return Ok(SessionTurnIngestQuantumOutcome::Idle);
    };
    run_leased_session_turn_ingest_quantum(request, stream)
}

pub fn run_session_turn_ingest_quantum_for_key(
    request: SessionTurnIngestDriverRequest<'_>,
    key: &SessionTurnIngestStreamKey,
) -> Result<SessionTurnIngestQuantumOutcome, SessionProviderError> {
    let lease_expires_at = request.now + DEFAULT_LEASE_DURATION;
    let Some(stream) = request
        .state
        .lease_session_turn_ingest_stream(key, request.lease_owner, request.now, lease_expires_at)
        .map_err(state_error)?
    else {
        return Ok(SessionTurnIngestQuantumOutcome::Idle);
    };
    run_leased_session_turn_ingest_quantum(request, stream)
}

fn run_leased_session_turn_ingest_quantum(
    request: SessionTurnIngestDriverRequest<'_>,
    stream: SessionTurnIngestStream,
) -> Result<SessionTurnIngestQuantumOutcome, SessionProviderError> {
    let identity = SessionProviderIdentity {
        model_name: String::new(),
        provider_name: stream.key.provider_name.clone(),
        provider_instance_id: Some(stream.key.provider_instance_id.clone()),
        settings_id: stream.key.settings_id.clone(),
    };
    match ingest_one_canonical_turn_page(SessionTurnIngestQuantumRequest {
        state: request.state,
        registry: request.registry,
        lease_owner: request.lease_owner,
        identity,
        session_id: &stream.key.session_id,
        effective_cwd: request.effective_cwd,
        cancellation: request.cancellation,
        timeout: DEFAULT_PAGE_TIMEOUT,
        max_turns: DEFAULT_MAX_TURNS,
        max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
        max_source_bytes: DEFAULT_MAX_SOURCE_BYTES,
        max_inline_body_bytes: DEFAULT_MAX_INLINE_BODY_BYTES,
    }) {
        Ok(outcome) => Ok(SessionTurnIngestQuantumOutcome::Applied {
            key: stream.key,
            inserted_turns: outcome.inserted_turns,
            duplicate_turns: outcome.duplicate_turns,
            checkpoint_generation: outcome.checkpoint_generation,
        }),
        Err(error) => dispose_failed_quantum(request, stream, error),
    }
}

fn dispose_failed_quantum(
    request: SessionTurnIngestDriverRequest<'_>,
    stream: SessionTurnIngestStream,
    error: SessionProviderError,
) -> Result<SessionTurnIngestQuantumOutcome, SessionProviderError> {
    let error_token = error.token().to_string();
    if stream_is_quarantined(request.state, &stream.key)? {
        return Ok(SessionTurnIngestQuantumOutcome::Quarantined {
            key: stream.key,
            error: error_token,
        });
    }
    if unsupported_error(&error) {
        request
            .state
            .mark_session_turn_ingest_unsupported(
                &stream.key,
                request.lease_owner,
                stream.checkpoint_generation,
                &error_token,
            )
            .map_err(state_error)?;
        return Ok(SessionTurnIngestQuantumOutcome::Unsupported {
            key: stream.key,
            error: error_token,
        });
    }
    if permanent_protocol_error(&error) {
        request
            .state
            .quarantine_session_turn_ingest_stream(
                &stream.key,
                request.lease_owner,
                stream.checkpoint_generation,
                &error_token,
            )
            .map_err(state_error)?;
        return Ok(SessionTurnIngestQuantumOutcome::Quarantined {
            key: stream.key,
            error: error_token,
        });
    }
    let retry_at = request.now + retry_backoff(stream.retry_count);
    request
        .state
        .retry_session_turn_ingest_stream(
            &stream.key,
            request.lease_owner,
            stream.checkpoint_generation,
            retry_at,
            &error_token,
        )
        .map_err(state_error)?;
    Ok(SessionTurnIngestQuantumOutcome::RetryScheduled {
        key: stream.key,
        error: error_token,
    })
}

fn stream_is_quarantined(
    state: &StateDb,
    key: &SessionTurnIngestStreamKey,
) -> Result<bool, SessionProviderError> {
    state
        .session_turn_ingest_stream(key)
        .map(|stream| stream.is_some_and(|stream| stream.status == "quarantined"))
        .map_err(state_error)
}

fn unsupported_error(error: &SessionProviderError) -> bool {
    matches!(
        error.token(),
        "session_capability_missing" | "session_turn_pages_capability_missing"
    )
}

fn permanent_protocol_error(error: &SessionProviderError) -> bool {
    matches!(
        error.token(),
        "provider_page_response_budget_exceeded"
            | "provider_page_identity_mismatch"
            | "provider_page_turn_count_invalid"
            | "provider_page_source_budget_exceeded"
            | "provider_page_warning_bound_exceeded"
            | "provider_page_sequence_mismatch"
            | "provider_page_snapshot_mismatch"
            | "provider_page_completion_tokens_invalid"
            | "provider_page_scan_progress_invalid"
            | "provider_page_empty_without_progress"
            | "provider_page_scan_token_unchanged"
            | "provider_page_tail_anchor_invalid"
            | "provider_page_duplicate_turn_id"
            | "provider_page_turn_sequence_mismatch"
            | "provider_page_turn_timestamp_invalid"
            | "provider_page_inline_body_missing"
            | "provider_page_inline_body_invalid"
            | "provider_page_inline_body_metadata_mismatch"
            | "provider_page_canonical_text_digest_mismatch"
            | "provider_page_body_state_invalid"
            | "provider_page_digest_invalid"
            | "provider_page_field_bound_invalid"
            | "provider_page_digest_failed"
            | "session_turn_stream_identity_mismatch"
            | "session_turn_stream_projection_mismatch"
            | "session_turn_checkpoint_invalid"
    )
}

fn retry_backoff(retry_count: u64) -> ChronoDuration {
    let exponent = retry_count.min(8) as u32;
    let seconds = 1_i64
        .checked_shl(exponent)
        .unwrap_or(MAX_RETRY_BACKOFF_SECS)
        .min(MAX_RETRY_BACKOFF_SECS);
    ChronoDuration::seconds(seconds)
}

fn state_error(error: String) -> SessionProviderError {
    SessionProviderError::new("session_turn_page_state_failed", error)
}

#[cfg(test)]
mod tests {
    use super::permanent_protocol_error;
    use crate::session_provider::SessionProviderError;

    #[test]
    fn deterministic_page_conflicts_are_permanent() {
        for token in [
            "provider_page_identity_mismatch",
            "provider_page_sequence_mismatch",
            "provider_page_snapshot_mismatch",
            "provider_page_duplicate_turn_id",
            "provider_page_inline_body_metadata_mismatch",
            "provider_page_canonical_text_digest_mismatch",
        ] {
            assert!(permanent_protocol_error(&SessionProviderError::new(
                token, token
            )));
        }
        for token in ["provider_page_failed", "provider_process_timeout"] {
            assert!(!permanent_protocol_error(&SessionProviderError::new(
                token, token
            )));
        }
    }
}
