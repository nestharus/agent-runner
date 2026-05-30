//! ## Declared roles
//!
//! `orchestration`, `mapper`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/run_tauri.rs
//!     role: adapter
//!     Translates:
//!       - Tauri application runtime contract
//!       - Tauri IPC command-registration contract
//!       - RuntimePaths construction contract
//!       - production AgentRuntimeServices construction contract
//! ```

use crate::AppState;
use crate::app_paths;
use crate::commands;
use crate::provider_settings;
use crate::test_model_command;
use crate::wiring;
use oulipoly_config as config;
use std::path::{Path, PathBuf};

const RUNTIME_SERVICES_INIT_EXPECT_MESSAGE: &str = "failed to initialize runtime services";
const TAURI_RUN_EXPECT_MESSAGE: &str = "error while running tauri application";

pub fn run_tauri() {
    let models_dir = default_models_dir();
    let config_root = app_paths::models_config_root(&models_dir);
    let runtime_paths = runtime_paths_for(&models_dir, &config_root);
    let services = wiring::AgentRuntimeServices::production(runtime_paths)
        .expect(RUNTIME_SERVICES_INIT_EXPECT_MESSAGE);

    let providers =
        app_paths::load_providers_for_models_dir_with(&models_dir, &*services.providers_config);
    let models = config::load_models(&models_dir, Some(&providers)).unwrap_or_default();

    tauri::Builder::default()
        .manage(AppState::new(models_dir.clone(), models, &services))
        .invoke_handler(tauri::generate_handler![
            crate::check_setup_needed,
            crate::start_setup,
            crate::start_cli_setup,
            crate::reload_models,
            crate::setup_respond,
            crate::cancel_setup,
            crate::detect_clis,
            crate::get_memory_graph,
            commands::models::list_models,
            commands::models::get_model,
            commands::models::save_model,
            commands::models::delete_model,
            commands::pools::list_pools,
            provider_settings::list_provider_settings_targets,
            provider_settings::get_provider_settings_schema,
            provider_settings::list_provider_settings,
            provider_settings::get_provider_settings,
            provider_settings::create_provider_settings,
            provider_settings::update_provider_settings,
            provider_settings::delete_provider_settings,
            provider_settings::validate_provider_settings,
            provider_settings::migrate_provider_settings,
            crate::refresh_quotas,
            commands::pools::update_pool,
            test_model_command::orchestration::test_model,
            crate::list_cli_providers,
            crate::get_cli_provider,
            crate::list_accounts,
            crate::add_account,
            crate::remove_account,
            crate::sync_provider,
            crate::discover_models_cmd,
            crate::list_discovered_models,
            crate::get_model_parameters,
        ])
        .run(tauri::generate_context!())
        .expect(TAURI_RUN_EXPECT_MESSAGE);
}

fn default_models_dir() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("oulipoly-agent-runner").join("models"))
        .unwrap_or_else(|| PathBuf::from("models"))
}

fn runtime_paths_for(models_dir: &Path, config_root: &Path) -> wiring::RuntimePaths {
    wiring::RuntimePaths {
        config_root: config_root.to_path_buf(),
        models_dir: models_dir.to_path_buf(),
        agents_dir: config_root.join("agents"),
        data_root: config_root.to_path_buf(),
        state_db_path: config_root.join("state.db"),
        lock_dir: config_root.join("locks"),
        working_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    }
}
