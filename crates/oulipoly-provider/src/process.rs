//! ## Declared roles
//!
//! Roles: orchestration, mapper, predicate, accessor, filter, validator.
//!
//! - orchestration: `ProcessRunner` starts provider subprocesses and
//!   `ProcessSupervisor` consumes bounded worker/cancellation events, polls
//!   child lifecycle state, and terminates/reaps process trees.
//! - mapper: `map_completed_process_outcome`, `termination_diagnostics_from_parts`,
//!   `provider_process_command`, `host_process_error`, `process_status`, `exit_code`, and
//!   `process_nonzero` translate OS process observations into provider DTOs
//!   and diagnostics.
//! - predicate: `chunk_metadata`, `timeout_expired`, `cancellation_requested`,
//!   `cancellation_grace_expired`, `should_force_kill`, `is_executable`, and
//!   byte-content predicates classify process and capture state.
//! - accessor: `ByteLimit::bytes`, `ProcessCommand::argv`,
//!   `ProcessOutcome::{argv, diagnostics}`, `CancellationToken::is_cancelled`,
//!   and test-only stdout/stderr text accessors expose owned state.
//! - filter: `capture_window`, `retain_prefix_chunk`, `retain_tail_chunk`,
//!   `ByteAccumulator`, `ByteTailAccumulator`, and `alive_descendants` retain
//!   bounded byte/process subsets from larger streams or process sets.
//! - validator: embedded process-runner and byte-accumulator tests validate
//!   bounded event delivery, poll cadence, worker failure, pipe pressure,
//!   process-tree cleanup, cancellation, and truncation contracts.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-provider/src/process.rs
//!     role: adapter
//!     Translates:
//!       - provider-cli-subprocess-contract
//!       - std-process-command-contract
//!       - std-process-exit-status-contract
//!       - process-supervision-liveness-contract
//!       - byte-limit-capture-contract
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-provider/src/process.rs
//!     role: intrinsic-surface
//!     Domain: provider subprocess supervision and bounded byte capture
//!     Owns:
//!       - ByteLimit, CapturedBytes, and accumulator truncation semantics
//!       - ProcessLimits lifecycle inputs using the shared core CancellationToken
//!       - ProcessCommand, ProcessOutcome, and ProcessRunner public surfaces
//!       - total-runtime and stdout-line-gap timeout behavior
//!       - cross-platform process group termination and executable checks
//! ```

use crate::error::{HostErrorKind, ProviderClientError, ProviderDiagnostics};
use crate::generated::ProcessStatus;
use oulipoly_core::CancellationRegistration;
pub use oulipoly_core::CancellationToken;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const STATUS_POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteLimit {
    bytes: usize,
}

impl ByteLimit {
    pub fn new(bytes: usize) -> Self {
        Self { bytes }
    }

    pub(crate) fn bytes(self) -> usize {
        self.bytes
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapturedBytes {
    pub bytes: Vec<u8>,
    pub limit: usize,
    pub captured_len: usize,
    pub discarded_len: usize,
    pub truncated: bool,
    pub contains_nul: bool,
    pub contains_high_bit: bool,
}

#[derive(Debug, Clone)]
pub struct ByteAccumulator {
    limit: ByteLimit,
    bytes: Vec<u8>,
    discarded_len: usize,
    contains_nul: bool,
    contains_high_bit: bool,
}

impl ByteAccumulator {
    pub fn new(limit: ByteLimit) -> Self {
        Self {
            limit,
            bytes: Vec::new(),
            discarded_len: 0,
            contains_nul: false,
            contains_high_bit: false,
        }
    }

    pub fn push(&mut self, chunk: &[u8]) {
        self.record_chunk_metadata(chunk_metadata(chunk));
        self.retain_prefix_chunk(chunk);
    }

    fn record_chunk_metadata(&mut self, metadata: ChunkMetadata) {
        self.contains_nul |= metadata.contains_nul;
        self.contains_high_bit |= metadata.contains_high_bit;
    }

    fn retain_prefix_chunk(&mut self, chunk: &[u8]) {
        let window = capture_window(self.limit, self.bytes.len(), chunk.len());
        self.bytes.extend_from_slice(&chunk[..window.captured_len]);
        self.discarded_len += window.discarded_len;
    }

    pub fn finish(self) -> CapturedBytes {
        let captured_len = self.bytes.len();
        CapturedBytes {
            bytes: self.bytes,
            limit: self.limit.bytes,
            captured_len,
            discarded_len: self.discarded_len,
            truncated: self.discarded_len > 0,
            contains_nul: self.contains_nul,
            contains_high_bit: self.contains_high_bit,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ByteTailAccumulator {
    limit: ByteLimit,
    bytes: Vec<u8>,
    discarded_len: usize,
    contains_nul: bool,
    contains_high_bit: bool,
}

impl ByteTailAccumulator {
    pub(crate) fn new(limit: ByteLimit) -> Self {
        Self {
            limit,
            bytes: Vec::new(),
            discarded_len: 0,
            contains_nul: false,
            contains_high_bit: false,
        }
    }

    pub(crate) fn push(&mut self, chunk: &[u8]) {
        self.record_chunk_metadata(chunk_metadata(chunk));
        self.retain_tail_chunk(chunk);
    }

    fn record_chunk_metadata(&mut self, metadata: ChunkMetadata) {
        self.contains_nul |= metadata.contains_nul;
        self.contains_high_bit |= metadata.contains_high_bit;
    }

    fn retain_tail_chunk(&mut self, chunk: &[u8]) {
        if self.limit.bytes == 0 {
            self.discarded_len += chunk.len();
            return;
        }

        self.bytes.extend_from_slice(chunk);
        if self.bytes.len() <= self.limit.bytes {
            return;
        }

        let overflow = self.bytes.len() - self.limit.bytes;
        self.bytes.drain(..overflow);
        self.discarded_len += overflow;
    }

    pub(crate) fn finish(self) -> CapturedBytes {
        let captured_len = self.bytes.len();
        CapturedBytes {
            bytes: self.bytes,
            limit: self.limit.bytes,
            captured_len,
            discarded_len: self.discarded_len,
            truncated: self.discarded_len > 0,
            contains_nul: self.contains_nul,
            contains_high_bit: self.contains_high_bit,
        }
    }
}

pub(crate) trait StdoutDrainOutput: Default + Send + 'static {
    fn captured_bytes(&self) -> CapturedBytes;

    fn processor_error(&self) -> Option<&ProviderClientError> {
        None
    }
}

impl StdoutDrainOutput for CapturedBytes {
    fn captured_bytes(&self) -> CapturedBytes {
        self.clone()
    }
}

pub(crate) trait StdoutProcessor: Send + 'static {
    type Output: StdoutDrainOutput;

    fn push(&mut self, chunk: &[u8]) -> Result<(), ProviderClientError>;
    fn finish(self, error: Option<ProviderClientError>) -> Self::Output;
}

struct ByteCaptureProcessor {
    accumulator: ByteAccumulator,
}

impl ByteCaptureProcessor {
    fn new(limit: ByteLimit) -> Self {
        Self {
            accumulator: ByteAccumulator::new(limit),
        }
    }
}

impl StdoutProcessor for ByteCaptureProcessor {
    type Output = CapturedBytes;

    fn push(&mut self, chunk: &[u8]) -> Result<(), ProviderClientError> {
        self.accumulator.push(chunk);
        Ok(())
    }

    fn finish(self, _error: Option<ProviderClientError>) -> Self::Output {
        self.accumulator.finish()
    }
}

struct ChunkMetadata {
    contains_nul: bool,
    contains_high_bit: bool,
}

struct CaptureWindow {
    captured_len: usize,
    discarded_len: usize,
}

fn chunk_metadata(chunk: &[u8]) -> ChunkMetadata {
    ChunkMetadata {
        contains_nul: chunk_contains_nul(chunk),
        contains_high_bit: chunk_contains_high_bit(chunk),
    }
}

fn chunk_contains_nul(chunk: &[u8]) -> bool {
    chunk.contains(&0)
}

fn chunk_contains_high_bit(chunk: &[u8]) -> bool {
    chunk.iter().any(|byte| *byte >= 0x80)
}

fn capture_window(limit: ByteLimit, current_len: usize, incoming_len: usize) -> CaptureWindow {
    let remaining = limit.bytes.saturating_sub(current_len);
    let captured_len = remaining.min(incoming_len);
    CaptureWindow {
        captured_len,
        discarded_len: incoming_len - captured_len,
    }
}

#[derive(Debug, Clone)]
pub struct ProcessLimits {
    pub timeout: Duration,
    pub kill_after_grace: Duration,
    pub stdout_limit: ByteLimit,
    pub stderr_limit: ByteLimit,
    pub cancellation: Option<CancellationToken>,
    pub spawn_observer: Option<ProcessSpawnObserver>,
}

#[derive(Clone)]
pub struct ProcessSpawnObserver {
    callback: Arc<dyn Fn(u32) -> Result<(), String> + Send + Sync>,
}

impl ProcessSpawnObserver {
    pub fn new(callback: impl Fn(u32) -> Result<(), String> + Send + Sync + 'static) -> Self {
        Self {
            callback: Arc::new(callback),
        }
    }

    fn observe(&self, child_id: u32) -> Result<(), String> {
        (self.callback)(child_id)
    }
}

impl std::fmt::Debug for ProcessSpawnObserver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessSpawnObserver")
            .finish_non_exhaustive()
    }
}

impl Default for ProcessLimits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            kill_after_grace: Duration::from_millis(100),
            stdout_limit: ByteLimit::new(1024 * 1024),
            stderr_limit: ByteLimit::new(128 * 1024),
            cancellation: None,
            spawn_observer: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProcessCommand {
    program: PathBuf,
    args: Vec<OsString>,
    pinned_executable: Option<Arc<File>>,
    is_script: bool,
    environment_removals: Vec<OsString>,
}

impl ProcessCommand {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            pinned_executable: None,
            is_script: false,
            environment_removals: Vec::new(),
        }
    }

    pub fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.args.push(arg.as_ref().to_os_string());
        self
    }

    pub fn argv(&self) -> Vec<OsString> {
        let mut argv = vec![self.program.as_os_str().to_os_string()];
        argv.extend(self.args.iter().cloned());
        argv
    }

    pub(crate) fn with_pinned_executable(mut self, executable: Option<Arc<File>>) -> Self {
        self.pinned_executable = executable;
        self
    }

    pub(crate) fn with_script(mut self, is_script: bool) -> Self {
        self.is_script = is_script;
        self
    }

    pub(crate) fn with_environment_removals(mut self, names: Vec<OsString>) -> Self {
        self.environment_removals = names;
        self
    }
}

