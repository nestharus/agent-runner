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

pub(super) struct MigrateRebuildPlan {
    pub(super) db_path: PathBuf,
    pub(super) backup_dir: PathBuf,
    state_members: Vec<PathBuf>,
    sidecar_members: Vec<PathBuf>,
    recovery_marker: PathBuf,
    resume_from_backup: bool,
}

impl MigrateRebuildPlan {
    pub(super) fn bind_to_authority_path(&mut self, db_path: &Path) -> Result<(), String> {
        if self.db_path != db_path {
            return Err(format!(
                "process_integrity: State-plus-sidecar rebuild target changed between recovery planning ({}) and authority acquisition ({}); retry without retargeting the data path",
                self.db_path.display(),
                db_path.display(),
            ));
        }
        self.db_path = db_path.to_path_buf();
        self.state_members = db_sidecar_paths(db_path);
        self.sidecar_members = mailbox_sidecar_paths(db_path);
        self.recovery_marker = recovery_marker_path(db_path);
        Ok(())
    }

    fn all_members(&self) -> Vec<PathBuf> {
        self.state_members
            .iter()
            .chain(&self.sidecar_members)
            .cloned()
            .collect()
    }

    fn rebind_recovery_identity_under_complete_authority(&mut self) -> Result<(), String> {
        let backup_root = prepare_migrate_backup_root(&self.db_path)?;
        match std::fs::symlink_metadata(&self.recovery_marker) {
            Ok(_) => {
                self.backup_dir =
                    read_rebuild_recovery_marker(&self.recovery_marker, &backup_root)?;
                self.resume_from_backup = true;
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if self.resume_from_backup {
                    return Err(format!(
                        "process_integrity: State-plus-sidecar rebuild recovery marker {} disappeared before complete rebuild authority was acquired",
                        self.recovery_marker.display()
                    ));
                }
                if self.backup_dir.parent() != Some(backup_root.as_path())
                    || self.backup_dir.exists()
                {
                    return Err(format!(
                        "process_integrity: State-plus-sidecar rebuild backup identity {} is no longer available after complete rebuild authority was acquired",
                        self.backup_dir.display()
                    ));
                }
                Ok(())
            }
            Err(error) => Err(format!(
                "Failed to inspect State-plus-sidecar rebuild recovery marker {} after authority acquisition: {error}",
                self.recovery_marker.display()
            )),
        }
    }
}

pub(super) fn migrate_rebuild_plan() -> Result<Option<MigrateRebuildPlan>, String> {
    let requested_db_path = default_state_db_path()?;
    super::accessor::validate_rebuild_path(&requested_db_path)?;
    let db_path = canonical_rebuild_target(&requested_db_path)?;
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
    oulipoly_state::rebuild_recovery::marker_path(db_path)
}

