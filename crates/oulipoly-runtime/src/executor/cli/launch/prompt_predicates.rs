//! ## Declared roles
//!
//! Roles: predicate.
//!
//! - predicate: answers whether a rendered prompt must be transported through
//!   a temporary file.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/launch/prompt_predicates.rs
//!     role: adapter
//!     Translates:
//!       - prompt-transport-contract
//!       - temp-prompt-file-contract
//! ```

const LARGE_PROMPT_THRESHOLD: usize = 100 * 1024;

pub(super) fn prompt_requires_temp_file(rendered_prompt: &str) -> bool {
    rendered_prompt.len() > LARGE_PROMPT_THRESHOLD
}
