//! ## Declared roles
//!
//! Roles: validator, formatter, parser, filter, predicate, mapper,
//! orchestration.
//!
//! - validator: [`validate_resume_strategy`] checks resume kind/flag/
//!   subcommand shape before formatting argv.
//! - formatter: [`append_validated_resume_args`].
//! - parser: [`expand_resume_pattern`] expands `{session_id}` placeholders;
//!   [`resume_acceptance_output`] concatenates stdout+stderr.
//! - filter: [`first_matching_pattern`].
//! - predicate: [`output_reports_missing_session`],
//!   [`resume_child_exited_successfully`], [`pattern_matches_output`].
//! - mapper: [`classify_resume_acceptance`] maps observed evidence onto
//!   [`ResumeAcceptanceResult`] (status + evidence text).
//! - orchestration: [`compose_resume_args`],
//!   [`compose_resume_provider_args`], [`append_resume_args`].
//!
//! ## ACR-251 canonical-doc-as-schema declaration (PP-009)
//!
//! Resume missing-session phrase set — the runtime is a documented consumer
//! of an implicit provider-output schema. The schema is pinned here:
//!
//! - Input: combined stdout + stderr from the resume child process. The
//!   combined text is lower-cased before phrase matching.
//! - Recognized lowercase substrings (exact text):
//!     - `"no conversation found"`
//!     - `"no session found"`
//! - Mapped result evidence (verbatim):
//!   `"resume_session_mismatch: provider reported missing session"`
//!
//! `tests/age_164_c5_resume_capture.rs` (`acr251_pp009_*`) pins these
//! strings; the push-pull auditor accepts this rustdoc as canonical-doc-
//! as-schema proof for PP-009.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/resume.rs
//!     role: adapter
//!     Translates:
//!       - oulipoly-config-resume-strategy-contract
//!       - resume-acceptance-rules-contract
//!       - resume-acceptance-result-contract
//! ```

use super::super::{ResumeAcceptanceResult, ResumeAcceptanceStatus};
use oulipoly_config::{ResumeKind, ResumeStrategy};

pub struct ResumePayload<'a> {
    pub session_id: &'a str,
    pub strategy: &'a ResumeStrategy,
}

pub fn compose_resume_args(
    strategy: &ResumeStrategy,
    session_id: &str,
) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    append_resume_args(&mut args, strategy, session_id)?;
    Ok(args)
}

pub(super) fn compose_resume_provider_args(
    mut provider_args: Vec<String>,
    resume: ResumePayload<'_>,
) -> Result<Vec<String>, String> {
    append_resume_args(&mut provider_args, resume.strategy, resume.session_id)?;
    Ok(provider_args)
}

fn append_resume_args(
    provider_args: &mut Vec<String>,
    strategy: &ResumeStrategy,
    session_id: &str,
) -> Result<(), String> {
    let args = validate_resume_strategy(strategy)?;
    append_validated_resume_args(provider_args, args, session_id);
    Ok(())
}

enum ValidatedResumeArgs<'a> {
    Flag(&'a str),
    Subcommand(&'a [String]),
}

fn validate_resume_strategy(strategy: &ResumeStrategy) -> Result<ValidatedResumeArgs<'_>, String> {
    match strategy.kind {
        ResumeKind::Flag => {
            let flag = strategy
                .flag
                .as_ref()
                .ok_or_else(|| "resume.flag is required".to_string())?;
            Ok(ValidatedResumeArgs::Flag(flag))
        }
        ResumeKind::Subcommand => {
            let subcommand = strategy
                .subcommand
                .as_ref()
                .ok_or_else(|| "resume.subcommand is required".to_string())?;
            if subcommand.is_empty() {
                return Err("resume.subcommand is required".to_string());
            }
            Ok(ValidatedResumeArgs::Subcommand(subcommand))
        }
    }
}

fn append_validated_resume_args(
    provider_args: &mut Vec<String>,
    args: ValidatedResumeArgs<'_>,
    session_id: &str,
) {
    match args {
        ValidatedResumeArgs::Flag(flag) => {
            provider_args.push(flag.to_string());
            provider_args.push(session_id.to_string());
        }
        ValidatedResumeArgs::Subcommand(subcommand) => {
            provider_args.extend(subcommand.iter().cloned());
            provider_args.push(session_id.to_string());
        }
    }
}

