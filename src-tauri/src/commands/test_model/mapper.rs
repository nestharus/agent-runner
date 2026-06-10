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
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
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

pub(crate) fn map_test_model_result(result: &ExecutionResult) -> TestModelResult {
    TestModelResult {
        success: result.exit_code == 0,
        stdout: String::from_utf8_lossy(&result.stdout).into_owned(),
        stderr: result.stderr.clone(),
        exit_code: result.exit_code,
    }
}
