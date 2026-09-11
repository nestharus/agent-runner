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
mod stdin;
mod stdin_access;
mod stdin_predicates;
mod terminal_outcome;

use super::provider_identity::ProviderRecognizer;
use super::session_capture::{CapturePlan, parse_stdout_json_event_session_id};
use super::spawn_identity::{
    ChildGenerationCustody, RunningRuntimeGeneration, SpawnIdentityContext,
    backfill_captured_session_id, child_custody_test_fault, mark_runtime_generation_spawn_failed,
    record_child_identity, register_runtime_generation_starting,
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
    crate::executor::cli::spawn_identity::configure_launch_custody(&mut cmd, spawn_identity)?;
    let child = match process::spawn_supervised_child(cmd, provider_name) {
        Ok(child) => child,
        Err(err) => {
            let _ = mark_runtime_generation_spawn_failed(spawn_identity);
            return Err(err);
        }
    };
    let mut custody = ChildGenerationCustody::new(child, spawn_identity)?;
    let recorded_generation = record_child_identity(custody.child().id(), spawn_identity)?;
    let drains = drain::start_child_drains(custody.child_mut())?;
    let stdin_writer = stdin::start_child_stdin_writer(custody.child_mut(), &mut config)?;
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

        child_custody_test_fault("headless_status_poll")?;
        if let Some(status) = custody
            .try_wait()
            .map_err(|error| errors::poll_child_status_error(&error))?
        {
            custody.observe_exit()?;
            break terminal_outcome::terminal_outcome_from_status(status);
        }

        child_custody_test_fault("headless_live_quota")?;
        if let Some(outcome) = live_quota_terminal_outcome(
            &mut custody,
            provider_name,
            config.recognizer,
            &stdout,
            &stderr,
        )? {
            if outcome.2.is_some() {
                custody.observe_exit()?;
            }
            break outcome;
        }

        match receive_with_poll_cadence(&drains.rx, SUPERVISOR_POLL_INTERVAL) {
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
    let compatibility_exit_code = if output.exit_code == 0 && output.terminal_reason.is_some() {
        1
    } else {
        output.exit_code
    };
    custody.complete_orderly(Some(output.exit_code), Some(compatibility_exit_code))?;
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
    custody: &mut ChildGenerationCustody<'_>,
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
    live_quota::terminate_for_live_quota(custody, live_signal).map(Some)
}

// EOF is not process/tree exit. Preserve polling cadence once both output
// senders have gone; recv_timeout alone returns immediately on disconnection.
fn receive_with_poll_cadence<T>(
    rx: &mpsc::Receiver<T>,
    interval: Duration,
) -> Result<T, mpsc::RecvTimeoutError> {
    let result = rx.recv_timeout(interval);
    if matches!(result, Err(mpsc::RecvTimeoutError::Disconnected)) {
        std::thread::sleep(interval);
    }
    result
}

#[cfg(test)]
mod eof_tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn output_eof_does_not_spin_while_proxy_waits_for_descendant() {
        let root = tempfile::tempdir().unwrap();
        let custody = oulipoly_core::launch_custody::LaunchCustody::start(root.path().join("proof")).unwrap();
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exec >/dev/null 2>&1; (n=0; while [ ! -f release ]; do n=$((n + 1)); [ $n -lt 500 ] || exit 75; /bin/sleep 0.01; done) & exit 7"])
            .current_dir(root.path()).env_clear()
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
        custody.configure(&mut command).unwrap();
        let mut child = command.spawn().unwrap();
        drop(command);
        let drains = drain::start_child_drains(&mut child).unwrap();
        assert!(matches!(drains.rx.recv_timeout(Duration::from_secs(2)), Err(mpsc::RecvTimeoutError::Disconnected)));
        assert!(child.try_wait().unwrap().is_none(), "proxy must still own a live descendant");
        let start = Instant::now();
        for _ in 0..3 {
            assert!(matches!(receive_with_poll_cadence(&drains.rx, SUPERVISOR_POLL_INTERVAL), Err(mpsc::RecvTimeoutError::Disconnected)));
        }
        assert!(start.elapsed() >= SUPERVISOR_POLL_INTERVAL * 3);
        std::fs::write(root.path().join("release"), b"").unwrap();
        assert_eq!(child.wait().unwrap().code(), Some(7));
        drain::finish_child_drains(drains, &mut Vec::new(), &mut Vec::new(), &mut Instant::now());
        custody.seal();
    }

    #[test]
    fn disconnected_output_retains_poll_cadence() {
        let (tx, rx) = mpsc::channel::<()>();
        drop(tx);
        let start = Instant::now();
        for _ in 0..3 {
            assert_eq!(receive_with_poll_cadence(&rx, SUPERVISOR_POLL_INTERVAL),
                Err(mpsc::RecvTimeoutError::Disconnected));
        }
        assert!(start.elapsed() >= SUPERVISOR_POLL_INTERVAL * 3);
    }
}
