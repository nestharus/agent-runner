//! Declared role: validator

pub(super) fn validate_migrate_rebuild_flag(rebuild: bool) -> Result<(), String> {
    if rebuild {
        Ok(())
    } else {
        Err(super::formatter::format_missing_rebuild_flag_error())
    }
}

pub(super) fn validate_backup_dir_available(
    backup_dir: Option<std::path::PathBuf>,
    root: &std::path::Path,
) -> Result<std::path::PathBuf, String> {
    backup_dir.ok_or_else(|| super::formatter::format_backup_dir_exhausted_error(root))
}

pub(super) fn validate_backup_source_file_name(
    file_name: Option<&std::ffi::OsStr>,
    missing_message: String,
) -> Result<&std::ffi::OsStr, String> {
    file_name.ok_or(missing_message)
}
