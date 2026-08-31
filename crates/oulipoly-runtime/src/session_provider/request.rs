use super::host::host_context;
use super::types::{
    SessionProviderEnumerateRequest, SessionProviderError, SessionProviderIdentity,
    SessionProviderLifecycleContext, SessionProviderPageCursor, SessionProviderReadPageRequest,
    SessionProviderTurnProjection,
};
use crate::provider_registry::DescribeHostOptions;
use crate::session_metadata::TranscriptLookupMode;
use oulipoly_provider::generated::{
    CONTRACT_VERSION, JsonObject, RequestEnvelope, SessionBaseParams, SessionEnumerateParams,
    SessionReadTurnsParams, SessionTurnPageProjection, SessionTurnPageStartMode,
    SessionTurnPagesV1Protocol,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) struct BuiltPageRequest {
    pub value: Value,
    pub request_token_sha256: String,
}

pub(super) fn base_request(
    identity: &SessionProviderIdentity,
    session_id: Option<&str>,
    effective_cwd: Option<&Path>,
    host_options: &DescribeHostOptions,
    extra: JsonObject,
    request_label: &str,
) -> Result<Value, SessionProviderError> {
    let request_id = session_request_id(request_label);
    let envelope = session_request_envelope(
        identity,
        session_id,
        effective_cwd,
        host_options,
        extra,
        request_id,
    );
    serialize_session_request(envelope)
}

fn session_request_id(request_label: &str) -> String {
    format!("session-{request_label}-{}", uuid::Uuid::new_v4())
}

fn session_request_envelope(
    identity: &SessionProviderIdentity,
    session_id: Option<&str>,
    effective_cwd: Option<&Path>,
    host_options: &DescribeHostOptions,
    extra: JsonObject,
    request_id: String,
) -> RequestEnvelope<SessionBaseParams> {
    let session_id = session_id_string(non_empty_session_id(session_id));
    RequestEnvelope {
        contract: CONTRACT_VERSION.to_string(),
        request_id,
        provider_instance_id: Some(provider_instance_id(identity)),
        host: host_context(effective_cwd, host_options),
        params: session_base_params(identity, session_id, extra),
    }
}

fn serialize_session_request(
    envelope: RequestEnvelope<SessionBaseParams>,
) -> Result<Value, SessionProviderError> {
    serde_json::to_value(envelope).map_err(|err| {
        SessionProviderError::new("session_request_serialize_failed", err.to_string())
    })
}

fn session_base_params(
    identity: &SessionProviderIdentity,
    session_id: Option<String>,
    mut extra: JsonObject,
) -> SessionBaseParams {
    extra.insert(
        "model_name".to_string(),
        Value::String(identity.model_name.clone()),
    );
    extra.insert(
        "provider_name".to_string(),
        Value::String(identity.provider_name.clone()),
    );
    SessionBaseParams {
        settings_id: identity.settings_id.clone(),
        session_id,
        extra,
    }
}

fn non_empty_session_id(session_id: Option<&str>) -> Option<&str> {
    session_id.filter(|value| !value.is_empty())
}

fn session_id_string(session_id: Option<&str>) -> Option<String> {
    session_id.map(str::to_string)
}

pub(super) fn locate_extra(
    mode: TranscriptLookupMode,
    purpose: Option<&str>,
    tail_bytes_hint: Option<usize>,
) -> JsonObject {
    let mut extra = JsonObject::new();
    extra.insert(
        "lookup_mode".to_string(),
        Value::String(
            match mode {
                TranscriptLookupMode::RequireExisting => "require_existing",
                TranscriptLookupMode::AllowMissing => "allow_missing",
            }
            .to_string(),
        ),
    );
    insert_optional_str(&mut extra, "purpose", purpose);
    insert_optional_usize(&mut extra, "tail_bytes_hint", tail_bytes_hint);
    extra
}

pub(super) fn capture_extra(invocation_uuid: &str) -> JsonObject {
    let mut extra = JsonObject::new();
    extra.insert(
        "invocation_uuid".to_string(),
        Value::String(invocation_uuid.to_string()),
    );
    extra
}

