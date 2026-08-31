use super::request::provider_instance_id;
use super::types::{
    SessionProviderError, SessionProviderPageCursor, SessionProviderPageTurn,
    SessionProviderReadPageRequest, SessionProviderReadPageResult, SessionProviderTurnProjection,
};
use chrono::{DateTime, Utc};
use oulipoly_provider::generated::{
    SessionReadTurnsResult as ProviderReadPageResult, SessionTurnBodyState,
    SessionTurnPageProjection,
};
use oulipoly_state::{
    SessionTurnIngestStreamKey, SessionTurnPageApply, SessionTurnPageBodyState,
    SessionTurnPageTurnIngest,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;

const MAX_ID_BYTES: usize = 1024;
const MAX_ROLE_BYTES: usize = 64;
const MAX_TIMESTAMP_BYTES: usize = 128;
const MAX_TOKEN_BYTES: usize = 4096;
const MAX_WARNING_BYTES: usize = 1024;
const MAX_WARNINGS: usize = 32;

pub(super) fn map_read_page_result(
    result: ProviderReadPageResult,
    request: &SessionProviderReadPageRequest<'_>,
    captured_response_bytes: usize,
    request_token_sha256: String,
) -> Result<SessionProviderReadPageResult, SessionProviderError> {
    validate_page_envelope(&result, request, captured_response_bytes)?;
    let turns = map_page_turns(&result, request)?;
    let page_digest = page_digest(&result)?;
    Ok(SessionProviderReadPageResult {
        provider_instance_id: result.provider_instance_id,
        settings_id: result.settings_id,
        session_id: result.session_id,
        projection: map_projection(result.turn_projection),
        snapshot_id: result.snapshot_id,
        page_index: result.page_index,
        page_start_sequence: result.page_start_sequence,
        turns,
        page_turn_count: result.page_turn_count,
        source_bytes_examined: result.source_bytes_examined,
        scan_progress: result.scan_progress,
        snapshot_complete: result.snapshot_complete,
        next_page_token: result.next_page_token,
        resume_token: result.resume_token,
        source_final: result.source_final,
        warnings: result.warnings,
        request_token_sha256,
        page_digest,
    })
}

pub(super) fn page_apply(
    key: SessionTurnIngestStreamKey,
    lease_owner: String,
    expected_generation: u64,
    page: SessionProviderReadPageResult,
) -> Result<SessionTurnPageApply, SessionProviderError> {
    let turns = page
        .turns
        .into_iter()
        .map(state_page_turn)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(SessionTurnPageApply {
        key,
        lease_owner,
        expected_generation,
        request_token_sha256: page.request_token_sha256,
        snapshot_id: page.snapshot_id,
        page_index: page.page_index,
        page_start_sequence: page.page_start_sequence,
        page_turn_count: page.page_turn_count,
        scan_progress: page.scan_progress,
        snapshot_complete: page.snapshot_complete,
        next_page_token: page.next_page_token,
        resume_token: page.resume_token,
        page_digest: page.page_digest,
        turns,
    })
}

fn validate_page_envelope(
    result: &ProviderReadPageResult,
    request: &SessionProviderReadPageRequest<'_>,
    captured_response_bytes: usize,
) -> Result<(), SessionProviderError> {
    if captured_response_bytes > request.max_response_bytes as usize {
        return Err(page_error("provider_page_response_budget_exceeded"));
    }
    if result.provider_instance_id != provider_instance_id(&request.identity)
        || result.settings_id != request.identity.settings_id
        || result.session_id != request.session_id
        || map_projection(result.turn_projection) != request.projection
    {
        return Err(page_error("provider_page_identity_mismatch"));
    }
    validate_bounded_field("snapshot_id", &result.snapshot_id, MAX_TOKEN_BYTES)?;
    validate_page_position(result, request)?;
    if result.page_turn_count != result.turns.len() as u64
        || result.page_turn_count > request.max_turns
    {
        return Err(page_error("provider_page_turn_count_invalid"));
    }
    if result.source_bytes_examined > request.max_source_bytes {
        return Err(page_error("provider_page_source_budget_exceeded"));
    }
    validate_completion_tokens(result)?;
    validate_scan_progress(result, request)?;
    if result.warnings.len() > MAX_WARNINGS
        || result
            .warnings
            .iter()
            .any(|warning| warning.len() > MAX_WARNING_BYTES)
    {
        return Err(page_error("provider_page_warning_bound_exceeded"));
    }
    Ok(())
}

fn validate_page_position(
    result: &ProviderReadPageResult,
    request: &SessionProviderReadPageRequest<'_>,
) -> Result<(), SessionProviderError> {
    if result.page_index != request.expected_page_index
        || result.page_start_sequence != request.expected_turn_sequence
    {
        return Err(page_error("provider_page_sequence_mismatch"));
    }
    match &request.cursor {
        SessionProviderPageCursor::Beginning { .. } | SessionProviderPageCursor::Tail
            if result.page_index == 0 =>
        {
            Ok(())
        }
        SessionProviderPageCursor::Continuation { snapshot_id, .. }
            if snapshot_id == &result.snapshot_id =>
        {
            Ok(())
        }
        _ => Err(page_error("provider_page_snapshot_mismatch")),
    }
}

fn validate_completion_tokens(result: &ProviderReadPageResult) -> Result<(), SessionProviderError> {
    if result.snapshot_complete != result.resume_token.is_some()
        || result.snapshot_complete == result.next_page_token.is_some()
    {
        return Err(page_error("provider_page_completion_tokens_invalid"));
    }
    validate_optional_token("next_page_token", result.next_page_token.as_deref())?;
    validate_optional_token("resume_token", result.resume_token.as_deref())
}

fn validate_scan_progress(
    result: &ProviderReadPageResult,
    request: &SessionProviderReadPageRequest<'_>,
) -> Result<(), SessionProviderError> {
    if result.scan_progress && (!result.turns.is_empty() || result.snapshot_complete) {
        return Err(page_error("provider_page_scan_progress_invalid"));
    }
    if result.turns.is_empty() && !result.snapshot_complete && !result.scan_progress {
        return Err(page_error("provider_page_empty_without_progress"));
    }
    if result.scan_progress
        && let SessionProviderPageCursor::Continuation { page_token, .. } = &request.cursor
        && result.next_page_token.as_deref() == Some(page_token)
    {
        return Err(page_error("provider_page_scan_token_unchanged"));
    }
    if matches!(request.cursor, SessionProviderPageCursor::Tail)
        && (!result.turns.is_empty() || !result.snapshot_complete)
    {
        return Err(page_error("provider_page_tail_anchor_invalid"));
    }
    Ok(())
}

fn map_page_turns(
    result: &ProviderReadPageResult,
    request: &SessionProviderReadPageRequest<'_>,
) -> Result<Vec<SessionProviderPageTurn>, SessionProviderError> {
    let mut seen = HashSet::with_capacity(result.turns.len());
    result
        .turns
        .iter()
        .enumerate()
        .map(|(offset, turn)| {
            if !seen.insert(turn.turn_id.as_str()) {
                return Err(page_error("provider_page_duplicate_turn_id"));
            }
            if turn.session_id != request.session_id
                || turn.snapshot_sequence != result.page_start_sequence + offset as u64
            {
                return Err(page_error("provider_page_turn_sequence_mismatch"));
            }
            map_page_turn(turn, request.max_inline_body_bytes)
        })
        .collect()
}

fn map_page_turn(
    turn: &oulipoly_provider::generated::SessionTurnPageTurn,
    max_inline_body_bytes: u64,
) -> Result<SessionProviderPageTurn, SessionProviderError> {
    validate_bounded_field("session_id", &turn.session_id, MAX_ID_BYTES)?;
    validate_bounded_field("turn_id", &turn.turn_id, MAX_ID_BYTES)?;
    validate_bounded_field("role", &turn.role, MAX_ROLE_BYTES)?;
    validate_bounded_field("timestamp", &turn.timestamp, MAX_TIMESTAMP_BYTES)?;
    if let Some(parent) = turn.parent_turn_id.as_deref() {
        validate_bounded_field("parent_turn_id", parent, MAX_ID_BYTES)?;
    }
    let timestamp = DateTime::parse_from_rfc3339(&turn.timestamp)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| page_error("provider_page_turn_timestamp_invalid"))?;
    let (body, canonical_text_digest_verified) =
        validate_and_map_body(turn, max_inline_body_bytes)?;
    Ok(SessionProviderPageTurn {
        session_id: turn.session_id.clone(),
        turn_id: turn.turn_id.clone(),
        snapshot_sequence: turn.snapshot_sequence,
        timestamp,
        role: turn.role.clone(),
        parent_turn_id: turn.parent_turn_id.clone(),
        is_sidechain: turn.is_sidechain,
        is_compaction_boundary: turn.is_compaction_boundary,
        body_state: map_body_state(turn.body_state),
        body,
        body_bytes: turn.body_bytes,
        body_sha256: turn.body_sha256.clone(),
        canonical_text_sha256: turn.canonical_text_sha256.clone(),
        canonical_text_digest_verified,
    })
}

