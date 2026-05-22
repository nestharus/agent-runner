//! ## Declared roles
//!
//! Roles: orchestration, validator, formatter, mapper, predicate, filter,
//! accessor.
//!
//! - orchestration: [`execute_with_supervisor`] runs the spawn / drain /
//!   poll / quota-recognition loop.
//! - orchestration: [`run_provider_supervisor`] wraps the supervisor loop
//!   with the executor-facing error shape.
//! - validator: [`spawn_supervised_child`] returns canonical
//!   `"Failed to spawn"` errors.
//! - formatter: [`write_prompt_payload`], [`write_stdin_error`].
//! - mapper: [`terminal_outcome_from_status`],
//!   [`supervised_output_from_terminal`].
//! - predicate: [`supervised_stdin_write_needed`],
//!   [`output_chunk_is_empty`], [`terminate_grace_period_elapsed`].
//! - filter: [`append_output_chunk`] (skips empty chunks).
//! - accessor: [`take_child_stdout`], [`take_child_stderr`],
//!   [`poll_child_status`].
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/supervision.rs
//!     role: adapter
//!     Translates:
//!       - std-process-child-lifecycle-contract
//!       - std-io-pipe-drain-contract
//!       - unix-process-group-contract
//!       - terminal-signal-classification-contract
//!       - provider-live-terminal-signal-contract
//! ```

use super::provider_identity::ProviderRecognizer;
use super::terminal_signal::{
    exit_code_from_status, recognize_terminal_signal, synthetic_exit_code,
    terminal_reason_from_signal, terminal_status_from_exit_status,
};
use crate::executor::terminal_signal::{
    TerminalSignal, TerminalSignalKind, TerminalStatusEvidence,
};
use oulipoly_config::{PromptMode, ProviderConfig};
use std::io::{Read, Write};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const STDIN_WRITE_CHUNK_SIZE: usize = 16 * 1024;
const SUPERVISOR_POLL_INTERVAL: Duration = Duration::from_millis(50);
const TERMINATE_GRACE_PERIOD: Duration = Duration::from_millis(250);

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
enum DrainStream {
    Stdout,
    Stderr,
}

type SupervisedTerminalOutcome = (
    TerminalStatusEvidence,
    Option<TerminalSignal>,
    Option<ExitStatus>,
);

pub(super) fn run_provider_supervisor(
    cmd: Command,
    provider: &ProviderConfig,
    supervisor_config: SupervisorConfig,
) -> Result<SupervisedOutput, String> {
    execute_with_supervisor(cmd, &provider.name, supervisor_config)
        .map_err(supervisor_error_for_executor)
}

fn supervisor_error_for_executor(err: String) -> String {
    if supervisor_error_is_spawn_error(&err) {
        err
    } else {
        wait_error_for_executor(&err)
    }
}

fn supervisor_error_is_spawn_error(err: &str) -> bool {
    err.starts_with("Failed to spawn")
}

fn live_signal_is_quota_exhausted_inband(signal: &TerminalSignal) -> bool {
    signal.kind == TerminalSignalKind::QuotaExhaustedInband
}

fn wait_error_for_executor(err: &str) -> String {
    format!("Failed to wait for process: {err}")
}

