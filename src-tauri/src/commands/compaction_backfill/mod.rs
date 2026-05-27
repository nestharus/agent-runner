//! Declared role: orchestration

pub(crate) mod accessor;
pub(crate) mod formatter;
pub(crate) mod orchestration;
pub(crate) mod report;

pub(crate) use formatter::render_compaction_backfill_report;
pub(crate) use orchestration::run_compaction_backfill;
pub(crate) use report::CompactionBackfillReport;

#[cfg(test)]
#[path = "tests.rs"]
mod compaction_backfill_tests;
