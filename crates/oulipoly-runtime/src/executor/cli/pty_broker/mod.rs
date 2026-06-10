//! Unix PTY broker for live interactive sessions.
//!
//! ## Declared roles
//!
//! Roles: orchestration, accessor, predicate, validator, formatter, parser,
//! mapper, filter.
//!
//! This module owns PTY/control-socket orchestration. Accessor helpers retrieve
//! terminal/socket/SQLite-sidecar data, predicate helpers answer availability or
//! readiness questions, formatter helpers construct user-facing errors, parser
//! helpers decode control frames, and mapper/filter helpers project low-level
//! protocol values into runner records.

use super::spawn_identity::{SpawnIdentityContext, record_child_identity};
use super::terminal_signal::{InteractiveSignalGuard, exit_code_from_status};
use crate::observability::{ObservabilityRoot, ProductionObservabilitySnapshotService};
use oulipoly_config::ProviderConfig;
use oulipoly_state::mailbox::{MailboxDb, SessionRuntimeIdleUpdate};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions, Permissions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

mod cancel;
mod transcript_view;
mod tui;

const CONTROL_MAGIC: &[u8; 4] = b"OPTY";
const CONTROL_VERSION: u8 = 1;
const CONTROL_OP_INJECT: u8 = 1;
pub const CONTROL_MAX_PAYLOAD_BYTES: usize = 64 * 1024;
const RELAY_BUFFER_BYTES: usize = 16 * 1024;
const RELAY_POLL_TIMEOUT_MS: i32 = 25;
const INJECT_DEBOUNCE: Duration = Duration::from_millis(250);
const INJECT_WAIT_LIMIT: Duration = Duration::from_millis(1500);
const CONTROL_IO_TIMEOUT: Duration = Duration::from_secs(2);
const UNIX_SOCKET_PATH_LIMIT: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PtyControlClientErrorKind {
    Connect,
    Protocol,
    Oversize,
    EmptyPayload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtyControlClientError {
    pub kind: PtyControlClientErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtyControlResponse {
    pub ack: bool,
    pub message: String,
}

pub fn inject_control_envelope(
    path: impl AsRef<Path>,
    payload: &str,
) -> Result<PtyControlResponse, PtyControlClientError> {
    let bytes = payload.as_bytes();
    validate_client_payload(bytes)?;
    let mut stream = UnixStream::connect(path.as_ref()).map_err(connect_error)?;
    stream
        .set_read_timeout(Some(CONTROL_IO_TIMEOUT))
        .map_err(protocol_error)?;
    stream
        .set_write_timeout(Some(CONTROL_IO_TIMEOUT))
        .map_err(protocol_error)?;
    write_inject_frame(&mut stream, bytes).map_err(protocol_error)?;
    read_control_response(&mut stream)
}

pub fn control_socket_accepts_connection(path: impl AsRef<Path>) -> bool {
    UnixStream::connect(path.as_ref()).is_ok()
}

pub fn unlink_control_socket_if_owned(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    control_socket_path_is_owned(path) && fs::remove_file(path).is_ok()
}

fn control_socket_path_is_owned(path: &Path) -> bool {
    matches!(control_socket_dir(), Ok(dir) if path.starts_with(&dir))
}

pub(super) fn controlling_terminal_available() -> bool {
    open_real_terminal().is_ok()
}

pub(super) fn execute_interactive_child(
    mut cmd: Command,
    provider: &ProviderConfig,
    context: Option<&SpawnIdentityContext>,
) -> Result<ExitStatus, String> {
    let real_tty = RealTerminal::open()?;
    let winsize = terminal_winsize(real_tty.fd()).map_err(format_terminal_window_size_error)?;
    let pty = PtyPair::open(&winsize, &real_tty.original)?;
    let control = ControlSocket::bind_for(context)?;
    configure_child_pty(&mut cmd, &pty)?;
    let mut child = cmd
        .spawn()
        .map_err(|err| format_provider_spawn_error(&provider.command, err))?;
    let recorded_context = control.as_ref().and_then(|control| {
        context.map(|context| context.with_pty_control_path(control.path_string()))
    });
    let _ = record_child_identity(child.id(), recorded_context.as_ref().or(context));
    let mut idle_guard = SessionRuntimeIdleGuard::new(context);
    drop(pty.slave);

    let signal_guard = InteractiveSignalGuard::install_process_group(child.id())?;
    let mut raw_tty = real_tty.into_raw_mode()?;
    let status = relay_until_exit(&mut raw_tty, &pty.master, control.as_ref(), &mut child)?;
    idle_guard.exit_code = Some(exit_code_from_status(&status));
    drop(signal_guard);
    Ok(status)
}

/// Launch the interactive child inside the split-pane observability TUI.
///
/// Identical lifecycle to [`execute_interactive_child`] except the child PTY is
/// sized to the TOP pane (one row reserved for the collapsed monitor) and child
/// output is relayed into a virtual terminal rendered by the TUI rather than
/// written straight to the real terminal.
pub(super) fn execute_interactive_child_observed(
    mut cmd: Command,
    provider: &ProviderConfig,
    context: Option<&SpawnIdentityContext>,
) -> Result<ExitStatus, String> {
    let real_tty = RealTerminal::open()?;
    let full = terminal_winsize(real_tty.fd()).map_err(format_terminal_window_size_error)?;
    let child_winsize = tui::top_pane_winsize(&full);
    let pty = PtyPair::open(&child_winsize, &real_tty.original)?;
    let control = ControlSocket::bind_for(context)?;
    configure_child_pty(&mut cmd, &pty)?;
    let mut child = cmd
        .spawn()
        .map_err(|err| format_provider_spawn_error(&provider.command, err))?;
    let recorded_context = control.as_ref().and_then(|control| {
        context.map(|context| context.with_pty_control_path(control.path_string()))
    });
    let _ = record_child_identity(child.id(), recorded_context.as_ref().or(context));
    let mut idle_guard = SessionRuntimeIdleGuard::new(context);
    drop(pty.slave);

    let signal_guard = InteractiveSignalGuard::install_process_group(child.id())?;
    let raw_tty = real_tty.into_raw_mode()?;
    let writer = raw_tty
        .writer_clone()
        .map_err(format_tui_writer_clone_error)?;
    let monitor =
        ProductionObservabilitySnapshotService::for_session(provider.session_storage.clone());
    let root = observability_root(provider, context);
    let status = tui::relay_until_exit_observed(
        raw_tty.fd(),
        writer,
        &pty.master,
        control.as_ref(),
        &mut child,
        &monitor,
        &root,
    );
    idle_guard.exit_code = status.as_ref().ok().map(exit_code_from_status);
    drop(signal_guard);
    drop(raw_tty);
    status
}

fn format_terminal_window_size_error(err: io::Error) -> String {
    format!("Failed to read terminal window size: {err}")
}

fn format_provider_spawn_error(command: &str, err: io::Error) -> String {
    format!("Failed to spawn '{command}': {err}")
}

fn format_tui_writer_clone_error(err: io::Error) -> String {
    format!("Failed to clone terminal for TUI: {err}")
}

/// Build the observability root for the monitor from the spawn identity context.
fn observability_root(
    provider: &ProviderConfig,
    context: Option<&SpawnIdentityContext>,
) -> ObservabilityRoot {
    ObservabilityRoot {
        invocation_uuid: context.map(|context| context.invocation_uuid().to_string()),
        session_id: context
            .and_then(SpawnIdentityContext::session_id)
            .map(str::to_string),
        provider_name: Some(provider.name.clone()),
        model_name: None,
    }
}

/// Whether the split-pane observability TUI should host this interactive
/// session. Gated to a real controlling terminal that is large enough, with a
/// usable `TERM`, outside auto-wake children, and not disabled by the operator.
pub(super) fn observed_tui_enabled() -> bool {
    if !controlling_terminal_available() {
        return false;
    }
    if std::env::var_os("OULIPOLY_AUTO_WAKE").is_some() {
        return false;
    }
    if tui_disabled_by_env() {
        return false;
    }
    if terminal_is_dumb() {
        return false;
    }
    terminal_dimensions_sufficient()
}

fn tui_disabled_by_env() -> bool {
    matches!(
        std::env::var("OULIPOLY_INTERACTIVE_TUI").ok().as_deref(),
        Some("0") | Some("off") | Some("false") | Some("no")
    )
}

fn terminal_is_dumb() -> bool {
    matches!(
        std::env::var("TERM").ok().as_deref(),
        Some("dumb") | Some("")
    )
}

fn terminal_dimensions_sufficient() -> bool {
    let winsize = real_terminal_dimensions();
    terminal_dimensions_are_sufficient(winsize.as_ref())
}

fn real_terminal_dimensions() -> Option<libc::winsize> {
    let tty = open_real_terminal_for_dimension_check()?;
    read_terminal_dimensions_for_check(&tty)
}

fn open_real_terminal_for_dimension_check() -> Option<File> {
    open_real_terminal().ok()
}

fn read_terminal_dimensions_for_check(tty: &File) -> Option<libc::winsize> {
    terminal_winsize(tty.as_raw_fd()).ok()
}

fn terminal_dimensions_are_sufficient(winsize: Option<&libc::winsize>) -> bool {
    winsize.is_some_and(tui::dimensions_sufficient)
}

fn validate_client_payload(bytes: &[u8]) -> Result<(), PtyControlClientError> {
    if bytes.is_empty() {
        return Err(empty_client_payload_error());
    }
    if client_payload_is_oversize(bytes) {
        return Err(oversize_client_payload_error());
    }
    Ok(())
}

fn client_payload_is_oversize(bytes: &[u8]) -> bool {
    bytes.len() > CONTROL_MAX_PAYLOAD_BYTES
}

fn empty_client_payload_error() -> PtyControlClientError {
    PtyControlClientError {
        kind: PtyControlClientErrorKind::EmptyPayload,
        message: "empty payload".to_string(),
    }
}

fn oversize_client_payload_error() -> PtyControlClientError {
    PtyControlClientError {
        kind: PtyControlClientErrorKind::Oversize,
        message: format!("payload exceeds {CONTROL_MAX_PAYLOAD_BYTES} bytes"),
    }
}

fn connect_error(err: io::Error) -> PtyControlClientError {
    PtyControlClientError {
        kind: PtyControlClientErrorKind::Connect,
        message: err.to_string(),
    }
}

fn protocol_error(err: io::Error) -> PtyControlClientError {
    PtyControlClientError {
        kind: PtyControlClientErrorKind::Protocol,
        message: err.to_string(),
    }
}

fn read_control_response(
    stream: &mut UnixStream,
) -> Result<PtyControlResponse, PtyControlClientError> {
    let header = read_response_header(stream)?;
    validate_response_header(&header)?;
    let message = read_response_message(stream, response_payload_len(&header)?)?;
    Ok(control_response_from_parts(header[5], message))
}

fn read_response_header(stream: &mut UnixStream) -> Result<[u8; 12], PtyControlClientError> {
    let mut header = [0_u8; 12];
    stream.read_exact(&mut header).map_err(protocol_error)?;
    Ok(header)
}

fn validate_response_header(header: &[u8; 12]) -> Result<(), PtyControlClientError> {
    if &header[..4] != CONTROL_MAGIC || header[4] != CONTROL_VERSION {
        return Err(PtyControlClientError {
            kind: PtyControlClientErrorKind::Protocol,
            message: "bad response header".to_string(),
        });
    }
    if header[6] != 0 || header[7] != 0 {
        return Err(PtyControlClientError {
            kind: PtyControlClientErrorKind::Protocol,
            message: "bad response reserved bytes".to_string(),
        });
    }
    Ok(())
}

fn response_payload_len(header: &[u8; 12]) -> Result<usize, PtyControlClientError> {
    let length = parse_response_payload_len(header);
    validate_response_payload_len(length)?;
    Ok(length)
}

fn parse_response_payload_len(header: &[u8; 12]) -> usize {
    u32::from_be_bytes([header[8], header[9], header[10], header[11]]) as usize
}

fn validate_response_payload_len(length: usize) -> Result<(), PtyControlClientError> {
    if response_payload_len_is_oversize(length) {
        return Err(oversized_response_payload_error());
    }
    Ok(())
}

fn response_payload_len_is_oversize(length: usize) -> bool {
    length > CONTROL_MAX_PAYLOAD_BYTES
}

fn oversized_response_payload_error() -> PtyControlClientError {
    PtyControlClientError {
        kind: PtyControlClientErrorKind::Protocol,
        message: "oversized response".to_string(),
    }
}

fn read_response_message(
    stream: &mut UnixStream,
    length: usize,
) -> Result<String, PtyControlClientError> {
    let mut message = vec![0_u8; length];
    stream.read_exact(&mut message).map_err(protocol_error)?;
    decode_response_message(message)
}

fn decode_response_message(message: Vec<u8>) -> Result<String, PtyControlClientError> {
    String::from_utf8(message).map_err(|err| PtyControlClientError {
        kind: PtyControlClientErrorKind::Protocol,
        message: err.to_string(),
    })
}

fn control_response_from_parts(status: u8, message: String) -> PtyControlResponse {
    PtyControlResponse {
        ack: status == 0,
        message,
    }
}

fn write_inject_frame(stream: &mut UnixStream, payload: &[u8]) -> io::Result<()> {
    let header = inject_frame_header(payload);
    write_frame_parts(stream, &header, payload)
}

fn inject_frame_header(payload: &[u8]) -> [u8; 12] {
    let mut header = [0_u8; 12];
    header[..4].copy_from_slice(CONTROL_MAGIC);
    header[4] = CONTROL_VERSION;
    header[5] = CONTROL_OP_INJECT;
    header[8..12].copy_from_slice(&(payload.len() as u32).to_be_bytes());
    header
}

fn write_frame_parts(stream: &mut UnixStream, header: &[u8; 12], payload: &[u8]) -> io::Result<()> {
    stream.write_all(header)?;
    stream.write_all(payload)
}

struct PtyPair {
    master: File,
    slave: File,
}

impl PtyPair {
    fn open(winsize: &libc::winsize, termios: &libc::termios) -> Result<Self, String> {
        let (master_fd, slave_fd) =
            open_pty_fds(winsize, termios).map_err(format_pty_open_error)?;
        Ok(pty_pair_from_fds(master_fd, slave_fd))
    }
}

fn open_pty_fds(
    winsize: &libc::winsize,
    termios: &libc::termios,
) -> Result<(RawFd, RawFd), io::Error> {
    let mut master_fd = -1;
    let mut slave_fd = -1;
    let size = *winsize;
    let rc = unsafe {
        libc::openpty(
            &mut master_fd,
            &mut slave_fd,
            std::ptr::null_mut(),
            termios,
            &size,
        )
    };
    if rc == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok((master_fd, slave_fd))
}

fn format_pty_open_error(err: io::Error) -> String {
    format!("Failed to allocate PTY: {err}")
}

fn pty_pair_from_fds(master_fd: RawFd, slave_fd: RawFd) -> PtyPair {
    let master = unsafe { File::from_raw_fd(master_fd) };
    let slave = unsafe { File::from_raw_fd(slave_fd) };
    PtyPair { master, slave }
}

struct RealTerminal {
    file: File,
    original: libc::termios,
}

impl RealTerminal {
    fn open() -> Result<Self, String> {
        let file = open_real_terminal().map_err(format_real_terminal_open_error)?;
        let original =
            terminal_attrs(file.as_raw_fd()).map_err(format_real_terminal_attrs_error)?;
        Ok(real_terminal_from_parts(file, original))
    }

    fn fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }

    fn into_raw_mode(self) -> Result<RawTerminalGuard, String> {
        let raw = raw_terminal_attrs(self.original);
        set_terminal_attrs(self.fd(), &raw).map_err(format_terminal_raw_mode_error)?;
        Ok(raw_terminal_guard(self))
    }
}

fn format_real_terminal_open_error(err: io::Error) -> String {
    format!("Failed to open /dev/tty: {err}")
}

fn format_real_terminal_attrs_error(err: io::Error) -> String {
    format!("Failed to read terminal attributes: {err}")
}

fn real_terminal_from_parts(file: File, original: libc::termios) -> RealTerminal {
    RealTerminal { file, original }
}

fn raw_terminal_attrs(original: libc::termios) -> libc::termios {
    let mut raw = original;
    unsafe { libc::cfmakeraw(&mut raw) };
    raw
}

fn format_terminal_raw_mode_error(err: io::Error) -> String {
    format!("Failed to enter terminal raw mode: {err}")
}

fn raw_terminal_guard(inner: RealTerminal) -> RawTerminalGuard {
    RawTerminalGuard { inner }
}

struct RawTerminalGuard {
    inner: RealTerminal,
}

impl RawTerminalGuard {
    fn fd(&self) -> RawFd {
        self.inner.fd()
    }

    /// A writable clone of the real terminal for the TUI render backend.
    fn writer_clone(&self) -> io::Result<File> {
        self.inner.file.try_clone()
    }
}

impl Drop for RawTerminalGuard {
    fn drop(&mut self) {
        let _ = set_terminal_attrs(self.inner.fd(), &self.inner.original);
    }
}

fn open_real_terminal() -> io::Result<File> {
    OpenOptions::new().read(true).write(true).open("/dev/tty")
}

fn terminal_attrs(fd: RawFd) -> io::Result<libc::termios> {
    let mut attrs = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(fd, &mut attrs) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(attrs)
}

fn set_terminal_attrs(fd: RawFd, attrs: &libc::termios) -> io::Result<()> {
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, attrs) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn terminal_winsize(fd: RawFd) -> io::Result<libc::winsize> {
    let mut winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut winsize) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(winsize)
}

