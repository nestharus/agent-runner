//! ## Declared roles
//!
//! `orchestration`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/models/orchestration.rs
//!     role: adapter
//!     Translates:
//!       - Tauri IPC model command contract
//!       - AppState model cache command contract
//!       - model persistence lifecycle contract
//!       - provider-settings refresh lifecycle contract
//!       - Result<String> error projection contract
//! ```
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: src-tauri/src/commands/models/orchestration.rs
//!     role: intrinsic-surface
//!     Domain: model-save lifecycle: validation, provider-aware render, file write, cache insert, and provider-settings refresh are one output-preserving command lifecycle.
//!     Owns:
//!       - src-tauri/src/commands/models/validator.rs
//!       - src-tauri/src/commands/models/formatter.rs
//!       - src-tauri/src/commands/models/accessor.rs
//!       - src-tauri/src/commands/provider_settings.rs
//!       - src-tauri/src/app_paths.rs
//!   - component: src-tauri/src/commands/models/orchestration.rs
//!     role: intrinsic-surface
//!     Domain: model-delete lifecycle: file deletion, cache removal, and provider-settings refresh are ordered side effects.
//!     Owns:
//!       - src-tauri/src/commands/models/formatter.rs
//!       - src-tauri/src/commands/models/accessor.rs
//!       - src-tauri/src/commands/provider_settings.rs
//! ```

use super::{ModelSummary, accessor, formatter, validator};
use crate::{AppState, app_state, provider_settings};
use oulipoly_config::ModelConfig;

#[tauri::command]
pub(crate) fn list_models(state: tauri::State<AppState>) -> Result<Vec<ModelSummary>, String> {
    let models = state.models.lock().map_err(|e| e.to_string())?;
    Ok(accessor::model_summaries(&models))
}

#[tauri::command]
pub(crate) fn get_model(
    state: tauri::State<AppState>,
    name: String,
) -> Result<ModelConfig, String> {
    let models = state.models.lock().map_err(|e| e.to_string())?;
    accessor::clone_model_by_name(&models, &name)
        .ok_or_else(|| formatter::model_not_found_error(&name))
}

#[tauri::command]
pub(crate) fn save_model(state: tauri::State<AppState>, model: ModelConfig) -> Result<(), String> {
    save_model_inner(&state, model)
}

pub fn save_model_inner(state: &AppState, model: ModelConfig) -> Result<(), String> {
    validator::validate_model_for_save(&model).map_err(formatter::format_model_validation_error)?;
    let toml_content = formatter::render_model_for_write(state, &model)?;
    let path = formatter::model_file_path(&state.models_dir, &model.name);

    accessor::persist_model_file(&state.models_dir, &path, &toml_content)
        .map_err(formatter::format_model_persistence_error)?;
    accessor::commit_saved_model(state, model)?;
    app_state::refresh_provider_registry(state)?;
    provider_settings::refresh_provider_settings_host(state)?;
    Ok(())
}

#[tauri::command]
pub(crate) fn delete_model(state: tauri::State<AppState>, name: String) -> Result<(), String> {
    let path = formatter::model_file_path(&state.models_dir, &name);
    if path.exists() {
        std::fs::remove_file(&path).map_err(formatter::delete_model_file_error)?;
    }
    accessor::remove_cached_model(&state, &name)?;
    app_state::refresh_provider_registry(&state)?;
    provider_settings::refresh_provider_settings_host(&state)?;
    Ok(())
}
