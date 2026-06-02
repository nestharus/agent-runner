//! ## Declared roles
//!
//! Roles: formatter.
//!
//! - formatter: materializes process commands from parsed command parts,
//!   arguments, working directories, and IPC environment bindings.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/launch/command_format.rs
//!     role: adapter
//!     Translates:
//!       - std-process-command-contract
//!       - cli-ipc-return-channel-contract
//! ```

use std::path::Path;
use std::process::Command;

pub(super) fn command_from_parts(
    parts: &[String],
    provider_args: &[String],
    working_dir: Option<&Path>,
    parent_invocation_env: Option<&str>,
    return_channel: Option<&Path>,
) -> Command {
    let mut cmd = Command::new(&parts[0]);
    for part in &parts[1..] {
        cmd.arg(part);
    }
    for arg in provider_args {
        cmd.arg(arg);
    }

    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }
    if let Some(parent_invocation_env) = parent_invocation_env {
        cmd.env("OULIPOLY_PARENT_INVOCATION", parent_invocation_env);
    }
    if let Some(return_channel) = return_channel {
        cmd.env("OULIPOLY_RETURN_CHANNEL", return_channel);
    } else {
        cmd.env_remove("OULIPOLY_RETURN_CHANNEL");
    }

    cmd
}

pub(in crate::executor::cli) fn append_command_args(cmd: &mut Command, args: &[String]) {
    for arg in args {
        cmd.arg(arg);
    }
}
