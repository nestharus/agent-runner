//! Declared role: formatter

use super::DryRunError;
use super::classifier::Candidates;
use super::db_copy::SnapshotPaths;
use super::preflight::IntegrityReport;
use super::sql::{
    CorrectiveApplyVerification, CorrectiveCounts, CorrectiveRollbackCounts, CwdCompleteness,
    ForwardCounts, LiveApplyVerification, RollbackCounts,
};
use std::fs;
use std::path::{Path, PathBuf};

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

pub(crate) struct ApplyReportInput {
    pub(crate) report_dir: PathBuf,
    pub(crate) source_path: PathBuf,
    pub(crate) backup_path: PathBuf,
    pub(crate) before: IntegrityReport,
    pub(crate) verification: LiveApplyVerification,
    pub(crate) candidates: Candidates,
    pub(crate) forward: ForwardCounts,
    pub(crate) provider_proof: ProviderProofStatus,
}

pub(crate) struct RollbackReportInput {
    pub(crate) report_dir: PathBuf,
    pub(crate) source_path: PathBuf,
    pub(crate) before: IntegrityReport,
    pub(crate) after: IntegrityReport,
    pub(crate) preimage_rows: i64,
    pub(crate) rollback: RollbackCounts,
    pub(crate) restored_mismatches: std::collections::BTreeMap<String, i64>,
}

pub(crate) struct CorrectiveDryRunReportInput {
    pub(crate) source_path: PathBuf,
    pub(crate) scratch_root: PathBuf,
    pub(crate) paths: SnapshotPaths,
    pub(crate) before_forward: IntegrityReport,
    pub(crate) after_idempotence: IntegrityReport,
    pub(crate) rollback_integrity: IntegrityReport,
    pub(crate) first_forward: CorrectiveCounts,
    pub(crate) idempotence: CorrectiveCounts,
    pub(crate) rollback: CorrectiveRollbackCounts,
}

pub(crate) struct CorrectiveApplyReportInput {
    pub(crate) report_dir: PathBuf,
    pub(crate) source_path: PathBuf,
    pub(crate) backup_path: PathBuf,
    pub(crate) before: IntegrityReport,
    pub(crate) verification: CorrectiveApplyVerification,
    pub(crate) corrective: CorrectiveCounts,
    pub(crate) provider_proof: ProviderProofStatus,
}