fn set_pty_winsize(fd: RawFd, winsize: &libc::winsize) -> io::Result<()> {
    if unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, winsize) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn configure_child_pty(cmd: &mut Command, pty: &PtyPair) -> Result<(), String> {
    let stdin = clone_pty_slave(&pty.slave).map_err(format_stdin_clone_error)?;
    let stdout = clone_pty_slave(&pty.slave).map_err(format_stdout_clone_error)?;
    let stderr = clone_pty_slave(&pty.slave).map_err(format_stderr_clone_error)?;
    let slave_fd = pty.slave.as_raw_fd();
    let master_fd = pty.master.as_raw_fd();
    configure_child_stdio(cmd, stdin, stdout, stderr);
    install_child_session_setup(cmd, slave_fd, master_fd);
    Ok(())
}

fn clone_pty_slave(slave: &File) -> io::Result<File> {
    slave.try_clone()
}

fn format_stdin_clone_error(err: io::Error) -> String {
    format!("Failed to clone PTY slave for stdin: {err}")
}

fn format_stdout_clone_error(err: io::Error) -> String {
    format!("Failed to clone PTY slave for stdout: {err}")
}

fn format_stderr_clone_error(err: io::Error) -> String {
    format!("Failed to clone PTY slave for stderr: {err}")
}