#[derive(Debug, Clone)]
pub struct ProcessOutcome<T = CapturedBytes> {
    pub status: ProcessStatus,
    pub stdout: T,
    pub stderr: CapturedBytes,
    pub stdin_closed_early: bool,
    pub host_cancellation_requested: bool,
    argv: Vec<OsString>,
}

impl ProcessOutcome<CapturedBytes> {
    #[cfg(test)]
    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout.bytes).into_owned()
    }

    #[cfg(test)]
    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr.bytes).into_owned()
    }
}

impl<T: StdoutDrainOutput> ProcessOutcome<T> {
    pub fn argv(&self) -> &[OsString] {
        &self.argv
    }

    pub fn diagnostics(&self) -> ProviderDiagnostics {
        ProviderDiagnostics {
            stdout: self.stdout.captured_bytes(),
            stderr: self.stderr.clone(),
            stdin_closed_early: self.stdin_closed_early,
            host_cancellation_requested: self.host_cancellation_requested,
            provider_exit_code: exit_code(&self.status),
            provider_process_nonzero: process_nonzero(&self.status),
            ..ProviderDiagnostics::default()
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProcessRunner {
    limits: ProcessLimits,
}

struct ProcessThreads<T: StdoutDrainOutput> {
    stdout: thread::JoinHandle<T>,
    stderr: thread::JoinHandle<CapturedBytes>,
    stdin: thread::JoinHandle<bool>,
}

#[derive(Default)]
struct PendingProcessEvents {
    latest_stdout_line: Option<Instant>,
    cancellation_requested: bool,
    stdout_processor_failed: bool,
    worker_failed: bool,
}

struct ProcessEvents {
    latest_stdout_line: Option<Instant>,
    cancellation_requested: bool,
    stdout_processor_failed: bool,
    worker_failed: bool,
}

#[derive(Clone)]
struct ProcessEventPublisher {
    bus: Arc<(Mutex<PendingProcessEvents>, Condvar)>,
}

struct ProcessEventSubscriber {
    bus: Arc<(Mutex<PendingProcessEvents>, Condvar)>,
}

struct JoinedProcessThreads<T: StdoutDrainOutput> {
    stdout: Option<T>,
    stderr: Option<CapturedBytes>,
    stdin_closed_early: Option<bool>,
    failed_workers: Vec<&'static str>,
}

struct TerminatedProcess {
    status: Option<ExitStatus>,
    force_killed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimeoutMode {
    TotalRuntime,
    StdoutLineGap,
}

struct ProcessSupervisor<'a, T: StdoutDrainOutput> {
    child: Child,
    command: ProcessCommand,
    threads: ProcessThreads<T>,
    events: ProcessEventSubscriber,
    timeout_mode: TimeoutMode,
    started: Instant,
    next_status_poll: Instant,
    last_stdout_line: Instant,
    cancellation_started: Option<Instant>,
    _cancellation_registration: Option<CancellationRegistration>,
    limits: &'a ProcessLimits,
    argv: Vec<OsString>,
}

impl ProcessRunner {
    pub fn new(limits: ProcessLimits) -> Self {
        Self { limits }
    }

    pub fn run<I, K, V>(
        &self,
        command: ProcessCommand,
        stdin_bytes: Vec<u8>,
        envs: I,
    ) -> Result<ProcessOutcome, ProviderClientError>
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.run_with_timeout_mode(command, stdin_bytes, envs, TimeoutMode::TotalRuntime)
    }

    pub(crate) fn run_with_stdout_line_gap_timeout<I, K, V>(
        &self,
        command: ProcessCommand,
        stdin_bytes: Vec<u8>,
        envs: I,
    ) -> Result<ProcessOutcome, ProviderClientError>
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.run_with_timeout_mode(command, stdin_bytes, envs, TimeoutMode::StdoutLineGap)
    }

    pub(crate) fn run_with_stdout_line_gap_timeout_and_stdout_processor<I, K, V, P>(
        &self,
        command: ProcessCommand,
        stdin_bytes: Vec<u8>,
        envs: I,
        stdout_processor: P,
    ) -> Result<ProcessOutcome<P::Output>, ProviderClientError>
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
        P: StdoutProcessor,
    {
        self.run_with_timeout_mode_and_stdout_processor(
            command,
            stdin_bytes,
            envs,
            TimeoutMode::StdoutLineGap,
            stdout_processor,
        )
    }

    fn run_with_timeout_mode<I, K, V>(
        &self,
        command: ProcessCommand,
        stdin_bytes: Vec<u8>,
        envs: I,
        timeout_mode: TimeoutMode,
    ) -> Result<ProcessOutcome, ProviderClientError>
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.run_with_timeout_mode_and_stdout_processor(
            command,
            stdin_bytes,
            envs,
            timeout_mode,
            ByteCaptureProcessor::new(self.limits.stdout_limit),
        )
    }

