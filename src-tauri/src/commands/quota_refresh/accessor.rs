//! ## Declared roles
//!
//! `accessor`, `mapper`, `predicate`, `formatter`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/quota_refresh/accessor.rs
//!     role: adapter
//!     Translates:
//!       - AppState model-cache mutex contract
//!       - providers.toml parent-path contract
//!       - state.db parent-path contract
//!       - quota service request contract
//!       - quota staleness predicate contract
//! ```

use crate::AppState;
use oulipoly_config::{ModelConfig, ProvidersConfig};
use oulipoly_runtime::quota;
use oulipoly_runtime::services::QuotaServiceRequest;
use oulipoly_state::StateDb;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::MutexGuard;

pub(crate) fn lock_models(
    state: &AppState,
) -> Result<MutexGuard<'_, HashMap<String, ModelConfig>>, String> {
    state.models.lock().map_err(|error| error.to_string())
}

pub(crate) fn providers_config_path(models_dir: &Path) -> PathBuf {
    config_root_for_models_dir(models_dir).join("providers.toml")
}

pub(crate) fn load_providers_config_or_default(
    state: &AppState,
    providers_path: &Path,
) -> ProvidersConfig {
    state
        .providers_config
        .load_providers(providers_path)
        .unwrap_or_default()
}

pub(crate) fn state_db_path(models_dir: &Path) -> PathBuf {
    config_root_for_models_dir(models_dir).join("state.db")
}

pub(crate) fn open_state_db(state: &AppState, db_path: &Path) -> Result<StateDb, String> {
    state
        .state_db_opener
        .open_at(db_path)
        .map_err(format_state_db_open_error)
}

pub(crate) fn provider_is_stale(db: &StateDb, provider_name: &str) -> bool {
    quota::is_stale(db, provider_name)
}

pub(crate) fn refresh_provider_quota(
    state: &AppState,
    provider_name: &str,
    providers_cfg: &ProvidersConfig,
    db: &StateDb,
) -> Result<quota::RefreshOutcome, String> {
    state
        .quota_service
        .refresh_quota(QuotaServiceRequest {
            provider_name: provider_name.to_string(),
            providers_cfg,
            in_flight: &state.quota_in_flight,
            state: db,
        })
        .map_err(|error| error.to_string())
        .map(|output| output.outcome)
}

fn config_root_for_models_dir(models_dir: &Path) -> &Path {
    models_dir.parent().unwrap_or(models_dir)
}

fn format_state_db_open_error(error: String) -> String {
    format!("Failed to open state DB: {error}")
}
