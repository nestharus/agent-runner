//! Role: mapper.

use super::identity::{ExternalSessionIdentity, provider_instance_id};
use super::provider_error::{ExternalSessionProviderError, map_schema_invalid_request_error};
use super::replace_input_mapper::PreparedReplaceInput;
use crate::provider_registry::DescribeHostOptions;
use oulipoly_provider::generated::{
    CONTRACT_VERSION, HostContext, JsonObject, RequestEnvelope, SessionBaseParams,
    SessionReplaceCanonicalTranscript, SessionReplaceParams,
};
use serde_json::Value;
use std::collections::BTreeMap;

pub(crate) const CANONICAL_FORMAT: &str = "oulipoly.canonical_transcript/v1";
pub(crate) const PROVIDER_OWNED_REPLACE_PROTOCOL: &str = "oulipoly.provider_owned_replace/v1";
pub(crate) const HOST_APPLY_CAPABILITY: &str = "replace_session_turns_from_canonical_v1";
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

pub(crate) struct RecoveryReplaceRequest<'a> {
    pub(crate) operation_id: &'a str,
    pub(crate) recovery_id: Option<&'a str>,
    pub(crate) action: &'a str,
    pub(crate) input: Option<&'a PreparedReplaceInput>,
}

pub(crate) fn build_export_request(
    identity: &ExternalSessionIdentity,
    session_id: &str,
    host_options: &DescribeHostOptions,
    request_id: String,
) -> Result<Value, ExternalSessionProviderError> {
    serialize_request(session_request_envelope(
        identity,
        Some(session_id),
        host_options,
        JsonObject::new(),
        request_id,
    ))
}

pub(crate) fn build_replace_request(
    identity: &ExternalSessionIdentity,
    session_id: &str,
    input: &PreparedReplaceInput,
    host_options: &DescribeHostOptions,
    request_id: String,
) -> Result<Value, ExternalSessionProviderError> {
    serialize_replace_request(replace_request_envelope(
        identity,
        session_id,
        input,
        host_options,
        request_id,
    ))
}

pub(crate) fn build_recovery_replace_request(
    identity: &ExternalSessionIdentity,
    session_id: &str,
    recovery: RecoveryReplaceRequest<'_>,
    host_options: &DescribeHostOptions,
    request_id: String,
) -> Result<Value, ExternalSessionProviderError> {
    serialize_replace_request(recovery_replace_request_envelope(
        identity,
        session_id,
        recovery,
        host_options,
        request_id,
    ))
}

fn session_request_envelope(
    identity: &ExternalSessionIdentity,
    session_id: Option<&str>,
    host_options: &DescribeHostOptions,
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
        host: host_context(host_options),
        params: SessionBaseParams {
            settings_id: identity.settings_id.clone(),
            session_id: session_id.map(str::to_string),
            extra,
        },
    }
}

fn host_context(host_options: &DescribeHostOptions) -> HostContext {
    HostContext {
        app: "oulipoly-agent-runner".to_string(),
        app_version: None,
        platform: Some(std::env::consts::OS.to_string()),
        working_directory: None,
        config_root: host_options
            .config_root
            .as_ref()
            .map(|path| path.display().to_string()),
        data_root: host_options
            .data_root
            .as_ref()
            .map(|path| path.display().to_string()),
        env: BTreeMap::new(),
        deadline_unix_ms: None,
    }
}

fn replace_request_envelope(
    identity: &ExternalSessionIdentity,
    session_id: &str,
    input: &PreparedReplaceInput,
    host_options: &DescribeHostOptions,
    request_id: String,
) -> RequestEnvelope<SessionReplaceParams> {
    let _canonical_input_len = input.bytes.len();
    RequestEnvelope {
        contract: CONTRACT_VERSION.to_string(),
        request_id,
        provider_instance_id: Some(provider_instance_id(identity)),
        host: host_context(host_options),
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
    recovery: RecoveryReplaceRequest<'_>,
    host_options: &DescribeHostOptions,
    request_id: String,
) -> RequestEnvelope<SessionReplaceParams> {
    let canonical_transcript = Some(recovery_canonical_transcript(recovery.input));
    RequestEnvelope {
        contract: CONTRACT_VERSION.to_string(),
        request_id,
        provider_instance_id: Some(provider_instance_id(identity)),
        host: host_context(host_options),
        params: SessionReplaceParams {
            settings_id: identity.settings_id.clone(),
            session_id: session_id.to_string(),
            model_name: identity.model_name.clone(),
            provider_name: identity.provider_name.clone(),
            replace_protocol: PROVIDER_OWNED_REPLACE_PROTOCOL.to_string(),
            operation_id: recovery.operation_id.to_string(),
            canonical_format: CANONICAL_FORMAT.to_string(),
            canonical_transcript,
            preimage_sha256_expected: None,
            host_apply_capability: Some(HOST_APPLY_CAPABILITY.to_string()),
            operation_mode: Some("recover".to_string()),
            recovery_action: Some(recovery.action.to_string()),
            recovery_id: recovery.recovery_id.map(str::to_string),
            extra: JsonObject::new(),
        },
    }
}

fn recovery_canonical_transcript(
    input: Option<&PreparedReplaceInput>,
) -> SessionReplaceCanonicalTranscript {
    match input {
        Some(input) => SessionReplaceCanonicalTranscript {
            kind: "bytes".to_string(),
            data_base64: input.data_base64.clone(),
            sha256: input.records_sha256.clone(),
            turn_count: input.turn_count,
        },
        None => SessionReplaceCanonicalTranscript {
            kind: "bytes".to_string(),
            data_base64: String::new(),
            sha256: EMPTY_SHA256.to_string(),
            turn_count: 0,
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
