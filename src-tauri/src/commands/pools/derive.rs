//! ## Declared roles
//!
//! `mapper`, `filter`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/pools/derive.rs
//!     role: adapter
//!     Translates:
//!       - ProviderConfig.name pool grouping contract
//!       - sorted/deduplicated command-set contract
//!       - sorted pool model-name contract
//! ```

use super::PoolSummary;
use oulipoly_config::ModelConfig;
use std::collections::HashMap;

pub fn derive_pools(models: &HashMap<String, ModelConfig>) -> Vec<PoolSummary> {
    let mut groups: HashMap<Vec<String>, Vec<String>> = HashMap::new();

    for model in models.values() {
        groups
            .entry(canonical_provider_names(model))
            .or_default()
            .push(model.name.clone());
    }

    let mut pools: Vec<PoolSummary> = groups
        .into_iter()
        .map(|(commands, mut model_names)| {
            model_names.sort();
            PoolSummary {
                model_count: model_names.len(),
                commands,
                model_names,
            }
        })
        .collect();

    pools.sort_by(|a, b| a.commands.cmp(&b.commands));
    pools
}

pub(crate) fn canonical_provider_names(model: &ModelConfig) -> Vec<String> {
    let cmds: Vec<String> = model.providers.iter().map(|p| p.name.clone()).collect();
    canonical_command_set(cmds)
}

pub(crate) fn canonical_command_set(mut commands: Vec<String>) -> Vec<String> {
    commands.sort();
    commands.dedup();
    commands
}

pub(crate) fn matching_model_names(
    models: &HashMap<String, ModelConfig>,
    original_commands: &[String],
) -> Vec<String> {
    models
        .values()
        .filter(|m| canonical_provider_names(m) == original_commands)
        .map(|m| m.name.clone())
        .collect()
}

pub(crate) fn provider_names_to_remove<'a>(
    original_commands: &'a [String],
    new_commands: &[String],
) -> Vec<&'a String> {
    original_commands
        .iter()
        .filter(|c| !new_commands.contains(c))
        .collect()
}

pub(crate) fn provider_names_to_add<'a>(
    original_commands: &[String],
    new_commands: &'a [String],
) -> Vec<&'a String> {
    new_commands
        .iter()
        .filter(|c| !original_commands.contains(c))
        .collect()
}
