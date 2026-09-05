//! Role: mapper, accessor, formatter.

use crate::provider_registry::DescribeHostOptions;
use crate::services::TerminalClassifyServiceRequest;
use base64::Engine;
use oulipoly_provider::generated::{
    CONTRACT_VERSION, HOST_TERMINAL_UNAVAILABLE_V1_ENV, HOST_TERMINAL_UNAVAILABLE_V1_ENV_VALUE,
    HostContext, TerminalClassifyParams, TerminalClassifyRequest,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn build_terminal_classify_request(
    request: &TerminalClassifyServiceRequest,
    provider_instance_id: &str,
    host_options: &DescribeHostOptions,
) -> Result<Value, serde_json::Error> {
    serde_json::to_value(TerminalClassifyRequest {
        contract: CONTRACT_VERSION.to_string(),
        request_id: request_id(),
        provider_instance_id: Some(provider_instance_id.to_string()),
        host: host_context(host_options),
        params: TerminalClassifyParams {
            stdout_base64: encode_bytes(&request.stdout),
            stderr_base64: encode_bytes(&request.stderr),
            status: request.status.clone(),
            observed_at_unix_ms: observed_at_unix_ms(request.observed_at),
        },
    })
}

fn encode_bytes(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn observed_at_unix_ms(observed_at: SystemTime) -> u64 {
    observed_at
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn host_context(host_options: &DescribeHostOptions) -> HostContext {
    HostContext {
        app: "oulipoly-agent-runner".to_string(),
        app_version: None,
        platform: Some(std::env::consts::OS.to_string()),
        working_directory: current_working_directory(),
        config_root: host_options_config_root(host_options),
        data_root: host_options_data_root(host_options),
        env: BTreeMap::from([(
            HOST_TERMINAL_UNAVAILABLE_V1_ENV.to_string(),
            HOST_TERMINAL_UNAVAILABLE_V1_ENV_VALUE.to_string(),
        )]),
        deadline_unix_ms: None,
    }
}

fn host_options_config_root(host_options: &DescribeHostOptions) -> Option<String> {
    let path = host_options.config_root.as_ref()?;
    Some(display_path(path))
}

fn host_options_data_root(host_options: &DescribeHostOptions) -> Option<String> {
    let path = host_options.data_root.as_ref()?;
    Some(display_path(path))
}

fn current_working_directory() -> Option<String> {
    let path = std::env::current_dir().ok()?;
    Some(display_path(&path))
}

fn display_path(path: &Path) -> String {
    path.display().to_string()
}

fn request_id() -> String {
    format!("external-provider-terminal-{}", uuid::Uuid::new_v4())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_classify_explicitly_selects_unavailable_terminal_kind() {
        let request = TerminalClassifyServiceRequest {
            model_name: "fixture-model".into(),
            provider_name: "fixture-provider".into(),
            settings_id: "fixture-account".into(),
            stdout: Vec::new(),
            stderr: Vec::new(),
            status: oulipoly_provider::generated::ProcessStatus::Exited { code: 1 },
            observed_at: UNIX_EPOCH,
        };
        let value = build_terminal_classify_request(
            &request,
            "fixture-provider",
            &DescribeHostOptions::default(),
        )
        .unwrap();
        assert_eq!(value["host"]["env"][HOST_TERMINAL_UNAVAILABLE_V1_ENV], "1");
        assert_eq!(value["host"]["env"].as_object().unwrap().len(), 1);
    }
}