fn execute_with_supervisor(
    mut cmd: Command,
    provider_name: &str,
    mut config: SupervisorConfig,
) -> Result<SupervisedOutput, String> {
    configure_supervised_command(&mut cmd, &config);
    configure_supervised_process_group(&mut cmd);
    let mut child = spawn_supervised_child(cmd, provider_name)?;
    let drains = start_child_drains(&mut child)?;
    let stdin_writer = start_child_stdin_writer(&mut child, &mut config)?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut last_output_seen = Instant::now();

    let (terminal_status, terminal_signal, real_status) = loop {
        drain_output_events(&drains.rx, &mut stdout, &mut stderr, &mut last_output_seen);

        if let Some(status) = poll_child_status(&mut child)? {
            break terminal_outcome_from_status(status);
        }

        let live_signal =
            recognize_live_terminal_signal(provider_name, config.recognizer, &stdout, &stderr);
        if live_signal_is_quota_exhausted_inband(&live_signal) {
            break terminate_for_live_quota(
                &mut child,
                provider_name,
                config.recognizer,
                &stdout,
                &stderr,
                live_signal,
            )?;
        }

        match drains.rx.recv_timeout(SUPERVISOR_POLL_INTERVAL) {
            Ok((stream, chunk)) => append_output_chunk(
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

    finish_child_drains(drains, &mut stdout, &mut stderr, &mut last_output_seen);
    let stdin_write_error = finish_stdin_writer(stdin_writer);
    let output = supervised_output_from_terminal(
        provider_name,
        config.recognizer,
        stdout,
        stderr,
        terminal_status,
        terminal_signal,
        real_status,
    );
    if stdin_write_error_is_fatal(stdin_write_error.as_deref(), &output)
        && let Some(err) = stdin_write_error
    {
        return Err(err);
    }
    Ok(output)
}

struct ChildDrains {
    rx: mpsc::Receiver<(DrainStream, Vec<u8>)>,
    stdout_handle: thread::JoinHandle<()>,
    stderr_handle: thread::JoinHandle<()>,
}

struct StdinWriter {
    handle: thread::JoinHandle<Result<(), String>>,
}

fn configure_supervised_command(cmd: &mut Command, config: &SupervisorConfig) {
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    if supervised_stdin_write_needed(config) {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }
}

#[cfg(target_os = "linux")]
fn configure_supervised_process_group(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;

    let parent_pid = unsafe { libc::getpid() };
    cmd.process_group(0);
    unsafe {
        cmd.pre_exec(move || validate_child_parent_after_process_group_setup(parent_pid));
    }
}

#[cfg(target_os = "linux")]
fn validate_child_parent_after_process_group_setup(parent_pid: libc::pid_t) -> std::io::Result<()> {
    install_parent_death_signal()?;
    validate_child_parent_pid(parent_pid)
}

#[cfg(target_os = "linux")]
fn install_parent_death_signal() -> std::io::Result<()> {
    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) } == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn validate_child_parent_pid(parent_pid: libc::pid_t) -> std::io::Result<()> {
    if unsafe { libc::getppid() } != parent_pid {
        Err(std::io::Error::from_raw_os_error(libc::ESRCH))
    } else {
        Ok(())
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
fn configure_supervised_process_group(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;

    cmd.process_group(0);
}

#[cfg(not(unix))]
fn configure_supervised_process_group(_cmd: &mut Command) {}

fn spawn_supervised_child(mut cmd: Command, provider_name: &str) -> Result<Child, String> {
    cmd.spawn()
        .map_err(|err| format!("Failed to spawn '{provider_name}': {err}"))
}

fn start_child_stdin_writer(
    child: &mut Child,
    config: &mut SupervisorConfig,
) -> Result<Option<StdinWriter>, String> {
    if !supervised_stdin_write_needed(config) {
        return Ok(None);
    }
    let Some(payload) = take_supervised_stdin_payload(config) else {
        return Ok(None);
    };
    let mut stdin = take_child_stdin(child)?;
    let handle = thread::spawn(move || {
        write_prompt_payload(&mut stdin, &payload).map_err(|err| write_stdin_error(&err))
    });
    Ok(Some(StdinWriter { handle }))
}

fn take_supervised_stdin_payload(config: &mut SupervisorConfig) -> Option<Vec<u8>> {
    config.prompt_payload.take()
}

fn take_child_stdin(child: &mut Child) -> Result<impl Write + Send + 'static, String> {
    child
        .stdin
        .take()
        .ok_or_else(|| "Child stdin was not piped".to_string())
}

fn supervised_stdin_write_needed(config: &SupervisorConfig) -> bool {
    config.prompt_mode == PromptMode::Stdin && config.prompt_payload.is_some()
}

fn write_prompt_payload<W: Write>(stdin: &mut W, payload: &[u8]) -> std::io::Result<()> {
    for chunk in payload.chunks(STDIN_WRITE_CHUNK_SIZE) {
        stdin.write_all(chunk)?;
    }
    stdin.flush()
}

fn write_stdin_error(err: &std::io::Error) -> String {
    format!("Failed to write to stdin: {err}")
}

fn finish_stdin_writer(stdin_writer: Option<StdinWriter>) -> Option<String> {
    let writer = stdin_writer?;
    match writer.handle.join() {
        Ok(Ok(())) => None,
        Ok(Err(err)) => Some(err),
        Err(_) => Some("Failed to write to stdin: writer thread panicked".to_string()),
    }
}

fn stdin_write_error_is_fatal(err: Option<&str>, output: &SupervisedOutput) -> bool {
    err.is_some() && output.terminal_signal.kind == TerminalSignalKind::CleanExit
}

fn start_child_drains(child: &mut Child) -> Result<ChildDrains, String> {
    let stdout = take_child_stdout(child)?;
    let stderr = take_child_stderr(child)?;
    let (tx, rx) = mpsc::channel();
    let stdout_handle = spawn_drain_thread(stdout, DrainStream::Stdout, tx.clone());
    let stderr_handle = spawn_drain_thread(stderr, DrainStream::Stderr, tx.clone());
    drop(tx);
    Ok(ChildDrains {
        rx,
        stdout_handle,
        stderr_handle,
    })
}

fn take_child_stdout(child: &mut Child) -> Result<impl Read + Send + 'static, String> {
    child
        .stdout
        .take()
        .ok_or_else(|| "Child stdout was not piped".to_string())
}

fn take_child_stderr(child: &mut Child) -> Result<impl Read + Send + 'static, String> {
    child
        .stderr
        .take()
        .ok_or_else(|| "Child stderr was not piped".to_string())
}

fn poll_child_status(child: &mut Child) -> Result<Option<ExitStatus>, String> {
    child
        .try_wait()
        .map_err(|err| format!("try_wait failed: {err}"))
}

fn terminal_outcome_from_status(status: ExitStatus) -> SupervisedTerminalOutcome {
    (
        terminal_status_from_exit_status(&status),
        None,
        Some(status),
    )
}

fn recognize_live_terminal_signal(
    provider_name: &str,
    recognizer: ProviderRecognizer,
    stdout: &[u8],
    stderr: &[u8],
) -> TerminalSignal {
    recognize_terminal_signal(
        provider_name,
        recognizer,
        stdout,
        stderr,
        TerminalStatusEvidence::Unknown,
    )
}

fn terminate_for_live_quota(
    child: &mut Child,
    provider_name: &str,
    recognizer: ProviderRecognizer,
    stdout: &[u8],
    stderr: &[u8],
    live_signal: TerminalSignal,
) -> Result<SupervisedTerminalOutcome, String> {
    if let Some(status) = try_wait_before_live_quota_terminate(child)? {
        Ok(live_quota_status_outcome(
            provider_name,
            recognizer,
            stdout,
            stderr,
            status,
        ))
    } else if let Some(status) = wait_for_child_after_live_quota(child)? {
        Ok(live_quota_status_outcome(
            provider_name,
            recognizer,
            stdout,
            stderr,
            status,
        ))
    } else {
        live_quota_termination_outcome(child, live_signal)
    }
}

fn try_wait_before_live_quota_terminate(child: &mut Child) -> Result<Option<ExitStatus>, String> {
    child
        .try_wait()
        .map_err(|err| format!("try_wait before quota terminate failed: {err}"))
}

fn live_quota_status_outcome(
    provider_name: &str,
    recognizer: ProviderRecognizer,
    stdout: &[u8],
    stderr: &[u8],
    status: ExitStatus,
) -> SupervisedTerminalOutcome {
    let terminal_status = terminal_status_from_exit_status(&status);
    let terminal_signal = recognize_terminal_signal(
        provider_name,
        recognizer,
        stdout,
        stderr,
        terminal_status.clone(),
    );
    (terminal_status, Some(terminal_signal), Some(status))
}

fn live_quota_termination_outcome(
    child: &mut Child,
    live_signal: TerminalSignal,
) -> Result<SupervisedTerminalOutcome, String> {
    Ok((
        TerminalStatusEvidence::Unknown,
        Some(live_signal),
        terminate_child(child)?,
    ))
}

fn wait_for_child_after_live_quota(child: &mut Child) -> Result<Option<ExitStatus>, String> {
    wait_for_child_until_termination_grace(child, "try_wait after live quota failed")
}

fn finish_child_drains(
    drains: ChildDrains,
    stdout: &mut Vec<u8>,
    stderr: &mut Vec<u8>,
    last_output_seen: &mut Instant,
) {
    let _ = drains.stdout_handle.join();
    let _ = drains.stderr_handle.join();
    drain_output_events(&drains.rx, stdout, stderr, last_output_seen);
}

fn supervised_output_from_terminal(
    provider_name: &str,
    recognizer: ProviderRecognizer,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    terminal_status: TerminalStatusEvidence,
    terminal_signal: Option<TerminalSignal>,
    real_status: Option<ExitStatus>,
) -> SupervisedOutput {
    let terminal_signal = terminal_signal.unwrap_or_else(|| {
        recognize_terminal_signal(
            provider_name,
            recognizer,
            &stdout,
            &stderr,
            terminal_status.clone(),
        )
    });
    let exit_code = real_status
        .as_ref()
        .map(exit_code_from_status)
        .unwrap_or_else(|| synthetic_exit_code(&terminal_signal));
    let terminal_reason = terminal_reason_from_signal(&terminal_signal, real_status.as_ref());

    SupervisedOutput {
        stdout,
        stderr,
        exit_code,
        terminal_reason,
        terminal_signal,
    }
}

fn spawn_drain_thread<R>(
    mut reader: R,
    stream: DrainStream,
    sender: mpsc::Sender<(DrainStream, Vec<u8>)>,
) -> thread::JoinHandle<()>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        while let Some(chunk) = read_drain_chunk(&mut reader, &mut buffer) {
            if send_drain_chunk(&sender, stream, chunk).is_err() {
                break;
            }
        }
    })
}

