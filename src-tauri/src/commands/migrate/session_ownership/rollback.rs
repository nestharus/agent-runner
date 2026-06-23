//! Declared role: orchestration

use super::DryRunError;
use super::live::open_live_migration_connection;
use super::preflight;
use super::report;
use super::sql;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub(crate) struct RollbackOptions {
    pub(crate) live_state_db_path: PathBuf,
}

#[derive(Debug)]
pub(crate) struct RollbackOutcome {
    pub(crate) report_path: PathBuf,
}

pub(crate) fn run_session_ownership_rollback(
    opts: RollbackOptions,
) -> Result<RollbackOutcome, DryRunError> {
    let conn = open_live_migration_connection(&opts.live_state_db_path)?;
    let before = preflight::preflight(&conn)?;
    let preimage_rows = sql::require_preimage_rows(&conn)?;
    let rollback = sql::apply_rollback_live(&conn)?;
    let restored_mismatches = sql::verify_rollback_restored(&conn)?;
    let after = preflight::inspect_integrity(&conn)?;
    if after.quick_check != "ok" {
        return Err(DryRunError::new(format!(
            "rollback quick_check failed: {}",
            after.quick_check
        )));
    }
    let source_path = opts.live_state_db_path.canonicalize()?;
    let report_path = report::write_rollback_report(&report::RollbackReportInput {
        report_dir: report::default_rollback_report_dir(&source_path),
        source_path,
        before,
        after,
        preimage_rows,
        rollback,
        restored_mismatches,
    })?;
    Ok(RollbackOutcome { report_path })
}