fn configure_child_stdio(cmd: &mut Command, stdin: File, stdout: File, stderr: File) {
    cmd.stdin(Stdio::from(stdin));
    cmd.stdout(Stdio::from(stdout));
    cmd.stderr(Stdio::from(stderr));
}

fn install_child_session_setup(cmd: &mut Command, slave_fd: RawFd, master_fd: RawFd) {
    use std::os::unix::process::CommandExt;

    unsafe {
        cmd.pre_exec(move || setup_child_session(slave_fd, master_fd));
    }
}

fn setup_child_session(slave_fd: RawFd, master_fd: RawFd) -> io::Result<()> {
    create_child_session()?;
    make_slave_controlling_terminal(slave_fd)?;
    set_child_foreground_process_group(slave_fd)?;
    close_child_side_fd(master_fd);
    close_child_side_fd(slave_fd);
    Ok(())
}

fn create_child_session() -> io::Result<()> {
    if unsafe { libc::setsid() } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn make_slave_controlling_terminal(slave_fd: RawFd) -> io::Result<()> {
    if unsafe { libc::ioctl(slave_fd, libc::TIOCSCTTY, 0) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn set_child_foreground_process_group(slave_fd: RawFd) -> io::Result<()> {
    let pid = unsafe { libc::getpid() };
    if unsafe { libc::tcsetpgrp(slave_fd, pid) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn close_child_side_fd(fd: RawFd) {
    if fd > 2 {
        unsafe { libc::close(fd) };
    }
}

struct ControlSocket {
    listener: UnixListener,
    path: PathBuf,
    owned_dir: PathBuf,
}

impl ControlSocket {
    fn bind_for(context: Option<&SpawnIdentityContext>) -> Result<Option<Self>, String> {
        let Some((session_id, invocation_uuid)) = control_socket_context(context) else {
            return Ok(None);
        };
        let (dir, path) = control_socket_location(session_id, invocation_uuid)?;
        create_private_dir(&dir)?;
        unlink_stale_or_refuse_active(&path, &dir)?;
        let listener = bind_control_listener(&path)?;
        set_control_socket_permissions(&path)?;
        Ok(Some(Self {
            listener,
            path,
            owned_dir: dir,
        }))
    }

    fn fd(&self) -> RawFd {
        self.listener.as_raw_fd()
    }

    fn path_string(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
}

fn control_socket_context(context: Option<&SpawnIdentityContext>) -> Option<(&str, &str)> {
    let context = context?;
    Some((context.session_id()?, context.invocation_uuid()))
}

fn bind_control_listener(path: &Path) -> Result<UnixListener, String> {
    let previous_umask = unsafe { libc::umask(0o077) };
    let listener =
        UnixListener::bind(path).map_err(|err| format_control_socket_bind_error(path, err));
    unsafe { libc::umask(previous_umask) };
    listener
}

fn format_control_socket_bind_error(path: &Path, err: io::Error) -> String {
    format!(
        "Failed to bind PTY control socket {}: {err}",
        path.display()
    )
}

fn set_control_socket_permissions(path: &Path) -> Result<(), String> {
    fs::set_permissions(path, Permissions::from_mode(0o600))
        .map_err(|err| format_control_socket_chmod_error(path, err))
}

fn format_control_socket_chmod_error(path: &Path, err: io::Error) -> String {
    format!(
        "Failed to chmod PTY control socket {}: {err}",
        path.display()
    )
}

impl Drop for ControlSocket {
    fn drop(&mut self) {
        unlink_owned_socket(&self.path, &self.owned_dir);
    }
}

fn control_socket_dir() -> Result<PathBuf, String> {
    if let Some(dir) = runtime_control_socket_dir() {
        return Ok(dir);
    }
    if let Some(dir) = state_control_socket_dir() {
        return Ok(dir);
    }
    let base = oulipoly_state::paths::data_dir().map_err(format_data_dir_error)?;
    Ok(data_control_socket_dir(base))
}

fn runtime_control_socket_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .map(|runtime| runtime.join("oulipoly-agent-runner/pty"))
}

fn state_control_socket_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .map(|state| state.join("oulipoly-agent-runner/runtime/pty"))
}

fn format_data_dir_error<T>(_: T) -> String {
    "Failed to resolve user data directory".to_string()
}

fn data_control_socket_dir(base: PathBuf) -> PathBuf {
    base.join("runtime/pty")
}

fn create_private_dir(dir: &Path) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|err| format_private_dir_create_error(dir, err))?;
    fs::set_permissions(dir, Permissions::from_mode(0o700))
        .map_err(|err| format_private_dir_chmod_error(dir, err))
}

fn format_private_dir_create_error(dir: &Path, err: io::Error) -> String {
    format!(
        "Failed to create PTY control socket directory {}: {err}",
        dir.display()
    )
}

fn format_private_dir_chmod_error(dir: &Path, err: io::Error) -> String {
    format!(
        "Failed to chmod PTY control socket directory {}: {err}",
        dir.display()
    )
}

fn control_socket_location(
    session_id: &str,
    invocation_uuid: &str,
) -> Result<(PathBuf, PathBuf), String> {
    let dir = control_socket_dir()?;
    Ok(control_socket_location_for_dir(
        dir,
        session_id,
        invocation_uuid,
    ))
}

fn control_socket_location_for_dir(
    dir: PathBuf,
    session_id: &str,
    invocation_uuid: &str,
) -> (PathBuf, PathBuf) {
    let path = control_socket_path(&dir, session_id, invocation_uuid);
    if socket_path_fits(&path) {
        return (dir, path);
    }
    fallback_control_socket_location(session_id, invocation_uuid)
}

fn fallback_control_socket_location(session_id: &str, invocation_uuid: &str) -> (PathBuf, PathBuf) {
    let fallback = short_control_socket_dir();
    let path = fallback_socket_path(&fallback, session_id, invocation_uuid);
    (fallback, path)
}

fn control_socket_path(dir: &Path, session_id: &str, invocation_uuid: &str) -> PathBuf {
    let basename = control_socket_basename(session_id, invocation_uuid);
    let candidate = control_socket_candidate_path(dir, &basename);
    if socket_path_fits(&candidate) {
        return candidate;
    }
    fallback_socket_path(dir, session_id, invocation_uuid)
}

fn control_socket_basename(session_id: &str, invocation_uuid: &str) -> String {
    format!(
        "{}.{}.sock",
        short_component(session_id),
        short_component(invocation_uuid)
    )
}

fn control_socket_candidate_path(dir: &Path, basename: &str) -> PathBuf {
    dir.join(basename)
}

fn socket_path_fits(path: &Path) -> bool {
    path.as_os_str().as_bytes().len() < UNIX_SOCKET_PATH_LIMIT
}

fn fallback_socket_path(dir: &Path, session_id: &str, invocation_uuid: &str) -> PathBuf {
    control_socket_candidate_path(dir, &fallback_socket_basename(session_id, invocation_uuid))
}

fn fallback_socket_basename(session_id: &str, invocation_uuid: &str) -> String {
    format!("{}.sock", stable_socket_hash(session_id, invocation_uuid))
}

fn short_control_socket_dir() -> PathBuf {
    short_control_socket_dir_for_uid(effective_uid())
}

fn effective_uid() -> libc::uid_t {
    unsafe { libc::geteuid() }
}

fn short_control_socket_dir_for_uid(uid: libc::uid_t) -> PathBuf {
    PathBuf::from("/tmp").join(short_control_socket_dir_name(uid))
}

fn short_control_socket_dir_name(uid: libc::uid_t) -> String {
    format!("oulipoly-agent-runner-pty-{uid}")
}

fn short_component(value: &str) -> String {
    let sanitized = sanitized_short_component(value);
    if component_is_empty(&sanitized) {
        "session".to_string()
    } else {
        sanitized
    }
}

fn sanitized_short_component(value: &str) -> String {
    value
        .chars()
        .filter(allowed_socket_component_char)
        .take(12)
        .collect()
}

fn allowed_socket_component_char(ch: &char) -> bool {
    ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_'
}

fn component_is_empty(value: &str) -> bool {
    value.is_empty()
}

fn stable_socket_hash(session_id: &str, invocation_uuid: &str) -> String {
    format_socket_hash(&socket_hash_digest(session_id, invocation_uuid))
}

fn socket_hash_digest(session_id: &str, invocation_uuid: &str) -> [u8; 16] {
    let mut hasher = Sha256::new();
    hasher.update(session_id.as_bytes());
    hasher.update([0]);
    hasher.update(invocation_uuid.as_bytes());
    let digest = hasher.finalize();
    digest[..16]
        .try_into()
        .expect("sha256 digest contains at least 16 bytes")
}

fn format_socket_hash(digest: &[u8; 16]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unlink_stale_or_refuse_active(path: &Path, owned_dir: &Path) -> Result<(), String> {
    if control_socket_is_absent(path) {
        return Ok(());
    }
    validate_control_socket_not_active(path)?;
    unlink_owned_socket(path, owned_dir);
    Ok(())
}

fn control_socket_is_absent(path: &Path) -> bool {
    !path.exists()
}

fn control_socket_is_active(path: &Path) -> bool {
    UnixStream::connect(path).is_ok()
}

fn validate_control_socket_not_active(path: &Path) -> Result<(), String> {
    if control_socket_is_active(path) {
        return Err(format_active_control_socket_error(path));
    }
    Ok(())
}

fn format_active_control_socket_error(path: &Path) -> String {
    format!("PTY control socket already active at {}", path.display())
}

fn unlink_owned_socket(path: &Path, owned_dir: &Path) {
    if path.starts_with(owned_dir) {
        let _ = fs::remove_file(path);
    }
}

struct SessionRuntimeIdleGuard {
    session_id: Option<String>,
    invocation_uuid: Option<String>,
    exit_code: Option<i32>,
}

impl SessionRuntimeIdleGuard {
    fn new(context: Option<&SpawnIdentityContext>) -> Self {
        Self {
            session_id: context
                .and_then(SpawnIdentityContext::session_id)
                .map(str::to_string),
            invocation_uuid: context.map(|context| context.invocation_uuid().to_string()),
            exit_code: None,
        }
    }
}

impl Drop for SessionRuntimeIdleGuard {
    fn drop(&mut self) {
        let (Some(session_id), Some(invocation_uuid)) = (&self.session_id, &self.invocation_uuid)
        else {
            return;
        };
        if let Ok(mut db) = MailboxDb::open_default() {
            let _ = db.mark_session_idle(SessionRuntimeIdleUpdate {
                session_id,
                invocation_uuid,
                last_exit_code: self.exit_code,
            });
        }
    }
}

fn relay_until_exit(
    real_tty: &mut RawTerminalGuard,
    master: &File,
    control: Option<&ControlSocket>,
    child: &mut std::process::Child,
) -> Result<ExitStatus, String> {
    let mut line_state = InputLineState::default();
    let mut current_winsize = terminal_winsize(real_tty.fd()).ok();
    let mut status = None;
    let mut buffer = vec![0_u8; RELAY_BUFFER_BYTES];
    while status.is_none() {
        maybe_propagate_winsize(
            real_tty.fd(),
            master.as_raw_fd(),
            child.id(),
            &mut current_winsize,
        );
        let ready = poll_relay_fds(
            real_tty.fd(),
            master.as_raw_fd(),
            control.map(ControlSocket::fd),
        )?;
        if ready.real_input {
            relay_real_input(
                real_tty.fd(),
                master.as_raw_fd(),
                &mut line_state,
                &mut buffer,
            )?;
        }
        if ready.pty_output {
            relay_pty_output(master.as_raw_fd(), real_tty.fd(), &mut buffer)?;
        }
        if ready.control
            && let Some(control) = control
        {
            let _ = handle_control_request(
                control,
                real_tty.fd(),
                master.as_raw_fd(),
                &mut line_state,
                &mut buffer,
            );
        }
        status = child.try_wait().map_err(format_child_poll_error)?;
    }
    drain_pty_output(master.as_raw_fd(), real_tty.fd(), &mut buffer)?;
    Ok(status.expect("status checked above"))
}

fn format_child_poll_error(err: io::Error) -> String {
    format!("Failed to poll interactive child: {err}")
}

#[derive(Default)]
struct ReadyFds {
    real_input: bool,
    pty_output: bool,
    control: bool,
}

fn poll_relay_fds(
    real_fd: RawFd,
    master_fd: RawFd,
    control_fd: Option<RawFd>,
) -> Result<ReadyFds, String> {
    let mut fds = relay_poll_fds(real_fd, master_fd, control_fd);
    poll_fds(&mut fds, format_relay_poll_error)?;
    Ok(ready_fds_from_pollfds(&fds))
}

fn relay_poll_fds(
    real_fd: RawFd,
    master_fd: RawFd,
    control_fd: Option<RawFd>,
) -> Vec<libc::pollfd> {
    let mut fds = relay_base_poll_fds(real_fd, master_fd);
    fds.extend(control_poll_fd(control_fd));
    fds
}

fn relay_base_poll_fds(real_fd: RawFd, master_fd: RawFd) -> Vec<libc::pollfd> {
    vec![poll_read_fd(real_fd), poll_read_fd(master_fd)]
}

fn control_poll_fd(control_fd: Option<RawFd>) -> Option<libc::pollfd> {
    control_fd.map(poll_read_fd)
}

fn poll_read_fd(fd: RawFd) -> libc::pollfd {
    libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    }
}

fn poll_fds(fds: &mut [libc::pollfd], format_error: fn(io::Error) -> String) -> Result<(), String> {
    let rc = unsafe {
        libc::poll(
            fds.as_mut_ptr(),
            fds.len() as libc::nfds_t,
            RELAY_POLL_TIMEOUT_MS,
        )
    };
    if rc >= 0 {
        return Ok(());
    }
    let err = io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::EINTR) {
        clear_poll_revents(fds);
        return Ok(());
    }
    Err(format_error(err))
}

fn clear_poll_revents(fds: &mut [libc::pollfd]) {
    for fd in fds {
        fd.revents = 0;
    }
}

fn format_relay_poll_error(err: io::Error) -> String {
    format!("Failed to poll PTY relay fds: {err}")
}

fn ready_fds_from_pollfds(fds: &[libc::pollfd]) -> ReadyFds {
    ReadyFds {
        real_input: readable(fds[0].revents),
        pty_output: readable(fds[1].revents),
        control: fds.get(2).is_some_and(|fd| readable(fd.revents)),
    }
}

fn readable(revents: i16) -> bool {
    revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0
}

fn relay_real_input(
    real_fd: RawFd,
    master_fd: RawFd,
    line_state: &mut InputLineState,
    buffer: &mut [u8],
) -> Result<(), String> {
    match read_fd(real_fd, buffer) {
        Ok(0) => Ok(()),
        Ok(n) => {
            line_state.observe_user_input(&buffer[..n]);
            write_all_fd(master_fd, &buffer[..n]).map_err(format_user_input_write_error)
        }
        Err(err) => Err(format_user_input_read_error(err)),
    }
}

fn format_user_input_write_error(err: io::Error) -> String {
    format!("Failed to write user input to PTY: {err}")
}

fn format_user_input_read_error(err: io::Error) -> String {
    format!("Failed to read user terminal input: {err}")
}

fn relay_pty_output(master_fd: RawFd, real_fd: RawFd, buffer: &mut [u8]) -> Result<(), String> {
    match read_fd(master_fd, buffer) {
        Ok(0) => Ok(()),
        Ok(n) => write_all_fd(real_fd, &buffer[..n]).map_err(format_pty_output_write_error),
        Err(err) if is_pty_eof_error(&err) => Ok(()),
        Err(err) => Err(format_pty_output_read_error(err)),
    }
}

fn format_pty_output_write_error(err: io::Error) -> String {
    format!("Failed to write PTY output to terminal: {err}")
}

fn format_pty_output_read_error(err: io::Error) -> String {
    format!("Failed to read PTY output: {err}")
}

fn drain_pty_output(master_fd: RawFd, real_fd: RawFd, buffer: &mut [u8]) -> Result<(), String> {
    loop {
        match drain_pty_step(master_fd, real_fd, buffer)? {
            DrainStep::Continue => {}
            DrainStep::Done => return Ok(()),
        }
    }
}

enum DrainStep {
    Continue,
    Done,
}

fn drain_pty_step(
    master_fd: RawFd,
    real_fd: RawFd,
    buffer: &mut [u8],
) -> Result<DrainStep, String> {
    if !poll_single_fd(master_fd)? {
        return Ok(DrainStep::Done);
    }
    let Some(n) = read_drain_pty_output(master_fd, buffer)? else {
        return Ok(DrainStep::Done);
    };
    write_drain_output(real_fd, &buffer[..n])?;
    Ok(DrainStep::Continue)
}

fn read_drain_pty_output(master_fd: RawFd, buffer: &mut [u8]) -> Result<Option<usize>, String> {
    read_fd(master_fd, buffer).map_or_else(drain_pty_read_error, drain_pty_read_success)
}

fn drain_pty_read_success(n: usize) -> Result<Option<usize>, String> {
    Ok(nonzero_read_len(n))
}

fn nonzero_read_len(n: usize) -> Option<usize> {
    (n != 0).then_some(n)
}

fn drain_pty_read_error(err: io::Error) -> Result<Option<usize>, String> {
    if is_pty_eof_error(&err) {
        return Ok(None);
    }
    Err(format_drain_pty_read_error(err))
}

fn write_drain_output(real_fd: RawFd, bytes: &[u8]) -> Result<(), String> {
    write_all_fd(real_fd, bytes).map_err(format_drain_pty_write_error)
}

fn format_drain_pty_read_error(err: io::Error) -> String {
    format!("Failed to drain PTY output: {err}")
}

fn format_drain_pty_write_error(err: io::Error) -> String {
    format!("Failed to drain PTY output to terminal: {err}")
}

fn poll_single_fd(fd: RawFd) -> Result<bool, String> {
    let mut pollfd = poll_read_fd(fd);
    poll_fds(std::slice::from_mut(&mut pollfd), format_drain_poll_error)?;
    Ok(readable(pollfd.revents))
}

fn format_drain_poll_error(err: io::Error) -> String {
    format!("Failed to poll PTY drain fd: {err}")
}

fn read_fd(fd: RawFd, buffer: &mut [u8]) -> io::Result<usize> {
    loop {
        let rc = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
        if rc >= 0 {
            return Ok(rc as usize);
        }
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EINTR) {
            continue;
        }
        return Err(err);
    }
}

