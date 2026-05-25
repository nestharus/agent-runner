//! Declared role: accessor

use super::formatter::{
    format_backup_dir_base_name, format_backup_dir_exhausted_error,
    format_backup_root_create_error, format_backup_source_missing_file_name_error,
    format_rebuild_sidecar_copy_error,
};
use super::predicate::unused_path;
use std::fs;
use std::path::{Path, PathBuf};

pub(super) fn unique_backup_dir(root: &Path) -> Result<PathBuf, String> {
    let base = backup_dir_base_name();
    first_available_backup_dir(backup_dir_candidates(root, &base))
        .ok_or_else(|| format_backup_dir_exhausted_error(root))
}

fn backup_dir_candidates(root: &Path, base: &str) -> Vec<PathBuf> {
    (0..1000)
        .map(|suffix| root.join(backup_dir_candidate_name(base, suffix)))
        .collect()
}

fn first_available_backup_dir(candidates: Vec<PathBuf>) -> Option<PathBuf> {
    candidates
        .into_iter()
        .find(|candidate| unused_path(candidate))
}

pub(super) fn create_backup_root_dir(backup_root: &Path) -> Result<(), String> {
    fs::create_dir_all(backup_root).map_err(format_backup_root_create_error)
}

pub(super) fn create_backup_dir(backup_dir: &Path) -> Result<(), String> {
    fs::create_dir(backup_dir).map_err(|e| {
        format!(
            "failed to create backup directory {}: {e}",
            backup_dir.display()
        )
    })
}

pub(super) fn remove_live_sidecars(sidecars: &[PathBuf]) -> Result<(), String> {
    for source in sidecars {
        if source.exists() {
            fs::remove_file(source)
                .map_err(|e| format!("failed to remove live {}: {e}", source.display()))?;
        }
    }
    Ok(())
}

pub(super) fn backup_rebuild_sidecars(
    sidecars: &[PathBuf],
    backup_dir: &Path,
) -> Result<(), String> {
    for source in sidecars {
        if source.exists() {
            backup_rebuild_sidecar(source, backup_dir)?;
        }
    }
    Ok(())
}

fn backup_rebuild_sidecar(source: &Path, backup_dir: &Path) -> Result<(), String> {
    let file_name = backup_source_file_name(source)?;
    copy_rebuild_sidecar(source, &backup_sidecar_destination(backup_dir, file_name))?;
    Ok(())
}

fn backup_source_file_name(source: &Path) -> Result<&std::ffi::OsStr, String> {
    source
        .file_name()
        .ok_or_else(|| format_backup_source_missing_file_name_error(source))
}

fn backup_sidecar_destination(backup_dir: &Path, file_name: &std::ffi::OsStr) -> PathBuf {
    backup_dir.join(file_name)
}

fn copy_rebuild_sidecar(source: &Path, destination: &Path) -> Result<(), String> {
    fs::copy(source, destination)
        .map(|_| ())
        .map_err(|e| format_rebuild_sidecar_copy_error(source, destination, e))
}

fn backup_dir_base_name() -> String {
    format_backup_dir_base_name(&backup_dir_timestamp(), backup_dir_process_id())
}

fn backup_dir_timestamp() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%S%.fZ").to_string()
}

fn backup_dir_process_id() -> u32 {
    std::process::id()
}

fn backup_dir_candidate_name(base: &str, suffix: usize) -> String {
    if suffix == 0 {
        base.to_string()
    } else {
        format!("{base}-{suffix}")
    }
}
