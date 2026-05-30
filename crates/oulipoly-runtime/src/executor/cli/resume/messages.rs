//! ## Declared roles
//!
//! Roles: formatter.
//!
//! ## ACR-251 canonical-doc-as-schema declaration (PP-009 evidence)
//!
//! Missing-session resume mapping emits this exact evidence string:
//!
//! `"resume_session_mismatch: provider reported missing session"`
//!
//! `tests/age_164_c5_resume_capture.rs` pins this string; this neutral message
//! leaf owns the evidence formatting contract.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/resume/messages.rs
//!     role: adapter
//!     Translates:
//!       - resume-acceptance-evidence-contract
//!       - provider-resume-validation-error-contract
//! ```

pub(super) fn resume_flag_required_message() -> String {
    "resume.flag is required".to_string()
}

pub(super) fn resume_subcommand_required_message() -> String {
    "resume.subcommand is required".to_string()
}

pub(super) fn rejected_resume_pattern_evidence(pattern: &str) -> String {
    format!("matched reject pattern: {pattern}")
}

pub(super) fn accepted_resume_pattern_evidence(pattern: &str) -> String {
    format!("matched accept pattern: {pattern}")
}

pub(super) fn missing_session_resume_evidence() -> String {
    "resume_session_mismatch: provider reported missing session".to_string()
}

pub(super) fn unconfirmed_resume_evidence(rules_configured: bool) -> &'static str {
    if rules_configured {
        "child exited 0 but no accepted resume pattern matched"
    } else {
        "child exited 0 but provider has no resume_acceptance rules configured"
    }
}

pub(super) fn rejected_resume_exit_evidence(exit_code: i32) -> String {
    format!("child exited {exit_code}; no rejection patterns matched")
}
