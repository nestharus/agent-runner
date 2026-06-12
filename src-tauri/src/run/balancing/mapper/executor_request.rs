use std::collections::HashMap;
use std::path::{Path, PathBuf};

use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig};
use oulipoly_runtime::services::ExecutorServiceRequest;

pub(in crate::run::balancing) struct BalancedExecutorRequestInput<'a> {
    pub(in crate::run::balancing) model: &'a ModelConfig,
    pub(in crate::run::balancing) provider: &'a ProviderConfig,
    pub(in crate::run::balancing) provider_index: usize,
    pub(in crate::run::balancing) prompt_mode: PromptMode,
    pub(in crate::run::balancing) prompt: &'a str,
    pub(in crate::run::balancing) working_dir: Option<&'a Path>,
    pub(in crate::run::balancing) models_dir: &'a Path,
    pub(in crate::run::balancing) extra_inputs: &'a HashMap<String, Vec<String>>,
    pub(in crate::run::balancing) invocation_env: &'a str,
    pub(in crate::run::balancing) start_known_provider_session_id: Option<String>,
}

pub(in crate::run::balancing) struct BalancedExecutorRequestInputSource<'a> {
    pub(in crate::run::balancing) model: &'a ModelConfig,
    pub(in crate::run::balancing) provider: &'a ProviderConfig,
    pub(in crate::run::balancing) provider_index: usize,
    pub(in crate::run::balancing) prompt_mode: PromptMode,
    pub(in crate::run::balancing) prompt: &'a str,
    pub(in crate::run::balancing) working_dir: Option<&'a Path>,
    pub(in crate::run::balancing) models_dir: &'a Path,
    pub(in crate::run::balancing) extra_inputs: &'a HashMap<String, Vec<String>>,
    pub(in crate::run::balancing) invocation_env: &'a str,
    pub(in crate::run::balancing) start_known_provider_session_id: Option<String>,
}

pub(in crate::run::balancing) fn balanced_executor_request_input(
    source: BalancedExecutorRequestInputSource<'_>,
) -> BalancedExecutorRequestInput<'_> {
    BalancedExecutorRequestInput {
        model: source.model,
        provider: source.provider,
        provider_index: source.provider_index,
        prompt_mode: source.prompt_mode,
        prompt: source.prompt,
        working_dir: source.working_dir,
        models_dir: source.models_dir,
        extra_inputs: source.extra_inputs,
        invocation_env: source.invocation_env,
        start_known_provider_session_id: source.start_known_provider_session_id,
    }
}

pub(in crate::run::balancing) fn balanced_executor_request(
    input: BalancedExecutorRequestInput<'_>,
) -> ExecutorServiceRequest {
    if let Some(start_known_provider_session_id) = input.start_known_provider_session_id {
        return ExecutorServiceRequest::EffectiveWithStartKnownProviderSessionId {
            model: input.model.clone(),
            provider: input.provider.clone(),
            provider_index: input.provider_index,
            prompt_mode: input.prompt_mode,
            prompt: input.prompt.to_string(),
            working_dir: input.working_dir.map(PathBuf::from),
            models_dir: Some(input.models_dir.to_path_buf()),
            extra_inputs: input.extra_inputs.clone(),
            parent_invocation_env: Some(input.invocation_env.to_string()),
            start_known_provider_session_id,
        };
    }
    ExecutorServiceRequest::Effective {
        model: input.model.clone(),
        provider: input.provider.clone(),
        provider_index: input.provider_index,
        prompt_mode: input.prompt_mode,
        prompt: input.prompt.to_string(),
        working_dir: input.working_dir.map(PathBuf::from),
        models_dir: Some(input.models_dir.to_path_buf()),
        extra_inputs: input.extra_inputs.clone(),
        parent_invocation_env: Some(input.invocation_env.to_string()),
    }
}

pub(in crate::run::balancing) type ExecutorModelInput<'a> =
    (&'a ModelConfig, &'a ProviderConfig, usize, PromptMode);
pub(in crate::run::balancing) type ExecutorPromptInput<'a> = (
    &'a str,
    Option<&'a Path>,
    &'a Path,
    &'a HashMap<String, Vec<String>>,
);
pub(in crate::run::balancing) type ExecutorInvocationInput<'a> = (&'a str, Option<String>);

pub(in crate::run::balancing) fn balanced_executor_request_for_attempt(
    model_input: ExecutorModelInput<'_>,
    prompt_input: ExecutorPromptInput<'_>,
    invocation_input: ExecutorInvocationInput<'_>,
) -> ExecutorServiceRequest {
    let (model, provider, provider_index, prompt_mode) = model_input;
    let (prompt, working_dir, models_dir, extra_inputs) = prompt_input;
    let (invocation_env, start_known_provider_session_id) = invocation_input;
    balanced_executor_request(balanced_executor_request_input(
        BalancedExecutorRequestInputSource {
            model,
            provider,
            provider_index,
            prompt_mode,
            prompt,
            working_dir,
            models_dir,
            extra_inputs,
            invocation_env,
            start_known_provider_session_id,
        },
    ))
}
