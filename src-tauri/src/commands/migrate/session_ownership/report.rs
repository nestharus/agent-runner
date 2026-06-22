//! Declared role: formatter

use super::classifier::Candidates;
use super::db_copy::SnapshotPaths;
use super::preflight::IntegrityReport;
use super::sql::{CwdCompleteness, ForwardCounts, RollbackCounts};
use super::DryRunError;
use std::fs;
use std::path::PathBuf;

pub(crate) struct ReportInput {
    pub(crate) source_path: PathBuf,
    pub(crate) scratch_root: PathBuf,
    pub(crate) paths: SnapshotPaths,
    pub(crate) before_forward: IntegrityReport,
    pub(crate) after_idempotence: IntegrityReport,
    pub(crate) rollback_integrity: IntegrityReport,
    pub(crate) candidates: Candidates,
    pub(crate) first_forward: ForwardCounts,
    pub(crate) idempotence: ForwardCounts,
    pub(crate) rollback: RollbackCounts,
    pub(crate) cwd: CwdCompleteness,
}

pub(crate) fn write_report(input: &ReportInput) -> Result<PathBuf, DryRunError> {
    let mut body = String::new();
    body.push_str("# Session Ownership Dry Run\n\n");
    body.push_str("live_db_mutated: no\n");
    body.push_str(&format!("source_path: {}\n", input.source_path.display()));
    body.push_str(&format!("scratch_root: {}\n", input.scratch_root.display()));
    body.push_str(&format!(
        "state_copy: {}\n",
        input.paths.state_copy.display()
    ));
    body.push_str(&format!(
        "rollback_copy: {}\n",
        input.paths.rollback_copy.display()
    ));
    if let Some(mailbox) = &input.paths.mailbox_copy {
        body.push_str(&format!("mailbox_copy: {}\n", mailbox.display()));
    }
    body.push_str(&format!(
        "report_path: {}\n\n",
        input.paths.report_path.display()
    ));

    push_integrity(&mut body, "before_forward", &input.before_forward);
    push_integrity(&mut body, "after_idempotence", &input.after_idempotence);
    push_integrity(&mut body, "rollback_copy", &input.rollback_integrity);

    body.push_str("\n## Candidate Counts\n");
    body.push_str(&format!(
        "candidate_chains: {}\n",
        input.candidates.candidate_chains
    ));
    body.push_str(&format!(
        "candidate_segments: {}\n",
        input.candidates.candidate_segments
    ));
    body.push_str(&format!(
        "eligible_segments: {}\n",
        input.candidates.eligible_segments
    ));
    body.push_str(&format!(
        "blocked_segments: {}\n",
        input.candidates.blocked_segments
    ));
    body.push_str(&format!(
        "issue52_unregistered_segments: {}\n",
        input.candidates.issue52_unregistered_segments
    ));

    push_counts(&mut body, "first_forward", &input.first_forward.counts);
    push_counts(
        &mut body,
        "idempotence_second_run",
        &input.idempotence.counts,
    );

    body.push_str("\n## Rollback Copy\n");
    body.push_str(&format!(
        "rollback copy restored: {}\n",
        if input.rollback.restored { "yes" } else { "no" }
    ));
    push_counts(&mut body, "rollback copy", &input.rollback.counts);
    push_counts(&mut body, "rollback copy", &input.rollback.mismatches);

    body.push_str("\n## Cwd Completeness\n");
    body.push_str(&format!(
        "cwd completeness missing: {}\n",
        input.cwd.missing
    ));
    body.push_str(&format!("cwd completeness null: {}\n", input.cwd.null));
    body.push_str(&format!(
        "cwd completeness non-absolute: {}\n",
        input.cwd.non_absolute
    ));

    fs::write(&input.paths.report_path, body)?;
    Ok(input.paths.report_path.clone())
}

fn push_integrity(body: &mut String, phase: &str, report: &IntegrityReport) {
    body.push_str(&format!("{phase} quick_check: {}\n", report.quick_check));
    body.push_str(&format!("{phase} user_version: {}\n", report.user_version));
}

fn push_counts(body: &mut String, phase: &str, counts: &std::collections::BTreeMap<String, i64>) {
    body.push_str(&format!("\n## {phase}\n"));
    for (key, value) in counts {
        body.push_str(&format!("{phase} {key}: {value}\n"));
    }
}
