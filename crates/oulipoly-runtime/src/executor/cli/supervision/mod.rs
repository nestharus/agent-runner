//! ## Declared roles
//!
//! Roles: mapper, orchestration, predicate.
//!
//! - orchestration: runs the supervised child lifecycle from command setup
//!   through spawn, pipe drains, stdin writing, live signal recognition,
//!   termination, and output mapping.
//! - mapper: builds and updates supervisor configuration values.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs
//!     role: adapter
//!     Translates:
//!       - std-process-child-lifecycle-contract
//!       - std-io-pipe-drain-contract
//!       - unix-process-group-contract
//!       - terminal-signal-classification-contract
//!       - provider-live-terminal-signal-contract
//! ```

mod drain;
mod drain_access;
mod drain_chunks;
mod errors;
mod live_quota;
mod predicates;
mod process;
mod process_validate;
mod status;
mod stdin;
mod stdin_access;
mod stdin_predicates;
mod terminal_outcome;
mod termination;

use super::provider_identity::ProviderRecognizer;
use super::spawn_identity::{SpawnIdentityContext, record_child_identity};
use crate::executor::terminal_signal::{TerminalSignal, TerminalStatusEvidence};
use oulipoly_config::{PromptMode, ProviderConfig};
use std::process::{Command, ExitStatus};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const SUPERVISOR_POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone, Debug)]
pub(super) struct SupervisorConfig {
    pub(super) prompt_mode: PromptMode,
    pub(super) prompt_payload: Option<Vec<u8>>,
    pub(super) recognizer: ProviderRecognizer,
}

impl SupervisorConfig {
    pub(super) fn production(
        provider: &ProviderConfig,
        prompt_mode: PromptMode,
        prompt_payload: Vec<u8>,
    ) -> Self {
        Self {
            prompt_mode,
            prompt_payload: (prompt_mode == PromptMode::Stdin).then_some(prompt_payload),
            recognizer: ProviderRecognizer::for_provider(provider),
        }
    }

    pub(super) fn with_prompt_contract(
        mut self,
        prompt_mode: PromptMode,
        prompt_payload: Option<Vec<u8>>,
    ) -> Self {
        self.prompt_mode = prompt_mode;
        self.prompt_payload = prompt_payload;
        self
    }
}

pub(super) struct SupervisedOutput {
    pub(super) stdout: Vec<u8>,
    pub(super) stderr: Vec<u8>,
    pub(super) exit_code: i32,
    pub(super) terminal_reason: Option<String>,
    pub(super) terminal_signal: TerminalSignal,
}

#[derive(Clone, Copy)]
pub(super) enum DrainStream {
    Stdout,
    Stderr,
}

pub(super) type SupervisedTerminalOutcome = (
    TerminalStatusEvidence,
    Option<TerminalSignal>,
    Option<ExitStatus>,
);

pub(super) fn run_provider_supervisor(
    cmd: Command,
    provider: &ProviderConfig,
    supervisor_config: SupervisorConfig,
    spawn_identity: Option<&SpawnIdentityContext>,
) -> Result<SupervisedOutput, String> {
    execute_with_supervisor(cmd, &provider.name, supervisor_config, spawn_identity)
        .map_err(errors::supervisor_error_for_executor)
}

fn execute_with_supervisor(
    mut cmd: Command,
    provider_name: &str,
    mut config: SupervisorConfig,
    spawn_identity: Option<&SpawnIdentityContext>,
) -> Result<SupervisedOutput, String> {
    process::configure_supervised_command(&mut cmd, &config);
    process::configure_supervised_process_group(&mut cmd);
    let mut child = process::spawn_supervised_child(cmd, provider_name)?;
    record_child_identity(child.id(), spawn_identity);
    let drains = drain::start_child_drains(&mut child)?;
    let stdin_writer = stdin::start_child_stdin_writer(&mut child, &mut config)?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut last_output_seen = Instant::now();

    let (terminal_status, terminal_signal, real_status) = loop {
        drain::drain_output_events(&drains.rx, &mut stdout, &mut stderr, &mut last_output_seen);

        if let Some(status) = status::poll_child_status(&mut child)? {
            break terminal_outcome::terminal_outcome_from_status(status);
        }

        if let Some(outcome) = live_quota_terminal_outcome(
            &mut child,
            provider_name,
            config.recognizer,
            &stdout,
            &stderr,
        )? {
            break outcome;
        }

        match drains.rx.recv_timeout(SUPERVISOR_POLL_INTERVAL) {
            Ok((stream, chunk)) => drain_chunks::append_output_chunk(
                stream,
                chunk,
                &mut stdout,
                &mut stderr,
                &mut last_output_seen,
            ),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {}
        }
    };

    drain::finish_child_drains(drains, &mut stdout, &mut stderr, &mut last_output_seen);
    let stdin_write_error = stdin::finish_stdin_writer(stdin_writer);
    let output = terminal_outcome::supervised_output_from_terminal(
        provider_name,
        config.recognizer,
        stdout,
        stderr,
        terminal_status,
        terminal_signal,
        real_status,
    );
    if stdin_predicates::stdin_write_error_is_fatal(stdin_write_error.as_deref(), &output)
        && let Some(err) = stdin_write_error
    {
        return Err(err);
    }
    Ok(output)
}

fn live_quota_terminal_outcome(
    child: &mut std::process::Child,
    provider_name: &str,
    recognizer: ProviderRecognizer,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<Option<SupervisedTerminalOutcome>, String> {
    let live_signal =
        live_quota::recognize_live_terminal_signal(provider_name, recognizer, stdout, stderr);
    if !predicates::live_signal_is_quota_exhausted_inband(&live_signal) {
        return Ok(None);
    }
    live_quota::terminate_for_live_quota(
        child,
        provider_name,
        recognizer,
        stdout,
        stderr,
        live_signal,
    )
    .map(Some)
}