    fn run_with_timeout_mode_and_stdout_processor<I, K, V, P>(
        &self,
        command: ProcessCommand,
        stdin_bytes: Vec<u8>,
        envs: I,
        timeout_mode: TimeoutMode,
        stdout_processor: P,
    ) -> Result<ProcessOutcome<P::Output>, ProviderClientError>
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
        P: StdoutProcessor,
    {
        let argv = command.argv();
        let mut child = spawn_provider_process(&command, envs)?;
        if let Err(error) = notify_spawn_observer(&self.limits.spawn_observer, child.id()) {
            return Err(terminate_after_spawn_observer_failure(
                child,
                &command,
                self.limits.kill_after_grace,
                error,
            ));
        }
        let (event_publisher, events) = process_event_bus();
        let threads = start_process_threads(
            &mut child,
            stdin_bytes,
            self.limits.stderr_limit,
            event_publisher.clone(),
            stdout_line_activity_publisher(timeout_mode, &event_publisher),
            stdout_processor,
        );

        let started = Instant::now();
        let cancellation_registration = self.limits.cancellation.as_ref().map(|cancellation| {
            cancellation.register(move || event_publisher.publish_cancellation())
        });
        ProcessSupervisor {
            child,
            command,
            threads,
            events,
            timeout_mode,
            started,
            next_status_poll: started,
            last_stdout_line: started,
            cancellation_started: None,
            _cancellation_registration: cancellation_registration,
            limits: &self.limits,
            argv,
        }
        .run()
    }
}

impl<'a, T: StdoutDrainOutput> ProcessSupervisor<'a, T> {
    fn run(mut self) -> Result<ProcessOutcome<T>, ProviderClientError> {
        loop {
            let events = self.record_pending_events();
            if events.worker_failed {
                return Err(self.terminate_and_collect(HostErrorKind::WaitFailed, false));
            }
            if events.stdout_processor_failed {
                return Err(self.terminate_after_stdout_processor_failure());
            }
            if Instant::now() >= self.next_status_poll {
                self.next_status_poll = Instant::now() + STATUS_POLL_INTERVAL;
                match poll_child_status(&mut self.child, &self.command) {
                    Ok(Some(status)) => return self.collect_completed(status),
                    Ok(None) => {}
                    Err(_) => {
                        return Err(self.terminate_and_collect(HostErrorKind::WaitFailed, false));
                    }
                }
            }

            if timeout_expired(
                self.timeout_mode,
                self.started,
                self.last_stdout_line,
                self.limits.timeout,
            ) {
                return Err(self.terminate_and_collect(HostErrorKind::Timeout, false));
            }

            if self.cancellation_started.as_ref().is_some_and(|started| {
                cancellation_grace_expired(started, self.limits.kill_after_grace)
            }) {
                return Err(self.force_kill_and_collect());
            }

            self.events.wait_for_event_or_poll(self.next_wait());
        }
    }

    fn record_pending_events(&mut self) -> ProcessEvents {
        let events = self.events.take_pending();
        if let Some(observed_at) = events.latest_stdout_line {
            self.last_stdout_line = observed_at;
        }
        if events.cancellation_requested {
            self.begin_cancellation();
        }
        events
    }

    fn next_wait(&self) -> Duration {
        let timeout_remaining = match self.timeout_mode {
            TimeoutMode::TotalRuntime => self.limits.timeout.saturating_sub(self.started.elapsed()),
            TimeoutMode::StdoutLineGap => self
                .limits
                .timeout
                .saturating_sub(self.last_stdout_line.elapsed()),
        };
        let cancellation_remaining = self
            .cancellation_started
            .as_ref()
            .map(|started| {
                cancellation_grace(self.limits.kill_after_grace).saturating_sub(started.elapsed())
            })
            .unwrap_or(STATUS_POLL_INTERVAL);
        self.next_status_poll
            .saturating_duration_since(Instant::now())
            .min(timeout_remaining)
            .min(cancellation_remaining)
    }

    fn begin_cancellation(&mut self) {
        if self.cancellation_started.is_none() {
            terminate_tree(&mut self.child);
            self.cancellation_started = Some(Instant::now());
            self.next_status_poll = Instant::now();
        }
    }

    fn collect_completed(
        self,
        status: ExitStatus,
    ) -> Result<ProcessOutcome<T>, ProviderClientError> {
        let host_cancellation_requested =
            self.cancellation_started.is_some() || cancellation_requested(self.limits);
        if host_cancellation_requested && self.timeout_mode == TimeoutMode::TotalRuntime {
            let diagnostics = map_termination_diagnostics(
                self.threads,
                TerminatedProcess {
                    status: Some(status),
                    force_killed: false,
                },
                true,
            );
            return Err(termination_transport_error(
                HostErrorKind::Cancelled,
                &self.command,
                diagnostics,
            ));
        }
        map_completed_process_outcome(
            status,
            self.threads,
            &self.command,
            self.argv,
            host_cancellation_requested,
        )
    }

    fn terminate_and_collect(
        mut self,
        kind: HostErrorKind,
        host_cancellation_requested: bool,
    ) -> ProviderClientError {
        terminate_tree(&mut self.child);
        let terminated = wait_for_terminated_process(&mut self.child, self.limits.kill_after_grace);
        let diagnostics =
            map_termination_diagnostics(self.threads, terminated, host_cancellation_requested);
        termination_transport_error(kind, &self.command, diagnostics)
    }

    fn force_kill_and_collect(mut self) -> ProviderClientError {
        kill_tree(&mut self.child);
        let terminated = TerminatedProcess {
            status: self.child.wait().ok(),
            force_killed: true,
        };
        let diagnostics = map_termination_diagnostics(self.threads, terminated, true);
        termination_transport_error(HostErrorKind::Cancelled, &self.command, diagnostics)
    }

    fn terminate_after_stdout_processor_failure(mut self) -> ProviderClientError {
        terminate_tree(&mut self.child);
        let terminated = wait_for_terminated_process(&mut self.child, self.limits.kill_after_grace);
        let status = terminated.status.map(process_status);
        let joined = join_process_threads(self.threads);
        let host_cancellation_requested =
            self.cancellation_started.is_some() || cancellation_requested(self.limits);
        if host_cancellation_requested {
            let diagnostics = termination_diagnostics_from_joined(joined, terminated, true);
            return termination_transport_error(
                HostErrorKind::Cancelled,
                &self.command,
                diagnostics,
            );
        }
        let processor_error = joined
            .stdout
            .as_ref()
            .and_then(StdoutDrainOutput::processor_error)
            .cloned()
            .unwrap_or_else(|| {
                ProviderClientError::host_transport(
                    HostErrorKind::Other("stdout_processor_failed".to_string()),
                    subcommand_for_error(&self.command),
                    None,
                    ProviderDiagnostics::default(),
                )
            });
        processor_failure_with_process_context(
            processor_error,
            joined,
            terminated,
            host_cancellation_requested,
            status,
        )
    }
}

fn termination_transport_error(
    kind: HostErrorKind,
    command: &ProcessCommand,
    diagnostics: ProviderDiagnostics,
) -> ProviderClientError {
    ProviderClientError::host_transport(kind, subcommand_for_error(command), None, diagnostics)
}

fn spawn_provider_process<I, K, V>(
    command: &ProcessCommand,
    envs: I,
) -> Result<Child, ProviderClientError>
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let mut process = build_provider_process(command, envs);
    process
        .spawn()
        .map_err(|error| host_process_error(HostErrorKind::SpawnFailed, command, error))
}

