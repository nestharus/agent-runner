//! ## Declared roles
//!
//! `mapper`, `validator`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/run/balancing/mapper/executor_request.rs
//!     role: adapter
//!     Translates:
//!       - balancing-attempt-input-contract
//!       - runtime-executor-service-request-contract
//!       - provider-session-start-mode-contract
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig};
use oulipoly_runtime::services::{ExecutorServiceRequest, ProviderSessionStartMode};

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
    pub(in crate::run::balancing) start_known_provider_session_mode:
        Option<ProviderSessionStartMode>,
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
    pub(in crate::run::balancing) start_known_provider_session_mode:
        Option<ProviderSessionStartMode>,
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
        start_known_provider_session_mode: source.start_known_provider_session_mode,
    }
}

pub(in crate::run::balancing) fn balanced_executor_request(
    input: BalancedExecutorRequestInput<'_>,
) -> ExecutorServiceRequest {
    if let Some(start_known_provider_session_id) = input.start_known_provider_session_id.as_ref() {
        let common = known_session_request_common(&input);
        let start_known_provider_session_id = start_known_provider_session_id.clone();
        return match required_start_known_provider_session_mode(
            input.start_known_provider_session_mode,
        ) {
            ProviderSessionStartMode::Create => {
                ExecutorServiceRequest::EffectiveWithCreateKnownProviderSessionId {
                    model: common.model,
                    provider: common.provider,
                    provider_index: common.provider_index,
                    prompt_mode: common.prompt_mode,
                    prompt: common.prompt,
                    working_dir: common.working_dir,
                    models_dir: common.models_dir,
                    extra_inputs: common.extra_inputs,
                    parent_invocation_env: common.parent_invocation_env,
                    start_known_provider_session_id,
                }
            }
            ProviderSessionStartMode::Resume => {
                ExecutorServiceRequest::EffectiveWithStartKnownProviderSessionId {
                    model: common.model,
                    provider: common.provider,
                    provider_index: common.provider_index,
                    prompt_mode: common.prompt_mode,
                    prompt: common.prompt,
                    working_dir: common.working_dir,
                    models_dir: common.models_dir,
                    extra_inputs: common.extra_inputs,
                    parent_invocation_env: common.parent_invocation_env,
                    start_known_provider_session_id,
                }
            }
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

struct KnownSessionRequestCommon {
    model: ModelConfig,
    provider: ProviderConfig,
    provider_index: usize,
    prompt_mode: PromptMode,
    prompt: String,
    working_dir: Option<PathBuf>,
    models_dir: Option<PathBuf>,
    extra_inputs: HashMap<String, Vec<String>>,
    parent_invocation_env: Option<String>,
}

fn known_session_request_common(
    input: &BalancedExecutorRequestInput<'_>,
) -> KnownSessionRequestCommon {
    KnownSessionRequestCommon {
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

fn required_start_known_provider_session_mode(
    mode: Option<ProviderSessionStartMode>,
) -> ProviderSessionStartMode {
    mode.expect("known provider session id requires a start mode")
}

pub(in crate::run::balancing) type ExecutorModelInput<'a> =
    (&'a ModelConfig, &'a ProviderConfig, usize, PromptMode);
pub(in crate::run::balancing) type ExecutorPromptInput<'a> = (
    &'a str,
    Option<&'a Path>,
    &'a Path,
    &'a HashMap<String, Vec<String>>,
);
pub(in crate::run::balancing) type ExecutorInvocationInput<'a> =
    (&'a str, Option<String>, Option<ProviderSessionStartMode>);

pub(in crate::run::balancing) fn balanced_executor_request_for_attempt(
    model_input: ExecutorModelInput<'_>,
    prompt_input: ExecutorPromptInput<'_>,
    invocation_input: ExecutorInvocationInput<'_>,
) -> ExecutorServiceRequest {
    let (model, provider, provider_index, prompt_mode) = model_input;
    let (prompt, working_dir, models_dir, extra_inputs) = prompt_input;
    let (invocation_env, start_known_provider_session_id, start_known_provider_session_mode) =
        invocation_input;
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
            start_known_provider_session_mode,
        },
    ))
}
