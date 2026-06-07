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
use std::path::Path;
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
        let settings_id = provider_settings_id(&input.provider);
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
    canonical_opencode_settings_id(&provider.name, &provider.command)
        .unwrap_or_else(|| provider.name.clone())
}

fn canonical_opencode_settings_id(name: &str, command: &str) -> Option<String> {
    opencode_settings_index(name, command).map(format_opencode_settings_id)
}

fn opencode_settings_index(name: &str, command: &str) -> Option<u8> {
    opencode_account_index(name).or_else(|| opencode_account_index_from_command(command))
}

fn format_opencode_settings_id(index: u8) -> String {
    match index {
        1 => "opencode1".to_string(),
        other => format!("opencode{other}"),
    }
}

fn opencode_account_index_from_command(command: &str) -> Option<u8> {
    let tokens = opencode_command_tokens(command);
    opencode_account_index_from_tokens(&tokens)
}

fn opencode_command_tokens(command: &str) -> Vec<String> {
    crate::executor::cli::shell_split(command)
}

fn opencode_account_index_from_tokens(tokens: &[String]) -> Option<u8> {
    tokens
        .iter()
        .rev()
        .find_map(|part| opencode_account_index(part))
}

fn opencode_account_index(value: &str) -> Option<u8> {
    opencode_account_name_index(basename_or_value(value))
}

fn basename_or_value(value: &str) -> &str {
    Path::new(value)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(value)
}

fn opencode_account_name_index(name: &str) -> Option<u8> {
    match name {
        "opencode" | "opencode1" => Some(1),
        "opencode2" => Some(2),
        "opencode3" => Some(3),
        "opencode4" => Some(4),
        "opencode5" => Some(5),
        _ => None,
    }
}
