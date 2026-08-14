//! Declared role: orchestration

use super::DryRunError;
use super::classifier;
use super::db_copy;
use super::preflight;
use super::report::{self, ProviderProofStatus};
use super::sql;
use super::target_resolution;
use oulipoly_state::{StateDb, StateDbWriterAuthority};
use rusqlite::Connection;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone)]
pub(crate) struct ApplyOptions {
    pub(crate) live_state_db_path: PathBuf,
    pub(crate) models_dir: Option<PathBuf>,
    pub(crate) backup_dir: PathBuf,
    pub(crate) skip_provider_proof: bool,
}

#[derive(Debug)]
pub(crate) struct ApplyOutcome {
    pub(crate) report_path: PathBuf,
    pub(crate) backup_path: PathBuf,
}

pub(crate) struct LiveMigrationConnection {
    connection: Connection,
    authority: StateDbWriterAuthority,
}

impl Deref for LiveMigrationConnection {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        &self.connection
    }
}

impl LiveMigrationConnection {
    pub(crate) fn state_path(&self) -> &Path {
        self.authority.path()
    }
}

pub(crate) fn run_session_ownership_apply(opts: ApplyOptions) -> Result<ApplyOutcome, DryRunError> {
    let conn = open_live_migration_connection(&opts.live_state_db_path)?;
    let source_path = conn.state_path().to_path_buf();
    let backup_path = db_copy::create_verified_live_backup(&source_path, &opts.backup_dir)?;
    let before = preflight::preflight(&conn)?;
    let target = target_resolution::resolve_target(opts.models_dir.as_deref())?;
    let provider_proof = prove_or_skip_provider(&target, opts.skip_provider_proof)?;
    let candidates = classifier::classify(&conn, &target)?;
    classifier::populate_sql_inputs_temp(&conn, &target, &candidates)?;
    let forward = match sql::apply_forward_live(&conn) {
        Ok(forward) => forward,
        Err(err) => {
            classifier::cleanup_temp_sql_inputs(&conn);
            return Err(err);
        }
    };
    classifier::cleanup_temp_sql_inputs(&conn);
    let verification = match sql::verify_live_apply(&conn, &before, &forward) {
        Ok(verification) => verification,
        Err(err) => return rollback_after_failed_verify(&conn, &backup_path, err),
    };
    let report_path = report::write_apply_report(&report::ApplyReportInput {
        report_dir: opts.backup_dir,
        source_path,
        backup_path: backup_path.clone(),
        before,
        verification,
        candidates,
        forward,
        provider_proof,
    })?;
    Ok(ApplyOutcome {
        report_path,
        backup_path,
    })
}

pub(crate) fn run_session_ownership_corrective_apply(
    opts: ApplyOptions,
) -> Result<ApplyOutcome, DryRunError> {
    let conn = open_live_migration_connection(&opts.live_state_db_path)?;
    let source_path = conn.state_path().to_path_buf();
    let backup_path = db_copy::create_verified_live_backup(&source_path, &opts.backup_dir)?;
    let before = preflight::preflight(&conn)?;
    let target = target_resolution::resolve_target(opts.models_dir.as_deref())?;
    let provider_proof = prove_or_skip_provider(&target, opts.skip_provider_proof)?;
    let plan = classifier::build_corrective_plan(&conn, &target)?;
    classifier::populate_corrective_plan_temp(&conn, &plan)?;
    let corrective = match sql::apply_corrective_forward_live(&conn) {
        Ok(corrective) => corrective,
        Err(err) => {
            classifier::cleanup_corrective_plan_temp(&conn);
            return Err(err);
        }
    };
    classifier::cleanup_corrective_plan_temp(&conn);
    let verification = match sql::verify_corrective_apply(
        &conn,
        &before,
        &corrective,
        &target.moved_family_provider_ref_models,
    ) {
        Ok(verification) => verification,
        Err(err) => return corrective_rollback_after_failed_verify(&conn, &backup_path, err),
    };
    let report_path = report::write_corrective_apply_report(&report::CorrectiveApplyReportInput {
        report_dir: opts.backup_dir,
        source_path,
        backup_path: backup_path.clone(),
        before,
        verification,
        corrective,
        provider_proof,
    })?;
    Ok(ApplyOutcome {
        report_path,
        backup_path,
    })
}

