//! ## Declared roles
//!
//! Roles: formatter.
//!
//! - formatter: formats temporary prompt file names, user-facing prompt
//!   instructions, and canonical prompt file write errors.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/launch/prompt_format.rs
//!     role: adapter
//!     Translates:
//!       - temp-prompt-file-contract
//!       - prompt-transport-contract
//! ```

pub(super) fn temp_prompt_filename(id: uuid::Uuid) -> String {
    format!("_agent_prompt_{id}.md")
}

pub(super) fn temp_prompt_instruction(filename: &str) -> String {
    format!("Follow the instructions in {filename}")
}

pub(super) fn temp_prompt_write_error(err: &std::io::Error) -> String {
    format!("Failed to write temp prompt file: {err}")
}
