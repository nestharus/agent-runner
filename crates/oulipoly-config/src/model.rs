//! ## Declared roles
//!
//! - accessor
//! - orchestration
//!
//! Role set: { accessor, orchestration }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-config/src/model.rs
//!     role: intrinsic-surface
//!     Domain: model-config-facade
//!     Owns:
//!       - public model module facade and frozen re-export surface
//!       - submodule declaration hub for model configuration concerns
//!       - crate-internal test support aliases for moved model loader helpers
//! ```

mod codex_overlap;
mod config;
mod error;
mod loader;
mod provider_config;
mod provider_name;
mod raw_toml;
mod resume;
mod session_capture;
mod session_storage;
mod tool_restrictions;

#[allow(unused_imports)]
pub(crate) use self::codex_overlap::validate_codex_model_arg_overlap;
pub use self::config::{InputDef, InputType, ModelConfig, PromptMode};
pub use self::error::ModelError;
pub use self::loader::{load_models, render_validated_model_toml};
pub use self::provider_config::{InvocationMode, ProviderConfig};
pub use self::provider_name::derive_provider_name;
pub use self::resume::{ResumeAcceptanceRules, ResumeKind, ResumeStrategy};
pub use self::session_capture::{SessionCapture, SessionCaptureKind};
pub use self::session_storage::{ScriptSessionStorageType, SessionStorage};
pub use self::tool_restrictions::{
    ClaudeRestrictions, CodexRestrictions, ToolRestrictionKind, ToolRestrictions,
};

#[cfg(test)]
use self::codex_overlap::{codex_arg_overlap, split_codex_arg_parts, CodexArgPart};

#[cfg(test)]
use self::loader::{
    construct_model_config_from_raw, emit_model_toml, parse_model_files, parse_model_toml,
    read_model_files, validate_model_toml_against_providers, validate_models_against_providers,
};

#[cfg(test)]
mod tests;
