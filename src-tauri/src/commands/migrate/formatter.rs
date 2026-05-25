//! Declared role: formatter

use super::rebuild::MigrateRebuildPlan;
use oulipoly_state::BackfillReport;
use std::path::Path;

pub(super) fn render_session_chain_backfill_report(report: &BackfillReport) {
    println!(
        "session chain backfill: chains={} segments={} skipped_existing={}",
        report.chains_inserted, report.segments_inserted, report.skipped_existing
    );
}

pub(super) fn render_missing_state_db_rebuild_message(db_path: &Path) {
    println!("no state.db to rebuild at {}", db_path.display());
}

pub(super) fn format_backup_root_create_error(error: std::io::Error) -> String {
    format!("failed to create backup directory: {error}")
}

pub(super) fn format_backup_source_missing_file_name_error(source: &Path) -> String {
    format!("backup source has no file name: {}", source.display())
}

pub(super) fn format_rebuild_sidecar_copy_error(
    source: &Path,
    destination: &Path,
    error: std::io::Error,
) -> String {
    let backup_dir = destination.parent().unwrap_or(destination);
    format!(
        "failed to back up {} to {}: {error}",
        source.display(),
        backup_dir.display()
    )
}

pub(super) fn render_migrate_rebuild_report(plan: &MigrateRebuildPlan) {
    println!("backup: {}", plan.backup_dir.display());
    println!("fresh state DB: {}", plan.db_path.display());
    println!(
        "historical state was not preserved in the live DB; backup is at {}",
        plan.backup_dir.display()
    );
}

pub(super) fn format_backup_dir_base_name(stamp: &str, pid: u32) -> String {
    format!("{stamp}-pid{pid}")
}

pub(super) fn format_backup_dir_exhausted_error(root: &Path) -> String {
    format!(
        "failed to allocate unique backup directory under {}",
        root.display()
    )
}
