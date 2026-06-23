//! Declared role: orchestration

mod classifier;
mod db_copy;
mod error;
mod live;
mod preflight;
mod report;
mod rollback;
mod sql;
mod target_resolution;

use std::path::PathBuf;

pub(crate) use error::DryRunError;
pub(crate) use live::{ApplyOptions, run_session_ownership_apply};
pub(crate) use rollback::{RollbackOptions, run_session_ownership_rollback};

#[derive(Debug, Clone)]
pub(crate) struct DryRunOptions {
    pub(crate) live_state_db_path: PathBuf,
    pub(crate) mailbox_db_path: Option<PathBuf>,
    pub(crate) models_dir: Option<PathBuf>,
    pub(crate) scratch_dir: PathBuf,
    pub(crate) skip_provider_proof: bool,
}

#[derive(Debug)]
pub(crate) struct DryRunOutcome {
    pub(crate) report_path: PathBuf,
}

pub(crate) fn run_session_ownership_dry_run(
    opts: DryRunOptions,
) -> Result<DryRunOutcome, DryRunError> {
    let paths = db_copy::snapshot_inputs(&opts)?;
    let mut copy = db_copy::open_copy(&paths.state_copy)?;
    let before_forward = preflight::preflight(&copy)?;
    let target = target_resolution::resolve_target(opts.models_dir.as_deref())?;
    if !opts.skip_provider_proof {
        target_resolution::prove_provider(&target)?;
    }
    let candidates = classifier::classify(&copy, &target)?;
    classifier::populate_sql_inputs(&mut copy, &target, &candidates)?;

    let first_forward = sql::apply_forward(&copy)?;
    db_copy::copy_state_to_rollback(&paths)?;

    let idempotence = sql::apply_forward(&copy)?;
    let after_idempotence = preflight::inspect_integrity(&copy)?;

    let rollback = {
        let rollback_copy = db_copy::open_copy(&paths.rollback_copy)?;
        sql::apply_rollback(&rollback_copy)?
    };
    let rollback_integrity = {
        let rollback_copy = db_copy::open_copy(&paths.rollback_copy)?;
        preflight::inspect_integrity(&rollback_copy)?
    };
    let cwd = sql::cwd_completeness(&copy, paths.mailbox_copy.as_deref())?;
    let report_path = report::write_report(&report::ReportInput {
        source_path: opts.live_state_db_path,
        scratch_root: opts.scratch_dir,
        paths,
        before_forward,
        after_idempotence,
        rollback_integrity,
        candidates,
        first_forward,
        idempotence,
        rollback,
        cwd,
    })?;
    Ok(DryRunOutcome { report_path })
}
