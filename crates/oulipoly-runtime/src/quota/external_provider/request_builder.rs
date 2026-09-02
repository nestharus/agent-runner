//! ## Declared roles
//!
//! `mapper`

use super::request_id_format::quota_request_id;
use crate::provider_registry::DescribeHostOptions;
use crate::services::QuotaServiceExternalProviderIdentity;
use oulipoly_provider::generated::{
    CONTRACT_VERSION, HostContext, JsonObject, QuotaBaseParams, QuotaProbeRequest,
    QuotaRefreshAuthParams, QuotaRefreshAuthRequest, QuotaSourceRequest,
};
use serde_json::Value;
use std::collections::BTreeMap;

pub(crate) fn build_quota_source_request(
    identity: &QuotaServiceExternalProviderIdentity,
    host_options: &DescribeHostOptions,
) -> Result<Value, serde_json::Error> {
    serde_json::to_value(QuotaSourceRequest {
        contract: CONTRACT_VERSION.to_string(),
        request_id: quota_request_id("source"),
        provider_instance_id: Some(identity.provider_instance_id.clone()),
        host: host_context(host_options),
        params: quota_base_params(identity),
    })
}

pub(crate) fn build_quota_probe_request(
    identity: &QuotaServiceExternalProviderIdentity,
    host_options: &DescribeHostOptions,
) -> Result<Value, serde_json::Error> {
    serde_json::to_value(QuotaProbeRequest {
        contract: CONTRACT_VERSION.to_string(),
        request_id: quota_request_id("probe"),
        provider_instance_id: Some(identity.provider_instance_id.clone()),
        host: host_context(host_options),
        params: quota_base_params(identity),
    })
}

pub(crate) fn build_quota_refresh_auth_request(
    identity: &QuotaServiceExternalProviderIdentity,
    host_options: &DescribeHostOptions,
) -> Result<Value, serde_json::Error> {
    serde_json::to_value(QuotaRefreshAuthRequest {
        contract: CONTRACT_VERSION.to_string(),
        request_id: quota_request_id("refresh-auth"),
        provider_instance_id: Some(identity.provider_instance_id.clone()),
        host: host_context(host_options),
        params: QuotaRefreshAuthParams {
            settings_id: identity.settings_id.clone(),
            force: Some(false),
            context: None,
        },
    })
}

fn quota_base_params(identity: &QuotaServiceExternalProviderIdentity) -> QuotaBaseParams {
    QuotaBaseParams {
        settings_id: identity.settings_id.clone(),
        model_name: None,
        context: Some(quota_context(identity)),
    }
}

fn quota_context(identity: &QuotaServiceExternalProviderIdentity) -> JsonObject {
    let mut context = JsonObject::new();
    context.insert(
        "provider_name".to_string(),
        Value::String(identity.provider_instance_id.clone()),
    );
    context
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
