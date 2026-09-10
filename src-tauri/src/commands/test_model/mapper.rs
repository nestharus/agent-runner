//! ## Declared roles
//!
//! `mapper`

use oulipoly_config::repositories::ProvidersConfigRepository;
use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig};
use oulipoly_runtime::executor::ExecutionResult;
use oulipoly_runtime::services::{
    DiagnosticsServicePort, ExecutorServicePort, ExecutorServiceRequest, RoutingServicePort,
};
use oulipoly_state::repositories::StateDbOpener;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Serialize, Clone)]
pub struct TestModelResult {
    pub success: bool,
    pub exit_code: i32,
    pub stdout_preview: String,
    pub stdout_preview_truncated: bool,
    pub stdout_bytes: u64,
    pub stdout_sha256: Option<String>,
    pub stdout_content_type: String,
    pub stderr_preview: String,
    pub stderr_preview_truncated: bool,
    pub stderr_bytes: u64,
    pub stderr_sha256: Option<String>,
    pub stderr_content_type: String,
    pub output_artifact_token: Option<String>,
}

pub struct TestModelServices<'a> {
    pub state_db_opener: &'a (dyn StateDbOpener + Send + Sync),
    pub providers_repository: &'a (dyn ProvidersConfigRepository + Send + Sync),
    pub routing_service: &'a dyn RoutingServicePort,
    pub executor_service: &'a dyn ExecutorServicePort,
    pub diagnostics_service: &'a dyn DiagnosticsServicePort,
}

pub(crate) fn test_model_services_from_parts<'a>(
    state_db_opener: &'a (dyn StateDbOpener + Send + Sync),
    providers_repository: &'a (dyn ProvidersConfigRepository + Send + Sync),
    routing_service: &'a dyn RoutingServicePort,
    executor_service: &'a dyn ExecutorServicePort,
    diagnostics_service: &'a dyn DiagnosticsServicePort,
) -> TestModelServices<'a> {
    TestModelServices {
        state_db_opener,
        providers_repository,
        routing_service,
        executor_service,
        diagnostics_service,
    }
}

pub(crate) fn build_effective_executor_request(
    model: ModelConfig,
    provider: ProviderConfig,
    provider_index: usize,
    prompt_mode: PromptMode,
    prompt: &str,
) -> ExecutorServiceRequest {
    ExecutorServiceRequest::Effective {
        model,
        provider,
        provider_index,
        prompt_mode,
        prompt: prompt.to_string(),
        working_dir: None,
        models_dir: None,
        extra_inputs: HashMap::new(),
        parent_invocation_env: None,
    }
}

pub(crate) fn map_effective_provider_from_sources(
    provider: ProviderConfig,
    prompt_mode: PromptMode,
) -> (ProviderConfig, PromptMode) {
    (provider, prompt_mode)
}

pub(crate) fn map_test_model_result(
    state: &oulipoly_state::StateDb,
    result: &ExecutionResult,
) -> Result<TestModelResult, String> {
    const PREVIEW_BYTES: usize = 4096;
    let stdout_preview_bytes = &result.stdout[..result.stdout.len().min(PREVIEW_BYTES)];
    let stderr_bytes = result.stderr.as_bytes();
    let stderr_preview_bytes = &stderr_bytes[..stderr_bytes.len().min(PREVIEW_BYTES)];
    let summary = result
        .output_spool
        .as_ref()
        .map(|spool| spool.summary().map_err(|error| error.to_string()))
        .transpose()?;
    let output_artifact_token = if let Some(spool) = &result.output_spool {
        let token = format!("test-model-{}", uuid::Uuid::new_v4());
        spool.persist_artifact(state, &token)?.then_some(token)
    } else {
        None
    };
    Ok(TestModelResult {
        success: result.exit_code == 0,
        exit_code: result.exit_code,
        stdout_preview: String::from_utf8_lossy(stdout_preview_bytes).into_owned(),
        stdout_preview_truncated: summary
            .as_ref()
            .is_some_and(|summary| summary.stdout_bytes > stdout_preview_bytes.len() as u64)
            || result.stdout.len() > stdout_preview_bytes.len(),
        stdout_bytes: summary
            .as_ref()
            .map_or(result.stdout.len() as u64, |summary| summary.stdout_bytes),
        stdout_sha256: summary
            .as_ref()
            .map(|summary| summary.stdout_sha256.clone()),
        stdout_content_type: if result.output_spool.is_some()
            || std::str::from_utf8(&result.stdout).is_err()
        {
            "application/octet-stream".to_string()
        } else {
            "text/plain; charset=utf-8".to_string()
        },
        stderr_preview: String::from_utf8_lossy(stderr_preview_bytes).into_owned(),
        stderr_preview_truncated: summary
            .as_ref()
            .is_some_and(|summary| summary.stderr_bytes > stderr_preview_bytes.len() as u64)
            || stderr_bytes.len() > stderr_preview_bytes.len(),
        stderr_bytes: summary
            .as_ref()
            .map_or(stderr_bytes.len() as u64, |summary| summary.stderr_bytes),
        stderr_sha256: summary
            .as_ref()
            .map(|summary| summary.stderr_sha256.clone()),
        stderr_content_type: if result.output_spool.is_some() {
            "application/octet-stream".to_string()
        } else {
            "text/plain; charset=utf-8".to_string()
        },
        output_artifact_token,
    })
}
