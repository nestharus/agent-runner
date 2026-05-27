use super::orchestration::ConfigMigrationReport;
use std::path::Path;

pub(crate) fn render_config_migration_report(report: &ConfigMigrationReport) {
    println!(
        "migrate-config: providers_touched={} model_files_rewritten={}",
        report.providers_touched, report.model_files_rewritten
    );
    for moved in &report.moved_blocks {
        println!("  moved {moved}");
    }
}

pub(crate) fn format_toml_read_error(path: &Path, error: std::io::Error) -> String {
    format!("Failed to read {}: {error}", path.display())
}

pub(crate) fn format_toml_parse_error(path: &Path, error: toml::de::Error) -> String {
    format!("TOML parse error in {}: {error}", path.display())
}

pub(crate) fn format_model_dir_read_error(models_dir: &Path, error: std::io::Error) -> String {
    format!("Failed to read {}: {error}", models_dir.display())
}

pub(crate) fn format_text_read_error(path: &Path, error: std::io::Error) -> String {
    format!("Failed to read {}: {error}", path.display())
}

pub(crate) fn format_create_dir_error(path: &Path, error: std::io::Error) -> String {
    format!("Failed to create {}: {error}", path.display())
}

pub(crate) fn format_text_write_error(path: &Path, error: std::io::Error) -> String {
    format!("Failed to write {}: {error}", path.display())
}

pub(crate) fn format_runtime_provider_entry_not_table(provider_name: &str) -> String {
    format!("providers.toml entry [{provider_name}] is not a table")
}

pub(crate) fn format_moved_runtime_block(path: &Path, key: &str, provider_name: &str) -> String {
    format!(
        "{}.{} -> providers.toml[{provider_name}]",
        path.display(),
        key
    )
}

pub(crate) fn format_moved_session_storage_block(
    sessions_path: &Path,
    provider_name: &str,
) -> String {
    format!(
        "{}[{provider_name}].turn_script -> providers.toml[{provider_name}].session_storage",
        sessions_path.display()
    )
}

pub(crate) fn shell_word_arg(input: &str) -> String {
    if input
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | '~'))
    {
        return input.to_string();
    }
    format!("'{}'", input.replace('\'', r#"'\''"#))
}

pub(crate) fn default_prompt_mode() -> String {
    "stdin".to_string()
}

pub(crate) fn provider_name_key() -> String {
    "name".to_string()
}

pub(crate) fn provider_args_key() -> String {
    "args".to_string()
}

pub(crate) fn provider_interactive_args_key() -> String {
    "interactive_args".to_string()
}

pub(crate) fn providers_key() -> String {
    "providers".to_string()
}

pub(crate) fn session_storage_key() -> String {
    "session_storage".to_string()
}

pub(crate) fn format_storage_command(adapter: &str, storage_root: &str) -> String {
    format!("{adapter} {storage_root}")
}

pub(crate) fn storage_kind_key() -> String {
    "kind".to_string()
}

pub(crate) fn storage_kind_value() -> String {
    "script".to_string()
}

pub(crate) fn storage_cwd_script_key() -> String {
    "cwd_script".to_string()
}

pub(crate) fn storage_transcript_script_key() -> String {
    "transcript_script".to_string()
}

pub(crate) fn storage_type_key() -> String {
    "storage_type".to_string()
}

pub(crate) fn storage_type_value(storage_type: &str) -> String {
    storage_type.to_string()
}

pub(crate) fn format_toml_conflict_error(
    existing: &toml::Value,
    value: &toml::Value,
    key: &str,
    provider_name: &str,
    path: &Path,
) -> String {
    format!(
        "conflicting {key} for provider {provider_name} while migrating {}: existing providers.toml value {existing:?}, model TOML value {value:?}",
        path.display()
    )
}

pub(crate) fn format_toml_serialize_error(path: &Path, error: toml::ser::Error) -> String {
    format!("Failed to serialize {}: {error}", path.display())
}

pub(crate) fn format_missing_old_provider_command_error() -> String {
    "old model provider is missing command".to_string()
}

pub(crate) fn format_migrated_provider_name_missing_error(path: &Path) -> String {
    format!(
        "Old per-provider config in {} is missing command; run `agents migrate-config` after adding it.",
        path.display()
    )
}

pub(crate) fn format_provider_entry_not_table_error(path: &Path) -> String {
    format!("provider entry in {} is not a table", path.display())
}

pub(crate) fn format_command_must_be_string_error(path: &Path) -> String {
    format!(
        "command in old per-provider config in {} must be a string",
        path.display()
    )
}

pub(crate) fn format_toml_string_array_error(key: &str) -> String {
    format!("{key} must be an array of strings")
}