pub(super) fn live_capture_extra(invocation_uuid: &str, provider_session_id: &str) -> JsonObject {
    let mut extra = capture_extra(invocation_uuid);
    extra.insert(
        "live_report".to_string(),
        serde_json::json!({
            "provider_session_id": provider_session_id,
            "invocation_uuid": invocation_uuid,
        }),
    );
    extra
}

pub(super) fn page_request(
    request: &SessionProviderReadPageRequest<'_>,
) -> Result<BuiltPageRequest, SessionProviderError> {
    validate_observation_nonce(request.projection, request.expected_delivery_nonce)?;
    let (start_mode, after_token, snapshot_id, page_token) = page_cursor_fields(&request.cursor);
    let mut host = host_context(request.effective_cwd, request.registry.host_options());
    host.deadline_unix_ms = Some(deadline_unix_ms(request.timeout)?);
    let request_token_sha256 = request_token_sha256(
        start_mode,
        after_token.as_deref(),
        snapshot_id.as_deref(),
        page_token.as_deref(),
        request.expected_delivery_nonce,
    );
    let envelope = RequestEnvelope {
        contract: CONTRACT_VERSION.to_string(),
        request_id: session_request_id("read-page"),
        provider_instance_id: Some(provider_instance_id(&request.identity)),
        host,
        params: SessionReadTurnsParams {
            settings_id: request.identity.settings_id.clone(),
            session_id: request.session_id.to_string(),
            read_protocol: SessionTurnPagesV1Protocol,
            turn_projection: provider_projection(request.projection),
            expected_delivery_nonce: request.expected_delivery_nonce.map(str::to_string),
            start_mode,
            after_token,
            snapshot_id,
            page_token,
            max_turns: request.max_turns,
            max_response_bytes: request.max_response_bytes,
            max_source_bytes: request.max_source_bytes,
            max_inline_body_bytes: request.max_inline_body_bytes,
        },
    };
    let value = serde_json::to_value(envelope).map_err(|error| {
        SessionProviderError::new("session_request_serialize_failed", error.to_string())
    })?;
    Ok(BuiltPageRequest {
        value,
        request_token_sha256,
    })
}

fn page_cursor_fields(
    cursor: &SessionProviderPageCursor,
) -> (
    SessionTurnPageStartMode,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    match cursor {
        SessionProviderPageCursor::Beginning { after_token } => (
            SessionTurnPageStartMode::Beginning,
            after_token.clone(),
            None,
            None,
        ),
        SessionProviderPageCursor::Tail => (SessionTurnPageStartMode::Tail, None, None, None),
        SessionProviderPageCursor::Continuation {
            snapshot_id,
            page_token,
        } => (
            SessionTurnPageStartMode::Continuation,
            None,
            Some(snapshot_id.clone()),
            Some(page_token.clone()),
        ),
    }
}

fn provider_projection(projection: SessionProviderTurnProjection) -> SessionTurnPageProjection {
    match projection {
        SessionProviderTurnProjection::CanonicalIngest => {
            SessionTurnPageProjection::CanonicalIngest
        }
        SessionProviderTurnProjection::UserObservation => {
            SessionTurnPageProjection::UserObservation
        }
    }
}

fn deadline_unix_ms(timeout: std::time::Duration) -> Result<u64, SessionProviderError> {
    let deadline = SystemTime::now().checked_add(timeout).ok_or_else(|| {
        SessionProviderError::new("session_page_deadline_invalid", "deadline overflow")
    })?;
    let millis = deadline
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            SessionProviderError::new("session_page_deadline_invalid", error.to_string())
        })?
        .as_millis();
    u64::try_from(millis).map_err(|_| {
        SessionProviderError::new("session_page_deadline_invalid", "deadline exceeds u64")
    })
}

fn request_token_sha256(
    start_mode: SessionTurnPageStartMode,
    after_token: Option<&str>,
    snapshot_id: Option<&str>,
    page_token: Option<&str>,
    expected_delivery_nonce: Option<&str>,
) -> String {
    let mode = match start_mode {
        SessionTurnPageStartMode::Beginning => "beginning",
        SessionTurnPageStartMode::Tail => "tail",
        SessionTurnPageStartMode::Continuation => "continuation",
    };
    let mut digest = Sha256::new();
    for value in [
        mode,
        after_token.unwrap_or(""),
        snapshot_id.unwrap_or(""),
        page_token.unwrap_or(""),
        expected_delivery_nonce.unwrap_or(""),
    ] {
        digest.update((value.len() as u64).to_be_bytes());
        digest.update(value.as_bytes());
    }
    format!("{:x}", digest.finalize())
}

