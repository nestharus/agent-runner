//! Declared role: orchestration, validator

use oulipoly_provider::client::{ProviderClient, ProviderClientOptions};
use oulipoly_provider::generated::{
    CONTRACT_VERSION, DescribeResult, EmptyParams, HostContext, RequestEnvelope,
};
use oulipoly_provider::resolver::{ProviderArtifactRef, ProviderResolveOptions};
use std::time::Duration;

pub(crate) fn prove_provider_artifact(artifact: ProviderArtifactRef) -> Result<(), String> {
    let client = ProviderClient::new(artifact, provider_client_options());
    let describe = client
        .invoke_typed::<DescribeResult, _>("describe", describe_request(), [])
        .map_err(|err| format!("provider proof failed: {err}"))?;
    validate_describe_result(&describe)
}

fn provider_client_options() -> ProviderClientOptions {
    ProviderClientOptions {
        resolver: ProviderResolveOptions::default().with_path_entries(
            std::env::var_os("PATH")
                .into_iter()
                .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>()),
        ),
        ..ProviderClientOptions::default().with_timeout(Duration::from_secs(5))
    }
}

fn describe_request() -> serde_json::Value {
    serde_json::to_value(RequestEnvelope {
        contract: CONTRACT_VERSION.to_string(),
        request_id: "s11-m2b-provider-proof".to_string(),
        provider_instance_id: None,
        host: HostContext {
            app: "oulipoly-agent-runner".to_string(),
            app_version: None,
            platform: None,
            working_directory: None,
            config_root: None,
            data_root: None,
            env: Default::default(),
            deadline_unix_ms: None,
        },
        params: EmptyParams {},
    })
    .unwrap_or_else(|_| serde_json::json!({}))
}

fn validate_describe_result(result: &DescribeResult) -> Result<(), String> {
    if result.preferred_contract == CONTRACT_VERSION
        || result
            .contract_versions
            .iter()
            .any(|version| version == CONTRACT_VERSION)
    {
        Ok(())
    } else {
        Err("provider proof failed: contract version missing".to_string())
    }
}
