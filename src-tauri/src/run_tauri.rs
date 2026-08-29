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
    let models_dir = default_models_dir().expect(RUNTIME_SERVICES_INIT_EXPECT_MESSAGE);
    let config_root = app_paths::models_config_root(&models_dir);
    let runtime_paths =
        runtime_paths_for(&models_dir, &config_root).expect(RUNTIME_SERVICES_INIT_EXPECT_MESSAGE);
    crate::wake_coordinator::start_wake_reclaim_maintenance_driver();
    let services = wiring::AgentRuntimeServices::production(runtime_paths)
        .expect(RUNTIME_SERVICES_INIT_EXPECT_MESSAGE);

    let providers =
        app_paths::load_providers_for_models_dir_with(&models_dir, &*services.providers_config);
    let models = config::load_models(&models_dir, Some(&providers)).unwrap_or_default();

    tauri::Builder::default()
        .manage(AppState::new(models_dir.clone(), models, &services))
        .invoke_handler(tauri::generate_handler![
            commands::setup_flow::check_setup_needed,
            commands::setup_flow::start_setup,
            commands::setup_flow::start_cli_setup,
            commands::models::reload::reload_models,
            commands::setup_flow::setup_respond,
            commands::setup_flow::cancel_setup,
            commands::setup_flow::detect_clis,
            commands::setup_flow::get_memory_graph,
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
            commands::quota_refresh::refresh_quotas,
            commands::pools::update_pool,
            test_model_command::orchestration::test_model,
            commands::providers_accounts::list_cli_providers,
            commands::providers_accounts::get_cli_provider,
            commands::providers_accounts::list_accounts,
            commands::providers_accounts::add_account,
            commands::providers_accounts::remove_account,
            commands::providers_accounts::sync_provider,
            commands::discovery::discover_models_cmd,
            commands::discovery::list_discovered_models,
            commands::discovery::get_model_parameters,
        ])
        .run(tauri::generate_context!())
        .expect(TAURI_RUN_EXPECT_MESSAGE);
}

fn default_models_dir() -> Result<PathBuf, String> {
    oulipoly_state::paths::config_dir().map(|root| root.join("models"))
}

fn runtime_paths_for(
    models_dir: &Path,
    config_root: &Path,
) -> Result<wiring::RuntimePaths, String> {
    let data_root = oulipoly_state::paths::data_dir()?;
    let working_dir = std::env::current_dir()
        .map_err(|error| format!("Could not resolve current working directory: {error}"))?;
    Ok(wiring::RuntimePaths {
        config_root: config_root.to_path_buf(),
        models_dir: models_dir.to_path_buf(),
        agents_dir: config_root.join("agents"),
        data_root: data_root.clone(),
        state_db_path: data_root.join("state.db"),
        lock_dir: data_root.join("locks"),
        working_dir,
    })
}
