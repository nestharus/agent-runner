//! ## Declared roles
//!
//! Roles: orchestration, mapper, predicate, accessor, filter, validator.
//!
//! - orchestration: `ProcessRunner` starts provider subprocesses, drains
//!   stdio on worker threads, supervises total-runtime or stdout-line-gap
//!   liveness, and terminates/reaps process trees on timeout or cancellation.
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
//!   pipe pressure, process-tree cleanup, cancellation, and truncation
//!   contracts.
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
//!       - CancellationToken and ProcessLimits lifecycle inputs
//!       - ProcessCommand, ProcessOutcome, and ProcessRunner public surfaces
//!       - total-runtime and stdout-line-gap timeout behavior
//!       - cross-platform process group termination and executable checks
//! ```

use crate::error::{HostErrorKind, ProviderClientError, ProviderDiagnostics};
use crate::generated::ProcessStatus;
use std::ffi::{OsStr, OsString};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

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
}

impl StdoutDrainOutput for CapturedBytes {
    fn captured_bytes(&self) -> CapturedBytes {
        self.clone()
    }
}

pub(crate) trait StdoutProcessor: Send + 'static {
    type Output: StdoutDrainOutput;

    fn push(&mut self, chunk: &[u8]);
    fn finish(self) -> Self::Output;
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

    fn push(&mut self, chunk: &[u8]) {
        self.accumulator.push(chunk);
    }

    fn finish(self) -> Self::Output {
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
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    pub fn cancel_after(&self, duration: Duration) {
        let token = self.clone();
        thread::spawn(move || {
            thread::sleep(duration);
            token.cancel();
        });
    }
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
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
    callback: Arc<dyn Fn(u32) + Send + Sync>,
}

impl ProcessSpawnObserver {
    pub fn new(callback: impl Fn(u32) + Send + Sync + 'static) -> Self {
        Self {
            callback: Arc::new(callback),
        }
    }

    fn observe(&self, child_id: u32) {
        (self.callback)(child_id);
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
}

impl ProcessCommand {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
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

struct TerminatedProcess {
    status: Option<ExitStatus>,
    force_killed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimeoutMode {
    TotalRuntime,
    StdoutLineGap,
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
        notify_spawn_observer(&self.limits.spawn_observer, child.id());
        let (stdout_line_activity, stdout_line_rx) = stdout_line_activity_channel(timeout_mode);
        let threads = start_process_threads(
            &mut child,
            stdin_bytes,
            self.limits.stderr_limit,
            stdout_line_activity,
            stdout_processor,
        );

        let started = Instant::now();
        let mut last_stdout_line = started;
        let mut cancellation_started = None;
        loop {
            record_stdout_line_activity(&stdout_line_rx, &mut last_stdout_line);

            if let Some(status) = poll_child_status(&mut child, &command)? {
                return Ok(map_completed_process_outcome(
                    status,
                    threads,
                    cancellation_started.is_some(),
                    &self.limits,
                    argv,
                ));
            }

            if timeout_expired(timeout_mode, started, last_stdout_line, self.limits.timeout) {
                return Err(self.terminate_and_collect(
                    child,
                    command,
                    threads,
                    HostErrorKind::Timeout,
                    false,
                ));
            }

            if cancellation_requested(&self.limits) {
                let cancelled_at = cancellation_started.get_or_insert_with(Instant::now);
                if cancellation_grace_expired(cancelled_at, self.limits.kill_after_grace) {
                    return Err(self.terminate_and_collect(
                        child,
                        command,
                        threads,
                        HostErrorKind::Cancelled,
                        true,
                    ));
                }
            }

            thread::sleep(Duration::from_millis(5));
        }
    }

