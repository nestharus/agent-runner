use super::formatter::render_config_migration_report;
use super::orchestration::migrate_config_files;
use crate::default_models_dir;
use std::path::{Path, PathBuf};

pub(crate) fn run_migrate_config(models_dir_override: Option<&Path>) -> Result<i32, String> {
    let (models_dir, providers_path) = migrate_config_paths(models_dir_override);
    let report = migrate_config_files(&models_dir, &providers_path)?;
    render_config_migration_report(&report);
    Ok(0)
}

pub(crate) fn migrate_config_paths(models_dir_override: Option<&Path>) -> (PathBuf, PathBuf) {
    let models_dir = models_dir_override
        .map(Path::to_path_buf)
        .unwrap_or_else(default_models_dir);
    let config_root = models_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let providers_path = config_root.join("providers.toml");
    (models_dir, providers_path)
}
