//! ## Declared roles
//!
//! `accessor`

mod derive;
mod errors;
mod input;
mod model_config;
mod provider_config;
mod resume;
mod session_capture;
mod session_storage;
mod toml_io;
mod tool_restrictions;

pub use derive::derive_provider_name;
pub use errors::ModelError;
pub use input::{InputDef, InputType};
pub use model_config::{InvocationMode, ModelConfig, PromptMode};
pub use provider_config::ProviderConfig;
pub use resume::{ResumeAcceptanceRules, ResumeKind, ResumeStrategy};
pub use session_capture::{SessionCapture, SessionCaptureKind};
pub use session_storage::{ScriptSessionStorageType, SessionStorage};
pub use toml_io::{load_models, render_validated_model_toml};
pub use tool_restrictions::{
    ClaudeRestrictions, CodexRestrictions, ToolRestrictionKind, ToolRestrictions,
};

#[cfg(test)]
pub(in crate::model) use toml_io::{
    CodexArgPart, codex_arg_overlap, construct_model_config_from_raw, emit_model_toml,
    parse_model_files, parse_model_toml, read_model_files, split_codex_arg_parts,
    validate_codex_model_arg_overlap, validate_model_toml_against_providers,
    validate_models_against_providers,
};

#[cfg(test)]
mod tests;
