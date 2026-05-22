//! ## Declared roles
//!
//! Roles: orchestration, parser, validator, formatter, accessor, mapper,
//! predicate.
//!
//! - orchestration: [`assemble_provider_launch`] composes return-channel
//!   preparation, policy emission, command construction, capture-plan
//!   building, prompt rendering, and supervisor-config selection.
//! - parser: [`parse_command_parts`] reuses `shell_split` to tokenize the
//!   provider command string.
//! - validator: [`validate_command_parts`] checks for an empty command.
//! - formatter: [`command_from_parts`] writes the cwd/env/argv onto the
//!   constructed `Command`; [`append_command_args`]; [`render_arg_prompt`];
//!   [`write_large_prompt_file`].
//! - accessor: [`build_command`] returns a configured `Command` ready for
//!   the supervisor.
//! - mapper: [`supervisor_config_for_launch`] maps launch inputs onto a
//!   [`SupervisorConfig`].
//! - predicate: [`prompt_requires_temp_file`].
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/launch.rs
//!     role: adapter
//!     Translates:
//!       - std-process-command-contract
//!       - provider-config-launch-contract
//!       - cli-ipc-return-channel-contract
//!       - provider-policy-launch-contract
//!       - session-capture-plan-contract
//! ```

use super::ipc::{ReturnChannel, prepare_return_channel};
use super::policy::provider_policy_launch_parts;
use super::provider_identity::shell_split;
use super::session_capture::{CapturePlan, build_capture_plan};
use super::supervision::SupervisorConfig;
use oulipoly_config::{PromptMode, ProviderConfig};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub(super) const LARGE_PROMPT_THRESHOLD: usize = 100 * 1024;

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
    let (capture_plan, capture_args, mut temp_files) = build_capture_plan(
        request.provider.session_capture.as_ref(),
        request.start_known_provider_session_id,
    )?;
    append_command_args(&mut cmd, &capture_args);
    append_command_args(&mut cmd, request.tail_args);
    render_prompt_for_command(
        &mut cmd,
        request.prompt_mode,
        rendered_prompt.as_deref(),
        request.working_dir,
        &mut temp_files,
    )?;
    let supervisor_config = supervisor_config_for_launch(
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

pub(super) fn build_command(
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
        working_dir,
        parent_invocation_env,
        return_channel,
    ))
}

fn parse_command_parts(command: &str) -> Vec<String> {
    shell_split(command)
}

fn validate_command_parts(parts: &[String]) -> Result<(), String> {
    if parts.is_empty() {
        return Err("Empty command".to_string());
    }
    Ok(())
}

fn command_from_parts(
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

pub(super) fn append_command_args(cmd: &mut Command, args: &[String]) {
    for arg in args {
        cmd.arg(arg);
    }
}

fn render_prompt_for_command(
    cmd: &mut Command,
    prompt_mode: PromptMode,
    rendered_prompt: Option<&str>,
    working_dir: Option<&Path>,
    temp_files: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let Some(rendered_prompt) = rendered_prompt else {
        cmd.stdin(Stdio::null());
        return Ok(());
    };
    match prompt_mode {
        PromptMode::Arg => render_arg_prompt(cmd, rendered_prompt, working_dir, temp_files),
        PromptMode::Stdin => {
            cmd.stdin(Stdio::piped());
            Ok(())
        }
    }
}

fn render_arg_prompt(
    cmd: &mut Command,
    rendered_prompt: &str,
    working_dir: Option<&Path>,
    temp_files: &mut Vec<PathBuf>,
) -> Result<(), String> {
    if prompt_requires_temp_file(rendered_prompt) {
        render_large_arg_prompt(cmd, rendered_prompt, working_dir, temp_files)?;
    } else {
        render_inline_arg_prompt(cmd, rendered_prompt);
    }
    set_arg_prompt_stdin(cmd);
    Ok(())
}

fn prompt_requires_temp_file(rendered_prompt: &str) -> bool {
    rendered_prompt.len() > LARGE_PROMPT_THRESHOLD
}

fn render_large_arg_prompt(
    cmd: &mut Command,
    rendered_prompt: &str,
    working_dir: Option<&Path>,
    temp_files: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let (path, instruction) = write_large_prompt_file(rendered_prompt, working_dir)?;
    cmd.arg(instruction);
    temp_files.push(path);
    Ok(())
}

fn render_inline_arg_prompt(cmd: &mut Command, rendered_prompt: &str) {
    cmd.arg(rendered_prompt);
}

fn set_arg_prompt_stdin(cmd: &mut Command) {
    cmd.stdin(Stdio::null());
}

fn write_large_prompt_file(
    rendered_prompt: &str,
    working_dir: Option<&Path>,
) -> Result<(PathBuf, String), String> {
    let dir = working_dir.unwrap_or(Path::new("."));
    let filename = format!("_agent_prompt_{}.md", uuid::Uuid::new_v4());
    let path = dir.join(&filename);
    std::fs::write(&path, rendered_prompt)
        .map_err(|e| format!("Failed to write temp prompt file: {e}"))?;
    Ok((path, format!("Follow the instructions in {filename}")))
}

fn supervisor_config_for_launch(
    provider: &ProviderConfig,
    prompt_mode: PromptMode,
    rendered_prompt: Option<String>,
    supervisor_config: Option<SupervisorConfig>,
) -> SupervisorConfig {
    let prompt_payload = rendered_prompt
        .and_then(|prompt| (prompt_mode == PromptMode::Stdin).then(|| prompt.into_bytes()));
    supervisor_config
        .unwrap_or_else(|| {
            SupervisorConfig::production(
                provider,
                prompt_mode,
                prompt_payload.clone().unwrap_or_default(),
            )
        })
        .with_prompt_contract(prompt_mode, prompt_payload)
}
