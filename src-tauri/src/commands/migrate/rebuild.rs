//! Declared roles: mapper, orchestration, accessor, predicate, formatter

use super::accessor::{default_state_db_path, state_db_parent_dir};
use super::backup::{
    backup_rebuild_sidecars, create_backup_dir, create_backup_root_dir, remove_live_sidecars,
    unique_backup_dir,
};
use super::formatter::{format_db_sidecar_path, render_missing_state_db_rebuild_message};
use super::predicate::missing_state_db;
use std::path::{Path, PathBuf};

pub(super) struct MigrateRebuildPlan {
    pub(super) db_path: PathBuf,
    pub(super) backup_dir: PathBuf,
    sidecars: Vec<PathBuf>,
}

impl MigrateRebuildPlan {
    pub(super) fn bind_to_authority_path(&mut self, db_path: &Path) {
        self.db_path = db_path.to_path_buf();
        self.sidecars = db_sidecar_paths(db_path);
    }
}

pub(super) fn migrate_rebuild_plan() -> Result<Option<MigrateRebuildPlan>, String> {
    let db_path = default_state_db_path()?;
    super::accessor::validate_rebuild_path(&db_path)?;
    if missing_state_db(&db_path)? {
        reject_orphaned_rebuild_artifacts(&db_path)?;
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
    Ok(migrate_rebuild_plan_value(
        db_path,
        unique_backup_dir(backup_root)?,
    ))
}

fn migrate_rebuild_plan_value(db_path: PathBuf, backup_dir: PathBuf) -> MigrateRebuildPlan {
    MigrateRebuildPlan {
        sidecars: db_sidecar_paths(&db_path),
        db_path,
        backup_dir,
    }
}

fn db_sidecar_paths(db_path: &Path) -> Vec<PathBuf> {
    vec![
        db_path.to_path_buf(),
        format_db_sidecar_path(db_path, "-journal"),
        format_db_sidecar_path(db_path, "-wal"),
        format_db_sidecar_path(db_path, "-shm"),
    ]
}

fn reject_orphaned_rebuild_artifacts(db_path: &Path) -> Result<(), String> {
    for artifact in db_sidecar_paths(db_path).into_iter().skip(1) {
        match std::fs::symlink_metadata(&artifact) {
            Ok(_) => {
                return Err(format!(
                    "State DB rebuild source is missing but a recovery artifact remains: {}",
                    artifact.display()
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "Failed to inspect State DB rebuild recovery artifact {}: {error}",
                    artifact.display()
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn execute_migrate_rebuild(plan: &MigrateRebuildPlan) -> Result<(), String> {
    create_backup_dir(&plan.backup_dir)?;
    backup_rebuild_sidecars(&plan.sidecars, &plan.backup_dir)?;
    remove_live_sidecars(&plan.sidecars)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_to_authority_path_replaces_every_destructive_source() {
        let mut plan = migrate_rebuild_plan_value(
            PathBuf::from("stale/state.db"),
            PathBuf::from("backups/run"),
        );
        let authority_path = Path::new("locked/state.db");

        plan.bind_to_authority_path(authority_path);

        assert_eq!(plan.db_path, authority_path);
        assert_eq!(plan.sidecars, db_sidecar_paths(authority_path));
    }

    #[test]
    fn missing_main_rejects_every_surviving_recovery_artifact() {
        let directory = tempfile::tempdir().unwrap();
        let db_path = directory.path().join("state.db");

        for suffix in ["-journal", "-wal", "-shm"] {
            let artifact = format_db_sidecar_path(&db_path, suffix);
            std::fs::write(&artifact, b"recovery").unwrap();

            let error = reject_orphaned_rebuild_artifacts(&db_path).unwrap_err();

            assert!(error.contains("recovery artifact remains"), "{error}");
            assert!(error.contains(suffix), "{error}");
            std::fs::remove_file(artifact).unwrap();
        }
        reject_orphaned_rebuild_artifacts(&db_path).unwrap();
    }
}