fn write_all_fd(fd: RawFd, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        let rc = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if rc >= 0 {
            bytes = &bytes[rc as usize..];
            continue;
        }
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EINTR) {
            continue;
        }
        return Err(err);
    }
    Ok(())
}

fn is_pty_eof_error(err: &io::Error) -> bool {
    err.raw_os_error() == Some(libc::EIO)
}

fn maybe_propagate_winsize(
    real_fd: RawFd,
    master_fd: RawFd,
    child_pid: u32,
    current: &mut Option<libc::winsize>,
) {
    let Ok(next) = terminal_winsize(real_fd) else {
        return;
    };
    if current.as_ref().is_some_and(|old| winsize_eq(old, &next)) {
        return;
    }
    if set_pty_winsize(master_fd, &next).is_ok() {
        send_signal_to_child_group(child_pid, libc::SIGWINCH);
        *current = Some(next);
    }
}

fn winsize_eq(left: &libc::winsize, right: &libc::winsize) -> bool {
    left.ws_row == right.ws_row
        && left.ws_col == right.ws_col
        && left.ws_xpixel == right.ws_xpixel
        && left.ws_ypixel == right.ws_ypixel
}

fn send_signal_to_child_group(child_pid: u32, signal: i32) {
    let pgid = -(child_pid as i32);
    let _ = unsafe { libc::kill(pgid, signal) };
}

