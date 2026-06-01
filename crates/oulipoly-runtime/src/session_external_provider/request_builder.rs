//! Role: mapper.

use super::identity::{ExternalSessionIdentity, provider_instance_id};
use super::provider_error::{ExternalSessionProviderError, map_schema_invalid_request_error};
use super::replace_input_mapper::PreparedReplaceInput;
use oulipoly_provider::generated::{
    CONTRACT_VERSION, HostContext, JsonObject, RequestEnvelope, SessionBaseParams,
};
use serde_json::Value;
use std::collections::BTreeMap;

pub(crate) const CANONICAL_FORMAT: &str = "oulipoly.canonical_transcript/v1";

pub(crate) fn build_export_request(
    identity: &ExternalSessionIdentity,
    session_id: &str,
    request_id: String,
) -> Result<Value, ExternalSessionProviderError> {
    serialize_request(session_request_envelope(
        identity,
        Some(session_id),
        JsonObject::new(),
        request_id,
    ))
}

pub(crate) fn build_replace_request(
    identity: &ExternalSessionIdentity,
    session_id: &str,
    input: &PreparedReplaceInput,
    request_id: String,
) -> Result<Value, ExternalSessionProviderError> {
    let mut extra = JsonObject::new();
    extra.insert(
        "canonical_format".to_string(),
        Value::String(CANONICAL_FORMAT.to_string()),
    );
    extra.insert(
        "data_base64".to_string(),
        Value::String(input.data_base64.clone()),
    );
    extra.insert(
        "preimage_sha256".to_string(),
        Value::String(input.actual_preimage_sha256.clone()),
    );
    serialize_request(session_request_envelope(
        identity,
        Some(session_id),
        extra,
        request_id,
    ))
}

fn session_request_envelope(
    identity: &ExternalSessionIdentity,
    session_id: Option<&str>,
    mut extra: JsonObject,
    request_id: String,
) -> RequestEnvelope<SessionBaseParams> {
    extra.insert(
        "model_name".to_string(),
        Value::String(identity.model_name.clone()),
    );
    extra.insert(
        "provider_name".to_string(),
        Value::String(identity.provider_name.clone()),
    );
    RequestEnvelope {
        contract: CONTRACT_VERSION.to_string(),
        request_id,
        provider_instance_id: Some(provider_instance_id(identity)),
        host: host_context(),
        params: SessionBaseParams {
            settings_id: identity.settings_id.clone(),
            session_id: session_id.map(str::to_string),
            extra,
        },
    }
}

fn host_context() -> HostContext {
    HostContext {
        app: "oulipoly-agent-runner".to_string(),
        app_version: None,
        platform: Some(std::env::consts::OS.to_string()),
        working_directory: None,
        config_root: None,
        data_root: None,
        env: BTreeMap::new(),
        deadline_unix_ms: None,
    }
}

fn serialize_request(
    envelope: RequestEnvelope<SessionBaseParams>,
) -> Result<Value, ExternalSessionProviderError> {
    serde_json::to_value(envelope).map_err(|_| map_schema_invalid_request_error())
}
