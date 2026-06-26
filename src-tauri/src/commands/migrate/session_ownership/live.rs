//! Declared role: orchestration

use super::DryRunError;
use super::classifier;
use super::db_copy;
use super::preflight;
use super::report::{self, ProviderProofStatus};
use super::sql;
use super::target_resolution;
use rusqlite::Connection;
use std::path::PathBuf;
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

pub(crate) fn run_session_ownership_apply(opts: ApplyOptions) -> Result<ApplyOutcome, DryRunError> {
    let backup_path =
        db_copy::create_verified_live_backup(&opts.live_state_db_path, &opts.backup_dir)?;
    let conn = open_live_migration_connection(&opts.live_state_db_path)?;
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
        source_path: absolute_path(&opts.live_state_db_path)?,
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
    let backup_path =
        db_copy::create_verified_live_backup(&opts.live_state_db_path, &opts.backup_dir)?;
    let conn = open_live_migration_connection(&opts.live_state_db_path)?;
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
        source_path: absolute_path(&opts.live_state_db_path)?,
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
) -> Result<Connection, DryRunError> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_millis(1000))?;
    Ok(conn)
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

fn absolute_path(path: &std::path::Path) -> Result<PathBuf, DryRunError> {
    path.canonicalize().map_err(Into::into)
}