pub(crate) fn open_live_migration_connection(
    path: &std::path::Path,
) -> Result<LiveMigrationConnection, DryRunError> {
    let authority = StateDb::acquire_writer_authority(path).map_err(DryRunError::new)?;
    let conn = Connection::open(authority.path())?;
    conn.busy_timeout(Duration::from_millis(1000))?;
    Ok(LiveMigrationConnection {
        connection: conn,
        authority,
    })
}

fn prove_or_skip_provider(
    target: &target_resolution::TargetResolution,
    skip_provider_proof: bool,
) -> Result<ProviderProofStatus, DryRunError> {
    if skip_provider_proof {
        Ok(ProviderProofStatus::Skipped)
    } else {
        target_resolution::prove_provider(target)?;
        Ok(ProviderProofStatus::Passed)
    }
}

fn rollback_after_failed_verify<T>(
    conn: &Connection,
    backup_path: &std::path::Path,
    err: DryRunError,
) -> Result<T, DryRunError> {
    let rollback_result = sql::apply_rollback_live(conn)
        .and_then(|_| sql::verify_rollback_restored(conn).map(|_| ()))
        .map(|_| "auto-rollback succeeded".to_string())
        .unwrap_or_else(|rollback_err| format!("auto-rollback failed: {rollback_err}"));
    Err(DryRunError::new(format!(
        "post-apply verification failed: {err}; {rollback_result}; backup={}",
        backup_path.display()
    )))
}

fn corrective_rollback_after_failed_verify<T>(
    conn: &Connection,
    backup_path: &std::path::Path,
    err: DryRunError,
) -> Result<T, DryRunError> {
    let rollback_result = sql::apply_corrective_rollback_live(conn)
        .and_then(|_| sql::verify_corrective_rollback_restored(conn).map(|_| ()))
        .map(|_| "corrective auto-rollback succeeded".to_string())
        .unwrap_or_else(|rollback_err| format!("corrective auto-rollback failed: {rollback_err}"));
    Err(DryRunError::new(format!(
        "post-apply verification failed: {err}; {rollback_result}; backup={}",
        backup_path.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_migration_connection_and_rebuild_authority_exclude_each_other() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        drop(StateDb::open(&state_path).unwrap());
        let migration = open_live_migration_connection(&state_path).unwrap();
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let rebuild_path = state_path.clone();
        let rebuild = std::thread::spawn(move || {
            sender
                .send(StateDb::acquire_rebuild_authority(&rebuild_path))
                .unwrap()
        });
        assert!(
            receiver
                .recv_timeout(std::time::Duration::from_millis(100))
                .is_err(),
            "rebuild escaped a live migration connection"
        );
        drop(migration);
        drop(receiver.recv().unwrap().unwrap());
        rebuild.join().unwrap();

        let authority = StateDb::acquire_rebuild_authority(&state_path).unwrap();
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let migration_path = state_path.clone();
        let opener = std::thread::spawn(move || {
            sender
                .send(open_live_migration_connection(&migration_path))
                .unwrap()
        });
        assert!(
            receiver
                .recv_timeout(std::time::Duration::from_millis(100))
                .is_err(),
            "live migration connection escaped rebuild authority"
        );
        drop(authority);
        drop(receiver.recv().unwrap().unwrap());
        opener.join().unwrap();
    }

    #[test]
    fn live_migration_rejects_a_preexisting_hard_link_alias() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        let alias_path = directory.path().join("alternate.db");
        drop(StateDb::open(&state_path).unwrap());
        std::fs::hard_link(&state_path, &alias_path).unwrap();

        let error = open_live_migration_connection(&alias_path).err().unwrap();

        assert!(
            error.to_string().contains("exactly one hard link"),
            "{error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn live_migration_retains_the_normalized_authority_path() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let target_path = directory.path().join("target.db");
        let alias_path = directory.path().join("state.db");
        drop(StateDb::open(&target_path).unwrap());
        symlink(&target_path, &alias_path).unwrap();

        let migration = open_live_migration_connection(&alias_path).unwrap();

        assert_eq!(migration.state_path(), target_path.canonicalize().unwrap());
    }
}
