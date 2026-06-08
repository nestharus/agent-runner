//! ## Declared roles
//!
//! `none`
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: src-tauri/src/lib.rs
//!     role: none
//!     Domain: tauri-client facade
//!     Owns:
//!       - functionless facade sentinel
//!       - public re-export compatibility
//!       - module declaration boundary
//! ```

mod app_paths;
mod app_state;
#[path = "lib_commands.rs"]
pub mod commands;
#[path = "commands/provider_settings.rs"]
pub mod provider_settings;
mod run_tauri;
pub mod setup;
pub mod terminal_outcome_adapter;
#[path = "commands/test_model/mod.rs"]
pub mod test_model_command;
pub mod usage;
#[allow(dead_code)]
mod wake_coordinator;
mod wiring;
pub mod zero_turn_orchestration;

pub use app_paths::{
    AppConfig, load_app_config, load_providers_for_models_dir, load_providers_for_models_dir_with,
    try_load_app_config,
};
pub use app_state::{AppState, AppStateTestServices};
pub use commands::models::ModelSummary;
pub use commands::models::save_model_inner;
pub use commands::pools::PoolSummary;
pub use commands::pools::derive_pools;
pub use commands::pools::update_pool_inner;
pub use run_tauri::run_tauri;
pub use test_model_command::{TestModelResult, effective_provider_for_model_provider};
