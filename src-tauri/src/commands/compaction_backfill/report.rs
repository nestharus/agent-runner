//! Declared role: mapper

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CompactionBackfillReport {
    pub(crate) turns_flagged: u64,
    pub(crate) sessions_processed: u64,
}

pub(super) fn empty_compaction_backfill_report() -> CompactionBackfillReport {
    compaction_backfill_report(0, 0)
}

fn compaction_backfill_report(
    turns_flagged: u64,
    sessions_processed: u64,
) -> CompactionBackfillReport {
    CompactionBackfillReport {
        turns_flagged,
        sessions_processed,
    }
}

pub(super) fn accumulate_compaction_backfill(report: &mut CompactionBackfillReport, flagged: u64) {
    report.turns_flagged += flagged;
    report.sessions_processed += 1;
}
