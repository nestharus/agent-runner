//! ## Declared roles
//!
//! Roles: orchestration, validator, formatter, mapper.
//!
//! - orchestration: public interactive entrypoints sequence validation,
//!   provider policy, optional resume args, command build, stdio inheritance,
//!   direct spawn/wait, signal guard lifetime, and result mapping.
//! - validator: [`validated_interactive_args`] accepts provider configs with
//!   interactive args or returns validation failure.
//! - formatter: [`interactive_args_missing_error`] formats the stable
//!   validation error.
//! - mapper: [`interactive_result_from_status`] maps [`std::process::ExitStatus`]
//!   plus provider recognizer evidence into [`InteractiveExecutionResult`].
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/interactive.rs
//!     role: adapter
//!     Translates:
//!       - executor-public-interactive-entrypoint-contract
//!       - provider-interactive-launch-contract
//!       - terminal-status-to-interactive-result-contract
//!       - unix-interactive-signal-guard-callsite-contract
//! ```

use super::super::TerminalSignal;
use super::launch::build_command;
use super::policy::apply_provider_policy;
use super::provider_identity::ProviderRecognizer;
use super::resume::{ResumePayload, compose_resume_provider_args};
use super::spawn_identity::{
    SpawnRuntimeMode, context_from_parent_invocation_env, record_child_identity,
};
use super::terminal_signal;
use oulipoly_config::ProviderConfig;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};

pub fn execute_interactive(
    provider: &ProviderConfig,
    working_dir: Option<&Path>,
    parent_invocation_env: Option<&str>,
    resume: Option<ResumePayload<'_>>,
) -> Result<i32, String> {
    execute_interactive_with_result(provider, working_dir, parent_invocation_env, resume)
        .map(|result| result.exit_code)
}

#[derive(Debug, Clone, PartialEq)]
pub struct InteractiveExecutionResult {
    pub exit_code: i32,
    pub terminal_reason: Option<String>,
    pub terminal_signal: Option<TerminalSignal>,
}

pub fn execute_interactive_with_result(
    provider: &ProviderConfig,
    working_dir: Option<&Path>,
    parent_invocation_env: Option<&str>,
    resume: Option<ResumePayload<'_>>,
) -> Result<InteractiveExecutionResult, String> {
    execute_interactive_with_result_and_model_identity(
        provider,
        working_dir,
        parent_invocation_env,
        resume,
        None,
    )
}

pub fn execute_interactive_with_result_and_model_identity(
    provider: &ProviderConfig,
    working_dir: Option<&Path>,
    parent_invocation_env: Option<&str>,
    resume: Option<ResumePayload<'_>>,
    model_name: Option<&str>,
) -> Result<InteractiveExecutionResult, String> {
    let resume_session_id = resume.as_ref().map(|resume| resume.session_id);
    let provider_args = interactive_provider_args(provider, resume)?;
    let cmd = interactive_command(provider, &provider_args, working_dir, parent_invocation_env)?;
    let mut child = spawn_interactive_child(cmd, provider)?;

    #[cfg(unix)]
    let signal_guard = terminal_signal::InteractiveSignalGuard::install(&mut child)?;
    record_interactive_child_identity(
        child.id(),
        parent_invocation_env,
        provider,
        model_name,
        resume_session_id,
        working_dir,
    );

    let status = wait_for_interactive_child(&mut child)?;

    #[cfg(unix)]
    drop(signal_guard);

    Ok(interactive_result_from_status(provider, &status))
}

fn interactive_provider_args(
    provider: &ProviderConfig,
    resume: Option<ResumePayload<'_>>,
) -> Result<Vec<String>, String> {
    let mut provider_args = validated_interactive_args(provider)?;
    let mut no_prompt = None;
    apply_provider_policy(provider, &mut provider_args, &mut no_prompt)?;
    match resume {
        Some(resume) => compose_resume_provider_args(provider_args, resume),
        None => Ok(provider_args),
    }
}

fn interactive_command(
    provider: &ProviderConfig,
    provider_args: &[String],
    working_dir: Option<&Path>,
    parent_invocation_env: Option<&str>,
) -> Result<Command, String> {
    let mut cmd = build_command(
        provider,
        provider_args,
        working_dir,
        parent_invocation_env,
        None,
    )?;
    configure_interactive_stdio(&mut cmd);
    Ok(cmd)
}

fn configure_interactive_stdio(cmd: &mut Command) {
    cmd.stdin(Stdio::inherit());
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());
}

fn spawn_interactive_child(mut cmd: Command, provider: &ProviderConfig) -> Result<Child, String> {
    cmd.spawn()
        .map_err(|e| format!("Failed to spawn '{}': {e}", provider.command))
}

fn record_interactive_child_identity(
    child_id: u32,
    parent_invocation_env: Option<&str>,
    provider: &ProviderConfig,
    model_name: Option<&str>,
    resume_session_id: Option<&str>,
    working_dir: Option<&Path>,
) {
    let spawn_identity = context_from_parent_invocation_env(
        parent_invocation_env,
        &provider.name,
        model_name,
        resume_session_id,
        SpawnRuntimeMode::PtyInteractive,
        working_dir,
    );
    record_child_identity(child_id, spawn_identity.as_ref());
}

fn wait_for_interactive_child(child: &mut Child) -> Result<ExitStatus, String> {
    child
        .wait()
        .map_err(|e| format!("Failed to wait for process: {e}"))
}

fn validated_interactive_args(provider: &ProviderConfig) -> Result<Vec<String>, String> {
    provider
        .interactive_args
        .clone()
        .ok_or_else(|| interactive_args_missing_error(provider))
}

fn interactive_args_missing_error(provider: &ProviderConfig) -> String {
    format!(
        "provider {} has no interactive_args; cannot launch interactively",
        provider.name
    )
}

fn interactive_result_from_status(
    provider: &ProviderConfig,
    status: &ExitStatus,
) -> InteractiveExecutionResult {
    let terminal_reason = terminal_signal::classify_terminal_reason(status);
    let terminal_signal = terminal_signal::recognize_terminal_signal(
        &provider.name,
        ProviderRecognizer::for_provider(provider),
        &[],
        &[],
        terminal_signal::terminal_status_from_exit_status(status),
    );
    InteractiveExecutionResult {
        exit_code: terminal_signal::exit_code_from_status(status),
        terminal_reason,
        terminal_signal: Some(terminal_signal),
    }
}
