//! Dedicated model-reload command owner.
//!
//! ## Declared roles
//!
//! `orchestration`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/models/reload.rs
//!     role: adapter
//!     Translates:
//!       - model-reload lifecycle contract
//!       - provider-config load contract
//!       - model-cache mutation contract
//!       - provider-settings refresh contract
//!       - Tauri command registration contract
//! ```

use crate::{AppState, app_paths, provider_settings};
use oulipoly_config as config;

#[tauri::command]
pub(crate) fn reload_models(state: tauri::State<AppState>) -> Result<(), String> {
    reload_models_inner(&state)
}

pub fn reload_models_inner(state: &AppState) -> Result<(), String> {
    let providers =
        app_paths::load_providers_for_models_dir_with(&state.models_dir, &*state.providers_config);
    let fresh = config::load_models(&state.models_dir, Some(&providers)).unwrap_or_default();
    let mut models = state.models.lock().map_err(|e| e.to_string())?;
    *models = fresh;
    drop(models);
    provider_settings::refresh_provider_settings_host(state)?;
    Ok(())
}
