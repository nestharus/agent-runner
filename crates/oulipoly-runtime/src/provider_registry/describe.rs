use super::{ProviderClientFactory, ProviderRegistryError};
use oulipoly_provider::client::{CancellationToken, ProviderClient, ProviderEnv};
use oulipoly_provider::error::ProviderClientError;
use oulipoly_provider::generated::{
    CONTRACT_VERSION, DescribeRequest, DescribeResult, EmptyParams, HOST_LAUNCH_OUTPUT_V1_ENV,
    HOST_LAUNCH_OUTPUT_V1_ENV_VALUE, HOST_PROMPT_ACCEPTANCE_V1_ENV,
    HOST_PROMPT_ACCEPTANCE_V1_ENV_VALUE, HOST_SESSION_TURN_PAGES_V1_ENV,
    HOST_SESSION_TURN_PAGES_V1_ENV_VALUE, HostContext,
};
use oulipoly_provider::resolver::ProviderArtifactRef;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub struct DescribeHostOptions {
    pub config_root: Option<PathBuf>,
    pub data_root: Option<PathBuf>,
}

pub fn describe_provider(
    factory: &ProviderClientFactory,
    artifact: ProviderArtifactRef,
    host_options: &DescribeHostOptions,
) -> Result<DescribeResult, ProviderRegistryError> {
    let client = factory.client_for(artifact);
    describe_provider_client(&client, host_options)
}

pub fn describe_provider_with_cancellation(
    factory: &ProviderClientFactory,
    artifact: ProviderArtifactRef,
    host_options: &DescribeHostOptions,
    cancellation: &CancellationToken,
) -> Result<DescribeResult, ProviderRegistryError> {
    let client = factory.client_for_with_cancellation(artifact, cancellation);
    describe_provider_client(&client, host_options)
}

pub(crate) fn describe_provider_client(
    client: &ProviderClient,
    host_options: &DescribeHostOptions,
) -> Result<DescribeResult, ProviderRegistryError> {
    let request = describe_request(host_options)?;
    client
        .invoke_typed::<DescribeResult, _>("describe", request, NoProviderEnv)
        .map_err(map_describe_error)
}

fn describe_request(_host_options: &DescribeHostOptions) -> Result<Value, ProviderRegistryError> {
    let env = BTreeMap::from([
        (
            HOST_PROMPT_ACCEPTANCE_V1_ENV.to_string(),
            HOST_PROMPT_ACCEPTANCE_V1_ENV_VALUE.to_string(),
        ),
        (
            HOST_LAUNCH_OUTPUT_V1_ENV.to_string(),
            HOST_LAUNCH_OUTPUT_V1_ENV_VALUE.to_string(),
        ),
        (
            HOST_SESSION_TURN_PAGES_V1_ENV.to_string(),
            HOST_SESSION_TURN_PAGES_V1_ENV_VALUE.to_string(),
        ),
    ]);
    serde_json::to_value(DescribeRequest {
        contract: CONTRACT_VERSION.to_string(),
        request_id: format!("provider-registry-{}", uuid::Uuid::new_v4()),
        provider_instance_id: Some("provider-registry".to_string()),
        host: HostContext {
            app: "oulipoly-agent-runner".to_string(),
            app_version: None,
            platform: None,
            working_directory: None,
            config_root: None,
            data_root: None,
            env,
            deadline_unix_ms: None,
        },
        params: EmptyParams {},
    })
    .map_err(|error| ProviderRegistryError::ProviderProtocol {
        kind: "schema_invalid_request".to_string(),
        source: Box::new(ProviderClientError::protocol(
            oulipoly_provider::error::HostErrorKind::SchemaInvalidRequest,
            "describe",
            None,
            oulipoly_provider::error::ProviderDiagnostics::with_description(error.to_string()),
        )),
    })
}

fn map_describe_error(source: ProviderClientError) -> ProviderRegistryError {
    match &source {
        ProviderClientError::Transport { kind, .. } => ProviderRegistryError::ProviderTransport {
            kind: kind.as_str().to_string(),
            source: Box::new(source),
        },
        ProviderClientError::Protocol { kind, .. } => ProviderRegistryError::ProviderProtocol {
            kind: kind.as_str().to_string(),
            source: Box::new(source),
        },
        ProviderClientError::ProviderCapability(error) => {
            ProviderRegistryError::ProviderDescribeFailed {
                category: error.error().category.clone(),
                code: error.error().code.clone(),
                source: Box::new(source),
            }
        }
    }
}

struct NoProviderEnv;

impl ProviderEnv for NoProviderEnv {
    fn into_env_vec(self) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
        Vec::new()
    }
}
