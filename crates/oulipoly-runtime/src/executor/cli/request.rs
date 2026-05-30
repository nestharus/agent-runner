//! ## Declared roles
//!
//! Roles: accessor.
//!
//! - accessor: [`EffectiveExecuteRequest`] exposes the borrowed effective
//!   execution request fields consumed by the executor facade.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/request.rs
//!     role: adapter
//!     Translates:
//!       - executor-effective-request-public-api-contract
//! ```

use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig};
use std::collections::HashMap;
use std::path::Path;

pub struct EffectiveExecuteRequest<'a> {
    pub model: &'a ModelConfig,
    pub provider: &'a ProviderConfig,
    pub provider_index: usize,
    pub prompt_mode: PromptMode,
    pub prompt: &'a str,
    pub working_dir: Option<&'a Path>,
    pub extra_inputs: &'a HashMap<String, Vec<String>>,
    pub parent_invocation_env: Option<&'a str>,
}
