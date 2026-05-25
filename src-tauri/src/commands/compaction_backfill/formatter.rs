//! Declared role: formatter

use super::report::CompactionBackfillReport;

pub(super) fn render_compaction_backfill_session(
    provider_name: &str,
    session_id: &str,
    flagged: u64,
) {
    println!(
        "{}",
        format_compaction_backfill_session_line(provider_name, session_id, flagged)
    );
}

pub(crate) fn render_compaction_backfill_report(report: &CompactionBackfillReport) {
    println!("{}", format_compaction_backfill_report_line(report));
}

pub(super) fn format_compaction_backfill_session_line(
    provider_name: &str,
    session_id: &str,
    flagged: u64,
) -> String {
    format!(
        "compaction backfill session: provider={} session_id={} flagged={}",
        provider_name, session_id, flagged
    )
}

pub(super) fn format_compaction_backfill_report_line(report: &CompactionBackfillReport) -> String {
    format!(
        "compaction backfill: {} turns flagged across {} sessions",
        report.turns_flagged, report.sessions_processed
    )
}