pub(crate) struct CorrectiveRollbackReportInput {
    pub(crate) report_dir: PathBuf,
    pub(crate) source_path: PathBuf,
    pub(crate) before: IntegrityReport,
    pub(crate) after: IntegrityReport,
    pub(crate) preimage_rows: i64,
    pub(crate) rollback: CorrectiveRollbackCounts,
    pub(crate) restored_mismatches: i64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ProviderProofStatus {
    Passed,
    Skipped,
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
    body.push_str(&format!(
        "segment_rows_merged_away: {}\n",
        input.candidates.segment_rows_merged_away
    ));
    body.push_str(&format!(
        "turn_rows_deduped_away: {}\n",
        input.candidates.turn_rows_deduped_away
    ));
    body.push_str(&format!(
        "segment_merge_survivors_updated: {}\n",
        input.candidates.segment_merge_survivors_updated
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

pub(crate) fn write_apply_report(input: &ApplyReportInput) -> Result<PathBuf, DryRunError> {
    fs::create_dir_all(&input.report_dir)?;
    let report_path = input.report_dir.join("session-ownership-apply-report.md");
    let mut body = String::new();
    body.push_str("# Session Ownership Apply\n\n");
    body.push_str("live_db_mutated: yes\n");
    body.push_str(&format!("source_path: {}\n", input.source_path.display()));
    body.push_str(&format!("backup_path: {}\n", input.backup_path.display()));
    body.push_str("preimage_table: s11_wu4_restore_session_ownership_preimage\n");
    body.push_str(&format!(
        "provider proof: {}\n\n",
        provider_proof_label(input.provider_proof)
    ));
    push_integrity(&mut body, "before_apply", &input.before);
    push_integrity(&mut body, "after_apply", &input.verification.integrity);
    push_candidate_counts(&mut body, &input.candidates);
    push_counts(&mut body, "planned", &input.forward.counts);
    push_counts(&mut body, "planned apply", &input.verification.planned);
    push_counts(&mut body, "applied", &input.verification.applied);
    body.push_str("\n## Post Apply Verification\n");
    body.push_str(&format!(
        "zero residual old-owned rows: {}\n",
        input.verification.residual_old_owned_rows == 0
    ));
    body.push_str(&format!(
        "residual_old_owned_rows: {}\n",
        input.verification.residual_old_owned_rows
    ));
    body.push_str(&format!(
        "zero remaining segment collisions: {}\n",
        input.verification.segment_collision_count == 0
    ));
    body.push_str(&format!(
        "post_apply_segment_collision_count: {}\n",
        input.verification.segment_collision_count
    ));
    body.push_str(&format!(
        "zero remaining turn collisions: {}\n",
        input.verification.turn_collision_count == 0
    ));
    body.push_str(&format!(
        "post_apply_turn_collision_count: {}\n",
        input.verification.turn_collision_count
    ));
    body.push_str(&format!(
        "preimage_rows: {}\n",
        input.verification.preimage_rows
    ));
    fs::write(&report_path, body)?;
    Ok(report_path)
}

pub(crate) fn write_rollback_report(input: &RollbackReportInput) -> Result<PathBuf, DryRunError> {
    fs::create_dir_all(&input.report_dir)?;
    let report_path = input
        .report_dir
        .join("session-ownership-rollback-report.md");
    let mut body = String::new();
    body.push_str("# Session Ownership Rollback\n\n");
    body.push_str("live_db_mutated: yes\n");
    body.push_str(&format!("source_path: {}\n", input.source_path.display()));
    body.push_str("preimage_table: s11_wu4_restore_session_ownership_preimage\n");
    body.push_str(&format!("preimage_rows: {}\n", input.preimage_rows));
    body.push_str("drift_check: passed\n");
    body.push_str(&format!(
        "restored: {}\n\n",
        input.rollback.restored && input.restored_mismatches.values().all(|value| *value == 0)
    ));
    push_integrity(&mut body, "before_rollback", &input.before);
    push_integrity(&mut body, "after_rollback", &input.after);
    push_counts(&mut body, "rollback", &input.rollback.counts);
    push_counts(&mut body, "rollback", &input.rollback.mismatches);
    push_counts(
        &mut body,
        "rollback restored verification",
        &input.restored_mismatches,
    );
    fs::write(&report_path, body)?;
    Ok(report_path)
}

pub(crate) fn write_corrective_dry_run_report(
    input: &CorrectiveDryRunReportInput,
) -> Result<PathBuf, DryRunError> {
    let mut body = String::new();
    body.push_str("# Session Ownership Corrective Dry Run\n\n");
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
    body.push_str(&format!(
        "report_path: {}\n",
        input.paths.report_path.display()
    ));
    body.push_str("preimage_table: s11_m2c_model_corrective_preimage\n\n");
    push_integrity(&mut body, "before_forward", &input.before_forward);
    push_integrity(&mut body, "after_idempotence", &input.after_idempotence);
    push_integrity(&mut body, "rollback_copy", &input.rollback_integrity);
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
    body.push_str("restored_model_semantics: backfill_default\n");
    push_counts(&mut body, "rollback copy", &input.rollback.counts);
    fs::write(&input.paths.report_path, body)?;
    Ok(input.paths.report_path.clone())
}

pub(crate) fn write_corrective_apply_report(
    input: &CorrectiveApplyReportInput,
) -> Result<PathBuf, DryRunError> {
    fs::create_dir_all(&input.report_dir)?;
    let report_path = input
        .report_dir
        .join("session-ownership-corrective-apply-report.md");
    let mut body = String::new();
    body.push_str("# Session Ownership Corrective Apply\n\n");
    body.push_str("live_db_mutated: yes\n");
    body.push_str(&format!("source_path: {}\n", input.source_path.display()));
    body.push_str(&format!("backup_path: {}\n", input.backup_path.display()));
    body.push_str("preimage_table: s11_m2c_model_corrective_preimage\n");
    body.push_str(&format!(
        "provider proof: {}\n\n",
        provider_proof_label(input.provider_proof)
    ));
    push_integrity(&mut body, "before_apply", &input.before);
    push_integrity(&mut body, "after_apply", &input.verification.integrity);
    push_counts(&mut body, "planned", &input.corrective.counts);
    body.push_str("\n## Post Apply Verification\n");
    body.push_str(&format!(
        "corrective_chain_model_updates_to_apply: {}\n",
        input.verification.planned
    ));
    body.push_str(&format!(
        "corrective_chain_model_updates_applied: {}\n",
        input.verification.applied
    ));
    body.push_str(&format!(
        "corrective_preimage_rows: {}\n",
        input.verification.preimage_rows
    ));
    body.push_str(&format!(
        "corrective_residual_default_rows: {}\n",
        input.verification.residual_default_rows
    ));
    fs::write(&report_path, body)?;
    Ok(report_path)
}

pub(crate) fn write_corrective_rollback_report(
    input: &CorrectiveRollbackReportInput,
) -> Result<PathBuf, DryRunError> {
    fs::create_dir_all(&input.report_dir)?;
    let report_path = input
        .report_dir
        .join("session-ownership-corrective-rollback-report.md");
    let mut body = String::new();
    body.push_str("# Session Ownership Corrective Rollback\n\n");
    body.push_str("live_db_mutated: yes\n");
    body.push_str(&format!("source_path: {}\n", input.source_path.display()));
    body.push_str("preimage_table: s11_m2c_model_corrective_preimage\n");
    body.push_str(&format!("preimage_rows: {}\n", input.preimage_rows));
    body.push_str("restored_model_semantics: backfill_default\n");
    body.push_str("drift_check: passed\n");
    body.push_str(&format!(
        "restored: {}\n\n",
        input.rollback.restored && input.restored_mismatches == 0
    ));
    push_integrity(&mut body, "before_rollback", &input.before);
    push_integrity(&mut body, "after_rollback", &input.after);
    push_counts(&mut body, "rollback", &input.rollback.counts);
    body.push_str(&format!(
        "rollback corrective_chain_model_rollback_mismatches: {}\n",
        input.restored_mismatches
    ));
    fs::write(&report_path, body)?;
    Ok(report_path)
}

fn provider_proof_label(status: ProviderProofStatus) -> &'static str {
    match status {
        ProviderProofStatus::Passed => "passed",
        ProviderProofStatus::Skipped => "skipped",
    }
}

pub(crate) fn default_rollback_report_dir(source_path: &Path) -> PathBuf {
    source_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn push_integrity(body: &mut String, phase: &str, report: &IntegrityReport) {
    body.push_str(&format!("{phase} quick_check: {}\n", report.quick_check));
    body.push_str(&format!("{phase} user_version: {}\n", report.user_version));
}

fn push_candidate_counts(body: &mut String, candidates: &Candidates) {
    body.push_str("\n## Candidate Counts\n");
    body.push_str(&format!(
        "candidate_chains: {}\n",
        candidates.candidate_chains
    ));
    body.push_str(&format!(
        "candidate_segments: {}\n",
        candidates.candidate_segments
    ));
    body.push_str(&format!(
        "eligible_segments: {}\n",
        candidates.eligible_segments
    ));
    body.push_str(&format!(
        "blocked_segments: {}\n",
        candidates.blocked_segments
    ));
    body.push_str(&format!(
        "issue52_unregistered_segments: {}\n",
        candidates.issue52_unregistered_segments
    ));
    body.push_str(&format!(
        "segment_rows_merged_away: {}\n",
        candidates.segment_rows_merged_away
    ));
    body.push_str(&format!(
        "turn_rows_deduped_away: {}\n",
        candidates.turn_rows_deduped_away
    ));
    body.push_str(&format!(
        "segment_merge_survivors_updated: {}\n",
        candidates.segment_merge_survivors_updated
    ));
}

fn push_counts(body: &mut String, phase: &str, counts: &std::collections::BTreeMap<String, i64>) {
    body.push_str(&format!("\n## {phase}\n"));
    for (key, value) in counts {
        body.push_str(&format!("{phase} {key}: {value}\n"));
    }
}