fn validate_observation_nonce(
    projection: SessionProviderTurnProjection,
    nonce: Option<&str>,
) -> Result<(), SessionProviderError> {
    let valid_nonce = nonce.is_some_and(|value| {
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    });
    match projection {
        SessionProviderTurnProjection::UserObservation if valid_nonce => Ok(()),
        SessionProviderTurnProjection::UserObservation => Err(SessionProviderError::new(
            "session_observation_nonce_invalid",
            "user-observation paging requires a 64-character lowercase hexadecimal delivery nonce",
        )),
        SessionProviderTurnProjection::CanonicalIngest if nonce.is_none() => Ok(()),
        SessionProviderTurnProjection::CanonicalIngest => Err(SessionProviderError::new(
            "session_observation_nonce_forbidden",
            "canonical-ingest paging must not include a delivery nonce",
        )),
    }
}

pub(super) fn enumerate_request(
    request: &SessionProviderEnumerateRequest<'_>,
    host_options: &DescribeHostOptions,
) -> Result<Value, SessionProviderError> {
    let envelope = RequestEnvelope {
        contract: CONTRACT_VERSION.to_string(),
        request_id: session_request_id("enumerate"),
        provider_instance_id: Some(provider_instance_id(&request.identity)),
        host: host_context(request.effective_cwd, host_options),
        params: SessionEnumerateParams {
            settings_id: request.identity.settings_id.clone(),
            limit: request.limit,
            cursor: request.cursor.map(str::to_string),
            include_cwd: Some(request.include_cwd),
            include_turn_count: Some(request.include_turn_count),
            since_unix_ms: request.since_unix_ms,
        },
    };
    serde_json::to_value(envelope).map_err(|err| {
        SessionProviderError::new("session_request_serialize_failed", err.to_string())
    })
}

pub(super) fn lifecycle_extra(context: &SessionProviderLifecycleContext<'_>) -> JsonObject {
    let mut extra = capture_extra(context.invocation_uuid);
    extra.insert(
        "invocation_row_id".to_string(),
        Value::Number(context.invocation_row_id.into()),
    );
    insert_path(&mut extra, "effective_cwd", context.effective_cwd);
    insert_optional_str(&mut extra, "pinned_target", context.pinned_target);
    insert_optional_str(
        &mut extra,
        "start_bound_provider_session_id",
        context.start_bound_provider_session_id,
    );
    extra
}

fn insert_path(extra: &mut JsonObject, key: &str, value: Option<&Path>) {
    if let Some(path) = value {
        extra.insert(key.to_string(), Value::String(path.display().to_string()));
    }
}

fn insert_optional_str(extra: &mut JsonObject, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        extra.insert(key.to_string(), Value::String(value.to_string()));
    }
}

fn insert_optional_usize(extra: &mut JsonObject, key: &str, value: Option<usize>) {
    if let Some(value) = value {
        extra.insert(key.to_string(), Value::from(value as u64));
    }
}

pub(super) fn provider_instance_id(identity: &SessionProviderIdentity) -> String {
    identity
        .provider_instance_id
        .clone()
        .unwrap_or_else(|| identity.provider_name.clone())
}

#[cfg(test)]
mod tests {
    use super::validate_observation_nonce;
    use crate::session_provider::SessionProviderTurnProjection;

    #[test]
    fn observation_nonce_is_required_exact_and_projection_scoped() {
        let nonce = "a".repeat(64);
        assert!(
            validate_observation_nonce(
                SessionProviderTurnProjection::UserObservation,
                Some(&nonce)
            )
            .is_ok()
        );
        for invalid in [None, Some("abc"), Some(&"A".repeat(64))] {
            assert!(
                validate_observation_nonce(SessionProviderTurnProjection::UserObservation, invalid)
                    .is_err()
            );
        }
        assert!(
            validate_observation_nonce(SessionProviderTurnProjection::CanonicalIngest, None)
                .is_ok()
        );
        assert!(
            validate_observation_nonce(
                SessionProviderTurnProjection::CanonicalIngest,
                Some(&nonce)
            )
            .is_err()
        );
    }
}
