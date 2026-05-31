//! ## Declared roles
//!
//! `orchestration`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/discovery/mod.rs
//!     role: adapter
//!     Translates:
//!       - Tauri IPC discovery command contract
//!       - discovered model DTO wire contract
//!       - model parameter DTO wire contract
//! ```

mod accessor;
mod formatter;
pub mod orchestration;
mod predicate;

pub(crate) use orchestration::{
    __cmd__discover_models_cmd, __cmd__get_model_parameters, __cmd__list_discovered_models,
    discover_models_cmd, get_model_parameters, list_discovered_models,
};