#[derive(Debug, Clone)]
struct InputLineState {
    at_line_boundary: bool,
    last_user_input_at: Option<Instant>,
}

impl Default for InputLineState {
    fn default() -> Self {
        Self {
            at_line_boundary: true,
            last_user_input_at: None,
        }
    }
}

impl InputLineState {
    fn observe_user_input(&mut self, bytes: &[u8]) {
        if no_user_input(bytes) {
            return;
        }
        self.last_user_input_at = Some(Instant::now());
        for byte in bytes.iter().copied() {
            self.at_line_boundary = line_boundary_after_input_byte(byte);
        }
    }

    fn is_safe_to_inject(&self) -> bool {
        self.at_line_boundary
            && self
                .last_user_input_at
                .map(|last| last.elapsed() >= INJECT_DEBOUNCE)
                .unwrap_or(true)
    }

    fn mark_submitted(&mut self) {
        self.at_line_boundary = true;
        self.last_user_input_at = None;
    }
}

fn no_user_input(bytes: &[u8]) -> bool {
    bytes.is_empty()
}

fn line_boundary_after_input_byte(byte: u8) -> bool {
    matches!(byte, b'\r' | b'\n' | 0x03 | 0x04 | 0x15)
}

fn handle_control_request(
    control: &ControlSocket,
    real_fd: RawFd,
    master_fd: RawFd,
    line_state: &mut InputLineState,
    buffer: &mut [u8],
) -> Result<(), String> {
    let (mut stream, _) = control
        .listener
        .accept()
        .map_err(format_control_accept_error)?;
    stream
        .set_read_timeout(Some(CONTROL_IO_TIMEOUT))
        .map_err(format_control_read_timeout_error)?;
    stream
        .set_write_timeout(Some(CONTROL_IO_TIMEOUT))
        .map_err(format_control_write_timeout_error)?;
    let response = process_control_request(&mut stream, real_fd, master_fd, line_state, buffer);
    let (ack, message) = control_response_parts(response);
    write_control_response(&mut stream, ack, &message).map_err(format_control_response_write_error)
}

