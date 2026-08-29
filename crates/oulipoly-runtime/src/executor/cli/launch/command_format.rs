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

use crate::executor::cli::spawn_identity::{
    provider_parent_invocation_env, split_invocation_launch_environment,
};
use oulipoly_core::AutoWakeEnvironmentVariable;

pub(super) fn command_from_parts(
    parts: &[String],
    provider_args: &[String],
    environment: &BTreeMap<String, String>,
    unset_environment: &[String],
    working_dir: Option<&Path>,
    parent_invocation_env: Option<&str>,
    return_channel: Option<&Path>,
) -> Result<Command, String> {
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
    for variable in AutoWakeEnvironmentVariable::ALL {
        cmd.env_remove(variable.name());
    }

    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }
    if let Some(selected_parent) = provider_parent_invocation_env(parent_invocation_env) {
        let selected_is_current = parent_invocation_env == Some(selected_parent.as_str());
        let (parent_identity, completion_authority) =
            split_invocation_launch_environment(&selected_parent)?;
        cmd.env("OULIPOLY_PARENT_INVOCATION", parent_identity);
        if let Some(completion_authority) = completion_authority {
            cmd.env(
                oulipoly_state::COMPLETION_REGISTRATION_AUTHORITY_ENV,
                completion_authority,
            );
        } else if selected_is_current {
            cmd.env_remove(oulipoly_state::COMPLETION_REGISTRATION_AUTHORITY_ENV);
        }
    } else {
        cmd.env_remove(oulipoly_state::COMPLETION_REGISTRATION_AUTHORITY_ENV);
    }
    if let Some(return_channel) = return_channel {
        cmd.env("OULIPOLY_RETURN_CHANNEL", return_channel);
    } else {
        cmd.env_remove("OULIPOLY_RETURN_CHANNEL");
    }
    pin_agent_data_dir(&mut cmd)?;

    Ok(cmd)
}

fn pin_agent_data_dir(cmd: &mut Command) -> Result<(), String> {
    let data_dir = oulipoly_state::paths::data_dir()?;
    cmd.env(oulipoly_state::paths::DATA_DIR_ENV, data_dir);
    Ok(())
}

pub(in crate::executor::cli) fn append_command_args(cmd: &mut Command, args: &[String]) {
    for arg in args {
        cmd.arg(arg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_authority_is_removed_from_observable_parent_identity() {
        let invocation = oulipoly_state::CompositeInvocationId {
            source: "fixture-provider".to_string(),
            id: "11111111-1111-4111-8111-111111111111".to_string(),
        };
        let authority =
            oulipoly_state::CompletionRegistrationAuthority::from_process_environment_value(
                "ab".repeat(32),
            )
            .unwrap();
        let launch = authority
            .invocation_launch_environment(&invocation)
            .unwrap();

        let (identity, transported_authority) =
            split_invocation_launch_environment(&launch).unwrap();

        assert_eq!(
            oulipoly_state::CompositeInvocationId::parse_env_value(&identity).unwrap(),
            invocation
        );
        assert!(!identity.contains(authority.process_environment_value()));
        assert_eq!(
            transported_authority.as_deref(),
            Some(authority.process_environment_value())
        );
        assert!(!format!("{authority:?}").contains(authority.process_environment_value()));
    }

    #[test]
    fn provider_launch_removes_runner_private_auto_wake_environment() {
        let environment = AutoWakeEnvironmentVariable::ALL
            .into_iter()
            .map(|variable| (variable.name().to_string(), "private".to_string()))
            .collect();

        let command = command_from_parts(
            &["provider".to_string()],
            &[],
            &environment,
            &[],
            None,
            None,
            None,
        )
        .unwrap();
        let explicit_environment: BTreeMap<_, _> = command
            .get_envs()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect();

        for variable in AutoWakeEnvironmentVariable::ALL {
            assert_eq!(explicit_environment.get(variable.name()), Some(&None));
        }
    }
}
