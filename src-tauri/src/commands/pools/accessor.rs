//! ## Declared roles
//!
//! `accessor`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/pools/accessor.rs
//!     role: adapter
//!     Translates:
//!       - AppState pool model-cache mutex contract
//!       - pool model cache update contract
//! ```

use crate::AppState;
use oulipoly_config::ModelConfig;
use std::collections::HashMap;
use std::sync::MutexGuard;

pub(crate) fn lock_models(
    state: &AppState,
) -> Result<MutexGuard<'_, HashMap<String, ModelConfig>>, String> {
    state.models.lock().map_err(|e| e.to_string())
}

pub(crate) fn commit_pool_model_update(
    models: &mut HashMap<String, ModelConfig>,
    name: String,
    model: ModelConfig,
) {
    models.insert(name, model);
}
