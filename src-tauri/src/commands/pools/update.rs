//! ## Declared roles
//!
//! `orchestration`, `mapper`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/pools/update.rs
//!     role: adapter
//!     Translates:
//!       - Tauri IPC pool command contract
//!       - AppState model cache rewrite contract
//!       - pool rewrite lifecycle contract
//!       - Result<String> error projection contract
//! ```
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: src-tauri/src/commands/pools/update.rs
//!     role: intrinsic-surface
//!     Domain: pool-update lifecycle: provider-name normalization, provider-aware render validation, file writes, and cache mutation are one rewrite lifecycle without provider-settings refresh.
//!     Owns:
//!       - src-tauri/src/commands/pools/accessor.rs
//!       - src-tauri/src/commands/pools/derive.rs
//!       - src-tauri/src/commands/pools/validator.rs
//!       - src-tauri/src/commands/pools/writer.rs
//!       - src-tauri/src/app_paths.rs
//! ```

use super::{PoolSummary, accessor, derive, validator, writer};
use crate::AppState;
use crate::app_paths::load_providers_for_models_dir_with;
use oulipoly_config::{self as config, ModelConfig};
use std::collections::HashMap;

#[tauri::command]
pub(crate) fn list_pools(state: tauri::State<AppState>) -> Result<Vec<PoolSummary>, String> {
    let models = state.models.lock().map_err(|e| e.to_string())?;
    Ok(derive::derive_pools(&models))
}

#[tauri::command]
pub(crate) fn update_pool(
    state: tauri::State<AppState>,
    original_commands: Vec<String>,
    new_commands: Vec<String>,
) -> Result<(), String> {
    update_pool_inner(&state, original_commands, new_commands)
}

pub fn update_pool_inner(
    state: &AppState,
    original_commands: Vec<String>,
    new_commands: Vec<String>,
) -> Result<(), String> {
    validator::validate_new_pool_commands(&new_commands)
        .map_err(writer::format_pool_validation_error)?;

    let orig_sorted = derive::canonical_command_set(original_commands);
    let new_sorted = derive::canonical_command_set(new_commands);
    let providers = load_providers_for_models_dir_with(&state.models_dir, &*state.providers_config);
    let mut models_guard = accessor::lock_models(state)?;
    let matching_names = derive::matching_model_names(&models_guard, &orig_sorted);
    validator::validate_matching_models_exist(&matching_names)
        .map_err(writer::format_pool_validation_error)?;

    let removed = derive::provider_names_to_remove(&orig_sorted, &new_sorted);
    let added = derive::provider_names_to_add(&orig_sorted, &new_sorted);
    let rewritten = apply_pool_provider_rewrites(&models_guard, &matching_names, &removed, &added);
    validator::validate_rewritten_pool_models(&rewritten)
        .map_err(writer::format_pool_validation_error)?;
    let updates = writer::render_pool_model_updates(&rewritten, &providers)?;

    for (name, model, toml_content) in updates {
        writer::write_pool_model_update(&state.models_dir, &name, &toml_content)?;
        accessor::commit_pool_model_update(&mut models_guard, name, model);
    }

    Ok(())
}

pub(crate) fn apply_pool_provider_rewrites(
    models: &HashMap<String, ModelConfig>,
    matching_names: &[String],
    removed: &[&String],
    added: &[&String],
) -> Vec<(String, ModelConfig)> {
    matching_names
        .iter()
        .map(|name| {
            let mut model = models.get(name).unwrap().clone();
            apply_pool_provider_rewrite(&mut model, removed, added);
            (name.clone(), model)
        })
        .collect()
}

pub(crate) fn apply_pool_provider_rewrite(
    model: &mut ModelConfig,
    removed: &[&String],
    added: &[&String],
) {
    model.providers.retain(|p| !removed.contains(&&p.name));
    for cmd in added {
        model.providers.push(config::ProviderConfig::model_provider(
            (*cmd).clone(),
            vec![],
        ));
    }
}
