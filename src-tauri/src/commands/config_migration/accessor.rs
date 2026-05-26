use super::filter::sort_paths;
use super::formatter::{format_model_dir_read_error, format_toml_read_error};
use super::parser::{parse_toml_table, serialize_toml_table};
use super::predicate::{is_toml_path, path_exists};
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

pub(crate) fn read_toml_text(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format_toml_read_error(path, e))
}

pub(crate) fn model_toml_paths(models_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut paths = if path_exists(models_dir) {
        toml_paths_from_dir_entries(read_model_dir_entries(models_dir)?)
    } else {
        Vec::new()
    };
    sort_paths(&mut paths);
    Ok(paths)
}

pub(crate) fn read_model_dir_entries(models_dir: &Path) -> Result<fs::ReadDir, String> {
    std::fs::read_dir(models_dir).map_err(|e| format_model_dir_read_error(models_dir, e))
}

pub(crate) fn toml_paths_from_dir_entries(entries: fs::ReadDir) -> Vec<PathBuf> {
    entries
        .filter_map(read_dir_entry_path)
        .filter(|path| is_toml_path(path))
        .collect()
}

pub(crate) fn read_dir_entry_path(entry: Result<fs::DirEntry, std::io::Error>) -> Option<PathBuf> {
    entry.ok().map(|entry| entry.path())
}

pub(crate) fn write_changed_providers_toml(
    providers_path: &Path,
    providers_root: &toml::Table,
) -> Result<(), String> {
    let providers_text = serialize_toml_table(providers_path, providers_root)?;
    let current = read_optional_text_file(providers_path)?;
    if providers_text != current {
        ensure_parent_dir(providers_path)?;
        write_text_file(providers_path, providers_text)?;
    }
    Ok(())
}

pub(crate) fn read_optional_text_file(path: &Path) -> Result<String, String> {
    if path.exists() {
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read {}: {e}", path.display()))
    } else {
        Ok(String::new())
    }
}

pub(crate) fn ensure_parent_dir(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
    }
    Ok(())
}

pub(crate) fn write_text_file(path: &Path, text: String) -> Result<(), String> {
    std::fs::write(path, text).map_err(|e| format!("Failed to write {}: {e}", path.display()))
}

pub(crate) fn runtime_provider_table<'a>(
    providers_root: &'a mut toml::Table,
    provider_name: &str,
) -> Result<&'a mut toml::Table, String> {
    let runtime = providers_root
        .entry(provider_name.to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    runtime
        .as_table_mut()
        .ok_or_else(|| format!("providers.toml entry [{provider_name}] is not a table"))
}
