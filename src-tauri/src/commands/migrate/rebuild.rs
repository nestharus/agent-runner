//! Declared roles: mapper, orchestration, accessor, predicate, formatter

use super::accessor::{default_state_db_path, state_db_parent_dir};
use super::backup::{
    backup_rebuild_sidecars, create_backup_dir, create_backup_root_dir, remove_live_sidecars,
    unique_backup_dir,
};
use super::formatter::{format_db_sidecar_path, render_missing_state_db_rebuild_message};
use super::predicate::missing_state_db;
use oulipoly_state::mailbox::{MailboxDb, MailboxDbRebuildAuthority};
use std::io::Write;
use std::path::{Path, PathBuf};

const REBUILD_RECOVERY_MARKER: &str = ".state-sidecar-rebuild-in-progress";

pub(super) struct MigrateRebuildPlan {
    pub(super) db_path: PathBuf,
    pub(super) backup_dir: PathBuf,
    state_members: Vec<PathBuf>,
    sidecar_members: Vec<PathBuf>,
    recovery_marker: PathBuf,
    resume_from_backup: bool,
}

impl MigrateRebuildPlan {
    pub(super) fn bind_to_authority_path(&mut self, db_path: &Path) {
        self.db_path = db_path.to_path_buf();
        self.state_members = db_sidecar_paths(db_path);
        self.sidecar_members = mailbox_sidecar_paths(db_path);
        self.recovery_marker = recovery_marker_path(db_path);
    }

    fn all_members(&self) -> Vec<PathBuf> {
        self.state_members
            .iter()
            .chain(&self.sidecar_members)
            .cloned()
            .collect()
    }
}

pub(super) fn migrate_rebuild_plan() -> Result<Option<MigrateRebuildPlan>, String> {
    let db_path = default_state_db_path()?;
    super::accessor::validate_rebuild_path(&db_path)?;
    let recovery_marker = recovery_marker_path(&db_path);
    if recovery_marker.exists() {
        let backup_root = prepare_migrate_backup_root(&db_path)?;
        let backup_dir = read_rebuild_recovery_marker(&recovery_marker, &backup_root)?;
        return Ok(Some(migrate_rebuild_plan_value(db_path, backup_dir, true)));
    }
    if missing_state_db(&db_path)? {
        reject_orphaned_rebuild_artifacts(&db_path)?;
        if any_rebuild_member_exists(&mailbox_sidecar_paths(&db_path))? {
            let backup_root = prepare_migrate_backup_root(&db_path)?;
            return Ok(Some(migrate_rebuild_plan_from_paths(
                db_path,
                &backup_root,
            )?));
        }
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
        false,
    ))
}

fn migrate_rebuild_plan_value(
    db_path: PathBuf,
    backup_dir: PathBuf,
    resume_from_backup: bool,
) -> MigrateRebuildPlan {
    MigrateRebuildPlan {
        state_members: db_sidecar_paths(&db_path),
        sidecar_members: mailbox_sidecar_paths(&db_path),
        recovery_marker: recovery_marker_path(&db_path),
        db_path,
        backup_dir,
        resume_from_backup,
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

fn mailbox_sidecar_paths(db_path: &Path) -> Vec<PathBuf> {
    let sidecar_path = MailboxDb::path_for_state_db(db_path);
    vec![
        sidecar_path.clone(),
        format_db_sidecar_path(&sidecar_path, "-journal"),
        format_db_sidecar_path(&sidecar_path, "-wal"),
        format_db_sidecar_path(&sidecar_path, "-shm"),
    ]
}

fn recovery_marker_path(db_path: &Path) -> PathBuf {
    db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(REBUILD_RECOVERY_MARKER)
}

fn any_rebuild_member_exists(paths: &[PathBuf]) -> Result<bool, String> {
    for path in paths {
        match std::fs::symlink_metadata(path) {
            Ok(_) => return Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "Failed to inspect rebuild member {}: {error}",
                    path.display()
                ));
            }
        }
    }
    Ok(false)
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

