//! ## Declared roles
//!
//! Roles: formatter.
//!
//! - formatter: renders prompt arguments, stdin posture, and large-prompt
//!   temporary file instructions for process launch.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/launch/prompt.rs
//!     role: adapter
//!     Translates:
//!       - std-process-command-contract
//!       - prompt-transport-contract
//!       - temp-prompt-file-contract
//! ```

use super::prompt_file::write_large_prompt_file;
use super::prompt_predicates::prompt_requires_temp_file;
use oulipoly_config::PromptMode;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub(super) fn render_prompt_for_command(
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
