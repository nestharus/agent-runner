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
use super::live_session_binding::InteractiveLiveSessionBinding;
#[cfg(unix)]
use super::live_session_binding::LiveSessionBindingServer;
use super::policy::apply_provider_policy;
use super::provider_identity::ProviderRecognizer;
#[cfg(unix)]
use super::pty_broker;
use super::resume::{ResumePayload, compose_resume_provider_args};
use super::spawn_identity::{
    SpawnIdentityContext, SpawnRuntimeMode, context_from_parent_invocation_env,
    mark_runtime_generation_exited, mark_runtime_generation_orderly_completed,
    mark_runtime_generation_spawn_failed, record_child_identity,
    register_runtime_generation_starting,
};
use super::terminal_signal;
use crate::provider_registry::ProviderRegistry;
use oulipoly_config::{ModelConfig, ProviderConfig};
use std::io;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};

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
    pub live_session_id: Option<String>,
    pub live_session_capture_required: bool,
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

pub(crate) fn execute_interactive_with_result_and_state_db_path(
    provider: &ProviderConfig,
    working_dir: Option<&Path>,
    parent_invocation_env: Option<&str>,
    resume: Option<ResumePayload<'_>>,
    state_db_path: Option<&Path>,
    live_session_binding: Option<InteractiveLiveSessionBinding>,
) -> Result<InteractiveExecutionResult, String> {
    let provider_registry = live_session_binding
        .as_ref()
        .map(|binding| Arc::clone(&binding.registry));
    execute_interactive_with_result_and_monitor_context(
        provider,
        working_dir,
        parent_invocation_env,
        resume,
        InteractiveMonitorContext {
            model_name: None,
            provider_registry,
            live_session_binding,
            state_db_path,
        },
    )
}

pub fn execute_interactive_with_result_and_model_identity(
    provider: &ProviderConfig,
    working_dir: Option<&Path>,
    parent_invocation_env: Option<&str>,
    resume: Option<ResumePayload<'_>>,
    model_name: Option<&str>,
) -> Result<InteractiveExecutionResult, String> {
    execute_interactive_with_result_and_monitor_context(
        provider,
        working_dir,
        parent_invocation_env,
        resume,
        InteractiveMonitorContext {
            model_name,
            provider_registry: None,
            live_session_binding: None,
            state_db_path: None,
        },
    )
}

pub fn execute_interactive_with_result_and_model_config(
    provider: &ProviderConfig,
    working_dir: Option<&Path>,
    parent_invocation_env: Option<&str>,
    resume: Option<ResumePayload<'_>>,
    model: &ModelConfig,
    provider_registry: Arc<ProviderRegistry>,
) -> Result<InteractiveExecutionResult, String> {
    execute_interactive_with_result_and_monitor_context(
        provider,
        working_dir,
        parent_invocation_env,
        resume,
        InteractiveMonitorContext {
            model_name: Some(&model.name),
            provider_registry: Some(provider_registry),
            live_session_binding: None,
            state_db_path: None,
        },
    )
}

struct InteractiveMonitorContext<'a> {
    model_name: Option<&'a str>,
    provider_registry: Option<Arc<ProviderRegistry>>,
    live_session_binding: Option<InteractiveLiveSessionBinding>,
    state_db_path: Option<&'a Path>,
}

