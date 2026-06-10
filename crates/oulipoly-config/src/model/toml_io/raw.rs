//! ## Declared roles
//!
//! `parser`

use serde::Deserialize;

use super::super::{
    InvocationMode, ProviderConfig, ResumeAcceptanceRules, ResumeStrategy, SessionCapture,
    SessionStorage, ToolRestrictions, derive_provider_name,
};
use crate::provider_implementation_ref::ProviderImplementationRef;

// --- Raw TOML structures for deserialization ---

#[derive(Debug, Clone, Deserialize)]
pub(in crate::model) struct RawModelToml {
    pub(in crate::model) command: Option<String>,
    pub(in crate::model) args: Option<Vec<String>>,
    pub(in crate::model) interactive_args: Option<Vec<String>>,
    pub(in crate::model) resume: Option<ResumeStrategy>,
    pub(in crate::model) prompt_mode: Option<String>,
    pub(in crate::model) session_capture: Option<SessionCapture>,
    pub(in crate::model) resume_acceptance: Option<ResumeAcceptanceRules>,
    pub(in crate::model) session_storage: Option<SessionStorage>,
    #[serde(default)]
    pub(in crate::model) provider: Option<ProviderImplementationRef>,
    pub(in crate::model) providers: Option<Vec<RawProvider>>,
    pub(in crate::model) inputs: Option<Vec<RawInput>>,
}

#[derive(Debug, Clone, Deserialize)]
pub(in crate::model) struct RawProvider {
    #[serde(default)]
    pub(in crate::model) name: Option<String>,
    pub(in crate::model) command: Option<String>,
    pub(in crate::model) args: Option<Vec<String>>,
    pub(in crate::model) interactive_args: Option<Vec<String>>,
    pub(in crate::model) resume: Option<ResumeStrategy>,
    pub(in crate::model) prompt_mode: Option<String>,
    pub(in crate::model) session_capture: Option<SessionCapture>,
    pub(in crate::model) resume_acceptance: Option<ResumeAcceptanceRules>,
    pub(in crate::model) session_storage: Option<SessionStorage>,
    pub(in crate::model) system_prompt_override: Option<String>,
    pub(in crate::model) tool_restrictions: Option<ToolRestrictions>,
    #[serde(default)]
    pub(in crate::model) invocation_mode: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(in crate::model) struct RawInput {
    pub(in crate::model) name: String,
    #[serde(rename = "type")]
    pub(in crate::model) type_name: String,
    #[serde(default)]
    pub(in crate::model) required: bool,
    #[serde(default)]
    pub(in crate::model) default_input: bool,
    pub(in crate::model) default: Option<toml::Value>,
    pub(in crate::model) description: Option<String>,
    pub(in crate::model) flag: Option<String>,
    // Type-specific fields
    pub(in crate::model) options: Option<Vec<String>>,
    pub(in crate::model) min: Option<f64>,
    pub(in crate::model) max: Option<f64>,
    pub(in crate::model) item_type: Option<String>,
    pub(in crate::model) min_items: Option<usize>,
    pub(in crate::model) max_items: Option<usize>,
}

pub(super) fn raw_provider_name(provider: &RawProvider) -> String {
    provider.name.clone().unwrap_or_else(|| {
        provider
            .command
            .as_deref()
            .map(|command| derive_provider_name(command, provider.args.as_deref().unwrap_or(&[])))
            .unwrap_or_else(|| "<unknown>".to_string())
    })
}

pub(super) fn raw_provider_to_config(raw: RawProvider) -> ProviderConfig {
    let args = raw.args.unwrap_or_default();
    let name = raw.name.unwrap_or_else(|| {
        raw.command
            .as_deref()
            .map(|command| derive_provider_name(command, &args))
            .unwrap_or_default()
    });
    ProviderConfig {
        name,
        command: String::new(),
        args,
        interactive_args: raw.interactive_args,
        resume: None,
        session_capture: None,
        resume_acceptance: None,
        session_storage: None,
        system_prompt_override: None,
        tool_restrictions: None,
        invocation_mode: InvocationMode::Headless,
    }
}
