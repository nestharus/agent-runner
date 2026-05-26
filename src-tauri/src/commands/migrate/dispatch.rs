//! Declared role: orchestration

use super::formatter::{render_migrate_rebuild_report, render_session_chain_backfill_report};
use super::rebuild::{execute_migrate_rebuild, migrate_rebuild_plan};
use super::validator::validate_migrate_rebuild_flag;
use crate::commands::compaction_backfill::run_compaction_backfill;
use oulipoly_state::StateDb;

pub(crate) fn run_migrate_db() -> Result<i32, String> {
    let state = StateDb::open_default()?;
    let report = state.backfill_session_chains()?;
    render_session_chain_backfill_report(&report);
    run_compaction_backfill(&state)?;
    Ok(0)
}

pub(crate) fn run_migrate(rebuild: bool) -> Result<i32, String> {
    validate_migrate_rebuild_flag(rebuild)?;
    run_migrate_rebuild()
}

fn run_migrate_rebuild() -> Result<i32, String> {
    let Some(plan) = migrate_rebuild_plan()? else {
        return Ok(0);
    };
    execute_migrate_rebuild(&plan)?;
    let fresh = StateDb::open(&plan.db_path)?;
    drop(fresh);
    render_migrate_rebuild_report(&plan);
    Ok(0)
}
