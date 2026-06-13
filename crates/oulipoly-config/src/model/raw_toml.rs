//! ## Declared roles
//!
//! - parser
//!
//! Role set: { parser }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-config/src/model/raw_toml.rs
//!     role: intrinsic-surface
//!     Domain: model-raw-toml-deserialization
//!     Owns:
//!       - RawModelToml deserialization shape for model TOML documents
//!       - RawProvider deserialization shape before root-provider validation
//!       - RawInput deserialization shape before typed input construction
//! ```
//!
use crate::provider_implementation_ref::ProviderImplementationRef;
use serde::Deserialize;

use super::resume::{ResumeAcceptanceRules, ResumeStrategy};
use super::session_capture::SessionCapture;
use super::session_storage::SessionStorage;
use super::tool_restrictions::ToolRestrictions;

#[derive(Debug, Clone, Deserialize)]
pub(super) struct RawModelToml {
    pub(super) command: Option<String>,
    pub(super) args: Option<Vec<String>>,
    pub(super) interactive_args: Option<Vec<String>>,
    pub(super) resume: Option<ResumeStrategy>,
    pub(super) prompt_mode: Option<String>,
    pub(super) session_capture: Option<SessionCapture>,
    pub(super) resume_acceptance: Option<ResumeAcceptanceRules>,
    pub(super) session_storage: Option<SessionStorage>,
    #[serde(default)]
    pub(super) provider: Option<ProviderImplementationRef>,
    pub(super) providers: Option<Vec<RawProvider>>,
    pub(super) inputs: Option<Vec<RawInput>>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct RawProvider {
    #[serde(default)]
    pub(super) name: Option<String>,
    pub(super) command: Option<String>,
    pub(super) args: Option<Vec<String>>,
    pub(super) interactive_args: Option<Vec<String>>,
    pub(super) resume: Option<ResumeStrategy>,
    pub(super) prompt_mode: Option<String>,
    pub(super) session_capture: Option<SessionCapture>,
    pub(super) resume_acceptance: Option<ResumeAcceptanceRules>,
    pub(super) session_storage: Option<SessionStorage>,
    pub(super) system_prompt_override: Option<String>,
    pub(super) tool_restrictions: Option<ToolRestrictions>,
    #[serde(default)]
    pub(super) invocation_mode: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct RawInput {
    pub(super) name: String,
    #[serde(rename = "type")]
    pub(super) type_name: String,
    #[serde(default)]
    pub(super) required: bool,
    #[serde(default)]
    pub(super) default_input: bool,
    pub(super) default: Option<toml::Value>,
    pub(super) description: Option<String>,
    pub(super) flag: Option<String>,
    // Type-specific fields
    pub(super) options: Option<Vec<String>>,
    pub(super) min: Option<f64>,
    pub(super) max: Option<f64>,
    pub(super) item_type: Option<String>,
    pub(super) min_items: Option<usize>,
    pub(super) max_items: Option<usize>,
}
