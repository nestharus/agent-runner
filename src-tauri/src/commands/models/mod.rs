//! ## Declared roles
//!
//! `none`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/models/mod.rs
//!     role: adapter
//!     Translates:
//!       - ModelSummary serialization contract
//!       - ModelSummary field-name wire contract
//!       - frontend model-list DTO compatibility contract
//! ```

mod accessor;
mod formatter;
pub mod orchestration;
mod validator;

use oulipoly_config as config;
use serde::Serialize;

#[derive(Serialize)]
pub struct ModelSummary {
    pub name: String,
    pub prompt_mode: config::PromptMode,
    pub provider_count: usize,
}

pub use orchestration::save_model_inner;
pub(crate) use orchestration::{
    __cmd__delete_model, __cmd__get_model, __cmd__list_models, __cmd__save_model, delete_model,
    get_model, list_models, save_model,
};
