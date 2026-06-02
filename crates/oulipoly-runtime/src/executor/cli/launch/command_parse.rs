//! ## Declared roles
//!
//! Roles: parser.
//!
//! - parser: parses provider command strings into process command parts.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/launch/command_parse.rs
//!     role: adapter
//!     Translates:
//!       - provider-config-launch-contract
//!       - std-process-command-contract
//! ```

use crate::executor::cli::provider_identity::shell_split;

pub(super) fn parse_command_parts(command: &str) -> Vec<String> {
    shell_split(command)
}