fn build_provider_process<I, K, V>(command: &ProcessCommand, envs: I) -> Command
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let mut process = provider_process_command(command, envs);
    configure_provider_process(&mut process, command);
    process
}

fn provider_process_command<I, K, V>(command: &ProcessCommand, envs: I) -> Command
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let mut process = Command::new(provider_execution_path(command));
    process
        .args(&command.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in envs {
        process.env(key, value);
    }
    for name in &command.environment_removals {
        process.env_remove(name);
    }
    process
}

fn configure_provider_process(process: &mut Command, command: &ProcessCommand) {
    configure_process_group(process);
    configure_pinned_executable(process, command);
}

#[cfg(unix)]
fn provider_execution_path(command: &ProcessCommand) -> PathBuf {
    use std::os::fd::AsRawFd;

    // Keep script-visible path semantics while it still names the selected inode.
    if command.is_script && script_path_still_names_pinned_executable(command) {
        return command.program.clone();
    }
    command
        .pinned_executable
        .as_ref()
        .map(|file| inherited_fd_path(file.as_raw_fd()))
        .unwrap_or_else(|| command.program.clone())
}

#[cfg(not(unix))]
fn provider_execution_path(command: &ProcessCommand) -> PathBuf {
    command.program.clone()
}

#[cfg(unix)]
fn script_path_still_names_pinned_executable(command: &ProcessCommand) -> bool {
    use std::os::unix::fs::MetadataExt;

    let Some(executable) = command.pinned_executable.as_ref() else {
        return false;
    };
    let Ok(pinned) = executable.metadata() else {
        return false;
    };
    let Ok(current) = command.program.metadata() else {
        return false;
    };
    pinned.dev() == current.dev() && pinned.ino() == current.ino()
}

#[cfg(target_os = "linux")]
fn inherited_fd_path(fd: std::os::fd::RawFd) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{fd}"))
}

#[cfg(all(unix, not(target_os = "linux")))]
fn inherited_fd_path(fd: std::os::fd::RawFd) -> PathBuf {
    PathBuf::from(format!("/dev/fd/{fd}"))
}

#[cfg(unix)]
fn configure_pinned_executable(process: &mut Command, command: &ProcessCommand) {
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;

    let Some(executable) = command.pinned_executable.as_ref() else {
        return;
    };
    let fd = executable.as_raw_fd();
    unsafe {
        process.pre_exec(move || {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags == -1 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn configure_pinned_executable(_process: &mut Command, _command: &ProcessCommand) {}

fn start_process_threads<P: StdoutProcessor>(
    child: &mut Child,
    stdin_bytes: Vec<u8>,
    stderr_limit: ByteLimit,
    process_events: ProcessEventPublisher,
    stdout_line_activity: Option<ProcessEventPublisher>,
    stdout_processor: P,
) -> ProcessThreads<P::Output> {
    let stdout = child
        .stdout
        .take()
        .expect("stdout pipe should be configured");
    let stderr = child
        .stderr
        .take()
        .expect("stderr pipe should be configured");
    let stdin = child.stdin.take().expect("stdin pipe should be configured");
    let stdout_events = process_events.clone();
    let stdout_processor_events = process_events.clone();
    let stderr_events = process_events.clone();
    ProcessThreads {
        stdout: thread::spawn(move || {
            run_process_worker(stdout_events, || {
                let output =
                    drain_reader_with_processor(stdout, stdout_line_activity, stdout_processor);
                if output.processor_error().is_some() {
                    stdout_processor_events.publish_stdout_processor_failure();
                }
                output
            })
        }),
        stderr: thread::spawn(move || {
            run_process_worker(stderr_events, || drain_reader(stderr, stderr_limit, None))
        }),
        stdin: thread::spawn(move || {
            run_process_worker(process_events, || write_stdin(stdin, stdin_bytes))
        }),
    }
}

fn run_process_worker<T>(events: ProcessEventPublisher, worker: impl FnOnce() -> T) -> T {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(worker)) {
        Ok(output) => output,
        Err(panic) => {
            events.publish_worker_failure();
            std::panic::resume_unwind(panic)
        }
    }
}

#[cfg(unix)]
fn poll_child_status(
    child: &mut Child,
    command: &ProcessCommand,
) -> Result<Option<ExitStatus>, ProviderClientError> {
    let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            child.id() as libc::id_t,
            info.as_mut_ptr(),
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result != 0 {
        return Err(host_process_error(
            HostErrorKind::WaitFailed,
            command,
            std::io::Error::last_os_error(),
        ));
    }
    let info = unsafe { info.assume_init() };
    if unsafe { info.si_pid() } == 0 {
        return Ok(None);
    }
    // WNOWAIT keeps the exact leader unreaped while its owned descendants are cleaned.
    kill_tree(child);
    child
        .wait()
        .map(Some)
        .map_err(|error| host_process_error(HostErrorKind::WaitFailed, command, error))
}

#[cfg(not(unix))]
fn poll_child_status(
    child: &mut Child,
    command: &ProcessCommand,
) -> Result<Option<ExitStatus>, ProviderClientError> {
    child
        .try_wait()
        .map_err(|error| host_process_error(HostErrorKind::WaitFailed, command, error))
}

fn map_completed_process_outcome<T: StdoutDrainOutput>(
    status: ExitStatus,
    threads: ProcessThreads<T>,
    command: &ProcessCommand,
    argv: Vec<OsString>,
    host_cancellation_requested: bool,
) -> Result<ProcessOutcome<T>, ProviderClientError> {
    let process_status = process_status(status);
    let joined = join_process_threads(threads);
    if !joined.failed_workers.is_empty() {
        return Err(process_worker_failure(
            command,
            joined,
            Some(process_status),
            host_cancellation_requested,
        ));
    }
    if let Some(error) = joined
        .stdout
        .as_ref()
        .and_then(StdoutDrainOutput::processor_error)
        .cloned()
    {
        let diagnostics =
            completed_process_diagnostics(&joined, &process_status, host_cancellation_requested);
        return Err(error.with_process_context(diagnostics, process_status));
    }
    Ok(ProcessOutcome {
        status: process_status,
        stdout: joined.stdout.expect("validated stdout worker result"),
        stderr: joined.stderr.expect("validated stderr worker result"),
        stdin_closed_early: joined
            .stdin_closed_early
            .expect("validated stdin worker result"),
        host_cancellation_requested,
        argv,
    })
}

fn process_event_bus() -> (ProcessEventPublisher, ProcessEventSubscriber) {
    let bus = Arc::new((Mutex::new(PendingProcessEvents::default()), Condvar::new()));
    (
        ProcessEventPublisher {
            bus: Arc::clone(&bus),
        },
        ProcessEventSubscriber { bus },
    )
}

fn stdout_line_activity_publisher(
    timeout_mode: TimeoutMode,
    event_publisher: &ProcessEventPublisher,
) -> Option<ProcessEventPublisher> {
    match timeout_mode {
        TimeoutMode::StdoutLineGap => Some(event_publisher.clone()),
        TimeoutMode::TotalRuntime => None,
    }
}

impl ProcessEventPublisher {
    fn publish_stdout_line(&self, observed_at: Instant) {
        let (state, wake) = &*self.bus;
        let mut state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let should_wake = state.latest_stdout_line.is_none();
        state.latest_stdout_line = Some(observed_at);
        if should_wake {
            wake.notify_one();
        }
    }

    fn publish_cancellation(&self) {
        let (state, wake) = &*self.bus;
        let mut state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !state.cancellation_requested {
            state.cancellation_requested = true;
            wake.notify_one();
        }
    }

    fn publish_worker_failure(&self) {
        let (state, wake) = &*self.bus;
        let mut state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !state.worker_failed {
            state.worker_failed = true;
            wake.notify_one();
        }
    }

    fn publish_stdout_processor_failure(&self) {
        let (state, wake) = &*self.bus;
        let mut state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !state.stdout_processor_failed {
            state.stdout_processor_failed = true;
            wake.notify_one();
        }
    }
}

impl ProcessEventSubscriber {
    fn take_pending(&self) -> ProcessEvents {
        let (state, _) = &*self.bus;
        let mut state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        ProcessEvents {
            latest_stdout_line: state.latest_stdout_line.take(),
            cancellation_requested: std::mem::take(&mut state.cancellation_requested),
            stdout_processor_failed: std::mem::take(&mut state.stdout_processor_failed),
            worker_failed: std::mem::take(&mut state.worker_failed),
        }
    }

    fn wait_for_event_or_poll(&self, timeout: Duration) {
        let (state, wake) = &*self.bus;
        let state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.latest_stdout_line.is_none()
            && !state.cancellation_requested
            && !state.stdout_processor_failed
            && !state.worker_failed
        {
            let _ = wake.wait_timeout(state, timeout);
        }
    }
}

fn timeout_expired(
    mode: TimeoutMode,
    started: Instant,
    last_stdout_line: Instant,
    timeout: Duration,
) -> bool {
    match mode {
        TimeoutMode::TotalRuntime => started.elapsed() >= timeout,
        TimeoutMode::StdoutLineGap => last_stdout_line.elapsed() >= timeout,
    }
}

fn cancellation_requested(limits: &ProcessLimits) -> bool {
    limits
        .cancellation
        .as_ref()
        .is_some_and(CancellationToken::is_cancelled)
}

fn cancellation_grace_expired(cancelled_at: &Instant, kill_after_grace: Duration) -> bool {
    cancelled_at.elapsed() >= cancellation_grace(kill_after_grace)
}

#[cfg(unix)]
fn wait_for_terminated_process(child: &mut Child, kill_after_grace: Duration) -> TerminatedProcess {
    let mut force_killed = false;
    let grace_started = Instant::now();
    let status = loop {
        match child_exited_without_reaping(child) {
            Ok(true) => {
                // Keep the leader waitable until its exact process group is clean.
                kill_tree(child);
                break child.wait().ok();
            }
            Ok(false) if should_force_kill(&grace_started, kill_after_grace) => {
                force_killed = true;
                kill_tree(child);
                break child.wait().ok();
            }
            Ok(false) => thread::sleep(Duration::from_millis(5)),
            Err(_) => break None,
        }
    };
    TerminatedProcess {
        status,
        force_killed,
    }
}

#[cfg(unix)]
fn child_exited_without_reaping(child: &Child) -> std::io::Result<bool> {
    let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            child.id() as libc::id_t,
            info.as_mut_ptr(),
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { info.assume_init().si_pid() } != 0)
}

#[cfg(not(unix))]
fn wait_for_terminated_process(child: &mut Child, kill_after_grace: Duration) -> TerminatedProcess {
    let mut force_killed = false;
    let grace_started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if should_force_kill(&grace_started, kill_after_grace) => {
                force_killed = true;
                kill_tree(child);
                break child.wait().ok();
            }
            Ok(None) => thread::sleep(Duration::from_millis(5)),
            Err(_) => break None,
        }
    };
    TerminatedProcess {
        status,
        force_killed,
    }
}

