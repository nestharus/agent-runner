//! ## Declared roles
//!
//! `filter`, `mapper`, `accessor`, `validator`, `predicate`, `orchestration`

use super::mapper::ProviderRuntimeParts;
use super::mapper::session_storage_from_entry;
use super::predicate::{is_toml_path, should_repair_empty_array};
use super::validator::validate_no_toml_conflict;
use std::path::{Path, PathBuf};

pub(crate) fn sort_paths(paths: &mut [PathBuf]) {
    paths.sort();
}

pub(crate) fn filter_toml_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths
        .into_iter()
        .filter(|path| is_toml_path(path))
        .collect()
}

pub(crate) fn take_global_prompt_mode(table: &mut toml::Table) -> Option<toml::Value> {
    table.remove("prompt_mode")
}

pub(crate) fn count_runtime_provider_tables(providers_root: &toml::Table) -> usize {
    providers_root
        .iter()
        .filter(|(_, value)| {
            value
                .as_table()
                .is_some_and(|table| table.contains_key("command"))
        })
        .count()
}

pub(crate) fn combined_runtime_interactive_args(
    runtime_parts: &ProviderRuntimeParts,
) -> Vec<String> {
    let mut combined = runtime_parts.command_runtime_args.clone();
    if let Some(runtime_interactive_args) = &runtime_parts.runtime_interactive_args {
        combined.extend(runtime_interactive_args.clone());
    }
    combined
}

pub(crate) struct SessionStorageBackfill {
    pub(crate) provider_name: String,
    pub(crate) storage: toml::Table,
}

pub(crate) fn session_storage_backfill_candidates(
    sessions: toml::Table,
) -> Vec<SessionStorageBackfill> {
    sessions
        .into_iter()
        .filter_map(session_storage_backfill_candidate)
        .collect()
}

pub(crate) fn session_storage_backfill_candidate(
    (provider_name, entry): (String, toml::Value),
) -> Option<SessionStorageBackfill> {
    Some(SessionStorageBackfill {
        provider_name,
        storage: session_storage_from_entry(&entry)?,
    })
}

pub(crate) fn set_or_conflict(
    table: &mut toml::Table,
    key: &str,
    value: toml::Value,
    provider_name: &str,
    path: &Path,
) -> Result<(), String> {
    if let Some(existing) = table.get(key) {
        validate_no_toml_conflict(existing, &value, key, provider_name, path)?;
        return Ok(());
    }
    table.insert(key.to_string(), value);
    Ok(())
}

pub(crate) fn set_or_repair_empty_array(
    table: &mut toml::Table,
    key: &str,
    value: toml::Value,
    provider_name: &str,
    path: &Path,
) -> Result<(), String> {
    if should_repair_empty_array(table.get(key), &value) {
        table.insert(key.to_string(), value);
        return Ok(());
    }
    set_or_conflict(table, key, value, provider_name, path)
}