fn canonical_rebuild_target(db_path: &Path) -> Result<PathBuf, String> {
    if db_path.exists() {
        return db_path.canonicalize().map_err(|error| {
            format!(
                "Failed to resolve State DB rebuild target {}: {error}",
                db_path.display()
            )
        });
    }
    let unresolved_parent = db_path.parent().unwrap_or_else(|| Path::new("."));
    let parent = match unresolved_parent.canonicalize() {
        Ok(parent) => parent,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if unresolved_parent.is_absolute() {
                unresolved_parent.to_path_buf()
            } else {
                std::env::current_dir()
                    .map_err(|current_error| {
                        format!(
                            "Failed to resolve State DB rebuild parent {}: {current_error}",
                            db_path.display()
                        )
                    })?
                    .join(unresolved_parent)
            }
        }
        Err(error) => {
            return Err(format!(
                "Failed to resolve State DB rebuild parent {}: {error}",
                db_path.display()
            ));
        }
    };
    let file_name = db_path.file_name().ok_or_else(|| {
        format!(
            "State DB rebuild target has no file name: {}",
            db_path.display()
        )
    })?;
    Ok(parent.join(file_name))
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
    plan: &mut MigrateRebuildPlan,
    sidecar_authority: &mut MailboxDbRebuildAuthority,
) -> Result<(), String> {
    if sidecar_authority.sqlite_member_paths() != plan.sidecar_members {
        return Err("PID mailbox rebuild authority does not match the rebuild plan".to_string());
    }
    plan.rebind_recovery_identity_under_complete_authority()?;
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
    if let Err(error) = file
        .write_all(backup_dir.as_bytes())
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.sync_all())
    {
        drop(file);
        let _ = std::fs::remove_file(&temporary);
        return Err(format!(
            "Failed to persist rebuild recovery marker: {error}"
        ));
    }
    drop(file);
    if let Err(error) = std::fs::hard_link(&temporary, marker) {
        let cleanup = match std::fs::remove_file(&temporary) {
            Ok(()) => String::new(),
            Err(cleanup_error) => format!("; staging cleanup also failed: {cleanup_error}"),
        };
        return Err(format!(
            "Failed to publish rebuild recovery marker without replacing existing authority: {error}{cleanup}"
        ));
    }
    std::fs::remove_file(&temporary).map_err(|error| {
        format!("Failed to clear published rebuild marker staging file: {error}")
    })?;
    let marker_parent = marker
        .parent()
        .ok_or_else(|| "State-plus-sidecar rebuild marker has no parent directory".to_string())?;
    std::fs::File::open(marker_parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("Failed to sync rebuild recovery marker directory: {error}"))
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
    use oulipoly_state::{StateDb, pid_identity::PidIdentityDb};

    #[test]
    fn binding_to_a_different_authority_path_fails_without_rebinding_sources() {
        let mut plan = migrate_rebuild_plan_value(
            PathBuf::from("stale/state.db"),
            PathBuf::from("backups/run"),
            false,
        );
        let authority_path = Path::new("locked/state.db");

        let error = plan.bind_to_authority_path(authority_path).unwrap_err();

        assert!(error.contains("target changed"), "{error}");
        assert_eq!(plan.db_path, Path::new("stale/state.db"));
        assert_eq!(
            plan.state_members,
            db_sidecar_paths(Path::new("stale/state.db"))
        );
        assert_eq!(
            plan.sidecar_members,
            mailbox_sidecar_paths(Path::new("stale/state.db"))
        );
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
        let marker = directory
            .path()
            .join(oulipoly_state::rebuild_recovery::STATE_SIDECAR_REBUILD_RECOVERY_MARKER);
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

    #[test]
    fn recovery_marker_publication_never_replaces_existing_authority() {
        let directory = tempfile::tempdir().unwrap();
        let backup_root = directory.path().join("state-backups");
        let incumbent_backup = backup_root.join("incumbent");
        let competing_backup = backup_root.join("competing");
        let marker = directory
            .path()
            .join(oulipoly_state::rebuild_recovery::STATE_SIDECAR_REBUILD_RECOVERY_MARKER);
        std::fs::create_dir_all(&incumbent_backup).unwrap();
        std::fs::create_dir(&competing_backup).unwrap();
        write_rebuild_recovery_marker(&marker, &incumbent_backup).unwrap();

        let error = write_rebuild_recovery_marker(&marker, &competing_backup).unwrap_err();

        assert!(
            error.contains("without replacing existing authority"),
            "{error}"
        );
        assert_eq!(
            read_rebuild_recovery_marker(&marker, &backup_root).unwrap(),
            incumbent_backup
        );
        assert!(!marker.with_extension("tmp").exists());
    }

    #[test]
    fn complete_authority_rebinds_a_fresh_plan_to_a_new_recovery_marker() {
        let directory = tempfile::tempdir().unwrap();
        let db_path = directory.path().join("state.db");
        let sidecar_path = MailboxDb::path_for_state_db(&db_path);
        let backup_root = directory.path().join("state-backups");
        let planned_backup = backup_root.join("planned");
        let incumbent_backup = backup_root.join("incumbent");
        std::fs::create_dir(&backup_root).unwrap();
        std::fs::create_dir(&incumbent_backup).unwrap();
        drop(StateDb::open(&db_path).unwrap());
        drop(MailboxDb::open(&sidecar_path).unwrap());
        let mut plan = migrate_rebuild_plan_value(db_path.clone(), planned_backup.clone(), false);
        let state_authority = StateDb::acquire_rebuild_authority(&db_path).unwrap();
        let mut sidecar_authority = MailboxDb::acquire_rebuild_authority(&state_authority).unwrap();
        write_rebuild_recovery_marker(&plan.recovery_marker, &incumbent_backup).unwrap();

        execute_migrate_rebuild(&mut plan, &mut sidecar_authority).unwrap();

        assert!(plan.resume_from_backup);
        assert_eq!(plan.backup_dir, incumbent_backup);
        assert!(!planned_backup.exists());
        assert_eq!(
            read_rebuild_recovery_marker(&plan.recovery_marker, &backup_root).unwrap(),
            plan.backup_dir
        );
    }

    #[test]
    fn waiting_second_owner_reuses_the_first_owners_published_recovery_identity() {
        let directory = tempfile::tempdir().unwrap();
        let db_path = directory.path().join("state.db");
        let sidecar_path = MailboxDb::path_for_state_db(&db_path);
        let backup_root = directory.path().join("state-backups");
        let backup_a = backup_root.join("owner-a");
        let backup_b = backup_root.join("owner-b");
        std::fs::create_dir(&backup_root).unwrap();
        drop(StateDb::open(&db_path).unwrap());
        drop(MailboxDb::open(&sidecar_path).unwrap());
        let mut plan_a = migrate_rebuild_plan_value(db_path.clone(), backup_a.clone(), false);
        let plan_b = migrate_rebuild_plan_value(db_path.clone(), backup_b.clone(), false);
        let state_authority_a = StateDb::acquire_rebuild_authority(&db_path).unwrap();
        let mut sidecar_authority_a =
            MailboxDb::acquire_rebuild_authority(&state_authority_a).unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (result_tx, result_rx) = std::sync::mpsc::channel();

        let owner_b = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let state_authority = StateDb::acquire_rebuild_authority(&db_path).unwrap();
            let mut sidecar_authority =
                MailboxDb::acquire_rebuild_authority(&state_authority).unwrap();
            let mut plan = plan_b;
            let result = execute_migrate_rebuild(&mut plan, &mut sidecar_authority);
            result_tx.send((result, plan)).unwrap();
        });
        started_rx.recv().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(matches!(
            result_rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));

        execute_migrate_rebuild(&mut plan_a, &mut sidecar_authority_a).unwrap();
        drop(sidecar_authority_a);
        drop(state_authority_a);
        let (result_b, rebound_plan_b) = result_rx.recv().unwrap();
        result_b.unwrap();
        owner_b.join().unwrap();

        assert!(rebound_plan_b.resume_from_backup);
        assert_eq!(rebound_plan_b.backup_dir, backup_a);
        assert!(!backup_b.exists());
        assert_eq!(
            read_rebuild_recovery_marker(&rebound_plan_b.recovery_marker, &backup_root).unwrap(),
            backup_a
        );
        assert!(backup_a.join("state.db").is_file());
        assert!(backup_a.join("pid-identity.db").is_file());
    }

    #[test]
    fn post_reset_interruption_retries_from_the_published_backup() {
        let directory = tempfile::tempdir().unwrap();
        let db_path = directory.path().join("state.db");
        let sidecar_path = MailboxDb::path_for_state_db(&db_path);
        let backup_root = directory.path().join("state-backups");
        let backup_dir = backup_root.join("rebuild-attempt");
        std::fs::create_dir(&backup_root).unwrap();
        drop(StateDb::open(&db_path).unwrap());
        drop(MailboxDb::open(&sidecar_path).unwrap());
        let mut plan = migrate_rebuild_plan_value(db_path.clone(), backup_dir.clone(), false);

        {
            let state_authority = StateDb::acquire_rebuild_authority(&db_path).unwrap();
            let mut sidecar_authority =
                MailboxDb::acquire_rebuild_authority(&state_authority).unwrap();
            execute_migrate_rebuild(&mut plan, &mut sidecar_authority).unwrap();
        }

        assert!(plan.recovery_marker.is_file());
        assert!(backup_dir.join("state.db").is_file());
        assert!(backup_dir.join("pid-identity.db").is_file());
        assert!(!db_path.exists());
        assert!(!sidecar_path.exists());

        let mut retry_plan = migrate_rebuild_plan_value(db_path.clone(), backup_dir, true);
        let state_authority = StateDb::acquire_rebuild_authority(&db_path).unwrap();
        let mut sidecar_authority = MailboxDb::acquire_rebuild_authority(&state_authority).unwrap();
        execute_migrate_rebuild(&mut retry_plan, &mut sidecar_authority).unwrap();
        StateDb::initialize_after_rebuild(&db_path, &state_authority).unwrap();
        sidecar_authority.initialize_after_rebuild().unwrap();
        complete_migrate_rebuild(&retry_plan).unwrap();
        drop(sidecar_authority);
        drop(state_authority);

        assert!(!retry_plan.recovery_marker.exists());
        drop(StateDb::open(&db_path).unwrap());
        drop(MailboxDb::open(&sidecar_path).unwrap());
    }

    #[test]
    fn post_initialization_interruption_blocks_writers_until_recovery_completes() {
        let directory = tempfile::tempdir().unwrap();
        let db_path = directory.path().join("state.db");
        let sidecar_path = MailboxDb::path_for_state_db(&db_path);
        let backup_dir = directory.path().join("state-backups/rebuild-attempt");
        std::fs::create_dir(directory.path().join("state-backups")).unwrap();
        drop(StateDb::open(&db_path).unwrap());
        drop(MailboxDb::open(&sidecar_path).unwrap());
        let mut plan = migrate_rebuild_plan_value(db_path.clone(), backup_dir.clone(), false);

        {
            let state_authority = StateDb::acquire_rebuild_authority(&db_path).unwrap();
            let mut sidecar_authority =
                MailboxDb::acquire_rebuild_authority(&state_authority).unwrap();
            execute_migrate_rebuild(&mut plan, &mut sidecar_authority).unwrap();
            StateDb::initialize_after_rebuild(&db_path, &state_authority).unwrap();
            sidecar_authority.initialize_after_rebuild().unwrap();
        }

        for error in [
            StateDb::open(&db_path).err().unwrap(),
            MailboxDb::open(&sidecar_path).err().unwrap(),
            PidIdentityDb::open(&sidecar_path).err().unwrap(),
        ] {
            assert!(
                error.contains("state_sidecar_rebuild_recovery_in_progress"),
                "{error}"
            );
        }

        let mut retry_plan = migrate_rebuild_plan_value(db_path.clone(), backup_dir, true);
        let state_authority = StateDb::acquire_rebuild_authority(&db_path).unwrap();
        let mut sidecar_authority = MailboxDb::acquire_rebuild_authority(&state_authority).unwrap();
        execute_migrate_rebuild(&mut retry_plan, &mut sidecar_authority).unwrap();
        StateDb::initialize_after_rebuild(&db_path, &state_authority).unwrap();
        sidecar_authority.initialize_after_rebuild().unwrap();
        complete_migrate_rebuild(&retry_plan).unwrap();
        drop(sidecar_authority);
        drop(state_authority);

        drop(StateDb::open(&db_path).unwrap());
        drop(MailboxDb::open(&sidecar_path).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn recovery_plan_for_one_target_rejects_retargeted_rebuild_authority() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let target_a = directory.path().join("target-a");
        let target_b = directory.path().join("target-b");
        let alias = directory.path().join("current");
        std::fs::create_dir(&target_a).unwrap();
        std::fs::create_dir(&target_b).unwrap();
        let state_a = target_a.join("state.db");
        let state_b = target_b.join("state.db");
        drop(StateDb::open(&state_a).unwrap());
        let state_b_db = StateDb::open(&state_b).unwrap();
        state_b_db
            .start_invocation(&oulipoly_state::InvocationStart {
                invocation_uuid: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb".to_string(),
                model_name: "retarget-neighbor".to_string(),
                provider_name: "fixture".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        drop(state_b_db);
        drop(MailboxDb::open(&MailboxDb::path_for_state_db(&state_b)).unwrap());
        let state_b_before = std::fs::read(&state_b).unwrap();
        let sidecar_b = MailboxDb::path_for_state_db(&state_b);
        let sidecar_b_before = std::fs::read(&sidecar_b).unwrap();
        let backup_a = target_a.join("state-backups/interrupted-a");
        std::fs::create_dir_all(&backup_a).unwrap();
        std::fs::write(backup_a.join("state.db"), b"backup-a").unwrap();
        write_rebuild_recovery_marker(&recovery_marker_path(&state_a), &backup_a).unwrap();
        symlink(&target_a, &alias).unwrap();
        let planned_target = canonical_rebuild_target(&alias.join("state.db")).unwrap();
        let mut plan = migrate_rebuild_plan_value(planned_target, backup_a, true);

        std::fs::remove_file(&alias).unwrap();
        symlink(&target_b, &alias).unwrap();
        let authority = StateDb::acquire_rebuild_authority(&alias.join("state.db")).unwrap();
        let error = plan.bind_to_authority_path(authority.path()).unwrap_err();
        drop(authority);

        assert!(error.contains("target changed"), "{error}");
        assert_eq!(std::fs::read(&state_b).unwrap(), state_b_before);
        assert_eq!(std::fs::read(&sidecar_b).unwrap(), sidecar_b_before);
        let rows: i64 = rusqlite::Connection::open(&state_b)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM invocations", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 1);
    }
}