fn should_force_kill(grace_started: &Instant, kill_after_grace: Duration) -> bool {
    grace_started.elapsed() >= kill_after_grace
}

fn map_termination_diagnostics<T: StdoutDrainOutput>(
    threads: ProcessThreads<T>,
    terminated: TerminatedProcess,
    host_cancellation_requested: bool,
) -> ProviderDiagnostics {
    let joined = join_process_threads(threads);
    termination_diagnostics_from_joined(joined, terminated, host_cancellation_requested)
}

fn termination_diagnostics_from_joined<T: StdoutDrainOutput>(
    joined: JoinedProcessThreads<T>,
    terminated: TerminatedProcess,
    host_cancellation_requested: bool,
) -> ProviderDiagnostics {
    let mut diagnostics = termination_diagnostics_from_parts(
        terminated,
        host_cancellation_requested,
        joined
            .stdout
            .as_ref()
            .map(StdoutDrainOutput::captured_bytes)
            .unwrap_or_default(),
        joined.stderr.unwrap_or_default(),
    );
    diagnostics.stdin_closed_early = joined.stdin_closed_early.unwrap_or_default();
    if !joined.failed_workers.is_empty() {
        diagnostics.description = Some(worker_failure_description(&joined.failed_workers));
    }
    diagnostics
}

fn processor_failure_with_process_context<T: StdoutDrainOutput>(
    error: ProviderClientError,
    joined: JoinedProcessThreads<T>,
    terminated: TerminatedProcess,
    host_cancellation_requested: bool,
    status: Option<ProcessStatus>,
) -> ProviderClientError {
    let description = error.diagnostics().description.clone();
    let mut diagnostics =
        termination_diagnostics_from_joined(joined, terminated, host_cancellation_requested);
    diagnostics.description = description;
    match status {
        Some(status) => error.with_process_context(diagnostics, status),
        None => replace_error_diagnostics(error, diagnostics),
    }
}

fn replace_error_diagnostics(
    error: ProviderClientError,
    diagnostics: ProviderDiagnostics,
) -> ProviderClientError {
    match error {
        ProviderClientError::Transport {
            kind,
            subcommand,
            request_id,
            description,
            process_status,
            ..
        } => ProviderClientError::Transport {
            kind,
            subcommand,
            request_id,
            description,
            diagnostics: Box::new(diagnostics),
            process_status,
        },
        ProviderClientError::Protocol {
            kind,
            subcommand,
            request_id,
            description,
            process_status,
            launch_failure_evidence,
            ..
        } => ProviderClientError::Protocol {
            kind,
            subcommand,
            request_id,
            description,
            diagnostics: Box::new(diagnostics),
            process_status,
            launch_failure_evidence,
        },
        ProviderClientError::ProviderCapability(error) => {
            ProviderClientError::ProviderCapability(error)
        }
    }
}

fn completed_process_diagnostics<T: StdoutDrainOutput>(
    joined: &JoinedProcessThreads<T>,
    status: &ProcessStatus,
    host_cancellation_requested: bool,
) -> ProviderDiagnostics {
    ProviderDiagnostics {
        stdout: joined
            .stdout
            .as_ref()
            .map(StdoutDrainOutput::captured_bytes)
            .unwrap_or_default(),
        stderr: joined.stderr.clone().unwrap_or_default(),
        stdin_closed_early: joined.stdin_closed_early.unwrap_or_default(),
        process_was_reaped: true,
        provider_process_nonzero: process_nonzero(status),
        provider_exit_code: exit_code(status),
        host_cancellation_requested,
        ..ProviderDiagnostics::default()
    }
}

fn join_process_threads<T: StdoutDrainOutput>(
    threads: ProcessThreads<T>,
) -> JoinedProcessThreads<T> {
    let stdout = threads.stdout.join().ok();
    let stderr = threads.stderr.join().ok();
    let stdin_closed_early = threads.stdin.join().ok();
    let mut failed_workers = Vec::new();
    if stdout.is_none() {
        failed_workers.push("stdout");
    }
    if stderr.is_none() {
        failed_workers.push("stderr");
    }
    if stdin_closed_early.is_none() {
        failed_workers.push("stdin");
    }
    JoinedProcessThreads {
        stdout,
        stderr,
        stdin_closed_early,
        failed_workers,
    }
}

