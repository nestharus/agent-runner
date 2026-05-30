//! ## Declared roles
//!
//! Roles: orchestration, parser, formatter.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/resume/output.rs
//!     role: adapter
//!     Translates:
//!       - resume-acceptance-output-contract
//! ```

pub(super) fn resume_acceptance_output(stdout: &[u8], stderr: &[u8]) -> String {
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