fn format_control_accept_error(err: io::Error) -> String {
    format!("Failed to accept PTY control connection: {err}")
}

fn format_control_read_timeout_error(err: io::Error) -> String {
    format!("Failed to set PTY control read timeout: {err}")
}

fn format_control_write_timeout_error(err: io::Error) -> String {
    format!("Failed to set PTY control write timeout: {err}")
}

fn control_response_parts(response: Result<(), String>) -> (bool, String) {
    match response {
        Ok(()) => (true, "ok".to_string()),
        Err(message) => (false, message),
    }
}

fn format_control_response_write_error(err: io::Error) -> String {
    format!("Failed to write PTY control response: {err}")
}

fn process_control_request(
    stream: &mut UnixStream,
    real_fd: RawFd,
    master_fd: RawFd,
    line_state: &mut InputLineState,
    buffer: &mut [u8],
) -> Result<(), String> {
    validate_control_request_peer(stream)?;
    let payload = read_control_request_payload(stream)?;
    wait_until_safe_to_inject(real_fd, master_fd, line_state, buffer)?;
    submit_control_request_payload(master_fd, &payload)?;
    line_state.mark_submitted();
    Ok(())
}

fn validate_control_request_peer(stream: &UnixStream) -> Result<(), String> {
    validate_peer_uid(stream)
}

