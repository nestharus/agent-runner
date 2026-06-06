//! Role: carrier.
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - surface_id: age217_s6a_dispatch_context
//!     component: crates/oulipoly-runtime/src/executor/external_provider/context.rs
//!     role: carrier
//!     carrier: ExternalProviderDispatchContext
//!     invariant: carrier == actual, with settings_id derived from the selected provider account
//! ```

use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub(crate) struct ExternalProviderDispatchContext {
    pub(crate) model: ModelConfig,
    pub(crate) provider: ProviderConfig,
    pub(crate) provider_index: usize,
    pub(crate) prompt_mode: PromptMode,
    pub(crate) prompt: String,
    pub(crate) extra_inputs: HashMap<String, Vec<String>>,
    pub(crate) working_dir: Option<PathBuf>,
    pub(crate) parent_invocation_env: Option<String>,
    pub(crate) start_known_provider_session_id: Option<String>,
    pub(crate) settings_id: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ExternalProviderDispatchInput {
    pub(crate) model: ModelConfig,
    pub(crate) provider: ProviderConfig,
    pub(crate) provider_index: usize,
    pub(crate) prompt_mode: PromptMode,
    pub(crate) prompt: String,
    pub(crate) extra_inputs: HashMap<String, Vec<String>>,
    pub(crate) working_dir: Option<PathBuf>,
    pub(crate) parent_invocation_env: Option<String>,
    pub(crate) start_known_provider_session_id: Option<String>,
}

impl From<ExternalProviderDispatchInput> for ExternalProviderDispatchContext {
    fn from(input: ExternalProviderDispatchInput) -> Self {
        let settings_id = input.provider.name.clone();
        Self {
            model: input.model,
            provider: input.provider,
            provider_index: input.provider_index,
            prompt_mode: input.prompt_mode,
            prompt: input.prompt,
            extra_inputs: input.extra_inputs,
            working_dir: input.working_dir,
            parent_invocation_env: input.parent_invocation_env,
            start_known_provider_session_id: input.start_known_provider_session_id,
            settings_id,
        }
    }
}