fn process_worker_failure<T: StdoutDrainOutput>(
    command: &ProcessCommand,
    joined: JoinedProcessThreads<T>,
    status: Option<ProcessStatus>,
    host_cancellation_requested: bool,
) -> ProviderClientError {
    let mut diagnostics = ProviderDiagnostics {
        stdout: joined
            .stdout
            .as_ref()
            .map(StdoutDrainOutput::captured_bytes)
            .unwrap_or_default(),
        stderr: joined.stderr.unwrap_or_default(),
        stdin_closed_early: joined.stdin_closed_early.unwrap_or_default(),
        process_was_reaped: status.is_some(),
        host_cancellation_requested,
        description: Some(worker_failure_description(&joined.failed_workers)),
        ..ProviderDiagnostics::default()
    };
    if let Some(status) = status {
        diagnostics.provider_exit_code = exit_code(&status);
        diagnostics.provider_process_nonzero = process_nonzero(&status);
    }
    ProviderClientError::host_transport(
        HostErrorKind::WaitFailed,
        subcommand_for_error(command),
        None,
        diagnostics,
    )
}

fn worker_failure_description(failed_workers: &[&str]) -> String {
    format!(
        "process worker thread panicked: {}",
        failed_workers.join(", ")
    )
}

fn termination_diagnostics_from_parts(
    terminated: TerminatedProcess,
    host_cancellation_requested: bool,
    stdout: CapturedBytes,
    stderr: CapturedBytes,
) -> ProviderDiagnostics {
    let mut diagnostics = ProviderDiagnostics {
        stdout,
        stderr,
        process_was_force_killed: terminated.force_killed,
        process_was_reaped: terminated.status.is_some(),
        host_cancellation_requested,
        ..ProviderDiagnostics::default()
    };
    if let Some(status) = terminated.status {
        let process_status = process_status(status);
        diagnostics.provider_exit_code = exit_code(&process_status);
        diagnostics.provider_process_nonzero = process_nonzero(&process_status);
    }
    diagnostics
}

fn notify_spawn_observer(
    observer: &Option<ProcessSpawnObserver>,
    child_id: u32,
) -> Result<(), String> {
    if let Some(observer) = observer {
        observer.observe(child_id)?;
    }
    Ok(())
}

fn terminate_after_spawn_observer_failure(
    mut child: Child,
    command: &ProcessCommand,
    kill_after_grace: Duration,
    error: String,
) -> ProviderClientError {
    terminate_tree(&mut child);
    let terminated = wait_for_terminated_process(&mut child, kill_after_grace);
    let mut diagnostics = ProviderDiagnostics::with_description(error);
    diagnostics.process_was_force_killed = terminated.force_killed;
    diagnostics.process_was_reaped = terminated.status.is_some();
    if let Some(status) = terminated.status {
        let status = process_status(status);
        diagnostics.provider_exit_code = exit_code(&status);
        diagnostics.provider_process_nonzero = process_nonzero(&status);
    }
    ProviderClientError::host_transport(
        HostErrorKind::Other("spawn_observer_failed".to_string()),
        subcommand_for_error(command),
        None,
        diagnostics,
    )
}

fn drain_reader(
    mut reader: impl Read,
    limit: ByteLimit,
    stdout_line_activity: Option<ProcessEventPublisher>,
) -> CapturedBytes {
    drain_reader_with_processor(
        &mut reader,
        stdout_line_activity,
        ByteCaptureProcessor::new(limit),
    )
}

fn drain_reader_with_processor<P: StdoutProcessor>(
    mut reader: impl Read,
    stdout_line_activity: Option<ProcessEventPublisher>,
    mut processor: P,
) -> P::Output {
    let mut buffer = [0_u8; 8192];
    let mut processor_error = None;
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                let chunk = &buffer[..read];
                notify_stdout_line_activity(&stdout_line_activity, chunk);
                if let Err(error) = processor.push(chunk) {
                    processor_error = Some(error);
                    break;
                }
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    processor.finish(processor_error)
}

fn notify_stdout_line_activity(activity: &Option<ProcessEventPublisher>, chunk: &[u8]) {
    let Some(activity) = activity else {
        return;
    };
    if chunk.contains(&b'\n') {
        activity.publish_stdout_line(Instant::now());
    }
}

fn write_stdin(mut stdin: impl Write, bytes: Vec<u8>) -> bool {
    match write_stdin_bytes(&mut stdin, &bytes) {
        Ok(()) => stdin_flush_closed_early(&mut stdin),
        Err(error) => stdin_error_closed_early(&error),
    }
}

fn write_stdin_bytes(stdin: &mut impl Write, bytes: &[u8]) -> std::io::Result<()> {
    let Some((first, remaining)) = bytes.split_first() else {
        return Ok(());
    };
    // Give a provider that rejects stdin a bounded chance to close before the bulk write.
    stdin.write_all(std::slice::from_ref(first))?;
    thread::sleep(Duration::from_millis(10));
    stdin.write_all(remaining)
}

fn stdin_flush_closed_early(stdin: &mut impl Write) -> bool {
    match flush_stdin(stdin) {
        Ok(()) => false,
        Err(error) => stdin_error_closed_early(&error),
    }
}

fn flush_stdin(stdin: &mut impl Write) -> std::io::Result<()> {
    stdin.flush()
}

fn stdin_error_closed_early(error: &std::io::Error) -> bool {
    error.kind() == ErrorKind::BrokenPipe || error.kind() == ErrorKind::WouldBlock
}

fn host_process_error(
    kind: HostErrorKind,
    command: &ProcessCommand,
    error: std::io::Error,
) -> ProviderClientError {
    ProviderClientError::host_transport(
        kind,
        subcommand_for_error(command),
        None,
        ProviderDiagnostics::with_description(error.to_string()),
    )
}

fn subcommand_for_error(command: &ProcessCommand) -> String {
    command
        .args
        .first()
        .map(|arg| arg.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".to_owned())
}

fn process_status(status: ExitStatus) -> ProcessStatus {
    if let Some(code) = status.code() {
        return ProcessStatus::Exited { code };
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return ProcessStatus::SignalTerminated { signal };
        }
    }
    ProcessStatus::Unknown
}

fn exit_code(status: &ProcessStatus) -> Option<i32> {
    match status {
        ProcessStatus::Exited { code } => Some(*code),
        _ => None,
    }
}

fn process_nonzero(status: &ProcessStatus) -> bool {
    matches!(status, ProcessStatus::Exited { code } if *code != 0)
        || matches!(status, ProcessStatus::SignalTerminated { .. })
}

fn cancellation_grace(configured: Duration) -> Duration {
    configured
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
}

#[cfg(windows)]
fn configure_process_group(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP);
}

#[cfg(not(any(unix, windows)))]
fn configure_process_group(_command: &mut Command) {}

#[cfg(unix)]
fn terminate_tree(child: &mut Child) {
    let group = -(child.id() as i32);
    unsafe {
        libc::kill(group, libc::SIGTERM);
    }
}

#[cfg(windows)]
fn terminate_tree(child: &mut Child) {
    let _ = child.kill();
}

#[cfg(not(any(unix, windows)))]
fn terminate_tree(child: &mut Child) {
    let _ = child.kill();
}

#[cfg(unix)]
fn kill_tree(child: &mut Child) {
    let group = -(child.id() as i32);
    unsafe {
        libc::kill(group, libc::SIGKILL);
    }
}

#[cfg(windows)]
fn kill_tree(child: &mut Child) {
    let _ = child.kill();
}