fn submit_control_request_payload(master_fd: RawFd, payload: &[u8]) -> Result<(), String> {
    write_all_fd(master_fd, payload).map_err(format_control_payload_write_error)?;
    write_all_fd(master_fd, b"\n").map_err(format_control_submit_write_error)
}

fn format_control_payload_write_error(err: io::Error) -> String {
    format!("pty_write_failed: {err}")
}

fn format_control_submit_write_error(err: io::Error) -> String {
    format!("pty_submit_failed: {err}")
}

fn read_control_request_payload(stream: &mut UnixStream) -> Result<Vec<u8>, String> {
    let header = read_control_request_header(stream)?;
    validate_control_request_header(&header)?;
    let length = control_request_payload_len(&header)?;
    let payload = read_control_request_bytes(stream, length)?;
    validate_control_request_utf8(&payload)?;
    Ok(payload)
}

fn read_control_request(stream: &mut UnixStream) -> Result<Vec<u8>, String> {
    read_control_request_payload(stream)
}

fn read_control_request_header(stream: &mut UnixStream) -> Result<[u8; 12], String> {
    let mut header = [0_u8; 12];
    stream
        .read_exact(&mut header)
        .map_err(format_control_request_header_read_error)?;
    Ok(header)
}

fn format_control_request_header_read_error(err: io::Error) -> String {
    format!("read_failed: {err}")
}

fn validate_control_request_header(header: &[u8; 12]) -> Result<(), String> {
    if &header[..4] != CONTROL_MAGIC {
        return Err("bad_magic".to_string());
    }
    if header[4] != CONTROL_VERSION {
        return Err("bad_version".to_string());
    }
    if header[5] != CONTROL_OP_INJECT {
        return Err("bad_op".to_string());
    }
    if header[6] != 0 || header[7] != 0 {
        return Err("bad_flags".to_string());
    }
    Ok(())
}

fn control_request_payload_len(header: &[u8; 12]) -> Result<usize, String> {
    let length = parse_control_request_payload_len(header);
    validate_control_request_payload_len(length)?;
    Ok(length)
}

fn parse_control_request_payload_len(header: &[u8; 12]) -> usize {
    u32::from_be_bytes([header[8], header[9], header[10], header[11]]) as usize
}

fn validate_control_request_payload_len(length: usize) -> Result<(), String> {
    if length == 0 {
        return Err("empty_payload".to_string());
    }
    if length > CONTROL_MAX_PAYLOAD_BYTES {
        return Err("oversize_frame".to_string());
    }
    Ok(())
}

fn read_control_request_bytes(stream: &mut UnixStream, length: usize) -> Result<Vec<u8>, String> {
    let mut payload = vec![0_u8; length];
    stream
        .read_exact(&mut payload)
        .map_err(format_control_request_payload_read_error)?;
    Ok(payload)
}

fn format_control_request_payload_read_error(err: io::Error) -> String {
    format!("read_payload_failed: {err}")
}

fn validate_control_request_utf8(payload: &[u8]) -> Result<(), String> {
    parse_control_request_utf8(payload)
        .map(|_| ())
        .map_err(|_| invalid_utf8_error())
}

fn parse_control_request_utf8(payload: &[u8]) -> Result<&str, std::str::Utf8Error> {
    std::str::from_utf8(payload)
}

fn invalid_utf8_error() -> String {
    "invalid_utf8".to_string()
}

fn wait_until_safe_to_inject(
    real_fd: RawFd,
    master_fd: RawFd,
    line_state: &mut InputLineState,
    buffer: &mut [u8],
) -> Result<(), String> {
    pump_until_safe_to_inject(real_fd, master_fd, line_state, buffer)?;
    validate_safe_to_inject(line_state)
}

fn pump_until_safe_to_inject(
    real_fd: RawFd,
    master_fd: RawFd,
    line_state: &mut InputLineState,
    buffer: &mut [u8],
) -> Result<(), String> {
    let start = Instant::now();
    while start.elapsed() < INJECT_WAIT_LIMIT {
        if line_state.is_safe_to_inject() {
            return Ok(());
        }
        pump_injection_wait_io(real_fd, master_fd, line_state, buffer)?;
    }
    Ok(())
}

fn pump_injection_wait_io(
    real_fd: RawFd,
    master_fd: RawFd,
    line_state: &mut InputLineState,
    buffer: &mut [u8],
) -> Result<(), String> {
    let ready = poll_relay_fds(real_fd, master_fd, None)?;
    if ready.real_input {
        relay_real_input(real_fd, master_fd, line_state, buffer)?;
    }
    if ready.pty_output {
        relay_pty_output(master_fd, real_fd, buffer)?;
    }
    Ok(())
}

fn validate_safe_to_inject(line_state: &InputLineState) -> Result<(), String> {
    if line_state.is_safe_to_inject() {
        return Ok(());
    }
    Err("unsafe_mid_line".to_string())
}

fn write_control_response(stream: &mut UnixStream, ack: bool, message: &str) -> io::Result<()> {
    let (header, bytes) = control_response_frame(ack, message);
    write_frame_parts(stream, &header, bytes)
}