    fn terminate_and_collect<T: StdoutDrainOutput>(
        &self,
        mut child: Child,
        command: ProcessCommand,
        threads: ProcessThreads<T>,
        kind: HostErrorKind,
        host_cancellation_requested: bool,
    ) -> ProviderClientError {
        terminate_tree(&mut child);
        let terminated = wait_for_terminated_process(&mut child, self.limits.kill_after_grace);
        let diagnostics =
            map_termination_diagnostics(threads, terminated, host_cancellation_requested);
        termination_transport_error(kind, &command, diagnostics)
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
    configure_provider_process(&mut process);
    process
}

fn provider_process_command<I, K, V>(command: &ProcessCommand, envs: I) -> Command
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let mut process = Command::new(&command.program);
    process
        .args(&command.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in envs {
        process.env(key, value);
    }
    process
}

fn configure_provider_process(process: &mut Command) {
    configure_process_group(process);
}

fn start_process_threads<P: StdoutProcessor>(
    child: &mut Child,
    stdin_bytes: Vec<u8>,
    stderr_limit: ByteLimit,
    stdout_line_activity: Option<Sender<Instant>>,
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
    ProcessThreads {
        stdout: thread::spawn(move || {
            drain_reader_with_processor(stdout, stdout_line_activity, stdout_processor)
        }),
        stderr: thread::spawn(move || drain_reader(stderr, stderr_limit, None)),
        stdin: thread::spawn(move || write_stdin(stdin, stdin_bytes)),
    }
}

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
    cancellation_started: bool,
    limits: &ProcessLimits,
    argv: Vec<OsString>,
) -> ProcessOutcome<T> {
    ProcessOutcome {
        status: process_status(status),
        stdout: join_stdout(threads.stdout),
        stderr: join_capture(threads.stderr),
        stdin_closed_early: join_stdin(threads.stdin),
        host_cancellation_requested: host_cancellation_was_requested(cancellation_started, limits),
        argv,
    }
}

fn host_cancellation_was_requested(cancellation_started: bool, limits: &ProcessLimits) -> bool {
    cancellation_started
        || limits
            .cancellation
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
}

fn stdout_line_activity_channel(
    timeout_mode: TimeoutMode,
) -> (Option<Sender<Instant>>, Option<Receiver<Instant>>) {
    match timeout_mode {
        TimeoutMode::StdoutLineGap => {
            let (tx, rx) = mpsc::channel();
            (Some(tx), Some(rx))
        }
        TimeoutMode::TotalRuntime => (None, None),
    }
}

fn record_stdout_line_activity(rx: &Option<Receiver<Instant>>, last_stdout_line: &mut Instant) {
    let Some(rx) = rx else {
        return;
    };
    while let Ok(observed_at) = rx.try_recv() {
        *last_stdout_line = observed_at;
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
    termination_diagnostics_from_parts(
        terminated,
        host_cancellation_requested,
        joined.stdout.captured_bytes(),
        joined.stderr,
    )
}

struct JoinedProcessThreads<T: StdoutDrainOutput> {
    stdout: T,
    stderr: CapturedBytes,
    _stdin_closed_early: bool,
}

fn join_process_threads<T: StdoutDrainOutput>(
    threads: ProcessThreads<T>,
) -> JoinedProcessThreads<T> {
    JoinedProcessThreads {
        stdout: join_stdout(threads.stdout),
        stderr: join_capture(threads.stderr),
        _stdin_closed_early: join_stdin(threads.stdin),
    }
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

fn notify_spawn_observer(observer: &Option<ProcessSpawnObserver>, child_id: u32) {
    if let Some(observer) = observer {
        observer.observe(child_id);
    }
}

fn drain_reader(
    mut reader: impl Read,
    limit: ByteLimit,
    stdout_line_activity: Option<Sender<Instant>>,
) -> CapturedBytes {
    drain_reader_with_processor(
        &mut reader,
        stdout_line_activity,
        ByteCaptureProcessor::new(limit),
    )
}

fn drain_reader_with_processor<P: StdoutProcessor>(
    mut reader: impl Read,
    stdout_line_activity: Option<Sender<Instant>>,
    mut processor: P,
) -> P::Output {
    let mut buffer = [0_u8; 8192];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                let chunk = &buffer[..read];
                notify_stdout_line_activity(&stdout_line_activity, chunk);
                processor.push(chunk);
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    processor.finish()
}

fn notify_stdout_line_activity(activity: &Option<Sender<Instant>>, chunk: &[u8]) {
    let Some(activity) = activity else {
        return;
    };
    for _ in chunk.iter().filter(|byte| **byte == b'\n') {
        let _ = activity.send(Instant::now());
    }
}

fn write_stdin(mut stdin: impl Write, bytes: Vec<u8>) -> bool {
    match write_stdin_bytes(&mut stdin, &bytes) {
        Ok(()) => stdin_flush_closed_early(&mut stdin),
        Err(error) => stdin_error_closed_early(&error),
    }
}

fn write_stdin_bytes(stdin: &mut impl Write, bytes: &[u8]) -> std::io::Result<()> {
    for (index, byte) in bytes.iter().enumerate() {
        stdin.write_all(std::slice::from_ref(byte))?;
        if index == 0 {
            thread::sleep(Duration::from_millis(10));
        }
    }
    Ok(())
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

fn join_capture(handle: thread::JoinHandle<CapturedBytes>) -> CapturedBytes {
    handle.join().unwrap_or_default()
}

fn join_stdout<T: StdoutDrainOutput>(handle: thread::JoinHandle<T>) -> T {
    handle.join().unwrap_or_default()
}

fn join_stdin(handle: thread::JoinHandle<bool>) -> bool {
    handle.join().unwrap_or(true)
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
        ByteAccumulator, ByteLimit, CancellationToken, ProcessCommand, ProcessLimits, ProcessRunner,
    };
    use crate::testkit::{FakeProvider, FakeProviderMode, LeakProbe};
    use serde_json::json;
    use std::path::PathBuf;
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
