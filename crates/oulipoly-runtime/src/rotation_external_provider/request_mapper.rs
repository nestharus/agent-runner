//! ## Declared roles
//! mapper, formatter

use super::{ExternalRotationError, ExternalRotationIdentity, error_formatter};
use crate::services::MigrationServiceRequest;
use oulipoly_provider::generated::{
    CONTRACT_VERSION, HostContext, JsonObject, MigrationObject, RequestEnvelope, RotationObject,
};
use serde_json::Value;
use std::collections::BTreeMap;

pub(super) fn rotation_request(
    identity: &ExternalRotationIdentity,
    request: &MigrationServiceRequest<'_>,
    operation: &str,
) -> Result<Value, ExternalRotationError> {
    serialize_request(RequestEnvelope {
        contract: CONTRACT_VERSION.to_string(),
        request_id: format!("s7c-{operation}"),
        provider_instance_id: identity.provider_instance_id.clone(),
        host: host_context(request),
        params: RotationObject {
            fields: request_fields(identity, request, operation),
        },
    })
}

pub(super) fn migration_request(
    identity: &ExternalRotationIdentity,
    request: &MigrationServiceRequest<'_>,
    operation: &str,
) -> Result<Value, ExternalRotationError> {
    serialize_request(RequestEnvelope {
        contract: CONTRACT_VERSION.to_string(),
        request_id: format!("s7c-{operation}"),
        provider_instance_id: identity.provider_instance_id.clone(),
        host: host_context(request),
        params: MigrationObject {
            fields: request_fields(identity, request, operation),
        },
    })
}

fn request_fields(
    identity: &ExternalRotationIdentity,
    request: &MigrationServiceRequest<'_>,
    operation: &str,
) -> JsonObject {
    let mut fields = BTreeMap::new();
    fields.insert(
        "operation".to_string(),
        Value::String(operation.to_string()),
    );
    fields.insert(
        "model_name".to_string(),
        Value::String(identity.model_name.clone()),
    );
    fields.insert(
        "settings_id".to_string(),
        Value::String(identity.settings_id.clone()),
    );
    fields.insert(
        "source_provider".to_string(),
        Value::String(identity.source_provider.clone()),
    );
    fields.insert(
        "target_provider".to_string(),
        Value::String(identity.target_provider.clone()),
    );
    fields.insert(
        "source_session_id".to_string(),
        Value::String(identity.source_session_id.clone()),
    );
    fields.insert(
        "chain_id".to_string(),
        Value::String(request.resolved.chain_id.clone()),
    );
    fields.insert(
        "transition_reason".to_string(),
        Value::String(rotation_transition_reason(request).to_string()),
    );
    fields
}

fn rotation_transition_reason(request: &MigrationServiceRequest<'_>) -> &'static str {
    if request.manual_target.is_some() {
        return "manual";
    }
    if request.active_exhausted {
        return "exhausted";
    }
    "quota_threshold"
}

fn host_context(request: &MigrationServiceRequest<'_>) -> HostContext {
    HostContext {
        app: "oulipoly-agent-runner".to_string(),
        app_version: None,
        platform: Some(std::env::consts::OS.to_string()),
        working_directory: Some(request.effective_cwd.display().to_string()),
        config_root: None,
        data_root: oulipoly_state::paths::data_dir()
            .ok()
            .map(|path| path.display().to_string()),
        env: BTreeMap::new(),
        deadline_unix_ms: None,
    }
}

fn serialize_request<T: serde::Serialize>(request: T) -> Result<Value, ExternalRotationError> {
    serde_json::to_value(request).map_err(|error| {
        error_formatter::protocol_invalid_response(format!("failed to encode request: {error}"))
    })
}
