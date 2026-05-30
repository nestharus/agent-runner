//! ## Declared roles
//!
//! Roles: orchestration, mapper, predicate.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/resume/acceptance.rs
//!     role: adapter
//!     Translates:
//!       - resume-acceptance-rules-contract
//!       - resume-acceptance-result-contract
//!       - provider-specific-resume-phrase-contract
//! ```

use super::messages::{
    accepted_resume_pattern_evidence, missing_session_resume_evidence,
    rejected_resume_exit_evidence, rejected_resume_pattern_evidence, unconfirmed_resume_evidence,
};
use super::output::resume_acceptance_output;
use super::patterns::{accepted_resume_pattern, rejected_resume_pattern};
use crate::executor::provider_specific::resume_acceptance::output_reports_missing_session;
use crate::executor::{ResumeAcceptanceResult, ResumeAcceptanceStatus};

pub(in crate::executor::cli) fn classify_resume_acceptance(
    rules: Option<&oulipoly_config::ResumeAcceptanceRules>,
    exit_code: i32,
    stdout: &[u8],
    stderr: &[u8],
    session_id: &str,
) -> ResumeAcceptanceResult {
    let output = resume_acceptance_output(stdout, stderr);
    if let Some(result) = configured_resume_acceptance_result(rules, exit_code, &output, session_id)
    {
        return result;
    }

    if output_reports_missing_session(&output) {
        return missing_session_resume_result();
    }

    if resume_child_exited_successfully(exit_code) {
        unconfirmed_resume_result(rules)
    } else {
        rejected_resume_exit_result(exit_code)
    }
}

fn configured_resume_acceptance_result(
    rules: Option<&oulipoly_config::ResumeAcceptanceRules>,
    exit_code: i32,
    output: &str,
    session_id: &str,
) -> Option<ResumeAcceptanceResult> {
    let rules = rules?;
    if let Some(pattern) = rejected_resume_pattern(rules, output, session_id) {
        return Some(rejected_resume_pattern_result(&pattern));
    }
    if resume_child_exited_successfully(exit_code)
        && let Some(pattern) = accepted_resume_pattern(rules, output, session_id)
    {
        return Some(accepted_resume_pattern_result(&pattern));
    }
    None
}

fn rejected_resume_pattern_result(pattern: &str) -> ResumeAcceptanceResult {
    resume_result_with_evidence(
        ResumeAcceptanceStatus::Rejected,
        rejected_resume_pattern_evidence(pattern),
    )
}

fn accepted_resume_pattern_result(pattern: &str) -> ResumeAcceptanceResult {
    resume_result_with_evidence(
        ResumeAcceptanceStatus::Accepted,
        accepted_resume_pattern_evidence(pattern),
    )
}

fn resume_result_with_evidence(
    status: ResumeAcceptanceStatus,
    evidence: String,
) -> ResumeAcceptanceResult {
    ResumeAcceptanceResult {
        status,
        evidence: Some(evidence),
    }
}

fn missing_session_resume_result() -> ResumeAcceptanceResult {
    resume_result_with_evidence(
        ResumeAcceptanceStatus::Rejected,
        missing_session_resume_evidence(),
    )
}

fn resume_child_exited_successfully(exit_code: i32) -> bool {
    exit_code == 0
}

fn unconfirmed_resume_result(
    rules: Option<&oulipoly_config::ResumeAcceptanceRules>,
) -> ResumeAcceptanceResult {
    let evidence = unconfirmed_resume_evidence(resume_rules_configured(rules));
    ResumeAcceptanceResult {
        status: ResumeAcceptanceStatus::Unconfirmed,
        evidence: Some(evidence.to_string()),
    }
}

fn resume_rules_configured(rules: Option<&oulipoly_config::ResumeAcceptanceRules>) -> bool {
    rules.is_some()
}

fn rejected_resume_exit_result(exit_code: i32) -> ResumeAcceptanceResult {
    resume_result_with_evidence(
        ResumeAcceptanceStatus::Rejected,
        rejected_resume_exit_evidence(exit_code),
    )
}