pub(super) fn classify_resume_acceptance(
    rules: Option<&oulipoly_config::ResumeAcceptanceRules>,
    exit_code: i32,
    stdout: &[u8],
    stderr: &[u8],
    session_id: &str,
) -> ResumeAcceptanceResult {
    let output = resume_acceptance_output(stdout, stderr);
    if let Some(rules) = rules {
        if let Some(pattern) = rejected_resume_pattern(rules, &output, session_id) {
            return rejected_resume_pattern_result(&pattern);
        }
        if resume_child_exited_successfully(exit_code)
            && let Some(pattern) = accepted_resume_pattern(rules, &output, session_id)
        {
            return accepted_resume_pattern_result(&pattern);
        }
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

fn resume_acceptance_output(stdout: &[u8], stderr: &[u8]) -> String {
    format_resume_acceptance_output(
        &decode_resume_output_chunk(stdout),
        &decode_resume_output_chunk(stderr),
    )
}

fn decode_resume_output_chunk(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn format_resume_acceptance_output(stdout: &str, stderr: &str) -> String {
    format!("{stdout}\n{stderr}")
}

fn rejected_resume_pattern(
    rules: &oulipoly_config::ResumeAcceptanceRules,
    output: &str,
    session_id: &str,
) -> Option<String> {
    first_matching_pattern(
        rules.rejected_output_patterns.as_deref(),
        output,
        session_id,
    )
}

fn accepted_resume_pattern(
    rules: &oulipoly_config::ResumeAcceptanceRules,
    output: &str,
    session_id: &str,
) -> Option<String> {
    first_matching_pattern(
        rules.accepted_output_patterns.as_deref(),
        output,
        session_id,
    )
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

fn rejected_resume_pattern_evidence(pattern: &str) -> String {
    format!("matched reject pattern: {pattern}")
}

fn accepted_resume_pattern_evidence(pattern: &str) -> String {
    format!("matched accept pattern: {pattern}")
}

fn output_reports_missing_session(output: &str) -> bool {
    let lower = output.to_lowercase();
    lower.contains("no conversation found") || lower.contains("no session found")
}

fn missing_session_resume_result() -> ResumeAcceptanceResult {
    ResumeAcceptanceResult {
        status: ResumeAcceptanceStatus::Rejected,
        evidence: Some("resume_session_mismatch: provider reported missing session".to_string()),
    }
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

fn unconfirmed_resume_evidence(rules_configured: bool) -> &'static str {
    if rules_configured {
        "child exited 0 but no accepted resume pattern matched"
    } else {
        "child exited 0 but provider has no resume_acceptance rules configured"
    }
}

fn rejected_resume_exit_result(exit_code: i32) -> ResumeAcceptanceResult {
    resume_result_with_evidence(
        ResumeAcceptanceStatus::Rejected,
        rejected_resume_exit_evidence(exit_code),
    )
}

fn rejected_resume_exit_evidence(exit_code: i32) -> String {
    format!("child exited {exit_code}; no rejection patterns matched")
}

fn first_matching_pattern(
    patterns: Option<&[String]>,
    output: &str,
    session_id: &str,
) -> Option<String> {
    patterns?
        .iter()
        .find(|pattern| pattern_matches_output(pattern, output, session_id))
        .cloned()
}

fn pattern_matches_output(pattern: &str, output: &str, session_id: &str) -> bool {
    output.contains(&expand_resume_pattern(pattern, session_id))
}

fn expand_resume_pattern(pattern: &str, session_id: &str) -> String {
    pattern.replace("{session_id}", session_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_resume_acceptance_without_rules_uses_no_rules_evidence() {
        let result =
            classify_resume_acceptance(None, 0, b"ok", b"", "5169694d-de0f-40d1-890c-6e28e55bab27");

        assert_eq!(result.status, ResumeAcceptanceStatus::Unconfirmed);
        assert_eq!(
            result.evidence.as_deref(),
            Some("child exited 0 but provider has no resume_acceptance rules configured")
        );
    }

    #[test]
    fn resume_provider_missing_session_classified() {
        let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
        let result = classify_resume_acceptance(
            None,
            1,
            b"",
            format!("No conversation found for session {session_id}").as_bytes(),
            session_id,
        );

        assert_eq!(result.status, ResumeAcceptanceStatus::Rejected);
        assert!(
            result
                .evidence
                .as_deref()
                .unwrap_or_default()
                .contains("resume_session_mismatch"),
            "{result:?}"
        );
    }
}
