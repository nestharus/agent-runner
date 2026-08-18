//! ## Declared roles
//!
//! `accessor`, `mapper`
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/external_provider/context.rs
//!     role: intrinsic-surface
//!     Domain: external-provider-dispatch-context-carrier
//!     Owns:
//!       - ExternalProviderDispatchContext carrier fields
//!       - ExternalProviderDispatchInput carrier fields
//!       - AccountSelection carrier fields
//!       - settings_id derivation invariant
//!       - provider session start intent fields
//! ```

use crate::services::ProviderSessionStartMode;
use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Clone)]
pub(crate) struct ExternalProviderDispatchContext {
    pub(crate) model: ModelConfig,
    pub(crate) provider: ProviderConfig,
    pub(crate) provider_index: usize,
    pub(crate) prompt_mode: PromptMode,
    pub(crate) prompt: String,
    pub(crate) extra_inputs: HashMap<String, Vec<String>>,
    pub(crate) working_dir: Option<PathBuf>,
    pub(crate) models_dir: Option<PathBuf>,
    pub(crate) parent_invocation_env: Option<String>,
    pub(crate) start_known_provider_session_id: Option<String>,
    pub(crate) start_known_provider_session_mode: Option<ProviderSessionStartMode>,
    pub(crate) settings_id: String,
}

#[derive(Clone)]
pub(crate) struct ExternalProviderDispatchInput {
    pub(crate) model: ModelConfig,
    pub(crate) provider: ProviderConfig,
    pub(crate) provider_index: usize,
    pub(crate) prompt_mode: PromptMode,
    pub(crate) prompt: String,
    pub(crate) extra_inputs: HashMap<String, Vec<String>>,
    pub(crate) working_dir: Option<PathBuf>,
    pub(crate) models_dir: Option<PathBuf>,
    pub(crate) parent_invocation_env: Option<String>,
    pub(crate) start_known_provider_session_id: Option<String>,
    pub(crate) start_known_provider_session_mode: Option<ProviderSessionStartMode>,
}

impl From<ExternalProviderDispatchInput> for ExternalProviderDispatchContext {
    fn from(input: ExternalProviderDispatchInput) -> Self {
        let settings_id = provider_settings_id(&input.provider);
        Self {
            model: input.model,
            provider: input.provider,
            provider_index: input.provider_index,
            prompt_mode: input.prompt_mode,
            prompt: input.prompt,
            extra_inputs: input.extra_inputs,
            working_dir: input.working_dir,
            models_dir: input.models_dir,
            parent_invocation_env: input.parent_invocation_env,
            start_known_provider_session_id: input.start_known_provider_session_id,
            start_known_provider_session_mode: input.start_known_provider_session_mode,
            settings_id,
        }
    }
}

/// One pool account to attempt during FIX #32 transport-timeout rotation.
#[derive(Debug, Clone)]
pub(crate) struct AccountSelection {
    pub(crate) provider: ProviderConfig,
    pub(crate) provider_index: usize,
}

impl ExternalProviderDispatchContext {
    /// Re-target this dispatch context at a different pool account, recomputing
    /// the per-account `settings_id`. All other fields (prompt, inputs, working
    /// dir, parent linkage) are account-independent and carried verbatim.
    pub(crate) fn with_account(&self, account: AccountSelection) -> Self {
        let settings_id = provider_settings_id(&account.provider);
        Self {
            provider: account.provider,
            provider_index: account.provider_index,
            settings_id,
            ..self.clone()
        }
    }
}

fn provider_settings_id(provider: &ProviderConfig) -> String {
    provider.name.clone()
}