pub(super) fn execute_migrate_rebuild(
    plan: &MigrateRebuildPlan,
    sidecar_authority: &mut MailboxDbRebuildAuthority,
) -> Result<(), String> {
    if sidecar_authority.sqlite_member_paths() != plan.sidecar_members {
        return Err("PID mailbox rebuild authority does not match the rebuild plan".to_string());
    }
    if !plan.resume_from_backup {
        create_backup_dir(&plan.backup_dir)?;
        backup_rebuild_sidecars(&plan.all_members(), &plan.backup_dir)?;
        write_rebuild_recovery_marker(&plan.recovery_marker, &plan.backup_dir)?;
    }
    sidecar_authority.reset()?;
    remove_live_sidecars(&plan.state_members)
}

pub(super) fn complete_migrate_rebuild(plan: &MigrateRebuildPlan) -> Result<(), String> {
    std::fs::remove_file(&plan.recovery_marker).map_err(|error| {
        format!(
            "Failed to clear State-plus-sidecar rebuild recovery marker {}: {error}",
            plan.recovery_marker.display()
        )
    })
}

fn write_rebuild_recovery_marker(marker: &Path, backup_dir: &Path) -> Result<(), String> {
    let backup_dir = backup_dir
        .to_str()
        .ok_or_else(|| "State-plus-sidecar rebuild backup path must be valid UTF-8".to_string())?;
    let temporary = marker.with_extension("tmp");
    match std::fs::remove_file(&temporary) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "Failed to clear stale rebuild recovery marker {}: {error}",
                temporary.display()
            ));
        }
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| format!("Failed to create rebuild recovery marker: {error}"))?;
    file.write_all(backup_dir.as_bytes())
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("Failed to persist rebuild recovery marker: {error}"))?;
    std::fs::rename(&temporary, marker)
        .map_err(|error| format!("Failed to publish rebuild recovery marker: {error}"))
}

fn read_rebuild_recovery_marker(marker: &Path, backup_root: &Path) -> Result<PathBuf, String> {
    let value = std::fs::read_to_string(marker)
        .map_err(|error| format!("Failed to read rebuild recovery marker: {error}"))?;
    let backup_dir = PathBuf::from(value.trim());
    if backup_dir.parent() != Some(backup_root) || !backup_dir.is_dir() {
        return Err(format!(
            "State-plus-sidecar rebuild recovery marker names an invalid backup: {}",
            backup_dir.display()
        ));
    }
    Ok(backup_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_to_authority_path_replaces_every_destructive_source() {
        let mut plan = migrate_rebuild_plan_value(
            PathBuf::from("stale/state.db"),
            PathBuf::from("backups/run"),
            false,
        );
        let authority_path = Path::new("locked/state.db");

        plan.bind_to_authority_path(authority_path);

        assert_eq!(plan.db_path, authority_path);
        assert_eq!(plan.state_members, db_sidecar_paths(authority_path));
        assert_eq!(plan.sidecar_members, mailbox_sidecar_paths(authority_path));
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

    #[test]
    fn published_recovery_marker_reuses_the_completed_backup_on_retry() {
        let directory = tempfile::tempdir().unwrap();
        let backup_root = directory.path().join("state-backups");
        let backup_dir = backup_root.join("rebuild-attempt");
        let marker = directory.path().join(REBUILD_RECOVERY_MARKER);
        std::fs::create_dir_all(&backup_dir).unwrap();
        std::fs::write(backup_dir.join("state.db"), b"backed-up-state").unwrap();
        std::fs::write(backup_dir.join("pid-identity.db"), b"backed-up-sidecar").unwrap();

        write_rebuild_recovery_marker(&marker, &backup_dir).unwrap();

        assert_eq!(
            read_rebuild_recovery_marker(&marker, &backup_root).unwrap(),
            backup_dir
        );
        assert_eq!(
            std::fs::read(backup_root.join("rebuild-attempt/state.db")).unwrap(),
            b"backed-up-state"
        );
        assert_eq!(
            std::fs::read(backup_root.join("rebuild-attempt/pid-identity.db")).unwrap(),
            b"backed-up-sidecar"
        );
    }
}
