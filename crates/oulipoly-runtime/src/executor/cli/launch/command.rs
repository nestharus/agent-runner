//! ## Declared roles
//!
//! Roles: orchestration.
//!
//! - orchestration: sequences command parsing, validation, and process command
//!   materialization.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/launch/command.rs
//!     role: adapter
//!     Translates:
//!       - std-process-command-contract
//!       - provider-config-launch-contract
//!       - cli-ipc-return-channel-contract
//! ```

use super::command_format::command_from_parts;
use super::command_parse::parse_command_parts;
use super::command_validate::validate_command_parts;
use oulipoly_config::ProviderConfig;
use std::path::Path;
use std::process::Command;

pub(in crate::executor::cli) fn build_command(
    provider: &ProviderConfig,
    provider_args: &[String],
    working_dir: Option<&Path>,
    parent_invocation_env: Option<&str>,
    return_channel: Option<&Path>,
) -> Result<Command, String> {
    let parts = parse_command_parts(&provider.command);
    validate_command_parts(&parts)?;
    Ok(command_from_parts(
        &parts,
        provider_args,
        &provider.environment,
        &provider.unset_environment,
        working_dir,
        parent_invocation_env,
        return_channel,
    ))
}
