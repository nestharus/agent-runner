//! ## Declared roles
//!
//! `accessor`, `parser`, `formatter`, `filter`, `predicate`, `validator`, `orchestration`, `mapper`

use super::filter::{filter_toml_paths, sort_paths};
use super::formatter::{
    format_create_dir_error, format_model_dir_read_error, format_runtime_provider_entry_not_table,
    format_text_read_error, format_text_write_error, format_toml_read_error,
};
use super::parser::{parse_toml_table, serialize_toml_table};
use super::predicate::path_exists;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) fn read_optional_toml_table(path: &Path) -> Result<toml::Table, String> {
    if path.exists() {
        read_toml_table(path)
    } else {
        Ok(toml::Table::new())
    }
}

pub(crate) fn read_toml_table(path: &Path) -> Result<toml::Table, String> {
    parse_toml_table(path, &read_toml_text(path)?)
}

pub(crate) fn read_existing_toml_table(path: &Path) -> Result<Option<toml::Table>, String> {
    if path_exists(path) {
        read_toml_table(path).map(Some)
    } else {
        Ok(None)
    }
}

pub(crate) fn read_toml_text(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format_toml_read_error(path, e))
}

pub(crate) fn model_toml_paths(models_dir: &Path) -> Result<Vec<PathBuf>, String> {
    Ok(sorted_toml_paths(model_dir_paths(models_dir)?))
}

pub(crate) fn model_dir_paths(models_dir: &Path) -> Result<Vec<PathBuf>, String> {
    if path_exists(models_dir) {
        Ok(dir_entry_paths(read_model_dir_entries(models_dir)?))
    } else {
        Ok(Vec::new())
    }
}

pub(crate) fn sorted_toml_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut paths = toml_paths_from_paths(paths);
    sort_paths(&mut paths);
    paths
}

pub(crate) fn read_model_dir_entries(models_dir: &Path) -> Result<fs::ReadDir, String> {
    std::fs::read_dir(models_dir).map_err(|e| format_model_dir_read_error(models_dir, e))
}

pub(crate) fn toml_paths_from_dir_entries(entries: fs::ReadDir) -> Vec<PathBuf> {
    toml_paths_from_paths(dir_entry_paths(entries))
}

pub(crate) fn dir_entry_paths(entries: fs::ReadDir) -> Vec<PathBuf> {
    entries.filter_map(read_dir_entry_path).collect()
}

pub(crate) fn toml_paths_from_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    filter_toml_paths(paths)
}

pub(crate) fn read_dir_entry_path(entry: Result<fs::DirEntry, std::io::Error>) -> Option<PathBuf> {
    entry.ok().map(|entry| entry.path())
}

pub(crate) fn write_changed_providers_toml(
    providers_path: &Path,
    providers_root: &toml::Table,
) -> Result<(), String> {
    write_text_file_when_changed(
        providers_path,
        serialized_providers_toml(providers_path, providers_root)?,
    )
}

pub(crate) fn serialized_providers_toml(
    providers_path: &Path,
    providers_root: &toml::Table,
) -> Result<String, String> {
    serialize_toml_table(providers_path, providers_root)
}

pub(crate) fn sessions_path_for_providers(providers_path: &Path) -> Option<PathBuf> {
    providers_path
        .parent()
        .map(|config_root| config_root.join("sessions.toml"))
}

pub(crate) fn write_text_file_when_changed(path: &Path, next: String) -> Result<(), String> {
    let current = read_optional_text_file(path)?;
    write_text_file_if_changed(path, next, &current)
}

pub(crate) fn text_changed(next: &str, current: &str) -> bool {
    next != current
}

pub(crate) fn write_text_file_if_changed(
    path: &Path,
    next: String,
    current: &str,
) -> Result<(), String> {
    if text_changed(&next, current) {
        write_text_file_with_parent_dir(path, next)?;
    }
    Ok(())
}

pub(crate) fn write_text_file_with_parent_dir(path: &Path, text: String) -> Result<(), String> {
    ensure_parent_dir(path)?;
    write_text_file(path, text)
}

pub(crate) fn read_optional_text_file(path: &Path) -> Result<String, String> {
    if path.exists() {
        std::fs::read_to_string(path).map_err(|e| format_text_read_error(path, e))
    } else {
        Ok(String::new())
    }
}

pub(crate) fn ensure_parent_dir(path: &Path) -> Result<(), String> {
    create_parent_dir_if_present(parent_dir(path))
}

pub(crate) fn create_parent_dir_if_present(parent: Option<&Path>) -> Result<(), String> {
    if let Some(parent) = parent {
        create_parent_dir(parent)?;
    }
    Ok(())
}

pub(crate) fn parent_dir(path: &Path) -> Option<&Path> {
    path.parent()
}

pub(crate) fn create_parent_dir(parent: &Path) -> Result<(), String> {
    std::fs::create_dir_all(parent).map_err(|e| format_create_dir_error(parent, e))
}

pub(crate) fn write_text_file(path: &Path, text: String) -> Result<(), String> {
    std::fs::write(path, text).map_err(|e| format_text_write_error(path, e))
}

pub(crate) fn runtime_provider_table<'a>(
    providers_root: &'a mut toml::Table,
    provider_name: &str,
) -> Result<&'a mut toml::Table, String> {
    validate_runtime_provider_table(
        runtime_provider_value(providers_root, provider_name),
        provider_name,
    )
}

pub(crate) fn runtime_provider_value<'a>(
    providers_root: &'a mut toml::Table,
    provider_name: &str,
) -> &'a mut toml::Value {
    providers_root
        .entry(provider_name.to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
}

pub(crate) fn validate_runtime_provider_table<'a>(
    runtime: &'a mut toml::Value,
    provider_name: &str,
) -> Result<&'a mut toml::Table, String> {
    runtime
        .as_table_mut()
        .ok_or_else(|| format_runtime_provider_entry_not_table(provider_name))
}

pub(crate) fn existing_runtime_provider_table<'a>(
    providers_root: &'a mut toml::Table,
    provider_name: &str,
) -> Option<&'a mut toml::Table> {
    providers_root
        .get_mut(provider_name)
        .and_then(toml::Value::as_table_mut)
}