fn control_response_frame(ack: bool, message: &str) -> ([u8; 12], &[u8]) {
    let mut header = [0_u8; 12];
    header[..4].copy_from_slice(CONTROL_MAGIC);
    header[4] = CONTROL_VERSION;
    header[5] = if ack { 0 } else { 1 };
    let bytes = message.as_bytes();
    header[8..12].copy_from_slice(&(bytes.len() as u32).to_be_bytes());
    (header, bytes)
}

#[cfg(target_os = "linux")]
fn validate_peer_uid(stream: &UnixStream) -> Result<(), String> {
    let cred = peer_credentials(stream).map_err(format_peercred_error)?;
    validate_credential_uid(cred.uid, effective_uid())
}

#[cfg(target_os = "linux")]
fn peer_credentials(stream: &UnixStream) -> Result<libc::ucred, io::Error> {
    let mut cred = unsafe { std::mem::zeroed::<libc::ucred>() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    if rc == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(cred)
}

fn format_peercred_error(err: io::Error) -> String {
    format!("peercred_failed: {err}")
}

#[cfg(target_os = "linux")]
fn validate_credential_uid(actual: libc::uid_t, expected: libc::uid_t) -> Result<(), String> {
    if actual != expected {
        return Err("peer_uid_mismatch".to_string());
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn validate_peer_uid(_stream: &UnixStream) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn input_line_state_tracks_boundary_and_debounce() {
        let mut state = InputLineState::default();
        assert!(state.is_safe_to_inject());

        state.observe_user_input(b"abc");
        assert!(!state.at_line_boundary);
        assert!(!state.is_safe_to_inject());

        state.observe_user_input(b"\n");
        assert!(state.at_line_boundary);
        assert!(!state.is_safe_to_inject());
        state.last_user_input_at =
            Some(Instant::now() - INJECT_DEBOUNCE - Duration::from_millis(1));
        assert!(state.is_safe_to_inject());
    }

    #[test]
    fn client_rejects_empty_or_oversize_payload() {
        assert_eq!(
            validate_client_payload(b"").unwrap_err().kind,
            PtyControlClientErrorKind::EmptyPayload
        );
        let oversized = vec![b'x'; CONTROL_MAX_PAYLOAD_BYTES + 1];
        assert_eq!(
            validate_client_payload(&oversized).unwrap_err().kind,
            PtyControlClientErrorKind::Oversize
        );
    }

    #[test]
    fn socket_path_uses_hash_when_runtime_dir_is_long() {
        let dir = PathBuf::from(format!("/tmp/{}", "x".repeat(140)));
        let path = control_socket_path(&dir, "session-a", "invocation-a");
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with(".sock")
        );
        assert!(path.file_name().unwrap().to_string_lossy().len() < 40);
    }

    #[test]
    fn control_socket_location_falls_back_when_directory_is_too_long() {
        let dir = PathBuf::from(format!("/tmp/{}", "x".repeat(140)));
        let (_dir, path) = control_socket_location_for_dir(dir, "session-a", "invocation-a");

        assert!(path.as_os_str().as_bytes().len() < UNIX_SOCKET_PATH_LIMIT);
        assert!(path.starts_with(short_control_socket_dir()));
    }

    #[test]
    fn control_request_inject_writes_payload_and_submit_delimiter() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        write_inject_frame(&mut client, b"[OULIPOLY NOTIFICATIONS]").unwrap();
        let (real_read_end, _real_write_end) = pipe_files();
        let (read_end, write_end) = pipe_files();
        let mut state = InputLineState::default();
        let mut buffer = [0_u8; 256];

        process_control_request(
            &mut server,
            real_read_end.as_raw_fd(),
            write_end.as_raw_fd(),
            &mut state,
            &mut buffer,
        )
        .unwrap();
        drop(write_end);

        let mut read_end = read_end;
        let mut received = String::new();
        read_end.read_to_string(&mut received).unwrap();
        assert_eq!(received, "[OULIPOLY NOTIFICATIONS]\n");
    }

    #[test]
    fn control_request_wait_observes_newline_from_real_input() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        write_inject_frame(&mut client, b"notify").unwrap();
        let (real_read_end, real_write_end) = pipe_files();
        let (read_end, write_end) = pipe_files();
        let mut state = InputLineState::default();
        state.observe_user_input(b"partial");
        let mut buffer = [0_u8; 256];
        let writer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            let mut real_write_end = real_write_end;
            real_write_end.write_all(b"\n").unwrap();
        });

        process_control_request(
            &mut server,
            real_read_end.as_raw_fd(),
            write_end.as_raw_fd(),
            &mut state,
            &mut buffer,
        )
        .unwrap();
        writer.join().unwrap();
        drop(write_end);

        let mut read_end = read_end;
        let mut received = String::new();
        read_end.read_to_string(&mut received).unwrap();
        assert_eq!(received, "\nnotify\n");
    }

    #[test]
    fn control_request_unsafe_midline_returns_err() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        write_inject_frame(&mut client, b"notify").unwrap();
        let (real_read_end, _real_write_end) = pipe_files();
        let (_read_end, write_end) = pipe_files();
        let mut state = InputLineState::default();
        state.observe_user_input(b"partial");
        let mut buffer = [0_u8; 256];

        let err = process_control_request(
            &mut server,
            real_read_end.as_raw_fd(),
            write_end.as_raw_fd(),
            &mut state,
            &mut buffer,
        )
        .unwrap_err();

        assert_eq!(err, "unsafe_mid_line");
    }

    #[test]
    fn control_request_rejects_bad_magic_or_oversize_frame() {
        let (mut bad_client, mut bad_server) = UnixStream::pair().unwrap();
        bad_client
            .write_all(b"BAD!\x01\x01\x00\x00\x00\x00\x00\x04test")
            .unwrap();
        assert_eq!(
            read_control_request(&mut bad_server).unwrap_err(),
            "bad_magic"
        );

        let (mut big_client, mut big_server) = UnixStream::pair().unwrap();
        let mut header = [0_u8; 12];
        header[..4].copy_from_slice(CONTROL_MAGIC);
        header[4] = CONTROL_VERSION;
        header[5] = CONTROL_OP_INJECT;
        header[8..12].copy_from_slice(&((CONTROL_MAX_PAYLOAD_BYTES as u32) + 1).to_be_bytes());
        big_client.write_all(&header).unwrap();
        assert_eq!(
            read_control_request(&mut big_server).unwrap_err(),
            "oversize_frame"
        );
    }

    fn pipe_files() -> (File, File) {
        let mut fds = [-1; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let read_end = unsafe { File::from_raw_fd(fds[0]) };
        let write_end = unsafe { File::from_raw_fd(fds[1]) };
        (read_end, write_end)
    }
}
