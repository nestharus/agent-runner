//! ## Declared roles
//!
//! `filter`, `mapper`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/quota_refresh/candidates.rs
//!     role: adapter
//!     Translates:
//!       - multi-provider quota-refresh candidate contract
//!       - sorted provider-name output contract
//!       - deduplicated provider-name output contract
//! ```

use oulipoly_config::ModelConfig;
use std::collections::{HashMap, HashSet};

pub(crate) fn provider_names_for_multi_provider_models(
    models: &HashMap<String, ModelConfig>,
) -> Vec<String> {
    sort_provider_names(deduplicate_provider_names(
        multi_provider_models(models)
            .flat_map(model_provider_names)
            .collect(),
    ))
}

fn multi_provider_models(
    models: &HashMap<String, ModelConfig>,
) -> impl Iterator<Item = &ModelConfig> {
    models.values().filter(|model| model.providers.len() > 1)
}

fn model_provider_names(model: &ModelConfig) -> impl Iterator<Item = String> + '_ {
    model.providers.iter().map(|provider| provider.name.clone())
}

fn deduplicate_provider_names(provider_names: Vec<String>) -> HashSet<String> {
    provider_names.into_iter().collect()
}

fn sort_provider_names(provider_names: HashSet<String>) -> Vec<String> {
    let mut provider_names: Vec<String> = provider_names.into_iter().collect();
    provider_names.sort();
    provider_names
}
