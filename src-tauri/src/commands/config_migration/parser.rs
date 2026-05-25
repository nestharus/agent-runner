use super::formatter::format_toml_parse_error;
use oulipoly_runtime::executor;
use std::path::Path;

pub(crate) fn parse_toml_table(path: &Path, text: &str) -> Result<toml::Table, String> {
    text.parse::<toml::Table>()
        .map_err(|e| format_toml_parse_error(path, e))
}

pub(crate) fn serialize_toml_table(path: &Path, table: &toml::Table) -> Result<String, String> {
    toml::to_string_pretty(table)
        .map_err(|e| format!("Failed to serialize {}: {e}", path.display()))
}

pub(crate) fn split_optional_command(command: Option<&str>) -> Vec<String> {
    command.map(executor::cli::shell_split).unwrap_or_default()
}

pub(crate) fn split_migration_command(command: &str) -> Vec<String> {
    executor::cli::shell_split(command)
}

pub(crate) fn turn_script_storage_parts(turn_script: &str) -> Option<(String, String)> {
    let parts = executor::cli::shell_split(turn_script);
    Some((parts.first()?.clone(), parts.get(1)?.clone()))
}
