//! ## Declared roles
//!
//! `accessor`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/models/accessor.rs
//!     role: adapter
//!     Translates:
//!       - AppState model-cache mutex contract
//!       - ModelConfig clone/read contract
//!       - model cache insertion contract
//!       - model cache removal contract
//!       - model file persistence contract
//! ```

use super::ModelSummary;
use crate::AppState;
use oulipoly_config::ModelConfig;
use std::collections::HashMap;
use std::path::Path;

pub(crate) enum ModelPersistenceError {
    CreateDir(std::io::Error),
    WriteFile(std::io::Error),
}

pub(crate) fn model_summaries(models: &HashMap<String, ModelConfig>) -> Vec<ModelSummary> {
    let mut summaries: Vec<ModelSummary> = models
        .values()
        .map(|m| ModelSummary {
            name: m.name.clone(),
            prompt_mode: m.prompt_mode,
            provider_count: m.providers.len(),
        })
        .collect();
    summaries.sort_by(|a, b| a.name.cmp(&b.name));
    summaries
}

pub(crate) fn clone_model_by_name(
    models: &HashMap<String, ModelConfig>,
    name: &str,
) -> Option<ModelConfig> {
    models.get(name).cloned()
}

pub(crate) fn persist_model_file(
    models_dir: &Path,
    path: &Path,
    toml_content: &str,
) -> Result<(), ModelPersistenceError> {
    std::fs::create_dir_all(models_dir).map_err(ModelPersistenceError::CreateDir)?;
    std::fs::write(path, toml_content).map_err(ModelPersistenceError::WriteFile)
}

pub(crate) fn commit_saved_model(state: &AppState, model: ModelConfig) -> Result<(), String> {
    let mut models = state.models.lock().map_err(|e| e.to_string())?;
    models.insert(model.name.clone(), model);
    Ok(())
}

pub(crate) fn remove_cached_model(state: &AppState, name: &str) -> Result<(), String> {
    let mut models = state.models.lock().map_err(|e| e.to_string())?;
    models.remove(name);
    Ok(())
}
