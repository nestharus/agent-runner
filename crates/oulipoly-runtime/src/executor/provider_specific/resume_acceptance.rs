//! ## Declared roles
//!
//! Roles: predicate.
//!
//! ## ACR-251 canonical-doc-as-schema declaration (PP-009 provider phrases)
//!
//! Resume missing-session phrase recognition consumes combined stdout + stderr
//! from the resume child process after lower-casing. Recognized lowercase
//! substrings (exact text):
//!
//! - `"no conversation found"`
//! - `"no session found"`
//! - `"session not found"` (OpenCode fixture; verify live phrasing in an
//!   isolated sandbox before applying production config)
//! - `"session <id> not found"` via tokenized `"session "` + `" not found"`
//!   matching (OpenCode fixture; same validation TODO)
//! - `"session does not exist"` (OpenCode fixture; same validation TODO)
//! - `"session id mismatch"` (OpenCode fixture; same validation TODO)
//!
//! `tests/age_164_c5_resume_capture.rs` (`acr251_pp009_*`) pins these strings;
//! this provider-specific island owns the phrase schema.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/provider_specific/resume_acceptance.rs
//!     role: adapter
//!     Translates:
//!       - provider-output-resume-mismatch-phrase-contract
//! ```

pub(in crate::executor) fn output_reports_missing_session(output: &str) -> bool {
    let lower = output.to_lowercase();
    lower.contains("no conversation found")
        || lower.contains("no session found")
        || lower.contains("session not found")
        || (lower.contains("session ") && lower.contains(" not found"))
        || lower.contains("session does not exist")
        || lower.contains("session id mismatch")
}
