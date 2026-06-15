use super::host::host_context;
use super::types::{
    SessionProviderError, SessionProviderIdentity, SessionProviderLifecycleContext,
};
use crate::session_metadata::TranscriptLookupMode;
use oulipoly_provider::generated::{
    CONTRACT_VERSION, JsonObject, RequestEnvelope, SessionBaseParams,
};
use serde_json::Value;
use std::path::Path;

pub(super) fn base_request(
    identity: &SessionProviderIdentity,
    session_id: Option<&str>,
    effective_cwd: Option<&Path>,
    extra: JsonObject,
    request_label: &str,
) -> Result<Value, SessionProviderError> {
    let request_id = session_request_id(request_label);
    let envelope = session_request_envelope(identity, session_id, effective_cwd, extra, request_id);
    serialize_session_request(envelope)
}

fn session_request_id(request_label: &str) -> String {
    format!("session-{request_label}-{}", uuid::Uuid::new_v4())
}

fn session_request_envelope(
    identity: &SessionProviderIdentity,
    session_id: Option<&str>,
    effective_cwd: Option<&Path>,
    extra: JsonObject,
    request_id: String,
) -> RequestEnvelope<SessionBaseParams> {
    let session_id = session_id_string(non_empty_session_id(session_id));
    RequestEnvelope {
        contract: CONTRACT_VERSION.to_string(),
        request_id,
        provider_instance_id: Some(provider_instance_id(identity)),
        host: host_context(effective_cwd),
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

pub(super) fn locate_extra(mode: TranscriptLookupMode) -> JsonObject {
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

pub(super) fn read_session_id_for_lifecycle<'a>(
    context: &'a SessionProviderLifecycleContext<'a>,
) -> Option<&'a str> {
    context
        .pinned_target
        .or(context.start_bound_provider_session_id)
}

fn provider_instance_id(identity: &SessionProviderIdentity) -> String {
    identity
        .provider_instance_id
        .clone()
        .unwrap_or_else(|| identity.provider_name.clone())
}