fn execute_interactive_with_result_and_monitor_context(
    provider: &ProviderConfig,
    working_dir: Option<&Path>,
    parent_invocation_env: Option<&str>,
    resume: Option<ResumePayload<'_>>,
    monitor: InteractiveMonitorContext<'_>,
) -> Result<InteractiveExecutionResult, String> {
    let resume_session_id = resume.as_ref().map(|resume| resume.session_id);
    let provider_args = interactive_provider_args(provider, resume)?;
    let spawn_identity = interactive_spawn_identity_context(
        parent_invocation_env,
        provider,
        monitor.model_name,
        resume_session_id,
        working_dir,
        monitor.state_db_path,
    )?;
    #[cfg(unix)]
    let mut live_session_server = monitor
        .live_session_binding
        .map(LiveSessionBindingServer::bind)
        .transpose()?;
    #[cfg(unix)]
    let live_session_capture_required = live_session_server.is_some();
    #[cfg(not(unix))]
    let live_session_capture_required = false;
    #[cfg(unix)]
    let live_session_state = live_session_server
        .as_ref()
        .map(LiveSessionBindingServer::session_state);
    #[cfg(not(unix))]
    let live_session_state: Option<Arc<Mutex<Option<String>>>> = None;
    #[cfg(unix)]
    if pty_broker::controlling_terminal_available() {
        let mut cmd =
            interactive_command(provider, &provider_args, working_dir, parent_invocation_env)?;
        if let Some(server) = live_session_server.as_ref() {
            server.configure_command(&mut cmd);
        }
        let provider_inspect = monitor
            .provider_registry
            .map(pty_broker::ProviderInspectMonitorContext::new);
        let status = if pty_broker::observed_tui_enabled() {
            pty_broker::execute_interactive_child_observed(
                cmd,
                provider,
                spawn_identity.as_ref(),
                provider_inspect.as_ref(),
                live_session_server,
            )?
        } else {
            pty_broker::execute_interactive_child(
                cmd,
                provider,
                spawn_identity.as_ref(),
                live_session_server,
            )?
        };
        return Ok(interactive_result_from_status(
            provider,
            &status,
            live_session_capture_required,
            live_session_state.as_ref(),
        ));
    }

    let mut cmd =
        interactive_command(provider, &provider_args, working_dir, parent_invocation_env)?;
    #[cfg(unix)]
    if let Some(server) = live_session_server.as_ref() {
        server.configure_command(&mut cmd);
    }
    configure_interactive_stdio(&mut cmd);
    register_runtime_generation_starting(spawn_identity.as_ref())?;
    let mut child = match spawn_interactive_child(cmd, provider) {
        Ok(child) => child,
        Err(err) => {
            let _ = mark_runtime_generation_spawn_failed(spawn_identity.as_ref());
            return Err(err);
        }
    };

    #[cfg(unix)]
    let signal_guard = terminal_signal::InteractiveSignalGuard::install(&mut child)?;
    let generation = match record_child_identity(child.id(), spawn_identity.as_ref()) {
        Ok(generation) => generation,
        Err(err) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = mark_runtime_generation_exited(spawn_identity.as_ref(), None);
            return Err(err);
        }
    };
    #[cfg(unix)]
    if let (Some(server), Some(context), Some(generation)) = (
        live_session_server.as_mut(),
        spawn_identity.as_ref(),
        generation,
    ) && let Err(err) = server.start(context.clone(), generation)
    {
        let _ = child.kill();
        let _ = child.wait();
        let _ = mark_runtime_generation_exited(spawn_identity.as_ref(), None);
        return Err(err);
    }

    let status = wait_for_interactive_child(&mut child)?;
    mark_runtime_generation_orderly_completed(
        spawn_identity.as_ref(),
        Some(crate::executor::cli::terminal_signal::exit_code_from_status(&status)),
    )?;

    #[cfg(unix)]
    drop(signal_guard);

    Ok(interactive_result_from_status(
        provider,
        &status,
        live_session_capture_required,
        live_session_state.as_ref(),
    ))
}

fn interactive_provider_args(
    provider: &ProviderConfig,
    resume: Option<ResumePayload<'_>>,
) -> Result<Vec<String>, String> {
    let mut provider_args = validated_interactive_args(provider)?;
    let mut no_prompt = None;
    apply_provider_policy(provider, &mut provider_args, &mut no_prompt)?;
    match resume {
        Some(resume) => compose_resume_provider_args(provider, provider_args, resume),
        None => Ok(provider_args),
    }
}

fn interactive_command(
    provider: &ProviderConfig,
    provider_args: &[String],
    working_dir: Option<&Path>,
    parent_invocation_env: Option<&str>,
) -> Result<Command, String> {
    build_command(
        provider,
        provider_args,
        working_dir,
        parent_invocation_env,
        None,
    )
}

fn configure_interactive_stdio(cmd: &mut Command) {
    cmd.stdin(Stdio::inherit());
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());
}

fn spawn_interactive_child(mut cmd: Command, provider: &ProviderConfig) -> Result<Child, String> {
    cmd.spawn()
        .map_err(|err| format_spawn_interactive_child_error(&provider.command, err))
}

fn format_spawn_interactive_child_error(command: &str, err: io::Error) -> String {
    format!("Failed to spawn '{command}': {err}")
}

fn interactive_spawn_identity_context(
    parent_invocation_env: Option<&str>,
    provider: &ProviderConfig,
    model_name: Option<&str>,
    resume_session_id: Option<&str>,
    working_dir: Option<&Path>,
    state_db_path: Option<&Path>,
) -> Result<Option<SpawnIdentityContext>, String> {
    let mailbox_db_path = state_db_path.map(oulipoly_state::mailbox::MailboxDb::path_for_state_db);
    let context = context_from_parent_invocation_env(
        parent_invocation_env,
        &provider.name,
        model_name,
        resume_session_id,
        SpawnRuntimeMode::PtyInteractive,
        working_dir,
        None,
    );
    Ok(context.map(|context| match mailbox_db_path {
        Some(path) => context.with_mailbox_db_path(path),
        None => context,
    }))
}

fn wait_for_interactive_child(child: &mut Child) -> Result<ExitStatus, String> {
    child.wait().map_err(format_interactive_wait_error)
}

fn format_interactive_wait_error(err: io::Error) -> String {
    format!("Failed to wait for process: {err}")
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
    live_session_capture_required: bool,
    live_session_state: Option<&Arc<Mutex<Option<String>>>>,
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
        live_session_id: live_session_state
            .and_then(|state| state.lock().ok())
            .and_then(|session_id| session_id.clone()),
        live_session_capture_required,
    }
}
