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
//!
//! The OpenCode phrase list is intentionally empty until a live isolated
//! OpenCode bad-`--session` run verifies deterministic wording.
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
    lower.contains("no conversation found") || lower.contains("no session found")
}