fn validate_and_map_body(
    turn: &oulipoly_provider::generated::SessionTurnPageTurn,
    max_inline_body_bytes: u64,
) -> Result<(Option<Value>, bool), SessionProviderError> {
    match turn.body_state {
        SessionTurnBodyState::Inline => {
            let chunks = turn
                .body
                .as_ref()
                .ok_or_else(|| page_error("provider_page_inline_body_missing"))?;
            let bytes = serde_json::to_vec(chunks)
                .map_err(|_| page_error("provider_page_inline_body_invalid"))?;
            if bytes.len() as u64 > max_inline_body_bytes
                || turn.body_bytes != Some(bytes.len() as u64)
                || turn.body_sha256.as_deref() != Some(sha256(&bytes).as_str())
            {
                return Err(page_error("provider_page_inline_body_metadata_mismatch"));
            }
            if let Some(expected) = turn.canonical_text_sha256.as_deref()
                && expected != canonical_text_sha256(chunks)
            {
                return Err(page_error("provider_page_canonical_text_digest_mismatch"));
            }
            serde_json::to_value(chunks)
                .map(|body| (Some(body), turn.canonical_text_sha256.is_some()))
                .map_err(|_| page_error("provider_page_inline_body_invalid"))
        }
        SessionTurnBodyState::Absent
            if turn.body.is_none()
                && turn.body_bytes.is_none()
                && turn.body_sha256.is_none()
                && turn.canonical_text_sha256.is_none() =>
        {
            Ok((None, false))
        }
        SessionTurnBodyState::OmittedOversize if turn.body.is_none() => {
            validate_optional_sha256(turn.body_sha256.as_deref())?;
            validate_optional_sha256(turn.canonical_text_sha256.as_deref())?;
            Ok((None, false))
        }
        _ => Err(page_error("provider_page_body_state_invalid")),
    }
}

