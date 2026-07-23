//! ## Declared roles
//!
//! - accessor
//! - mapper
//!
//! Role set: { accessor, mapper }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-config/src/model/provider_config.rs
//!     role: intrinsic-surface
//!     Domain: model-provider-config
//!     Owns:
//!       - ProviderConfig account reference and model-level provider arguments
//!       - Provider account child-environment additions and removals
//!       - InvocationMode CLI launch mode marker subordinate to provider config
//!       - Resume/session/tool restriction carriers attached to a model provider entry
//! ```
//!
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::provider_name::derive_provider_name;
use super::resume::{ResumeAcceptanceRules, ResumeStrategy};
use super::session_capture::SessionCapture;
use super::session_storage::SessionStorage;
use super::tool_restrictions::ToolRestrictions;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum InvocationMode {
    #[default]
    Headless,
    Proxy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Stable identifier for this provider (the CLI account / binary).
    /// Used as the key for per-account state like quota tracking.
    /// If omitted in TOML, derived from command+args via `derive_provider_name`.
    pub name: String,
    #[serde(default)]
    pub command: String,
    pub args: Vec<String>,
    /// Values added to or replaced in the inherited child environment.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub environment: BTreeMap<String, String>,
    /// Inherited names removed before `environment` values are applied.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unset_environment: Vec<String>,
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
            environment: BTreeMap::new(),
            unset_environment: Vec::new(),
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
            environment: BTreeMap::new(),
            unset_environment: Vec::new(),
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
