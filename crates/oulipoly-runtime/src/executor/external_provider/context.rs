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
//!       - settings_id derivation invariant
//!       - provider session start intent fields
//! ```

use crate::services::{MailboxDeliveryCorrelation, ProviderSessionStartMode};
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
    pub(crate) mailbox_delivery_correlation: Option<MailboxDeliveryCorrelation>,
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
    pub(crate) mailbox_delivery_correlation: Option<MailboxDeliveryCorrelation>,
}

impl From<ExternalProviderDispatchInput> for ExternalProviderDispatchContext {
    fn from(input: ExternalProviderDispatchInput) -> Self {
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
            mailbox_delivery_correlation: input.mailbox_delivery_correlation,
            settings_id: String::new(),
        }
    }
}

impl ExternalProviderDispatchContext {
    pub(crate) fn with_settings_id(&self, settings_id: &str) -> Self {
        Self {
            settings_id: settings_id.to_string(),
            ..self.clone()
        }
    }
}
