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

pub(crate) fn format_moved_runtime_block(path: &Path, key: &str, provider_name: &str) -> String {
    format!(
        "{}.{} -> providers.toml[{provider_name}]",
        path.display(),
        key
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