#[cfg(not(any(unix, windows)))]
fn kill_tree(child: &mut Child) {
    let _ = child.kill();
}

pub(crate) fn is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        return true;
    }
    #[cfg(not(any(unix, windows)))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ByteAccumulator, ByteLimit, CancellationToken, ProcessCommand, ProcessLimits,
        ProcessRunner, ProcessSpawnObserver, STATUS_POLL_INTERVAL, StdoutProcessor,
        process_event_bus, write_stdin_bytes,
    };
    use crate::error::{CapturedBytes, ProviderClientError};
    use crate::testkit::{FakeProvider, FakeProviderMode, LeakProbe};
    use serde_json::json;
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[test]
    fn process_writes_one_json_object_closes_stdin_and_drains_stderr() {
        let fake = FakeProvider::compile(fake_provider_source());
        let request = serde_json::to_vec(&describe_request()).expect("request should serialize");
        let outcome = ProcessRunner::new(ProcessLimits::default())
            .run(
                ProcessCommand::new(fake.path()).arg("describe"),
                request,
                FakeProviderMode::StdinEof.env(),
            )
            .expect("stdin eof fixture should complete");

        assert!(outcome.status.exited_successfully());
        assert!(outcome.stderr_text().contains("observed stdin eof"));
        assert_eq!(outcome.argv(), &[fake.path().to_owned(), "describe".into()]);
    }

    #[test]
    fn process_concurrently_drains_stdout_and_stderr_under_pipe_pressure() {
        let fake = FakeProvider::compile(fake_provider_source());
        let request = vec![b'x'; 2 * 1024 * 1024];
        let limits = ProcessLimits {
            timeout: Duration::from_secs(3),
            stdout_limit: ByteLimit::new(64 * 1024),
            stderr_limit: ByteLimit::new(64 * 1024),
            ..ProcessLimits::default()
        };

        let outcome = ProcessRunner::new(limits)
            .run(
                ProcessCommand::new(fake.path()).arg("describe"),
                request,
                FakeProviderMode::PipePressure.env(),
            )
            .expect("pipe pressure fixture should not deadlock");

        assert!(outcome.status.exited_successfully());
        assert!(outcome.stdout.truncated);
        assert!(outcome.stderr.truncated);
        assert_eq!(outcome.stdout.captured_len, 64 * 1024);
        assert_eq!(outcome.stderr.captured_len, 64 * 1024);
    }

    #[cfg(unix)]
    #[test]
    fn spawn_observer_failure_synchronously_terminates_and_reaps_exact_child() {
        let fake = FakeProvider::compile(fake_provider_source());
        let observed_pid = Arc::new(Mutex::new(None));
        let pid_slot = Arc::clone(&observed_pid);
        let limits = ProcessLimits {
            timeout: Duration::from_secs(30),
            kill_after_grace: Duration::from_millis(25),
            spawn_observer: Some(ProcessSpawnObserver::new(move |pid| {
                *pid_slot.lock().unwrap() = Some(pid);
                Err("injected generation binding failure".to_string())
            })),
            ..ProcessLimits::default()
        };

        let error = ProcessRunner::new(limits)
            .run(
                ProcessCommand::new(fake.path()).arg("launch"),
                serde_json::to_vec(&describe_request()).expect("request should serialize"),
                FakeProviderMode::Sleep.env(),
            )
            .expect_err("observer failure must abort provider launch");

        assert_eq!(error.transport_kind(), "spawn_observer_failed");
        assert!(error.diagnostics().process_was_reaped);
        let pid = observed_pid.lock().unwrap().expect("spawned child pid") as libc::pid_t;
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "child remained live or zombie"
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
        let mut status = 0;
        assert_eq!(
            unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) },
            -1
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD),
            "exact child was not reaped"
        );
    }

    #[test]
    fn successful_exit_cleans_descendants_before_joining_pipe_drains() {
        let fake = FakeProvider::compile(fake_provider_source());
        let leak_probe = LeakProbe::new();
        let limits = ProcessLimits {
            timeout: Duration::from_secs(5),
            kill_after_grace: Duration::from_millis(50),
            ..ProcessLimits::default()
        };

        let started = std::time::Instant::now();
        let outcome = ProcessRunner::new(limits)
            .run(
                ProcessCommand::new(fake.path()).arg("describe"),
                serde_json::to_vec(&describe_request()).expect("request should serialize"),
                FakeProviderMode::ExitWithPipeHoldingDescendant.env_with_probe(&leak_probe),
            )
            .expect("successful provider exit should not wait for leaked pipe holders");

        assert!(
            started.elapsed() < Duration::from_millis(500),
            "successful provider exit waited for a descendant to close inherited pipes"
        );
        assert!(outcome.status.exited_successfully());
        leak_probe.assert_no_descendants();
    }

    #[test]
    fn host_timeout_kills_direct_child_and_descendants() {
        let fake = FakeProvider::compile(fake_provider_source());
        let leak_probe = LeakProbe::new();
        let limits = ProcessLimits {
            timeout: Duration::from_millis(150),
            kill_after_grace: Duration::from_millis(50),
            ..ProcessLimits::default()
        };

        let error = ProcessRunner::new(limits)
            .run(
                ProcessCommand::new(fake.path()).arg("describe"),
                serde_json::to_vec(&describe_request()).expect("request should serialize"),
                FakeProviderMode::ChildGrandchild.env_with_probe(&leak_probe),
            )
            .expect_err("sleeping process tree should hit host timeout");

        assert_eq!(error.transport_kind(), "host_timeout");
        leak_probe.assert_no_descendants();
    }

    #[test]
    fn explicit_cancellation_kills_process_group_and_reaps_child() {
        let fake = FakeProvider::compile(fake_provider_source());
        let token = CancellationToken::new();
        let leak_probe = LeakProbe::new();
        let limits = ProcessLimits {
            timeout: Duration::from_secs(30),
            kill_after_grace: Duration::from_millis(50),
            cancellation: Some(token.clone()),
            ..ProcessLimits::default()
        };

        token.cancel_after(Duration::from_millis(100));
        let error = ProcessRunner::new(limits)
            .run(
                ProcessCommand::new(fake.path()).arg("launch"),
                serde_json::to_vec(&describe_request()).expect("request should serialize"),
                FakeProviderMode::ChildGrandchild.env_with_probe(&leak_probe),
            )
            .expect_err("cancelled process tree should return cancellation");

        assert_eq!(error.transport_kind(), "host_cancelled");
        assert!(!error.diagnostics().process_was_force_killed);
        assert!(error.diagnostics().process_was_reaped);
        leak_probe.assert_no_descendants();
    }

    #[test]
    fn kill_after_grace_force_kills_and_reports_reaped_status() {
        let fake = FakeProvider::compile(fake_provider_source());
        let token = CancellationToken::new();
        let leak_probe = LeakProbe::new();
        let limits = ProcessLimits {
            timeout: Duration::from_secs(30),
            kill_after_grace: Duration::from_millis(25),
            cancellation: Some(token.clone()),
            ..ProcessLimits::default()
        };

        let probe_for_cancellation = leak_probe.clone();
        let cancellation = std::thread::spawn(move || {
            probe_for_cancellation.wait_for_descendants();
            token.cancel();
        });
        let error = ProcessRunner::new(limits)
            .run(
                ProcessCommand::new(fake.path()).arg("describe"),
                serde_json::to_vec(&describe_request()).expect("request should serialize"),
                FakeProviderMode::SigtermResistantChildGrandchild.env_with_probe(&leak_probe),
            )
            .expect_err("sleeping provider should be force killed after grace");
        cancellation
            .join()
            .expect("cancellation thread should complete");

        assert_eq!(error.transport_kind(), "host_cancelled");
        assert!(error.diagnostics().process_was_force_killed);
        assert!(error.diagnostics().process_was_reaped);
        leak_probe.assert_no_descendants();
    }

    #[test]
    fn leader_exit_during_grace_does_not_leave_resistant_pipe_holder() {
        let fake = FakeProvider::compile(fake_provider_source());
        let token = CancellationToken::new();
        let leak_probe = LeakProbe::new();
        let limits = ProcessLimits {
            timeout: Duration::from_secs(30),
            kill_after_grace: Duration::from_millis(100),
            cancellation: Some(token.clone()),
            ..ProcessLimits::default()
        };
        token.cancel_after(Duration::from_millis(100));

        let started = std::time::Instant::now();
        let error = ProcessRunner::new(limits)
            .run(
                ProcessCommand::new(fake.path()).arg("describe"),
                serde_json::to_vec(&describe_request()).expect("request should serialize"),
                FakeProviderMode::SigtermExitingLeaderResistantDescendant
                    .env_with_probe(&leak_probe),
            )
            .expect_err("cancelled mixed process tree should terminate");
        assert_eq!(error.transport_kind(), "host_cancelled");
        assert!(error.diagnostics().process_was_reaped);
        assert!(started.elapsed() < Duration::from_secs(2));
        leak_probe.assert_no_descendants();
    }

    #[test]
    fn cancellation_that_exits_during_grace_reports_no_force_kill() {
        let fake = FakeProvider::compile(fake_provider_source());
        let token = CancellationToken::new();
        let limits = ProcessLimits {
            timeout: Duration::from_secs(30),
            kill_after_grace: Duration::from_millis(200),
            cancellation: Some(token.clone()),
            ..ProcessLimits::default()
        };

        token.cancel_after(Duration::from_millis(50));
        let error = ProcessRunner::new(limits)
            .run(
                ProcessCommand::new(fake.path()).arg("describe"),
                serde_json::to_vec(&describe_request()).expect("request should serialize"),
                FakeProviderMode::Sleep.env(),
            )
            .expect_err("sleeping provider should be cancelled gracefully by SIGTERM");

        assert_eq!(error.transport_kind(), "host_cancelled");
        assert!(!error.diagnostics().process_was_force_killed);
        assert!(error.diagnostics().process_was_reaped);
    }

    #[test]
    fn cancellation_sends_termination_before_kill_grace_elapses() {
        let fake = FakeProvider::compile(fake_provider_source());
        let token = CancellationToken::new();
        let limits = ProcessLimits {
            timeout: Duration::from_secs(30),
            kill_after_grace: Duration::from_secs(1),
            cancellation: Some(token.clone()),
            ..ProcessLimits::default()
        };

        token.cancel_after(Duration::from_millis(100));
        let started = std::time::Instant::now();
        let error = ProcessRunner::new(limits)
            .run(
                ProcessCommand::new(fake.path()).arg("describe"),
                serde_json::to_vec(&describe_request()).expect("request should serialize"),
                FakeProviderMode::Sleep.env(),
            )
            .expect_err("sleeping provider should be cancelled");

        assert_eq!(error.transport_kind(), "host_cancelled");
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "SIGTERM was delayed until the force-kill grace elapsed"
        );
        assert!(!error.diagnostics().process_was_force_killed);
    }

    #[test]
    fn steady_state_status_polling_is_bounded() {
        let polls_per_second =
            Duration::from_secs(1).as_millis() / STATUS_POLL_INTERVAL.as_millis();

        assert_eq!(polls_per_second, 20);
        assert_eq!(polls_per_second * 100, 2_000);
    }

    #[test]
    fn stdout_line_activity_is_coalesced_to_one_pending_event() {
        let (publisher, subscriber) = process_event_bus();
        for _ in 0..100_000 {
            publisher.publish_stdout_line(std::time::Instant::now());
        }

        assert!(subscriber.take_pending().latest_stdout_line.is_some());
        assert!(subscriber.take_pending().latest_stdout_line.is_none());
    }

    #[test]
    fn stdin_payload_is_written_in_bulk() {
        let mut writer = CountingWriter::default();
        let payload = vec![b'x'; 1024 * 1024];

        write_stdin_bytes(&mut writer, &payload).expect("bulk stdin write should succeed");

        assert_eq!(writer.write_calls, 2);
        assert_eq!(writer.written, payload.len());
    }

    #[test]
    fn stdout_worker_panic_is_reported_as_wait_failure() {
        let fake = FakeProvider::compile(fake_provider_source());
        let started = std::time::Instant::now();
        let error = ProcessRunner::new(ProcessLimits::default())
            .run_with_stdout_line_gap_timeout_and_stdout_processor(
                ProcessCommand::new(fake.path()).arg("launch"),
                serde_json::to_vec(&describe_request()).expect("request should serialize"),
                FakeProviderMode::LaunchPartialHang.env(),
                PanickingStdoutProcessor,
            )
            .expect_err("stdout worker panic should fail the process outcome");

        assert_eq!(error.transport_kind(), "wait_failed");
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "worker panic remained hidden until the process timeout"
        );
        assert_eq!(
            error.diagnostics().description.as_deref(),
            Some("process worker thread panicked: stdout")
        );
    }

    #[test]
    fn byte_accumulator_is_byte_oriented_and_records_truncation_metadata() {
        let mut accumulator = ByteAccumulator::new(ByteLimit::new(5));
        accumulator.push(b"a");
        accumulator.push("é".as_bytes());
        accumulator.push(&[0, 0xff, b'z']);
        let captured = accumulator.finish();

        assert_eq!(captured.bytes, vec![b'a', 0xc3, 0xa9, 0, 0xff]);
        assert!(captured.truncated);
        assert_eq!(captured.captured_len, 5);
        assert_eq!(captured.discarded_len, 1);
        assert!(captured.contains_nul);
        assert!(captured.contains_high_bit);
    }

    #[test]
    fn early_stdin_close_continues_draining_stdout_stderr() {
        let fake = FakeProvider::compile(fake_provider_source());
        let request = vec![b'x'; 1024 * 1024];
        let outcome = ProcessRunner::new(ProcessLimits::default())
            .run(
                ProcessCommand::new(fake.path()).arg("describe"),
                request,
                FakeProviderMode::EarlyStdinSuccess.env(),
            )
            .expect("early stdin close should not stop stdout drain");

        assert!(outcome.stdin_closed_early);
        assert!(outcome.status.exited_successfully());
        assert!(outcome.stdout_text().contains("\"ok\":true"));
    }

    #[derive(Default)]
    struct CountingWriter {
        write_calls: usize,
        written: usize,
    }

    impl Write for CountingWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.write_calls += 1;
            self.written += bytes.len();
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    struct PanickingStdoutProcessor;

    impl StdoutProcessor for PanickingStdoutProcessor {
        type Output = CapturedBytes;

        fn push(&mut self, _chunk: &[u8]) -> Result<(), ProviderClientError> {
            panic!("injected stdout worker panic");
        }

        fn finish(self, _error: Option<ProviderClientError>) -> Self::Output {
            CapturedBytes::default()
        }
    }

    fn fake_provider_source() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/provider_client/fake_provider.rs")
    }

    fn describe_request() -> serde_json::Value {
        json!({
            "contract": crate::generated::CONTRACT_VERSION,
            "request_id": "request-example-001",
            "provider_instance_id": "fake-provider",
            "host": {
                "app": "oulipoly-test",
                "app_version": "0.0.0-test",
                "platform": std::env::consts::OS,
                "working_directory": ".",
                "config_root": ".",
                "data_root": ".",
                "env": {}
            },
            "params": {}
        })
    }
}
