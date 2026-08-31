//! ## Declared roles
//!
//! - accessor
//!
//! Role set: { accessor }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-config/src/providers/raw_entry.rs
//!     role: intrinsic-surface
//!     Domain: raw-providers-toml-schema
//!     Owns:
//!       - raw providers.toml HashMap carrier and deserialization-only provider entry schema
//!       - serde field defaults for provider account TOML ingestion
//!       - raw provider account child-environment additions and removals
//!       - model-derived raw field carriers subordinate to provider account parsing
//! ```
//!
use crate::model::{
    ResumeAcceptanceRules, ResumeStrategy, SessionCapture, SessionStorage, ToolRestrictions,
};
use crate::provider_implementation_ref::ProviderImplementationRef;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

pub(crate) type RawProvidersToml = HashMap<String, RawEntry>;
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct RawEntry {
    #[serde(default)]
    pub(crate) implementation: Option<ProviderImplementationRef>,
    #[serde(default)]
    pub(crate) quota_script: Option<String>,
    #[serde(default)]
    pub(crate) auth_refresh_command: Option<String>,
    #[serde(default)]
    pub(crate) command: Option<String>,
    #[serde(default)]
    pub(crate) args: Vec<String>,
    #[serde(default)]
    pub(crate) environment: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) unset_environment: Vec<String>,
    #[serde(default)]
    pub(crate) interactive_args: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) prompt_mode: Option<String>,
    #[serde(default)]
    pub(crate) resume: Option<ResumeStrategy>,
    #[serde(default)]
    pub(crate) session_capture: Option<SessionCapture>,
    #[serde(default)]
    pub(crate) resume_acceptance: Option<ResumeAcceptanceRules>,
    #[serde(default)]
    pub(crate) session_storage: Option<SessionStorage>,
    #[serde(default)]
    pub(crate) system_prompt_override: Option<String>,
    #[serde(default)]
    pub(crate) tool_restrictions: Option<ToolRestrictions>,
    #[serde(default)]
    pub(crate) invocation_mode: Option<String>,
}