fn canonical_text_sha256(chunks: &[oulipoly_provider::generated::SessionTurnBodyChunk]) -> String {
    let text = chunks
        .iter()
        .filter_map(|chunk| chunk.text.as_deref())
        .collect::<String>();
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    sha256(normalized.trim().as_bytes())
}

fn validate_optional_sha256(value: Option<&str>) -> Result<(), SessionProviderError> {
    if value.is_none_or(|hash| {
        hash.len() == 64
            && hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    }) {
        Ok(())
    } else {
        Err(page_error("provider_page_digest_invalid"))
    }
}

fn validate_bounded_field(name: &str, value: &str, max: usize) -> Result<(), SessionProviderError> {
    if value.is_empty() || value.len() > max {
        Err(SessionProviderError::new(
            "provider_page_field_bound_invalid",
            format!("provider page field {name} exceeded its bound"),
        ))
    } else {
        Ok(())
    }
}

fn validate_optional_token(name: &str, token: Option<&str>) -> Result<(), SessionProviderError> {
    if let Some(token) = token {
        validate_bounded_field(name, token, MAX_TOKEN_BYTES)?;
    }
    Ok(())
}

fn map_projection(projection: SessionTurnPageProjection) -> SessionProviderTurnProjection {
    match projection {
        SessionTurnPageProjection::CanonicalIngest => {
            SessionProviderTurnProjection::CanonicalIngest
        }
        SessionTurnPageProjection::UserObservation => {
            SessionProviderTurnProjection::UserObservation
        }
    }
}

fn map_body_state(state: SessionTurnBodyState) -> SessionTurnPageBodyState {
    match state {
        SessionTurnBodyState::Inline => SessionTurnPageBodyState::Inline,
        SessionTurnBodyState::Absent => SessionTurnPageBodyState::Absent,
        SessionTurnBodyState::OmittedOversize => SessionTurnPageBodyState::OmittedOversize,
    }
}

fn state_page_turn(
    turn: SessionProviderPageTurn,
) -> Result<SessionTurnPageTurnIngest, SessionProviderError> {
    let body = turn
        .body
        .map(|body| serde_json::to_string(&body))
        .transpose()
        .map_err(|_| page_error("provider_page_inline_body_invalid"))?;
    Ok(SessionTurnPageTurnIngest {
        session_id: turn.session_id,
        turn_id: turn.turn_id,
        snapshot_sequence: turn.snapshot_sequence,
        timestamp: turn.timestamp,
        role: turn.role,
        parent_turn_id: turn.parent_turn_id,
        is_sidechain: turn.is_sidechain,
        is_compaction_boundary: turn.is_compaction_boundary,
        body_state: turn.body_state,
        body,
        body_bytes: turn.body_bytes,
        body_sha256: turn.body_sha256,
        canonical_text_sha256: turn.canonical_text_sha256,
        canonical_text_digest_verified: turn.canonical_text_digest_verified,
    })
}

fn page_digest(result: &ProviderReadPageResult) -> Result<String, SessionProviderError> {
    serde_json::to_vec(result)
        .map(|bytes| sha256(&bytes))
        .map_err(|_| page_error("provider_page_digest_failed"))
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn page_error(token: &'static str) -> SessionProviderError {
    SessionProviderError::new(token, token)
}
