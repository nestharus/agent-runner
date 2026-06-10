//! ## Declared roles
//!
//! `accessor`
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-config/src/model/model_config.rs
//!     role: intrinsic-surface
//!     Domain: model_provider_session_config
//!     Owns:
//!       - prompt mode (positional, file, stdin)
//! ```

use crate::provider_implementation_ref::ProviderImplementationRef;
use serde::{Deserialize, Serialize};

use super::{InputDef, ProviderConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum InvocationMode {
    #[default]
    Headless,
    Proxy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub name: String,
    pub prompt_mode: PromptMode,
    pub providers: Vec<ProviderConfig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<InputDef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderImplementationRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptMode {
    Stdin,
    Arg,
}

impl ModelConfig {
    pub fn default_input(&self) -> Option<&InputDef> {
        self.inputs.iter().find(|i| i.default_input)
    }
}