fn read_drain_chunk<R: Read>(reader: &mut R, buffer: &mut [u8]) -> Option<Vec<u8>> {
    let count = read_drain_count(reader, buffer)?;
    Some(drain_chunk_from_count(buffer, count))
}

fn read_drain_count<R: Read>(reader: &mut R, buffer: &mut [u8]) -> Option<usize> {
    match reader.read(buffer) {
        Ok(0) | Err(_) => None,
        Ok(n) => Some(n),
    }
}

fn drain_chunk_from_count(buffer: &[u8], count: usize) -> Vec<u8> {
    buffer[..count].to_vec()
}

fn send_drain_chunk(
    sender: &mpsc::Sender<(DrainStream, Vec<u8>)>,
    stream: DrainStream,
    chunk: Vec<u8>,
) -> Result<(), mpsc::SendError<(DrainStream, Vec<u8>)>> {
    sender.send((stream, chunk))
}

fn drain_output_events(
    rx: &mpsc::Receiver<(DrainStream, Vec<u8>)>,
    stdout: &mut Vec<u8>,
    stderr: &mut Vec<u8>,
    last_output_seen: &mut Instant,
) {
    while let Ok((stream, chunk)) = rx.try_recv() {
        append_output_chunk(stream, chunk, stdout, stderr, last_output_seen);
    }
}

