//! Role: mapper.

use super::identity::{ExternalSessionIdentity, provider_instance_id};
use super::provider_error::{ExternalSessionProviderError, map_schema_invalid_request_error};
use super::replace_input_mapper::PreparedReplaceInput;
use oulipoly_provider::generated::{
    CONTRACT_VERSION, HostContext, JsonObject, RequestEnvelope, SessionBaseParams,
    SessionReplaceCanonicalTranscript, SessionReplaceParams,
};
use serde_json::Value;
use std::collections::BTreeMap;

pub(crate) const CANONICAL_FORMAT: &str = "oulipoly.canonical_transcript/v1";
pub(crate) const PROVIDER_OWNED_REPLACE_PROTOCOL: &str = "oulipoly.provider_owned_replace/v1";
pub(crate) const HOST_APPLY_CAPABILITY: &str = "replace_session_turns_from_canonical_v1";

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
    serialize_replace_request(replace_request_envelope(
        identity, session_id, input, request_id,
    ))
}

pub(crate) fn build_recovery_replace_request(
    identity: &ExternalSessionIdentity,
    session_id: &str,
    operation_id: &str,
    recovery_id: Option<&str>,
    recovery_action: &str,
    request_id: String,
) -> Result<Value, ExternalSessionProviderError> {
    build_recovery_replace_request_with_input(
        identity,
        session_id,
        operation_id,
        recovery_id,
        recovery_action,
        None,
        request_id,
    )
}

pub(crate) fn build_recovery_replace_request_with_input(
    identity: &ExternalSessionIdentity,
    session_id: &str,
    operation_id: &str,
    recovery_id: Option<&str>,
    recovery_action: &str,
    input: Option<&PreparedReplaceInput>,
    request_id: String,
) -> Result<Value, ExternalSessionProviderError> {
    serialize_replace_request(recovery_replace_request_envelope(
        identity,
        session_id,
        operation_id,
        recovery_id,
        recovery_action,
        input,
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

fn replace_request_envelope(
    identity: &ExternalSessionIdentity,
    session_id: &str,
    input: &PreparedReplaceInput,
    request_id: String,
) -> RequestEnvelope<SessionReplaceParams> {
    let _canonical_input_len = input.bytes.len();
    RequestEnvelope {
        contract: CONTRACT_VERSION.to_string(),
        request_id,
        provider_instance_id: Some(provider_instance_id(identity)),
        host: host_context(),
        params: SessionReplaceParams {
            settings_id: identity.settings_id.clone(),
            session_id: session_id.to_string(),
            model_name: identity.model_name.clone(),
            provider_name: identity.provider_name.clone(),
            replace_protocol: PROVIDER_OWNED_REPLACE_PROTOCOL.to_string(),
            operation_id: input.operation_id.clone(),
            canonical_format: CANONICAL_FORMAT.to_string(),
            canonical_transcript: Some(SessionReplaceCanonicalTranscript {
                kind: "bytes".to_string(),
                data_base64: input.data_base64.clone(),
                sha256: input.records_sha256.clone(),
                turn_count: input.turn_count,
            }),
            preimage_sha256_expected: input.preimage_sha256_expected.clone(),
            host_apply_capability: Some(HOST_APPLY_CAPABILITY.to_string()),
            operation_mode: None,
            recovery_action: None,
            recovery_id: None,
            extra: JsonObject::new(),
        },
    }
}

fn recovery_replace_request_envelope(
    identity: &ExternalSessionIdentity,
    session_id: &str,
    operation_id: &str,
    recovery_id: Option<&str>,
    recovery_action: &str,
    input: Option<&PreparedReplaceInput>,
    request_id: String,
) -> RequestEnvelope<SessionReplaceParams> {
    let canonical_transcript = input.map(|input| SessionReplaceCanonicalTranscript {
        kind: "bytes".to_string(),
        data_base64: input.data_base64.clone(),
        sha256: input.records_sha256.clone(),
        turn_count: input.turn_count,
    });
    RequestEnvelope {
        contract: CONTRACT_VERSION.to_string(),
        request_id,
        provider_instance_id: Some(provider_instance_id(identity)),
        host: host_context(),
        params: SessionReplaceParams {
            settings_id: identity.settings_id.clone(),
            session_id: session_id.to_string(),
            model_name: identity.model_name.clone(),
            provider_name: identity.provider_name.clone(),
            replace_protocol: PROVIDER_OWNED_REPLACE_PROTOCOL.to_string(),
            operation_id: operation_id.to_string(),
            canonical_format: CANONICAL_FORMAT.to_string(),
            canonical_transcript,
            preimage_sha256_expected: None,
            host_apply_capability: None,
            operation_mode: Some("recover".to_string()),
            recovery_action: Some(recovery_action.to_string()),
            recovery_id: recovery_id.map(str::to_string),
            extra: JsonObject::new(),
        },
    }
}

fn serialize_request(
    envelope: RequestEnvelope<SessionBaseParams>,
) -> Result<Value, ExternalSessionProviderError> {
    serde_json::to_value(envelope).map_err(|_| map_schema_invalid_request_error())
}

fn serialize_replace_request(
    envelope: RequestEnvelope<SessionReplaceParams>,
) -> Result<Value, ExternalSessionProviderError> {
    serde_json::to_value(envelope).map_err(|_| map_schema_invalid_request_error())
}
