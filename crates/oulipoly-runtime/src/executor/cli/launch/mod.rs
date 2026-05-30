//! ## Declared roles
//!
//! Roles: orchestration.
//!
//! - orchestration: assembles provider launches by sequencing IPC setup,
//!   policy launch parts, command construction, capture handoff, prompt
//!   rendering, and supervisor configuration.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/launch/mod.rs
//!     role: adapter
//!     Translates:
//!       - std-process-command-contract
//!       - provider-config-launch-contract
//!       - cli-ipc-return-channel-contract
//!       - provider-policy-launch-contract
//!       - session-capture-plan-contract
//! ```

mod capture;
mod command;
mod command_format;
mod command_parse;
mod command_validate;
mod prompt;
mod prompt_file;
mod prompt_format;
mod prompt_path;
mod prompt_predicates;
mod supervisor_config;

use super::ipc::{ReturnChannel, prepare_return_channel};
use super::policy::provider_policy_launch_parts;
use super::session_capture::CapturePlan;
use super::supervision::SupervisorConfig;
use oulipoly_config::{PromptMode, ProviderConfig};
use std::path::{Path, PathBuf};
use std::process::Command;

pub(in crate::executor::cli) use command::build_command;
pub(in crate::executor::cli) use command_format::append_command_args;

pub(super) struct ProviderLaunchRequest<'a> {
    pub(super) provider: &'a ProviderConfig,
    pub(super) provider_args: &'a [String],
    pub(super) tail_args: &'a [String],
    pub(super) prompt_mode: PromptMode,
    pub(super) prompt: Option<&'a str>,
    pub(super) working_dir: Option<&'a Path>,
    pub(super) input_args: &'a [String],
    pub(super) parent_invocation_env: Option<&'a str>,
    pub(super) start_known_provider_session_id: Option<&'a str>,
}

pub(super) struct ProviderLaunch {
    pub(super) cmd: Command,
    pub(super) supervisor_config: SupervisorConfig,
    pub(super) capture_plan: CapturePlan,
    pub(super) return_channel: Option<ReturnChannel>,
    pub(super) temp_files: Vec<PathBuf>,
}

pub(super) fn assemble_provider_launch(
    request: ProviderLaunchRequest<'_>,
    supervisor_config: Option<SupervisorConfig>,
) -> Result<ProviderLaunch, String> {
    let return_channel = prepare_return_channel(request.parent_invocation_env)?;
    let (base_args, rendered_prompt) =
        provider_policy_launch_parts(request.provider, request.provider_args, request.prompt)?;
    let mut cmd = build_command(
        request.provider,
        &base_args,
        request.working_dir,
        request.parent_invocation_env,
        return_channel
            .as_ref()
            .map(|channel| channel.path.as_path()),
    )?;
    append_command_args(&mut cmd, request.input_args);
    let (capture_plan, capture_args, mut temp_files) = capture::build_launch_capture_plan(
        request.provider.session_capture.as_ref(),
        request.start_known_provider_session_id,
    )?;
    append_command_args(&mut cmd, &capture_args);
    append_command_args(&mut cmd, request.tail_args);
    prompt::render_prompt_for_command(
        &mut cmd,
        request.prompt_mode,
        rendered_prompt.as_deref(),
        request.working_dir,
        &mut temp_files,
    )?;
    let supervisor_config = supervisor_config::supervisor_config_for_launch(
        request.provider,
        request.prompt_mode,
        rendered_prompt,
        supervisor_config,
    );

    Ok(ProviderLaunch {
        cmd,
        supervisor_config,
        capture_plan,
        return_channel,
        temp_files,
    })
}
