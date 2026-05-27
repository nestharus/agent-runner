//! Declared role: orchestration

use super::formatter::render_config_migration_report;
use super::mapper::migrate_config_paths;
use super::orchestration::migrate_config_files;
use std::path::Path;

pub(crate) fn run_migrate_config(models_dir_override: Option<&Path>) -> Result<i32, String> {
    let (models_dir, providers_path) = migrate_config_paths(models_dir_override);
    let report = migrate_config_files(&models_dir, &providers_path)?;
    render_config_migration_report(&report);
    Ok(0)
}
