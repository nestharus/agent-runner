//! Declared role: orchestration

use super::accessor::{backfill_session, segments};
use super::formatter::render_compaction_backfill_session;
use super::report::{accumulate_compaction_backfill, empty_compaction_backfill_report};
use super::{CompactionBackfillReport, render_compaction_backfill_report};
use oulipoly_state::StateDb;

pub(crate) fn run_compaction_backfill(state: &StateDb) -> Result<CompactionBackfillReport, String> {
    let mut report = empty_compaction_backfill_report();
    for (provider_name, session_id) in segments(state)? {
        let flagged = backfill_session(state, &provider_name, &session_id)?;
        accumulate_compaction_backfill(&mut report, flagged);
        render_compaction_backfill_session(&provider_name, &session_id, flagged);
    }
    render_compaction_backfill_report(&report);
    Ok(report)
}
