//! Declared role: orchestration

use super::formatter::{render_migrate_rebuild_report, render_session_chain_backfill_report};
use super::rebuild::{execute_migrate_rebuild, migrate_rebuild_plan};
use super::validator::validate_migrate_rebuild_flag;
use crate::commands::compaction_backfill::run_compaction_backfill;
use std::path::Path;

pub(crate) fn run_migrate_db() -> Result<i32, String> {
    let state = super::accessor::open_default_state_db()?;
    let report = state.backfill_session_chains()?;
    render_session_chain_backfill_report(&report);
    run_compaction_backfill(&state)?;
    Ok(0)
}

pub(crate) fn run_migrate(rebuild: bool) -> Result<i32, String> {
    validate_migrate_rebuild_flag(rebuild)?;
    run_migrate_rebuild()
}

pub(crate) fn run_migrate_session_ownership(
    dry_run: bool,
    scratch_dir: &Path,
    state_db: Option<&Path>,
    models_dir: Option<&Path>,
) -> Result<i32, String> {
    if !dry_run {
        return Err("migrate-session-ownership requires --dry-run".to_string());
    }
    let live_state_db_path = match state_db {
        Some(path) => path.to_path_buf(),
        None => super::accessor::default_state_db_path()?,
    };
    let outcome = super::session_ownership::run_session_ownership_dry_run(
        super::session_ownership::DryRunOptions {
            live_state_db_path,
            mailbox_db_path: None,
            models_dir: models_dir.map(Path::to_path_buf),
            scratch_dir: scratch_dir.to_path_buf(),
        },
    )
    .map_err(|err| err.to_string())?;
    println!("report={}", outcome.report_path.display());
    println!("live_db_mutated=no");
    Ok(0)
}

fn run_migrate_rebuild() -> Result<i32, String> {
    let Some(plan) = migrate_rebuild_plan()? else {
        return Ok(0);
    };
    execute_migrate_rebuild(&plan)?;
    let fresh = super::accessor::open_state_db(&plan.db_path)?;
    drop(fresh);
    render_migrate_rebuild_report(&plan);
    Ok(0)
}
