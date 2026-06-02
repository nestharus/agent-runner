//! ## Declared roles
//!
//! `none`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/pools/mod.rs
//!     role: adapter
//!     Translates:
//!       - PoolSummary serialization contract
//!       - PoolSummary field-name wire contract
//!       - frontend pool-list DTO compatibility contract
//! ```

mod accessor;
pub mod derive;
pub mod update;
mod validator;
mod writer;

use serde::Serialize;

#[derive(Serialize, Clone, Debug)]
pub struct PoolSummary {
    pub commands: Vec<String>,
    pub model_count: usize,
    pub model_names: Vec<String>,
}

pub use derive::derive_pools;
pub use update::update_pool_inner;
pub(crate) use update::{__cmd__list_pools, __cmd__update_pool, list_pools, update_pool};
