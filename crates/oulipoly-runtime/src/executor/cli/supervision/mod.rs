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
use super::session_capture::{CapturePlan, parse_stdout_json_event_session_id};
use super::spawn_identity::{
    RunningRuntimeGeneration, SpawnIdentityContext, backfill_captured_session_id,
    mark_runtime_generation_exited, mark_runtime_generation_orderly_completed,
    mark_runtime_generation_spawn_failed, record_child_identity,
    register_runtime_generation_starting,
};
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
    pub(super) streamed_session_id: Option<String>,
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
    capture_plan: &CapturePlan,
) -> Result<SupervisedOutput, String> {
    execute_with_supervisor(
        cmd,
        &provider.name,
        supervisor_config,
        spawn_identity,
        capture_plan,
    )
    .map_err(errors::supervisor_error_for_executor)
}

fn execute_with_supervisor(
    mut cmd: Command,
    provider_name: &str,
    mut config: SupervisorConfig,
    spawn_identity: Option<&SpawnIdentityContext>,
    capture_plan: &CapturePlan,
) -> Result<SupervisedOutput, String> {
    process::configure_supervised_command(&mut cmd, &config);
    process::configure_supervised_process_group(&mut cmd);
    register_runtime_generation_starting(spawn_identity)?;
    let mut child = match process::spawn_supervised_child(cmd, provider_name) {
        Ok(child) => child,
        Err(err) => {
            let _ = mark_runtime_generation_spawn_failed(spawn_identity);
            return Err(err);
        }
    };
    let recorded_generation = match record_child_identity(child.id(), spawn_identity) {
        Ok(generation) => generation,
        Err(err) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = mark_runtime_generation_exited(spawn_identity, None);
            return Err(err);
        }
    };
    let drains = drain::start_child_drains(&mut child)?;
    let stdin_writer = stdin::start_child_stdin_writer(&mut child, &mut config)?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut streamed_session_id = None;
    let mut last_output_seen = Instant::now();

    let (terminal_status, terminal_signal, real_status) = loop {
        drain::drain_output_events(&drains.rx, &mut stdout, &mut stderr, &mut last_output_seen);
        observe_streamed_session_id(
            capture_plan,
            &stdout,
            &mut streamed_session_id,
            spawn_identity,
            recorded_generation.as_ref(),
        );

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
        observe_streamed_session_id(
            capture_plan,
            &stdout,
            &mut streamed_session_id,
            spawn_identity,
            recorded_generation.as_ref(),
        );
    };

    drain::finish_child_drains(drains, &mut stdout, &mut stderr, &mut last_output_seen);
    observe_streamed_session_id(
        capture_plan,
        &stdout,
        &mut streamed_session_id,
        spawn_identity,
        recorded_generation.as_ref(),
    );
    let stdin_write_error = stdin::finish_stdin_writer(stdin_writer);
    let mut output = terminal_outcome::supervised_output_from_terminal(
        provider_name,
        config.recognizer,
        stdout,
        stderr,
        terminal_status,
        terminal_signal,
        real_status,
    );
    output.streamed_session_id = streamed_session_id;
    mark_runtime_generation_orderly_completed(spawn_identity, Some(output.exit_code))?;
    if stdin_predicates::stdin_write_error_is_fatal(stdin_write_error.as_deref(), &output)
        && let Some(err) = stdin_write_error
    {
        return Err(err);
    }
    Ok(output)
}

fn observe_streamed_session_id(
    capture_plan: &CapturePlan,
    stdout: &[u8],
    streamed_session_id: &mut Option<String>,
    spawn_identity: Option<&SpawnIdentityContext>,
    recorded_generation: Option<&RunningRuntimeGeneration>,
) {
    if streamed_session_id.is_some() {
        return;
    }
    let CapturePlan::StdoutJsonEvent {
        event_type,
        event_id_path,
        ..
    } = capture_plan
    else {
        return;
    };
    if let Ok(session_id) = parse_stdout_json_event_session_id(stdout, event_type, event_id_path)
        && backfill_captured_session_id(spawn_identity, recorded_generation, &session_id).is_ok()
    {
        *streamed_session_id = Some(session_id);
    }
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
