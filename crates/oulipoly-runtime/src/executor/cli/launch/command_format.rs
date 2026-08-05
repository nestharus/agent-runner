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

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use crate::executor::cli::spawn_identity::provider_parent_invocation_env;

pub(super) fn command_from_parts(
    parts: &[String],
    provider_args: &[String],
    environment: &BTreeMap<String, String>,
    unset_environment: &[String],
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
    for name in unset_environment {
        cmd.env_remove(name);
    }
    cmd.envs(environment);

    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }
    if let Some(parent_invocation_env) = provider_parent_invocation_env(parent_invocation_env) {
        cmd.env("OULIPOLY_PARENT_INVOCATION", parent_invocation_env);
    }
    if let Some(return_channel) = return_channel {
        cmd.env("OULIPOLY_RETURN_CHANNEL", return_channel);
    } else {
        cmd.env_remove("OULIPOLY_RETURN_CHANNEL");
    }
    pin_agent_data_dir(&mut cmd);

    cmd
}

fn pin_agent_data_dir(cmd: &mut Command) {
    if let Ok(data_dir) = oulipoly_state::paths::data_dir() {
        cmd.env(oulipoly_state::paths::DATA_DIR_ENV, data_dir);
    }
}

pub(in crate::executor::cli) fn append_command_args(cmd: &mut Command, args: &[String]) {
    for arg in args {
        cmd.arg(arg);
    }
}
