//! ## Declared roles
//!
//! `mapper`

use serde::{Deserialize, Serialize};

use super::{
    InvocationMode, ResumeAcceptanceRules, ResumeStrategy, SessionCapture, SessionStorage,
    ToolRestrictions, derive_provider_name,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Stable identifier for this provider (the CLI account / binary).
    /// Used as the key for per-account state like quota tracking.
    /// If omitted in TOML, derived from command+args via `derive_provider_name`.
    pub name: String,
    #[serde(default)]
    pub command: String,
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interactive_args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<ResumeStrategy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_capture: Option<SessionCapture>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_acceptance: Option<ResumeAcceptanceRules>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_storage: Option<SessionStorage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt_override: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_restrictions: Option<ToolRestrictions>,
    #[serde(default)]
    pub invocation_mode: InvocationMode,
}

impl ProviderConfig {
    /// Build a ProviderConfig, auto-deriving `name` from command+args.
    pub fn new(command: impl Into<String>, args: Vec<String>) -> Self {
        let command = command.into();
        let name = derive_provider_name(&command, &args);
        Self {
            name,
            command,
            args,
            interactive_args: None,
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: InvocationMode::Headless,
        }
    }

    pub fn model_provider(name: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            name: name.into(),
            command: String::new(),
            args,
            interactive_args: None,
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: InvocationMode::Headless,
        }
    }
}
