//! ## Declared roles
//!
//! Roles: filter, predicate, formatter.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/resume/patterns.rs
//!     role: adapter
//!     Translates:
//!       - resume-acceptance-rules-contract
//! ```

pub(super) fn rejected_resume_pattern(
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

pub(super) fn accepted_resume_pattern(
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

pub(super) fn pattern_matches_output(pattern: &str, output: &str, session_id: &str) -> bool {
    output.contains(&expand_resume_pattern(pattern, session_id))
}

fn expand_resume_pattern(pattern: &str, session_id: &str) -> String {
    pattern.replace("{session_id}", session_id)
}
