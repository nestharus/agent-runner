//! Declared role: mapper

use super::accessor::{default_state_db_path, state_db_parent_dir};
use super::backup::{
    backup_rebuild_sidecars, create_backup_dir, create_backup_root_dir, remove_live_sidecars,
    unique_backup_dir,
};
use super::formatter::render_missing_state_db_rebuild_message;
use super::predicate::missing_state_db;
use std::path::{Path, PathBuf};

pub(super) struct MigrateRebuildPlan {
    pub(super) db_path: PathBuf,
    pub(super) backup_dir: PathBuf,
    sidecars: Vec<PathBuf>,
}

pub(super) fn migrate_rebuild_plan() -> Result<Option<MigrateRebuildPlan>, String> {
    let db_path = default_state_db_path()?;
    if missing_state_db(&db_path) {
        render_missing_state_db_rebuild_message(&db_path);
        return Ok(None);
    }
    let backup_root = prepare_migrate_backup_root(&db_path)?;
    Ok(Some(migrate_rebuild_plan_from_paths(
        db_path,
        &backup_root,
    )?))
}

fn prepare_migrate_backup_root(db_path: &Path) -> Result<PathBuf, String> {
    let data_dir = state_db_parent_dir(db_path)?;
    let backup_root = data_dir.join("state-backups");
    create_backup_root_dir(&backup_root)?;
    Ok(backup_root)
}

fn migrate_rebuild_plan_from_paths(
    db_path: PathBuf,
    backup_root: &Path,
) -> Result<MigrateRebuildPlan, String> {
    Ok(MigrateRebuildPlan {
        backup_dir: unique_backup_dir(backup_root)?,
        sidecars: db_sidecar_paths(&db_path),
        db_path,
    })
}

fn db_sidecar_paths(db_path: &Path) -> Vec<PathBuf> {
    vec![
        db_path.to_path_buf(),
        PathBuf::from(format!("{}-wal", db_path.display())),
        PathBuf::from(format!("{}-shm", db_path.display())),
    ]
}

pub(super) fn execute_migrate_rebuild(plan: &MigrateRebuildPlan) -> Result<(), String> {
    create_backup_dir(&plan.backup_dir)?;
    backup_rebuild_sidecars(&plan.sidecars, &plan.backup_dir)?;
    remove_live_sidecars(&plan.sidecars)
}
