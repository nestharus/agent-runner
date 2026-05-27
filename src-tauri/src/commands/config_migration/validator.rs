//! ## Declared roles
//!
//! `validator`, `mapper`, `filter`

use super::formatter::{
    format_command_must_be_string_error, format_migrated_provider_name_missing_error,
    format_missing_old_provider_command_error, format_provider_entry_not_table_error,
    format_toml_conflict_error, format_toml_string_array_error,
};
use std::path::Path;

pub(crate) fn validate_old_top_level_provider(provider: &toml::Table) -> Result<(), String> {
    if !provider.contains_key("command") {
        return Err(format_missing_old_provider_command_error());
    }
    Ok(())
}

pub(crate) fn validate_migrated_provider_name(
    provider_name: Option<String>,
    path: &Path,
) -> Result<String, String> {
    provider_name.ok_or_else(|| format_migrated_provider_name_missing_error(path))
}

pub(crate) fn validate_no_toml_conflict(
    existing: &toml::Value,
    value: &toml::Value,
    key: &str,
    provider_name: &str,
    path: &Path,
) -> Result<(), String> {
    if existing != value {
        Err(format_toml_conflict_error(
            existing,
            value,
            key,
            provider_name,
            path,
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn provider_table_from_value(
    provider_value: toml::Value,
    path: &Path,
) -> Result<toml::Table, String> {
    provider_value
        .as_table()
        .cloned()
        .ok_or_else(|| format_provider_entry_not_table_error(path))
}

pub(crate) fn take_provider_command(
    provider: &mut toml::Table,
    path: &Path,
) -> Result<Option<String>, String> {
    provider
        .remove("command")
        .map(|value| {
            value
                .as_str()
                .map(ToString::to_string)
                .ok_or_else(|| format_command_must_be_string_error(path))
        })
        .transpose()
}

pub(crate) fn take_string_array(table: &mut toml::Table, key: &str) -> Result<Vec<String>, String> {
    take_optional_string_array(table, key).map(|value| value.unwrap_or_default())
}

pub(crate) fn take_optional_string_array(
    table: &mut toml::Table,
    key: &str,
) -> Result<Option<Vec<String>>, String> {
    let Some(value) = table.remove(key) else {
        return Ok(None);
    };
    value
        .as_array()
        .ok_or_else(|| format_toml_string_array_error(key))?
        .iter()
        .map(|item| {
            item.as_str()
                .map(ToString::to_string)
                .ok_or_else(|| format_toml_string_array_error(key))
        })
        .collect::<Result<Vec<_>, String>>()
        .map(Some)
}