fn append_output_chunk(
    stream: DrainStream,
    chunk: Vec<u8>,
    stdout: &mut Vec<u8>,
    stderr: &mut Vec<u8>,
    last_output_seen: &mut Instant,
) {
    if output_chunk_is_empty(&chunk) {
        return;
    }
    append_non_empty_output_chunk(stream, &chunk, stdout, stderr);
    record_output_seen(last_output_seen);
}

fn output_chunk_is_empty(chunk: &[u8]) -> bool {
    chunk.is_empty()
}

fn append_non_empty_output_chunk(
    stream: DrainStream,
    chunk: &[u8],
    stdout: &mut Vec<u8>,
    stderr: &mut Vec<u8>,
) {
    match stream {
        DrainStream::Stdout => stdout.extend_from_slice(chunk),
        DrainStream::Stderr => stderr.extend_from_slice(chunk),
    }
}

fn record_output_seen(last_output_seen: &mut Instant) {
    *last_output_seen = Instant::now();
}

fn terminate_child(child: &mut Child) -> Result<Option<ExitStatus>, String> {
    if let Some(status) = try_wait_before_terminate(child)? {
        return Ok(mapped_child_status(status));
    }

    send_child_sigterm(child);
    if let Some(status) = wait_for_child_after_sigterm(child)? {
        return Ok(mapped_child_status(status));
    }

    send_child_sigkill(child)?;
    reap_child_after_kill(child)
}

fn try_wait_before_terminate(child: &mut Child) -> Result<Option<ExitStatus>, String> {
    child
        .try_wait()
        .map_err(|err| format!("try_wait before terminate failed: {err}"))
}

fn wait_for_child_after_sigterm(child: &mut Child) -> Result<Option<ExitStatus>, String> {
    wait_for_child_until_termination_grace(child, "try_wait after terminate failed")
}

fn wait_for_child_until_termination_grace(
    child: &mut Child,
    try_wait_context: &str,
) -> Result<Option<ExitStatus>, String> {
    let started = Instant::now();
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|err| format!("{try_wait_context}: {err}"))?
        {
            return Ok(Some(status));
        }
        if terminate_grace_period_elapsed(started) {
            return Ok(None);
        }
        thread::sleep(SUPERVISOR_POLL_INTERVAL);
    }
}

fn terminate_grace_period_elapsed(started: Instant) -> bool {
    started.elapsed() >= TERMINATE_GRACE_PERIOD
}

fn mapped_child_status(status: ExitStatus) -> Option<ExitStatus> {
    Some(status)
}

#[cfg(unix)]
fn send_child_sigkill(child: &mut Child) -> Result<(), String> {
    let pid = child_process_group_id(child);
    let rc = unsafe { libc::killpg(pid, libc::SIGKILL) };
    if rc == -1 {
        child
            .kill()
            .map_err(|err| format!("Failed to kill child process: {err}"))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn send_child_sigkill(child: &mut Child) -> Result<(), String> {
    child
        .kill()
        .map_err(|err| format!("Failed to kill child process: {err}"))
}

fn reap_child_after_kill(child: &mut Child) -> Result<Option<ExitStatus>, String> {
    child
        .wait()
        .map(Some)
        .map_err(|err| format!("Failed to reap child process: {err}"))
}

#[cfg(unix)]
fn send_child_sigterm(child: &Child) {
    use signal_hook::consts::signal::SIGTERM;
    let _ = unsafe { libc::killpg(child_process_group_id(child), SIGTERM) };
}

#[cfg(unix)]
fn child_process_group_id(child: &Child) -> libc::pid_t {
    child.id() as libc::pid_t
}

#[cfg(not(unix))]
fn send_child_sigterm(child: &mut Child) {
    let _ = child.kill();
}
