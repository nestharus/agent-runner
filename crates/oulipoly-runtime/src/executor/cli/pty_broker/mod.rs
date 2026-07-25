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
use crate::observability::{
    ObservabilityRoot, ObservabilitySnapshotPort, ProductionObservabilitySnapshotService,
};
use crate::provider_registry::ProviderRegistry;
use crate::session_provider::SessionProviderIdentity;
use chrono::{SecondsFormat, Utc};
use oulipoly_config::ProviderConfig;
use oulipoly_state::mailbox::{MailboxDb, MailboxRow, SessionRuntimeIdleUpdate};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions, Permissions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

mod cancel;
mod outbound_observer;
mod snapshot_worker;
mod transcript_view;
mod tui;

const CONTROL_MAGIC: &[u8; 4] = b"OPTY";
const CONTROL_VERSION: u8 = 1;
const CONTROL_OP_INJECT: u8 = 1;
pub const CONTROL_MAX_PAYLOAD_BYTES: usize = 64 * 1024;
const RELAY_BUFFER_BYTES: usize = 16 * 1024;
const RELAY_POLL_TIMEOUT_MS: i32 = 25;
/// Short child-output debounce before proactive injection.
/// This is not an idle/quiescence requirement: redraw-oriented TUIs can emit
/// periodic prompt/status bytes while idle, so this window only avoids writing
/// into a rapid burst or between nearby terminal bytes.
const INJECT_CHILD_OUTPUT_DEBOUNCE: Duration = Duration::from_millis(125);
pub const USER_INPUT_IDLE_INJECT_MS: u64 = 1500;
/// User-input idle fallback for proactive injection when terminal Enter parsing
/// missed a submit sequence. Lower values deliver faster but increase the risk
/// of appending to a user's brief composing pause; higher values delay delivery.
const USER_INPUT_IDLE_INJECT: Duration = Duration::from_millis(USER_INPUT_IDLE_INJECT_MS);
const INJECT_WAIT_LIMIT: Duration = Duration::from_millis(1500);
const CONTROL_IO_TIMEOUT: Duration = Duration::from_secs(2);
const DELIVERY_ATTEMPT_PREFIX: &str = "[OULIPOLY-DELIVERY ";
const DELIVERY_ATTEMPT_SUFFIX: char = ']';
const UNIX_SOCKET_PATH_LIMIT: usize = 100;
const NOTIFY_TRACE_MAX_BYTES: u64 = 1024 * 1024;
const NOTIFY_TRACE_FILE: &str = "notify-trace.log";
const NOTIFY_TRACE_ROTATED_FILE: &str = "notify-trace.log.1";
const OVERLAY_INPUT_TRACE_MAX_BYTES: u64 = 1024 * 1024;
const OVERLAY_INPUT_TRACE_FILE: &str = "overlay-input-trace.log";
const OVERLAY_INPUT_TRACE_ROTATED_FILE: &str = "overlay-input-trace.log.1";
const BOUNDARY_PROBE_MAX_BYTES: usize = 16;

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

pub(super) struct ProviderInspectMonitorContext {
    registry: Arc<ProviderRegistry>,
}

struct ProviderSessionObservationContext {
    registry: Arc<ProviderRegistry>,
    identity: SessionProviderIdentity,
    provider_session_id: String,
    invocation_uuid: String,
    effective_cwd: Option<PathBuf>,
}

impl ProviderInspectMonitorContext {
    pub(super) fn new(registry: Arc<ProviderRegistry>) -> Self {
        Self { registry }
    }
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

pub fn render_mailbox_notification_envelope(
    rows: &[MailboxRow],
    remaining_count: usize,
    attempt_id: &str,
) -> String {
    let mut rendered = String::new();
    rendered.push_str("[OULIPOLY NOTIFICATIONS]\n");
    rendered.push_str(
        "The following background agent-bash workloads completed while this session was inactive.\n\n",
    );
    for (index, row) in rows.iter().enumerate() {
        rendered.push_str(&format!(
            "{}. kind: {}\n   handle: {}\n   rc: {}\n   state_dir: {}\n   meta: {}\n   log: {}\n   rc_file: {}\n\n",
            index + 1,
            sanitize_mailbox_value(&row.kind),
            sanitize_mailbox_value(&row.handle),
            row.rc,
            quote_mailbox_path(&row.state_dir),
            quote_mailbox_path(&row.meta_path),
            quote_mailbox_path(&row.log_path),
            quote_mailbox_path(&row.rc_path),
        ));
    }
    if remaining_count > 0 {
        rendered.push_str(&format!(
            "{remaining_count} additional notification(s) remain queued for the next resume.\n\n"
        ));
    }
    rendered.push_str(
        "Use the paths above if you need details. Do not assume log content unless you inspect it.\n",
    );
    rendered.push_str(DELIVERY_ATTEMPT_PREFIX);
    rendered.push_str(attempt_id);
    rendered.push(DELIVERY_ATTEMPT_SUFFIX);
    rendered.push('\n');
    rendered.push_str("[END OULIPOLY NOTIFICATIONS]");
    rendered
}

fn quote_mailbox_path(path: &str) -> String {
    format!("\"{}\"", sanitize_mailbox_value(path))
}

fn sanitize_mailbox_value(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\n', "\\n")
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
/// sized to the TOP pane (persistent overlay rows reserved) and child
/// output is relayed into a virtual terminal rendered by the TUI rather than
/// written straight to the real terminal.
pub(super) fn execute_interactive_child_observed(
    mut cmd: Command,
    provider: &ProviderConfig,
    context: Option<&SpawnIdentityContext>,
    provider_inspect: Option<&ProviderInspectMonitorContext>,
) -> Result<ExitStatus, String> {
    let real_tty = RealTerminal::open()?;
    let full = terminal_winsize(real_tty.fd()).map_err(format_terminal_window_size_error)?;
    let child_winsize = tui::top_pane_winsize(&full);
    let pty = PtyPair::open(&child_winsize, &real_tty.original)?;
    let control = ControlSocket::bind_for(context)?;
    configure_child_pty(&mut cmd, &pty)?;
    let provider_session =
        provider_session_observation_context(provider, context, provider_inspect);
    let monitor: Box<dyn ObservabilitySnapshotPort + Send> = Box::new(observability_monitor(
        provider,
        provider_inspect.is_some(),
        provider_session.as_ref().ok(),
    ));
    let outbound_source = outbound_observer_source(provider_session);
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
    let root = observability_root(provider, context);
    let status = tui::relay_until_exit_observed(
        raw_tty.fd(),
        writer,
        &pty.master,
        control.as_ref(),
        &mut child,
        monitor,
        root,
        outbound_source,
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
        model_name: context
            .and_then(SpawnIdentityContext::model_name)
            .map(str::to_string),
    }
}

fn observability_monitor(
    provider: &ProviderConfig,
    provider_inspect_requested: bool,
    provider_session: Option<&ProviderSessionObservationContext>,
) -> ProductionObservabilitySnapshotService {
    match provider_session {
        Some(provider_session) => provider_inspect_monitor(provider_session),
        None if provider_inspect_requested => {
            ProductionObservabilitySnapshotService::for_session(None)
        }
        None => {
            ProductionObservabilitySnapshotService::for_session(provider.session_storage.clone())
        }
    }
}

fn provider_inspect_monitor(
    context: &ProviderSessionObservationContext,
) -> ProductionObservabilitySnapshotService {
    ProductionObservabilitySnapshotService::for_provider_inspect_registry(
        Arc::clone(&context.registry),
        context.identity.clone(),
        context.provider_session_id.clone(),
        context.effective_cwd.clone(),
    )
}

fn provider_session_observation_context(
    provider: &ProviderConfig,
    context: Option<&SpawnIdentityContext>,
    inspect: Option<&ProviderInspectMonitorContext>,
) -> Result<ProviderSessionObservationContext, &'static str> {
    let context = context.ok_or("awaiting_session_identity")?;
    let model_name = context.model_name().ok_or("awaiting_session_identity")?;
    let provider_session_id = context.session_id().ok_or("awaiting_session_identity")?;
    let inspect = inspect.ok_or("session_turn_source_unavailable")?;
    let identity = provider_inspect_identity(inspect.registry.as_ref(), model_name, &provider.name)
        .ok_or("session_turn_source_unavailable")?;
    Ok(ProviderSessionObservationContext {
        registry: Arc::clone(&inspect.registry),
        identity,
        provider_session_id: provider_session_id.to_string(),
        invocation_uuid: context.invocation_uuid().to_string(),
        effective_cwd: context.effective_cwd().map(PathBuf::from),
    })
}

fn outbound_observer_source(
    context: Result<ProviderSessionObservationContext, &'static str>,
) -> outbound_observer::OutboundObserverSource {
    match context {
        Ok(context) => outbound_observer::OutboundObserverSource::Provider(
            outbound_observer::ProviderSessionTurnSource::new(
                context.registry,
                context.identity,
                context.provider_session_id,
                context.invocation_uuid,
                context.effective_cwd,
            ),
        ),
        Err(detail) => outbound_observer::OutboundObserverSource::Unavailable(detail.to_string()),
    }
}

fn provider_inspect_identity(
    registry: &ProviderRegistry,
    model_name: &str,
    provider_name: &str,
) -> Option<SessionProviderIdentity> {
    let model_name =
        registry.resolve_model_name_for_provider_instance(model_name, provider_name)?;
    let describe = registry
        .describe_model_provider_instance(&model_name, provider_name)
        .ok()?;
    if !describe.capabilities.session {
        return None;
    }
    Some(SessionProviderIdentity {
        model_name,
        provider_name: provider_name.to_string(),
        provider_instance_id: Some(format_provider_instance_id(&describe.provider_id)),
        settings_id: provider_name.to_string(),
    })
}

fn format_provider_instance_id(provider_id: &str) -> String {
    format!("{provider_id}-instance")
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
    session_id: String,
    invocation_uuid: String,
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
            session_id: session_id.to_string(),
            invocation_uuid: invocation_uuid.to_string(),
        }))
    }

    fn fd(&self) -> RawFd {
        self.listener.as_raw_fd()
    }

    fn path_string(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }

    fn session_id(&self) -> &str {
        &self.session_id
    }

    fn invocation_uuid(&self) -> &str {
        &self.invocation_uuid
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
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)?;
    runtime_control_socket_dir_for(runtime)
}

/// Resolve the runtime-tier control-socket directory for a given
/// `XDG_RUNTIME_DIR` value, falling through when it is unusable.
///
/// `XDG_RUNTIME_DIR` is created and owned by the login session. If the variable
/// still points at a directory that no longer exists — e.g. the runtime tmpfs
/// (`/run/user/<uid>`) was torn down by a host/WSL crash and not recreated —
/// then binding a control socket underneath it fails (the user cannot recreate
/// `/run/user/<uid>`), which would turn every interactive spawn into a
/// `spawn_error`. Returning `None` here lets `control_socket_dir` fall through
/// to the state/data locations instead of hard-failing.
fn runtime_control_socket_dir_for(runtime: PathBuf) -> Option<PathBuf> {
    if !runtime.is_dir() {
        return None;
    }
    Some(runtime.join("oulipoly-agent-runner/pty"))
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
            if db
                .accepted_delivery_attempt_windows(session_id)
                .is_ok_and(|windows| {
                    windows.iter().any(|window| {
                        window.delivery_invocation_uuid.as_str() == invocation_uuid.as_str()
                    })
                })
            {
                return;
            }
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
    let mut pending_child_input = PendingChildInput::new();
    let mut child_output_state = ChildOutputState::default();
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
            !pending_child_input.is_empty(),
        )?;
        if ready.pty_writable {
            flush_pending_child_input(master.as_raw_fd(), &mut pending_child_input)?;
        }
        if ready.real_input {
            relay_real_input(
                real_tty.fd(),
                &mut line_state,
                &mut pending_child_input,
                &mut buffer,
            )?;
        }
        if ready.pty_output && relay_pty_output(master.as_raw_fd(), real_tty.fd(), &mut buffer)? {
            child_output_state.observe_child_output();
        }
        if ready.control
            && let Some(control) = control
        {
            let mut request_io = ControlRequestIo {
                real_fd: real_tty.fd(),
                master_fd: master.as_raw_fd(),
                child_pid: Some(child.id()),
                line_state: &mut line_state,
                child_output_state: &mut child_output_state,
                pending_child_input: &mut pending_child_input,
                buffer: &mut buffer,
            };
            let _ = handle_control_request(control, &mut request_io);
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
    pty_writable: bool,
    control: bool,
}

fn poll_relay_fds(
    real_fd: RawFd,
    master_fd: RawFd,
    control_fd: Option<RawFd>,
    want_child_write: bool,
) -> Result<ReadyFds, String> {
    let mut fds = relay_poll_fds(real_fd, master_fd, control_fd, want_child_write);
    poll_fds(&mut fds, format_relay_poll_error)?;
    Ok(ready_fds_from_pollfds(&fds))
}

fn relay_poll_fds(
    real_fd: RawFd,
    master_fd: RawFd,
    control_fd: Option<RawFd>,
    want_child_write: bool,
) -> Vec<libc::pollfd> {
    let mut fds = relay_base_poll_fds(real_fd, master_fd, want_child_write);
    fds.extend(control_poll_fd(control_fd));
    fds
}

fn relay_base_poll_fds(
    real_fd: RawFd,
    master_fd: RawFd,
    want_child_write: bool,
) -> Vec<libc::pollfd> {
    vec![
        poll_read_fd(real_fd),
        poll_master_fd(master_fd, want_child_write),
    ]
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

fn poll_master_fd(fd: RawFd, want_child_write: bool) -> libc::pollfd {
    let mut pollfd = poll_read_fd(fd);
    if want_child_write {
        pollfd.events |= libc::POLLOUT;
    }
    pollfd
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
        pty_writable: writable(fds[1].revents),
        control: fds.get(2).is_some_and(|fd| readable(fd.revents)),
    }
}

fn readable(revents: i16) -> bool {
    revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0
}

fn writable(revents: i16) -> bool {
    revents & libc::POLLOUT != 0
}

fn relay_real_input(
    real_fd: RawFd,
    line_state: &mut InputLineState,
    pending_child_input: &mut PendingChildInput,
    buffer: &mut [u8],
) -> Result<(), String> {
    match read_fd(real_fd, buffer) {
        Ok(0) => Ok(()),
        Ok(n) => {
            line_state.observe_user_input(&buffer[..n]);
            pending_child_input.enqueue(&buffer[..n]);
            Ok(())
        }
        Err(err) => Err(format_user_input_read_error(err)),
    }
}

fn flush_pending_child_input(
    master_fd: RawFd,
    pending_child_input: &mut PendingChildInput,
) -> Result<(), String> {
    pending_child_input
        .flush_some(master_fd)
        .map(|_| ())
        .map_err(format_user_input_write_error)
}

fn format_user_input_write_error(err: io::Error) -> String {
    format!("Failed to write user input to PTY: {err}")
}

fn format_user_input_read_error(err: io::Error) -> String {
    format!("Failed to read user terminal input: {err}")
}

fn relay_pty_output(master_fd: RawFd, real_fd: RawFd, buffer: &mut [u8]) -> Result<bool, String> {
    match read_fd(master_fd, buffer) {
        Ok(0) => Ok(false),
        Ok(n) => write_all_fd(real_fd, &buffer[..n])
            .map(|_| true)
            .map_err(format_pty_output_write_error),
        Err(err) if is_pty_eof_error(&err) => Ok(false),
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

struct PendingChildInput {
    bytes: Vec<u8>,
    drained: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FlushProgress {
    wrote: usize,
    fully_drained: bool,
}

impl PendingChildInput {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            drained: 0,
        }
    }

    fn enqueue(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    fn is_empty(&self) -> bool {
        self.pending_len() == 0
    }

    fn pending_len(&self) -> usize {
        self.bytes.len().saturating_sub(self.drained)
    }

    fn take_pending(&mut self) -> Vec<u8> {
        let pending = self.bytes[self.drained..].to_vec();
        self.bytes.clear();
        self.drained = 0;
        pending
    }

    fn flush_some(&mut self, fd: RawFd) -> io::Result<FlushProgress> {
        if self.is_empty() {
            self.compact_if_drained();
            return Ok(FlushProgress {
                wrote: 0,
                fully_drained: true,
            });
        }

        let wrote = write_some_fd(fd, &self.bytes[self.drained..])?;
        self.drained += wrote;
        let fully_drained = self.is_empty();
        if fully_drained {
            self.compact_if_drained();
        }
        Ok(FlushProgress {
            wrote,
            fully_drained,
        })
    }

    fn compact_if_drained(&mut self) {
        if self.drained == self.bytes.len() {
            self.bytes.clear();
            self.drained = 0;
        }
    }
}

fn queue_control_injection(
    pending: &mut PendingChildInput,
    payload: &[u8],
    bracketed: bool,
    submit: bool,
) {
    pending.enqueue(&tui::control_payload_bytes(payload, bracketed));
    if submit {
        pending.enqueue(b"\r");
    }
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

fn write_some_fd(fd: RawFd, bytes: &[u8]) -> io::Result<usize> {
    if bytes.is_empty() {
        return Ok(0);
    }
    let original_flags = fd_flags(fd)?;
    let restore_flags = set_nonblocking_for_write(fd, original_flags)?;
    let result = write_some_nonblocking(fd, bytes);
    restore_fd_flags(fd, restore_flags)?;
    result
}

fn write_some_nonblocking(fd: RawFd, bytes: &[u8]) -> io::Result<usize> {
    loop {
        let rc = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if rc >= 0 {
            return Ok(rc as usize);
        }
        let err = io::Error::last_os_error();
        match err.raw_os_error() {
            Some(code) if code == libc::EINTR => continue,
            Some(code) if code == libc::EAGAIN || code == libc::EWOULDBLOCK => return Ok(0),
            _ => return Err(err),
        }
    }
}

fn fd_flags(fd: RawFd) -> io::Result<i32> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(flags)
    }
}

fn set_nonblocking_for_write(fd: RawFd, flags: i32) -> io::Result<Option<i32>> {
    if flags & libc::O_NONBLOCK != 0 {
        return Ok(None);
    }
    let rc = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if rc == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(Some(flags))
    }
}

fn restore_fd_flags(fd: RawFd, flags: Option<i32>) -> io::Result<()> {
    let Some(flags) = flags else {
        return Ok(());
    };
    let rc = unsafe { libc::fcntl(fd, libc::F_SETFL, flags) };
    if rc == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
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
    enter_escape_state: EnterEscapeState,
    boundary_probe: Vec<u8>,
    mouse_skipped_count: u64,
    last_submit_trace: Option<InputLineTraceSnapshot>,
}

#[derive(Debug, Clone)]
struct InputLineTraceSnapshot {
    input_empty: bool,
    at_boundary: bool,
    mid_escape: bool,
    last_user_input_ms: Option<u128>,
    user_input_idle: bool,
    boundary_probe: String,
    mouse_skipped_count: u64,
}

#[derive(Debug, Default, Clone)]
pub(super) struct ChildOutputState {
    last_child_output_at: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ForegroundOwnerState {
    Owner,
    Other { foreground_pgid: libc::pid_t },
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InjectionUnsafeReason {
    MidLine,
    ChildOutputActive,
    ForegroundOther,
    ForegroundUnknown,
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
enum EnterEscapeState {
    #[default]
    None,
    Esc,
    Csi,
    CsiOne,
    CsiThirteen,
    CsiThirteenModifierStart,
    CsiThirteenModifierDigits,
    CsiParams,
    CsiMouseSgrStart,
    CsiMouseSgrParams,
    LegacyMouseByte1,
    LegacyMouseByte2,
    LegacyMouseByte3,
    ApplicationKeypad,
}

impl ChildOutputState {
    pub(super) fn observe_child_output(&mut self) {
        self.last_child_output_at = Some(Instant::now());
    }

    fn is_quiescent(&self) -> bool {
        self.last_child_output_at
            .map(|last| last.elapsed() >= INJECT_CHILD_OUTPUT_DEBOUNCE)
            .unwrap_or(true)
    }

    fn millis_since_last_child_output(&self) -> Option<u128> {
        self.last_child_output_at
            .map(|last| last.elapsed().as_millis())
    }
}

impl Default for InputLineState {
    fn default() -> Self {
        Self {
            at_line_boundary: true,
            last_user_input_at: None,
            enter_escape_state: EnterEscapeState::None,
            boundary_probe: Vec::new(),
            mouse_skipped_count: 0,
            last_submit_trace: None,
        }
    }
}

impl InputLineState {
    fn observe_user_input(&mut self, bytes: &[u8]) {
        if no_user_input(bytes) {
            return;
        }
        self.last_submit_trace = None;
        let now = Instant::now();
        let mut saw_line_affecting_input = false;
        for byte in bytes.iter().copied() {
            saw_line_affecting_input |= self.observe_user_input_byte(byte);
        }
        if saw_line_affecting_input {
            self.last_user_input_at = Some(now);
        }
    }

    fn observe_user_input_byte(&mut self, byte: u8) -> bool {
        self.record_boundary_probe_byte(byte);
        if self.enter_escape_state != EnterEscapeState::None {
            return self.observe_enter_escape_byte(byte);
        }
        self.observe_non_escape_byte(byte)
    }

    fn observe_enter_escape_byte(&mut self, byte: u8) -> bool {
        let next = match (self.enter_escape_state, byte) {
            (EnterEscapeState::Esc, b'[') => Some(EnterEscapeState::Csi),
            (EnterEscapeState::Esc, b'O') => Some(EnterEscapeState::ApplicationKeypad),
            (EnterEscapeState::Csi, b'<') => Some(EnterEscapeState::CsiMouseSgrStart),
            (EnterEscapeState::Csi, b'M') => Some(EnterEscapeState::LegacyMouseByte1),
            (EnterEscapeState::Csi, b'1') => Some(EnterEscapeState::CsiOne),
            (EnterEscapeState::Csi, b'0'..=b'9' | b';') => Some(EnterEscapeState::CsiParams),
            (EnterEscapeState::CsiOne, b'3') => Some(EnterEscapeState::CsiThirteen),
            (EnterEscapeState::CsiOne, b'0'..=b'9' | b';') => Some(EnterEscapeState::CsiParams),
            (EnterEscapeState::CsiThirteen, b';') => {
                Some(EnterEscapeState::CsiThirteenModifierStart)
            }
            (EnterEscapeState::CsiThirteen, b'0'..=b'9') => Some(EnterEscapeState::CsiParams),
            (EnterEscapeState::CsiThirteenModifierStart, b'0'..=b'9') => {
                Some(EnterEscapeState::CsiThirteenModifierDigits)
            }
            (EnterEscapeState::CsiThirteenModifierStart, b';') => Some(EnterEscapeState::CsiParams),
            (EnterEscapeState::CsiThirteenModifierDigits, b'0'..=b'9') => {
                Some(EnterEscapeState::CsiThirteenModifierDigits)
            }
            (EnterEscapeState::CsiThirteenModifierDigits, b';') => {
                Some(EnterEscapeState::CsiParams)
            }
            (EnterEscapeState::CsiParams, b'0'..=b'9' | b';') => Some(EnterEscapeState::CsiParams),
            (EnterEscapeState::CsiMouseSgrStart, b'0'..=b'9' | b';') => {
                Some(EnterEscapeState::CsiMouseSgrParams)
            }
            (EnterEscapeState::CsiMouseSgrParams, b'0'..=b'9' | b';') => {
                Some(EnterEscapeState::CsiMouseSgrParams)
            }
            (EnterEscapeState::LegacyMouseByte1, _) => Some(EnterEscapeState::LegacyMouseByte2),
            (EnterEscapeState::LegacyMouseByte2, _) => Some(EnterEscapeState::LegacyMouseByte3),
            (EnterEscapeState::CsiThirteen, b'u') => {
                self.complete_enter_escape();
                return true;
            }
            (EnterEscapeState::CsiThirteenModifierDigits, b'u') => {
                self.complete_enter_escape();
                return true;
            }
            (EnterEscapeState::ApplicationKeypad, b'M') => {
                self.complete_enter_escape();
                return true;
            }
            (EnterEscapeState::CsiOne, b'M')
            | (EnterEscapeState::CsiThirteen, b'M')
            | (EnterEscapeState::CsiThirteenModifierStart, b'M')
            | (EnterEscapeState::CsiThirteenModifierDigits, b'M')
            | (EnterEscapeState::CsiParams, b'M')
            | (EnterEscapeState::CsiMouseSgrStart, b'M' | b'm')
            | (EnterEscapeState::CsiMouseSgrParams, b'M' | b'm')
            | (EnterEscapeState::LegacyMouseByte3, _) => {
                self.complete_mouse_sequence();
                return false;
            }
            _ => None,
        };

        if let Some(next) = next {
            self.enter_escape_state = next;
            false
        } else {
            self.enter_escape_state = EnterEscapeState::None;
            let _ = self.observe_non_escape_byte(byte);
            true
        }
    }

    fn observe_non_escape_byte(&mut self, byte: u8) -> bool {
        if byte == 0x1b {
            self.enter_escape_state = EnterEscapeState::Esc;
            false
        } else {
            self.at_line_boundary = line_boundary_after_input_byte(byte);
            if self.at_line_boundary {
                self.clear_boundary_probe();
            }
            true
        }
    }

    fn complete_enter_escape(&mut self) {
        self.enter_escape_state = EnterEscapeState::None;
        self.at_line_boundary = true;
        self.clear_boundary_probe();
    }

    fn complete_mouse_sequence(&mut self) {
        self.enter_escape_state = EnterEscapeState::None;
        self.mouse_skipped_count = self.mouse_skipped_count.saturating_add(1);
    }

    fn is_safe_to_inject(&self) -> bool {
        !self.mid_escape() && (self.at_line_boundary || self.user_input_idle())
    }

    fn input_empty(&self) -> bool {
        self.at_line_boundary && self.enter_escape_state == EnterEscapeState::None
    }

    fn mark_submitted(&mut self) {
        self.last_submit_trace = Some(self.trace_snapshot());
        self.at_line_boundary = true;
        self.last_user_input_at = None;
        self.enter_escape_state = EnterEscapeState::None;
        self.clear_boundary_probe();
    }

    fn millis_since_last_user_input(&self) -> Option<u128> {
        self.last_user_input_at
            .map(|last| last.elapsed().as_millis())
    }

    fn mid_escape(&self) -> bool {
        self.enter_escape_state != EnterEscapeState::None
    }

    fn user_input_idle(&self) -> bool {
        self.last_user_input_at
            .map(|last| last.elapsed() >= USER_INPUT_IDLE_INJECT)
            .unwrap_or(true)
    }

    fn record_boundary_probe_byte(&mut self, byte: u8) {
        if self.boundary_probe.len() == BOUNDARY_PROBE_MAX_BYTES {
            self.boundary_probe.remove(0);
        }
        self.boundary_probe.push(byte);
    }

    fn clear_boundary_probe(&mut self) {
        self.boundary_probe.clear();
        self.mouse_skipped_count = 0;
    }

    fn boundary_probe_trace_value(&self) -> String {
        if self.boundary_probe.is_empty() {
            return "none".to_string();
        }
        format!("hex:{}", bytes_to_lower_hex(&self.boundary_probe))
    }

    fn trace_snapshot(&self) -> InputLineTraceSnapshot {
        InputLineTraceSnapshot {
            input_empty: self.input_empty(),
            at_boundary: self.at_line_boundary,
            mid_escape: self.mid_escape(),
            last_user_input_ms: self.millis_since_last_user_input(),
            user_input_idle: self.user_input_idle(),
            boundary_probe: self.boundary_probe_trace_value(),
            mouse_skipped_count: self.mouse_skipped_count,
        }
    }

    fn trace_snapshot_for_decision(&self, decision: &str) -> InputLineTraceSnapshot {
        if decision == "inject"
            && let Some(snapshot) = &self.last_submit_trace
        {
            return snapshot.clone();
        }
        self.trace_snapshot()
    }
}

fn bytes_to_lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn no_user_input(bytes: &[u8]) -> bool {
    bytes.is_empty()
}

fn line_boundary_after_input_byte(byte: u8) -> bool {
    matches!(byte, b'\r' | b'\n' | 0x03 | 0x04 | 0x15)
}

struct ControlRequestIo<'a> {
    real_fd: RawFd,
    master_fd: RawFd,
    child_pid: Option<u32>,
    line_state: &'a mut InputLineState,
    child_output_state: &'a mut ChildOutputState,
    pending_child_input: &'a mut PendingChildInput,
    buffer: &'a mut [u8],
}

struct PreparedControlPayload {
    bytes: Vec<u8>,
    delivery_attempt_id: Option<String>,
}

fn handle_control_request(
    control: &ControlSocket,
    io: &mut ControlRequestIo<'_>,
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
    let response = process_control_request_with_pending(
        &mut stream,
        io,
        Some((control.session_id(), control.invocation_uuid())),
    );
    let (ack, message) = control_response_parts(response);
    trace_notify_gate_decision(
        control,
        io.master_fd,
        io.child_pid,
        io.line_state,
        io.child_output_state,
        if ack { "inject" } else { "skip" },
        &message,
    );
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

#[cfg(test)]
fn process_control_request(
    stream: &mut UnixStream,
    real_fd: RawFd,
    master_fd: RawFd,
    line_state: &mut InputLineState,
    buffer: &mut [u8],
) -> Result<(), String> {
    let mut pending_child_input = PendingChildInput::new();
    let mut child_output_state = ChildOutputState::default();
    let result = {
        let mut request_io = ControlRequestIo {
            real_fd,
            master_fd,
            child_pid: None,
            line_state,
            child_output_state: &mut child_output_state,
            pending_child_input: &mut pending_child_input,
            buffer,
        };
        process_control_request_with_pending(stream, &mut request_io, None)
    };
    if result.is_ok() {
        flush_pending_child_input_to_completion(master_fd, &mut pending_child_input)?;
    }
    result
}

fn process_control_request_with_pending(
    stream: &mut UnixStream,
    io: &mut ControlRequestIo<'_>,
    expected_target: Option<(&str, &str)>,
) -> Result<(), String> {
    validate_control_request_peer(stream)?;
    let payload = prepare_control_payload(read_control_request_payload(stream)?, expected_target)?;
    if payload.bytes.is_empty() {
        return acknowledge_control_payload(&payload);
    }
    // Safe proactive delivery means the PTY foreground process group is still
    // the agent, child output has cleared the short debounce, and either the
    // line parser saw a boundary or user input has been idle long enough to
    // tolerate terminals whose Enter sequence is not recognized here.
    wait_until_safe_to_inject(
        io.real_fd,
        io.master_fd,
        io.child_pid,
        io.line_state,
        io.child_output_state,
        io.pending_child_input,
        io.buffer,
    )?;
    submit_control_request_payload(io.pending_child_input, &payload.bytes);
    io.line_state.mark_submitted();
    acknowledge_control_payload(&payload)
}

fn prepare_control_payload(
    payload: Vec<u8>,
    expected_target: Option<(&str, &str)>,
) -> Result<PreparedControlPayload, String> {
    let Some(attempt_id) = delivery_attempt_id(&payload) else {
        return Ok(PreparedControlPayload {
            bytes: payload,
            delivery_attempt_id: None,
        });
    };
    let Some(db) = MailboxDb::open_default_if_exists()? else {
        return Ok(PreparedControlPayload {
            bytes: payload,
            delivery_attempt_id: None,
        });
    };
    let Some(window) = db.delivery_attempt_window(&attempt_id)? else {
        return Ok(PreparedControlPayload {
            bytes: payload,
            delivery_attempt_id: None,
        });
    };
    if let Some((session_id, invocation_uuid)) = expected_target
        && (window.session_id != session_id || window.delivery_invocation_uuid != invocation_uuid)
    {
        return Err("mailbox_delivery_target_mismatch".to_string());
    }
    if window.rows.is_empty() {
        return Ok(PreparedControlPayload {
            bytes: Vec::new(),
            delivery_attempt_id: None,
        });
    }
    if window.acknowledged_at.is_none()
        && db
            .accepted_delivery_attempt_windows(&window.session_id)?
            .into_iter()
            .any(|owner| {
                owner.attempt_id != attempt_id
                    && owner.delivery_invocation_uuid == window.delivery_invocation_uuid
            })
    {
        return Err("mailbox_delivery_owned".to_string());
    }
    let bytes = if window.acknowledged_at.is_some() {
        Vec::new()
    } else {
        render_mailbox_notification_envelope(&window.rows, window.remaining_count, &attempt_id)
            .into_bytes()
    };
    Ok(PreparedControlPayload {
        bytes,
        delivery_attempt_id: Some(attempt_id),
    })
}

fn acknowledge_control_payload(payload: &PreparedControlPayload) -> Result<(), String> {
    let Some(attempt_id) = payload.delivery_attempt_id.as_deref() else {
        return Ok(());
    };
    let Some(mut db) = MailboxDb::open_default_if_exists()? else {
        return Err("Mailbox sidecar disappeared while acknowledging delivery".to_string());
    };
    if db.record_delivery_attempt_transport_ack(attempt_id)? {
        Ok(())
    } else {
        Err(format!(
            "Mailbox delivery attempt {attempt_id} is no longer registered"
        ))
    }
}

fn delivery_attempt_id(payload: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(payload).ok()?;
    let marker_start = text.rfind(DELIVERY_ATTEMPT_PREFIX)? + DELIVERY_ATTEMPT_PREFIX.len();
    let marker_tail = &text[marker_start..];
    let marker_end = marker_tail.find(DELIVERY_ATTEMPT_SUFFIX)?;
    let attempt_id = &marker_tail[..marker_end];
    (!attempt_id.is_empty()).then(|| attempt_id.to_string())
}

fn validate_control_request_peer(stream: &UnixStream) -> Result<(), String> {
    validate_peer_uid(stream)
}

fn submit_control_request_payload(pending_child_input: &mut PendingChildInput, payload: &[u8]) {
    // Submit with a carriage return (`\r`), the byte the Enter key sends. Raw-mode TUI
    // children submit on `\r` and treat `\n` as a literal newline, so a `\n` here would
    // leave the payload sitting unsubmitted in the input box. Cooked-mode children still
    // submit because the pty's `ICRNL` maps the incoming `\r` to `\n`.
    queue_control_injection(pending_child_input, payload, false, true);
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
    child_pid: Option<u32>,
    line_state: &mut InputLineState,
    child_output_state: &mut ChildOutputState,
    pending_child_input: &mut PendingChildInput,
    buffer: &mut [u8],
) -> Result<(), String> {
    pump_until_safe_to_inject(
        real_fd,
        master_fd,
        child_pid,
        line_state,
        child_output_state,
        pending_child_input,
        buffer,
    )?;
    validate_safe_to_inject(master_fd, child_pid, line_state, child_output_state)
}

fn pump_until_safe_to_inject(
    real_fd: RawFd,
    master_fd: RawFd,
    child_pid: Option<u32>,
    line_state: &mut InputLineState,
    child_output_state: &mut ChildOutputState,
    pending_child_input: &mut PendingChildInput,
    buffer: &mut [u8],
) -> Result<(), String> {
    let start = Instant::now();
    while start.elapsed() < INJECT_WAIT_LIMIT {
        if safe_to_inject(master_fd, child_pid, line_state, child_output_state).is_ok() {
            return Ok(());
        }
        pump_injection_wait_io(
            real_fd,
            master_fd,
            line_state,
            child_output_state,
            pending_child_input,
            buffer,
        )?;
    }
    Ok(())
}

fn pump_injection_wait_io(
    real_fd: RawFd,
    master_fd: RawFd,
    line_state: &mut InputLineState,
    child_output_state: &mut ChildOutputState,
    pending_child_input: &mut PendingChildInput,
    buffer: &mut [u8],
) -> Result<(), String> {
    let ready = poll_relay_fds(real_fd, master_fd, None, !pending_child_input.is_empty())?;
    if ready.pty_writable {
        flush_pending_child_input(master_fd, pending_child_input)?;
    }
    if ready.real_input {
        relay_real_input(real_fd, line_state, pending_child_input, buffer)?;
    }
    if ready.pty_output && relay_pty_output(master_fd, real_fd, buffer)? {
        child_output_state.observe_child_output();
    }
    Ok(())
}

#[cfg(test)]
fn flush_pending_child_input_to_completion(
    master_fd: RawFd,
    pending_child_input: &mut PendingChildInput,
) -> Result<(), String> {
    while !pending_child_input.is_empty() {
        flush_pending_child_input(master_fd, pending_child_input)?;
    }
    Ok(())
}

fn validate_safe_to_inject(
    master_fd: RawFd,
    child_pid: Option<u32>,
    line_state: &InputLineState,
    child_output_state: &ChildOutputState,
) -> Result<(), String> {
    safe_to_inject(master_fd, child_pid, line_state, child_output_state)
        .map_err(unsafe_reason_message)
}

fn safe_to_inject(
    master_fd: RawFd,
    child_pid: Option<u32>,
    line_state: &InputLineState,
    child_output_state: &ChildOutputState,
) -> Result<(), InjectionUnsafeReason> {
    safe_to_inject_for_foreground(
        foreground_owner_state(master_fd, child_pid),
        line_state,
        child_output_state,
    )
}

fn safe_to_inject_for_foreground(
    foreground: ForegroundOwnerState,
    line_state: &InputLineState,
    child_output_state: &ChildOutputState,
) -> Result<(), InjectionUnsafeReason> {
    if line_state.mid_escape() {
        return Err(InjectionUnsafeReason::MidLine);
    }
    match foreground {
        ForegroundOwnerState::Owner => {}
        ForegroundOwnerState::Other { .. } => return Err(InjectionUnsafeReason::ForegroundOther),
        ForegroundOwnerState::Unknown => return Err(InjectionUnsafeReason::ForegroundUnknown),
    }
    if !child_output_state.is_quiescent() {
        return Err(InjectionUnsafeReason::ChildOutputActive);
    }
    if !line_state.is_safe_to_inject() {
        return Err(InjectionUnsafeReason::MidLine);
    }
    Ok(())
}

fn foreground_owner_state(master_fd: RawFd, child_pid: Option<u32>) -> ForegroundOwnerState {
    let Some(child_pid) = child_pid else {
        return ForegroundOwnerState::Owner;
    };
    let foreground_pgid = unsafe { libc::tcgetpgrp(master_fd) };
    if foreground_pgid == -1 {
        return ForegroundOwnerState::Unknown;
    }
    if foreground_pgid == child_pid as libc::pid_t {
        ForegroundOwnerState::Owner
    } else {
        ForegroundOwnerState::Other { foreground_pgid }
    }
}

fn unsafe_reason_message(reason: InjectionUnsafeReason) -> String {
    match reason {
        InjectionUnsafeReason::MidLine => "unsafe_mid_line",
        InjectionUnsafeReason::ChildOutputActive => "unsafe_child_output_active",
        InjectionUnsafeReason::ForegroundOther => "unsafe_foreground_process",
        InjectionUnsafeReason::ForegroundUnknown => "unsafe_foreground_unknown",
    }
    .to_string()
}

fn trace_notify_enabled() -> bool {
    matches!(
        std::env::var("OULIPOLY_TRACE_NOTIFY").ok().as_deref(),
        Some("1") | Some("true") | Some("yes") | Some("on")
    )
}

pub fn append_notify_trace_record(fields: &str) {
    let Some(path) = notify_trace_path() else {
        return;
    };
    let line = format!(
        "{} {}\n",
        Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        fields.trim()
    );
    let _ = append_notify_trace_line(&path, &line);
}

fn notify_trace_path() -> Option<PathBuf> {
    Some(notify_trace_dir()?.join(NOTIFY_TRACE_FILE))
}

pub(super) fn append_overlay_input_trace_record(bytes_hex: &str, classification: &str) {
    #[cfg(test)]
    {
        let _ = (bytes_hex, classification);
    }
    #[cfg(not(test))]
    {
        let Some(path) = overlay_input_trace_path() else {
            return;
        };
        let line = overlay_input_trace_line(bytes_hex, classification);
        let _ = append_overlay_input_trace_line(&path, &line);
    }
}

fn overlay_input_trace_line(bytes_hex: &str, classification: &str) -> String {
    format!(
        "{} trigger=overlay-input bytes_hex={} classification={}\n",
        Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        trace_token(bytes_hex),
        trace_token(classification)
    )
}

#[cfg(not(test))]
fn overlay_input_trace_path() -> Option<PathBuf> {
    Some(notify_trace_dir()?.join(OVERLAY_INPUT_TRACE_FILE))
}

fn notify_trace_dir() -> Option<PathBuf> {
    if let Some(state_home) = non_empty_env_path("XDG_STATE_HOME") {
        return Some(state_home.join("oulipoly-agent-runner"));
    }
    Some(non_empty_env_path("HOME")?.join(".local/state/oulipoly-agent-runner"))
}

fn non_empty_env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn append_notify_trace_line(path: &Path, line: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    rotate_notify_trace_if_needed(path, line.len() as u64)?;
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(line.as_bytes())
}

fn append_overlay_input_trace_line(path: &Path, line: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    rotate_overlay_input_trace_if_needed(path, line.len() as u64)?;
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(line.as_bytes())
}

fn rotate_notify_trace_if_needed(path: &Path, next_len: u64) -> io::Result<()> {
    let Ok(metadata) = fs::metadata(path) else {
        return Ok(());
    };
    if metadata.len().saturating_add(next_len) <= NOTIFY_TRACE_MAX_BYTES {
        return Ok(());
    }
    let rotated = path.with_file_name(NOTIFY_TRACE_ROTATED_FILE);
    match fs::remove_file(&rotated) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    fs::rename(path, rotated)
}

fn rotate_overlay_input_trace_if_needed(path: &Path, next_len: u64) -> io::Result<()> {
    let Ok(metadata) = fs::metadata(path) else {
        return Ok(());
    };
    if metadata.len().saturating_add(next_len) <= OVERLAY_INPUT_TRACE_MAX_BYTES {
        return Ok(());
    }
    let rotated = path.with_file_name(OVERLAY_INPUT_TRACE_ROTATED_FILE);
    match fs::remove_file(&rotated) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    fs::rename(path, rotated)
}

fn trace_notify_gate_decision(
    control: &ControlSocket,
    master_fd: RawFd,
    child_pid: Option<u32>,
    line_state: &InputLineState,
    child_output_state: &ChildOutputState,
    decision: &str,
    status: &str,
) {
    let foreground = foreground_owner_state(master_fd, child_pid);
    let line = line_state.trace_snapshot_for_decision(decision);
    let inject_status = notify_trace_inject_status(decision, status);
    let reason = notify_trace_gate_reason(foreground, &line, child_output_state);
    let record = format!(
        "trigger=pty-control \
         session_id={} invocation_uuid={} input_empty={} at_boundary={} mid_escape={} \
         last_user_input_ms={} user_input_idle_ms={} user_input_idle={} \
         user_input_idle_threshold_ms={} boundary_probe={} mouse_skipped={} quiescent={} \
         last_child_output_ms={} foreground={} decision={} inject_status={} \
         reason={} consumed=unknown",
        control.session_id(),
        control.invocation_uuid(),
        line.input_empty,
        line.at_boundary,
        line.mid_escape,
        optional_millis_trace_value(line.last_user_input_ms),
        optional_millis_trace_value(line.last_user_input_ms),
        line.user_input_idle,
        USER_INPUT_IDLE_INJECT_MS,
        line.boundary_probe,
        line.mouse_skipped_count,
        child_output_state.is_quiescent(),
        optional_millis_trace_value(child_output_state.millis_since_last_child_output()),
        foreground_trace_value(foreground),
        notify_trace_decision(decision, &inject_status),
        inject_status,
        reason,
    );
    append_notify_trace_record(&record);
    if trace_notify_enabled() {
        eprintln!("oulipoly_notify_trace {record}");
    }
}

fn notify_trace_gate_reason(
    foreground: ForegroundOwnerState,
    line: &InputLineTraceSnapshot,
    child_output_state: &ChildOutputState,
) -> &'static str {
    if line.mid_escape {
        return "mid_escape";
    }
    match foreground {
        ForegroundOwnerState::Owner => {}
        ForegroundOwnerState::Other { .. } => return "foreground_process",
        ForegroundOwnerState::Unknown => return "foreground_unknown",
    }
    if !child_output_state.is_quiescent() {
        return "child_output_active";
    }
    if line.at_boundary {
        return "line_boundary";
    }
    if line.user_input_idle {
        return "user_input_idle";
    }
    "user_input_active"
}

pub fn notify_trace_inject_status(decision: &str, status: &str) -> String {
    match (decision, status) {
        ("inject", "ok") => "acked".to_string(),
        (_, "unsafe_foreground_process") => "foreground-not-child".to_string(),
        (_, value) => trace_token(value),
    }
}

pub fn notify_trace_decision(decision: &str, inject_status: &str) -> String {
    if decision == "inject" {
        "inject".to_string()
    } else {
        format!("skip-{inject_status}")
    }
}

fn optional_millis_trace_value(value: Option<u128>) -> String {
    value.map_or_else(|| "unknown".to_string(), |value| value.to_string())
}

fn foreground_trace_value(foreground: ForegroundOwnerState) -> String {
    match foreground {
        ForegroundOwnerState::Owner => "owner".to_string(),
        ForegroundOwnerState::Other { foreground_pgid } => format!("other:{foreground_pgid}"),
        ForegroundOwnerState::Unknown => "unknown".to_string(),
    }
}

pub fn trace_token(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch.is_ascii_whitespace() { '_' } else { ch })
        .collect()
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
    use crate::provider_registry::ProviderRegistryOptions;
    use oulipoly_config::{
        ModelConfig, PromptMode, ProviderConfig, ProvidersConfig,
        provider_implementation_ref::ProviderImplementationRef,
    };
    use std::os::unix::fs::PermissionsExt;
    use std::thread;

    #[test]
    fn provider_default_identity_uses_unique_account_artifact() {
        let temp = tempfile::tempdir().unwrap();
        let script = write_session_describe_provider(temp.path(), "provider.py");
        let registry = provider_registry(&[
            provider_model("z-model", "provider-account", &script),
            provider_model("a-model", "provider-account", &script),
        ]);

        let identity =
            provider_inspect_identity(&registry, "<provider-default>", "provider-account")
                .expect("a unique account artifact should resolve provider-default identity");

        assert_eq!(identity.model_name, "a-model");
        assert_eq!(identity.provider_name, "provider-account");
        assert_eq!(
            identity.provider_instance_id.as_deref(),
            Some("fixture-instance")
        );
        assert_eq!(identity.settings_id, "provider-account");
    }

    #[test]
    fn provider_default_identity_rejects_conflicting_account_artifacts() {
        let temp = tempfile::tempdir().unwrap();
        let first = write_session_describe_provider(temp.path(), "provider-a.py");
        let second = write_session_describe_provider(temp.path(), "provider-b.py");
        let registry = provider_registry(&[
            provider_model("a-model", "provider-account", &first),
            provider_model("b-model", "provider-account", &second),
        ]);

        assert!(
            provider_inspect_identity(&registry, "<provider-default>", "provider-account",)
                .is_none(),
            "provider-default identity must remain unavailable when the account maps to multiple artifacts"
        );
    }

    fn provider_registry(models: &[ModelConfig]) -> ProviderRegistry {
        ProviderRegistry::from_model_configs_with_provider_config(
            models,
            &ProvidersConfig::default(),
            ProviderRegistryOptions::default(),
        )
        .unwrap()
    }

    fn provider_model(name: &str, provider_name: &str, script: &Path) -> ModelConfig {
        ModelConfig {
            name: name.to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![ProviderConfig::model_provider(provider_name, Vec::new())],
            inputs: Vec::new(),
            provider: Some(ProviderImplementationRef {
                path: Some(script.display().to_string()),
                crate_name: None,
                version: None,
                binary: None,
                script: None,
            }),
        }
    }

    fn write_session_describe_provider(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(
            &path,
            r#"#!/usr/bin/env python3
import json
import sys

request = json.load(sys.stdin)
print(json.dumps({
    "contract": request["contract"],
    "request_id": request["request_id"],
    "ok": True,
    "result": {
        "provider_id": "fixture",
        "display_name": "Fixture",
        "contract_versions": [request["contract"]],
        "preferred_contract": request["contract"],
        "capabilities": {
            "launch": False,
            "policy": False,
            "quota": False,
            "session": True,
            "session_enumerate": False,
            "terminal": False,
            "rotation": False,
            "discovery": False,
            "settings": False,
            "setup_brain": False,
            "setup": False,
            "migration": False
        },
        "settings_schema_id": "fixture-settings"
    }
}))
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }

    #[test]
    fn input_line_state_tracks_boundary_and_idle_fallback() {
        let mut state = InputLineState::default();
        assert!(state.is_safe_to_inject());

        state.observe_user_input(b"abc");
        assert!(!state.at_line_boundary);
        assert!(!state.is_safe_to_inject());

        state.observe_user_input(b"\n");
        assert!(state.at_line_boundary);
        assert!(state.is_safe_to_inject());

        state.observe_user_input(b"stale-midline");
        assert!(!state.at_line_boundary);
        state.last_user_input_at =
            Some(Instant::now() - USER_INPUT_IDLE_INJECT - Duration::from_millis(1));
        assert!(state.is_safe_to_inject());
    }

    #[test]
    fn input_line_state_recognizes_enter_escape_sequences() {
        let mut state = InputLineState::default();

        state.observe_user_input(b"partial\x1b[13u");
        assert!(state.at_line_boundary);
        assert_eq!(state.enter_escape_state, EnterEscapeState::None);

        state.observe_user_input(b"partial\x1b[13;5u");
        assert!(state.at_line_boundary);
        assert_eq!(state.enter_escape_state, EnterEscapeState::None);

        state.observe_user_input(b"partial\x1bOM");
        assert!(state.at_line_boundary);
        assert_eq!(state.enter_escape_state, EnterEscapeState::None);
    }

    #[test]
    fn input_line_state_keeps_split_csi_u_enter_pending_until_complete() {
        let mut state = InputLineState::default();
        state.observe_user_input(b"partial");

        state.observe_user_input(b"\x1b[1");
        assert!(!state.at_line_boundary);
        assert_eq!(state.enter_escape_state, EnterEscapeState::CsiOne);
        state.last_user_input_at =
            Some(Instant::now() - USER_INPUT_IDLE_INJECT - Duration::from_millis(1));
        assert!(!state.is_safe_to_inject());

        state.observe_user_input(b"3u");
        assert!(state.at_line_boundary);
        assert_eq!(state.enter_escape_state, EnterEscapeState::None);

        state.observe_user_input(b"partial\x1b[13;");
        assert!(!state.at_line_boundary);
        assert_eq!(
            state.enter_escape_state,
            EnterEscapeState::CsiThirteenModifierStart
        );
        state.observe_user_input(b"5u");
        assert!(state.at_line_boundary);
        assert_eq!(state.enter_escape_state, EnterEscapeState::None);
    }

    #[test]
    fn input_line_state_skips_sgr_mouse_after_boundary_without_recent_user_input() {
        let mut state = InputLineState::default();

        state.observe_user_input(b"\x1b[<35;79;1M\x1b[<0;79;1m");

        assert!(state.at_line_boundary);
        assert_eq!(state.enter_escape_state, EnterEscapeState::None);
        assert_eq!(state.last_user_input_at, None);
        assert_eq!(state.mouse_skipped_count, 2);
        assert!(state.is_safe_to_inject());
        assert_eq!(state.trace_snapshot().mouse_skipped_count, 2);
    }

    #[test]
    fn input_line_state_skips_mouse_sequences_interleaved_with_typing() {
        let mut state = InputLineState::default();
        state.observe_user_input(b"a");
        let typed_at = Instant::now() - USER_INPUT_IDLE_INJECT - Duration::from_millis(1);
        state.last_user_input_at = Some(typed_at);

        state.observe_user_input(b"\x1b[<35;79;1M");

        assert!(!state.at_line_boundary);
        assert_eq!(state.enter_escape_state, EnterEscapeState::None);
        assert_eq!(state.last_user_input_at, Some(typed_at));
        assert_eq!(state.mouse_skipped_count, 1);

        state.observe_user_input(b"b");

        assert!(!state.at_line_boundary);
        assert_ne!(state.last_user_input_at, Some(typed_at));
    }

    #[test]
    fn input_line_state_skips_legacy_and_urxvt_mouse_sequences() {
        let mut state = InputLineState::default();

        state.observe_user_input(b"\x1b[Mabc\x1b[35;79;1M");

        assert!(state.at_line_boundary);
        assert_eq!(state.enter_escape_state, EnterEscapeState::None);
        assert_eq!(state.last_user_input_at, None);
        assert_eq!(state.mouse_skipped_count, 2);
        assert!(state.is_safe_to_inject());
    }

    #[test]
    fn input_line_state_handles_split_sgr_mouse_sequence() {
        let mut state = InputLineState::default();

        state.observe_user_input(b"\x1b[<35");

        assert!(state.at_line_boundary);
        assert_eq!(
            state.enter_escape_state,
            EnterEscapeState::CsiMouseSgrParams
        );
        assert_eq!(state.last_user_input_at, None);
        assert!(state.mid_escape());
        assert!(!state.is_safe_to_inject());

        state.observe_user_input(b";79;1M");

        assert!(state.at_line_boundary);
        assert_eq!(state.enter_escape_state, EnterEscapeState::None);
        assert_eq!(state.last_user_input_at, None);
        assert_eq!(state.mouse_skipped_count, 1);
        assert!(state.is_safe_to_inject());
    }

    #[test]
    fn child_output_state_uses_short_redraw_tolerant_debounce() {
        let state = ChildOutputState {
            last_child_output_at: Some(Instant::now()),
        };
        assert!(!state.is_quiescent());

        let state = ChildOutputState {
            last_child_output_at: Some(
                Instant::now() - INJECT_CHILD_OUTPUT_DEBOUNCE - Duration::from_millis(1),
            ),
        };
        assert!(state.is_quiescent());
        assert!(INJECT_CHILD_OUTPUT_DEBOUNCE < Duration::from_millis(200));
    }

    #[test]
    fn safe_to_inject_allows_idle_redraw_after_short_debounce() {
        let line_state = InputLineState::default();
        let output_state = ChildOutputState {
            last_child_output_at: Some(
                Instant::now() - INJECT_CHILD_OUTPUT_DEBOUNCE - Duration::from_millis(1),
            ),
        };

        assert_eq!(
            safe_to_inject_for_foreground(ForegroundOwnerState::Owner, &line_state, &output_state),
            Ok(())
        );
    }

    #[test]
    fn safe_to_inject_declines_during_short_output_debounce() {
        let line_state = InputLineState::default();
        let output_state = ChildOutputState {
            last_child_output_at: Some(Instant::now()),
        };

        assert_eq!(
            safe_to_inject_for_foreground(ForegroundOwnerState::Owner, &line_state, &output_state),
            Err(InjectionUnsafeReason::ChildOutputActive)
        );
    }

    #[test]
    fn safe_to_inject_keeps_foreground_process_group_guard() {
        let line_state = InputLineState::default();
        let output_state = ChildOutputState::default();

        assert_eq!(
            safe_to_inject_for_foreground(
                ForegroundOwnerState::Other {
                    foreground_pgid: 42
                },
                &line_state,
                &output_state
            ),
            Err(InjectionUnsafeReason::ForegroundOther)
        );
    }

    #[test]
    fn safe_to_inject_keeps_user_typing_guard() {
        let mut line_state = InputLineState::default();
        line_state.observe_user_input(b"partial");
        let output_state = ChildOutputState::default();

        assert_eq!(
            safe_to_inject_for_foreground(ForegroundOwnerState::Owner, &line_state, &output_state),
            Err(InjectionUnsafeReason::MidLine)
        );
    }

    #[test]
    fn safe_to_inject_allows_idle_midline_when_owner_and_quiescent() {
        let mut line_state = InputLineState::default();
        line_state.observe_user_input(b"submitted-but-parser-missed-boundary");
        line_state.last_user_input_at =
            Some(Instant::now() - USER_INPUT_IDLE_INJECT - Duration::from_millis(1));
        let output_state = ChildOutputState::default();

        assert_eq!(
            safe_to_inject_for_foreground(ForegroundOwnerState::Owner, &line_state, &output_state),
            Ok(())
        );
    }

    #[test]
    fn safe_to_inject_blocks_idle_midline_when_foreground_is_not_owner() {
        let mut line_state = InputLineState::default();
        line_state.observe_user_input(b"stale-midline");
        line_state.last_user_input_at =
            Some(Instant::now() - USER_INPUT_IDLE_INJECT - Duration::from_millis(1));
        let output_state = ChildOutputState::default();

        assert_eq!(
            safe_to_inject_for_foreground(
                ForegroundOwnerState::Other {
                    foreground_pgid: 42
                },
                &line_state,
                &output_state,
            ),
            Err(InjectionUnsafeReason::ForegroundOther)
        );
    }

    #[test]
    fn safe_to_inject_keeps_mid_escape_as_hard_block_even_when_idle() {
        let mut line_state = InputLineState::default();
        line_state.observe_user_input(b"\x1b[13;");
        line_state.last_user_input_at =
            Some(Instant::now() - USER_INPUT_IDLE_INJECT - Duration::from_millis(1));
        let output_state = ChildOutputState::default();

        assert_eq!(
            safe_to_inject_for_foreground(ForegroundOwnerState::Owner, &line_state, &output_state),
            Err(InjectionUnsafeReason::MidLine)
        );
    }

    #[test]
    fn safe_to_inject_blocks_idle_midline_during_child_output_burst() {
        let mut line_state = InputLineState::default();
        line_state.observe_user_input(b"stale-midline");
        line_state.last_user_input_at =
            Some(Instant::now() - USER_INPUT_IDLE_INJECT - Duration::from_millis(1));
        let mut output_state = ChildOutputState::default();
        output_state.observe_child_output();

        assert_eq!(
            safe_to_inject_for_foreground(ForegroundOwnerState::Owner, &line_state, &output_state),
            Err(InjectionUnsafeReason::ChildOutputActive)
        );
    }

    #[test]
    fn input_line_state_boundary_probe_is_bounded_hex_since_last_boundary() {
        let mut state = InputLineState::default();
        state.observe_user_input(b"abcdefghijklmnopqrstuvwxyz");

        assert_eq!(
            state.boundary_probe_trace_value(),
            "hex:6b6c6d6e6f707172737475767778797a"
        );

        state.observe_user_input(b"\n");

        assert_eq!(state.boundary_probe_trace_value(), "none");
    }

    #[test]
    fn inject_trace_snapshot_preserves_idle_reason_and_boundary_probe() {
        let mut state = InputLineState::default();
        state.observe_user_input(b"missed-submit\x1b[13;9~");
        state.last_user_input_at =
            Some(Instant::now() - USER_INPUT_IDLE_INJECT - Duration::from_millis(1));

        state.mark_submitted();

        let line = state.trace_snapshot_for_decision("inject");
        let output_state = ChildOutputState::default();
        assert!(!line.input_empty);
        assert!(!line.at_boundary);
        assert!(line.user_input_idle);
        assert_eq!(line.boundary_probe, "hex:65642d7375626d69741b5b31333b397e");
        assert_eq!(
            notify_trace_gate_reason(ForegroundOwnerState::Owner, &line, &output_state),
            "user_input_idle"
        );
    }

    #[test]
    fn input_line_state_preserves_existing_boundaries_and_midline_detection() {
        for byte in [b'\r', b'\n', 0x03, 0x04, 0x15] {
            let mut state = InputLineState::default();
            state.observe_user_input(b"partial");
            assert!(!state.at_line_boundary);

            state.observe_user_input(&[byte]);
            assert!(state.at_line_boundary, "byte {byte:#04x} should submit");
        }

        let mut state = InputLineState::default();
        state.observe_user_input(b"partial");
        assert!(!state.at_line_boundary);

        state.observe_user_input(b"\x1b[A");
        assert!(!state.at_line_boundary);
        assert_eq!(state.enter_escape_state, EnterEscapeState::None);
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
    fn runtime_control_socket_dir_falls_through_when_runtime_dir_missing() {
        // A stale XDG_RUNTIME_DIR (e.g. /run/user/<uid> wiped by a host/WSL
        // crash) must not be used — otherwise every interactive spawn fails to
        // bind its control socket. Returning None lets control_socket_dir fall
        // through to the state/data tiers.
        let missing = PathBuf::from("/nonexistent-xdg-runtime-6b1e2226/does-not-exist");
        assert!(!missing.is_dir(), "precondition: path must not exist");
        assert_eq!(runtime_control_socket_dir_for(missing), None);
    }

    #[test]
    fn runtime_control_socket_dir_used_when_runtime_dir_exists() {
        let existing = std::env::temp_dir();
        assert_eq!(
            runtime_control_socket_dir_for(existing.clone()),
            Some(existing.join("oulipoly-agent-runner/pty"))
        );
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
        assert_eq!(received, "[OULIPOLY NOTIFICATIONS]\r");
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
        assert_eq!(received, "\nnotify\r");
    }

    #[test]
    fn control_request_idle_midline_injects_once_after_user_idle_threshold() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        write_inject_frame(&mut client, b"notify").unwrap();
        let (real_read_end, _real_write_end) = pipe_files();
        let (read_end, write_end) = pipe_files();
        let mut state = InputLineState::default();
        state.observe_user_input(b"submitted-but-parser-missed-boundary");
        state.last_user_input_at =
            Some(Instant::now() - USER_INPUT_IDLE_INJECT - Duration::from_millis(1));
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
        assert_eq!(received, "notify\r");
    }

    #[test]
    fn control_request_sgr_mouse_after_boundary_injects_without_idle_fallback() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        write_inject_frame(&mut client, b"notify").unwrap();
        let (real_read_end, _real_write_end) = pipe_files();
        let (read_end, write_end) = pipe_files();
        let mut state = InputLineState::default();
        state.observe_user_input(b"\x1b[<35;79;1M\x1b[<0;79;1m");
        let mut buffer = [0_u8; 256];
        let started = Instant::now();

        process_control_request(
            &mut server,
            real_read_end.as_raw_fd(),
            write_end.as_raw_fd(),
            &mut state,
            &mut buffer,
        )
        .unwrap();

        assert!(
            started.elapsed() < Duration::from_millis(200),
            "mouse-only input should not wait for user-input idle fallback"
        );
        drop(write_end);
        let mut read_end = read_end;
        let mut received = String::new();
        read_end.read_to_string(&mut received).unwrap();
        assert_eq!(received, "notify\r");
    }

    #[test]
    fn control_request_mid_escape_returns_err() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        write_inject_frame(&mut client, b"notify").unwrap();
        let (real_read_end, _real_write_end) = pipe_files();
        let (read_end, write_end) = pipe_files();
        let mut state = InputLineState::default();
        state.observe_user_input(b"\x1b[13;");
        state.last_user_input_at =
            Some(Instant::now() - USER_INPUT_IDLE_INJECT - Duration::from_millis(1));
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
        drop(write_end);
        let mut read_end = read_end;
        let mut received = String::new();
        read_end.read_to_string(&mut received).unwrap();
        assert!(received.is_empty(), "unsafe injection wrote: {received:?}");
    }

    #[test]
    fn control_request_idle_redraw_output_injects_once_after_short_debounce() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        write_inject_frame(&mut client, b"notify").unwrap();
        let (real_fd, _real_peer) = socketpair_files();
        let (master, child_peer) = socketpair_files();
        let child_writer = child_peer.try_clone().unwrap();
        let child_reader = child_peer;
        let mut state = InputLineState::default();
        let mut output_state = ChildOutputState::default();
        output_state.observe_child_output();
        let mut pending = PendingChildInput::new();
        let mut buffer = [0_u8; 256];
        let writer = thread::spawn(move || {
            let mut child_writer = child_writer;
            thread::sleep(INJECT_CHILD_OUTPUT_DEBOUNCE + Duration::from_millis(175));
            for _ in 0..6 {
                if child_writer.write_all(b"child output\n").is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(250));
            }
        });
        let started = Instant::now();

        let result = {
            let mut request_io = ControlRequestIo {
                real_fd: real_fd.as_raw_fd(),
                master_fd: master.as_raw_fd(),
                child_pid: None,
                line_state: &mut state,
                child_output_state: &mut output_state,
                pending_child_input: &mut pending,
                buffer: &mut buffer,
            };
            process_control_request_with_pending(&mut server, &mut request_io, None)
        };

        assert_eq!(result, Ok(()));
        assert!(
            started.elapsed() < Duration::from_millis(800),
            "idle redraws must not require the old long quiet window"
        );
        set_nonblocking(child_reader.as_raw_fd());
        let mut injected = Vec::new();
        flush_pending_child_input_to_completion(master.as_raw_fd(), &mut pending).unwrap();
        drain_available(child_reader.as_raw_fd(), &mut injected).unwrap();
        drop(master);
        writer.join().unwrap();
        assert_eq!(
            String::from_utf8_lossy(&injected).matches("notify").count(),
            1
        );
    }

    #[test]
    fn control_request_active_child_output_returns_err_without_injection() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        write_inject_frame(&mut client, b"notify").unwrap();
        let (real_fd, _real_peer) = socketpair_files();
        let (master, child_peer) = socketpair_files();
        let child_writer = child_peer.try_clone().unwrap();
        let child_reader = child_peer;
        let mut state = InputLineState::default();
        let mut output_state = ChildOutputState::default();
        output_state.observe_child_output();
        let mut pending = PendingChildInput::new();
        let mut buffer = [0_u8; 256];
        let writer = thread::spawn(move || {
            let mut child_writer = child_writer;
            for _ in 0..48 {
                if child_writer.write_all(b"child output\n").is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(35));
            }
        });

        let err = {
            let mut request_io = ControlRequestIo {
                real_fd: real_fd.as_raw_fd(),
                master_fd: master.as_raw_fd(),
                child_pid: None,
                line_state: &mut state,
                child_output_state: &mut output_state,
                pending_child_input: &mut pending,
                buffer: &mut buffer,
            };
            process_control_request_with_pending(&mut server, &mut request_io, None).unwrap_err()
        };

        writer.join().unwrap();
        assert_eq!(err, "unsafe_child_output_active");
        assert!(pending.is_empty());
        set_nonblocking(child_reader.as_raw_fd());
        let mut injected = Vec::new();
        drain_available(child_reader.as_raw_fd(), &mut injected).unwrap();
        assert!(
            !String::from_utf8_lossy(&injected).contains("notify"),
            "unsafe injection wrote: {injected:?}"
        );
    }

    #[test]
    fn notify_trace_file_append_is_default_and_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(NOTIFY_TRACE_FILE);

        append_notify_trace_line(
            &path,
            "2026-06-27T00:00:00.000Z trigger=pty-control decision=inject inject_status=acked\n",
        )
        .unwrap();

        let trace = fs::read_to_string(&path).unwrap();
        assert!(trace.contains("decision=inject"));
        assert!(trace.contains("inject_status=acked"));
    }

    #[test]
    fn notify_trace_file_rotates_when_size_cap_would_be_exceeded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(NOTIFY_TRACE_FILE);
        fs::write(&path, vec![b'x'; NOTIFY_TRACE_MAX_BYTES as usize]).unwrap();

        append_notify_trace_line(&path, "next\n").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "next\n");
        assert!(dir.path().join(NOTIFY_TRACE_ROTATED_FILE).exists());
    }

    #[test]
    fn overlay_input_trace_file_records_ctrl_enter_classification() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(OVERLAY_INPUT_TRACE_FILE);
        let line = overlay_input_trace_line("1b5b31333b3575", "ctrl_enter");

        append_overlay_input_trace_line(&path, &line).unwrap();

        let trace = fs::read_to_string(&path).unwrap();
        assert!(trace.contains("trigger=overlay-input"));
        assert!(trace.contains("bytes_hex=1b5b31333b3575"));
        assert!(trace.contains("classification=ctrl_enter"));
    }

    #[test]
    fn overlay_input_trace_file_rotates_when_size_cap_would_be_exceeded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(OVERLAY_INPUT_TRACE_FILE);
        fs::write(&path, vec![b'x'; OVERLAY_INPUT_TRACE_MAX_BYTES as usize]).unwrap();

        append_overlay_input_trace_line(&path, "next\n").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "next\n");
        assert!(dir.path().join(OVERLAY_INPUT_TRACE_ROTATED_FILE).exists());
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

    #[test]
    fn pending_child_input_flush_some_is_nonblocking_under_backpressure() {
        let (_read_end, write_end) = pipe_files();
        set_nonblocking(write_end.as_raw_fd());
        let mut pending = PendingChildInput::new();
        pending.enqueue(&deterministic_bytes(LARGE_CHILD_INPUT_BYTES));

        let mut saw_partial_pending = false;
        let mut saw_would_block = false;
        for _ in 0..128 {
            let before = pending.pending_len();
            let progress = pending.flush_some(write_end.as_raw_fd()).unwrap();
            assert!(
                progress.wrote < before,
                "one bounded flush must not drain the whole queue: {progress:?}, before={before}"
            );
            if !progress.fully_drained && !pending.is_empty() {
                saw_partial_pending = true;
            }
            if progress.wrote == 0 {
                saw_would_block = true;
                assert_eq!(write_some_fd(write_end.as_raw_fd(), b"x").unwrap(), 0);
                break;
            }
        }

        assert!(
            saw_partial_pending,
            "queue should remain pending under backpressure"
        );
        assert!(
            saw_would_block,
            "nonblocking pipe should eventually report EAGAIN"
        );
        assert!(!pending.is_empty());
    }

    #[test]
    fn pending_child_input_preserves_order_and_integrity_across_partial_flushes() {
        let (read_end, write_end) = pipe_files();
        set_nonblocking(read_end.as_raw_fd());
        set_nonblocking(write_end.as_raw_fd());
        let expected = deterministic_bytes(LARGE_CHILD_INPUT_BYTES);
        let mut pending = PendingChildInput::new();
        pending.enqueue(&expected);

        let received =
            flush_pending_to_pipe(&mut pending, write_end.as_raw_fd(), read_end.as_raw_fd());

        assert!(pending.is_empty());
        assert_eq!(received, expected);
    }

    #[test]
    fn poll_relay_fds_observes_real_ctrl_c_while_child_burst_is_pending() {
        let (real_read, mut real_write) = pipe_files();
        let (master, _peer) = socketpair_files();
        let mut pending = PendingChildInput::new();
        pending.enqueue(&deterministic_bytes(LARGE_CHILD_INPUT_BYTES));
        real_write.write_all(&[0x03]).unwrap();

        let ready = poll_relay_fds(real_read.as_raw_fd(), master.as_raw_fd(), None, true).unwrap();

        assert!(
            ready.real_input,
            "Ctrl+C must remain observable while draining child input"
        );
        assert!(
            ready.pty_writable,
            "pending child input should arm writable readiness"
        );
        assert!(pending.pending_len() >= LARGE_CHILD_INPUT_BYTES);
        let mut byte = [0_u8; 1];
        assert_eq!(read_fd(real_read.as_raw_fd(), &mut byte).unwrap(), 1);
        assert_eq!(byte[0], 0x03);
        assert!(
            !pending.is_empty(),
            "Ctrl+C should be read before the burst fully flushes"
        );
    }

    #[test]
    fn poll_relay_fds_reports_pty_writable_only_when_child_write_is_requested() {
        let (real_read, _real_write) = pipe_files();
        let (master, _peer) = socketpair_files();

        assert!(!writable(0));
        assert!(writable(libc::POLLOUT));
        assert!(!writable(libc::POLLHUP));
        assert!(!writable(libc::POLLERR));
        assert!(writable(libc::POLLOUT | libc::POLLHUP));

        let idle_pollfds = relay_poll_fds(real_read.as_raw_fd(), master.as_raw_fd(), None, false);
        let idle_master = idle_pollfds
            .iter()
            .find(|pollfd| pollfd.fd == master.as_raw_fd())
            .expect("master fd should be present in idle poll set");
        assert_eq!(
            idle_master.events & libc::POLLOUT,
            0,
            "idle relay must not request POLLOUT"
        );

        let draining_pollfds =
            relay_poll_fds(real_read.as_raw_fd(), master.as_raw_fd(), None, true);
        let draining_master = draining_pollfds
            .iter()
            .find(|pollfd| pollfd.fd == master.as_raw_fd())
            .expect("master fd should be present in draining poll set");
        assert!(
            draining_master.events & libc::POLLOUT != 0,
            "pending queue must request POLLOUT"
        );
    }

    #[test]
    fn pending_child_input_keeps_control_payload_after_queued_user_bytes() {
        for (bracketed, submit, expected_control) in [
            (false, true, b"[notification payload]".to_vec()),
            (
                true,
                true,
                b"\x1b[200~[notification payload]\x1b[201~".to_vec(),
            ),
            (false, false, b"[notification payload]".to_vec()),
        ] {
            let (read_end, write_end) = pipe_files();
            set_nonblocking(read_end.as_raw_fd());
            set_nonblocking(write_end.as_raw_fd());
            let queued_user = deterministic_bytes(64 * 1024);
            let mut expected = queued_user.clone();
            expected.extend_from_slice(&expected_control);
            if submit {
                expected.push(b'\r');
            }
            let mut pending = PendingChildInput::new();
            pending.enqueue(&queued_user);

            queue_control_injection(&mut pending, b"[notification payload]", bracketed, submit);
            let received =
                flush_pending_to_pipe(&mut pending, write_end.as_raw_fd(), read_end.as_raw_fd());

            assert_eq!(received, expected);
            assert_eq!(&received[..queued_user.len()], queued_user.as_slice());
            let control_start = queued_user.len();
            let control_end = control_start + expected_control.len();
            assert_eq!(
                &received[control_start..control_end],
                expected_control.as_slice()
            );
            if submit {
                assert_eq!(received[control_end], b'\r');
            } else {
                assert_eq!(control_end, received.len());
                assert!(!received.ends_with(b"\r"));
            }
        }
    }

    const LARGE_CHILD_INPUT_BYTES: usize = 256 * 1024;

    fn deterministic_bytes(len: usize) -> Vec<u8> {
        (0..len)
            .map(|index| ((index.wrapping_mul(31) + index / 7) % 251) as u8)
            .collect()
    }

    fn set_nonblocking(fd: RawFd) {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        assert_ne!(flags, -1, "F_GETFL failed: {}", io::Error::last_os_error());
        let rc = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
        assert_ne!(rc, -1, "F_SETFL failed: {}", io::Error::last_os_error());
    }

    fn flush_pending_to_pipe(
        pending: &mut PendingChildInput,
        write_fd: RawFd,
        read_fd: RawFd,
    ) -> Vec<u8> {
        let mut received = Vec::new();
        let mut attempts = 0;
        while !pending.is_empty() {
            attempts += 1;
            assert!(
                attempts < 4096,
                "pending queue did not drain deterministically"
            );
            let before = pending.pending_len();
            let progress = pending.flush_some(write_fd).unwrap();
            assert!(progress.wrote <= before);
            assert_eq!(progress.fully_drained, pending.is_empty());
            let drained = drain_available(read_fd, &mut received).unwrap();
            assert!(
                progress.wrote > 0 || drained > 0,
                "flush made no progress and no pipe bytes were available"
            );
        }
        drain_available(read_fd, &mut received).unwrap();
        received
    }

    fn drain_available(read_fd: RawFd, output: &mut Vec<u8>) -> io::Result<usize> {
        let mut total = 0;
        let mut buffer = [0_u8; 8192];
        loop {
            let rc = unsafe { libc::read(read_fd, buffer.as_mut_ptr().cast(), buffer.len()) };
            if rc > 0 {
                let n = rc as usize;
                output.extend_from_slice(&buffer[..n]);
                total += n;
                continue;
            }
            if rc == 0 {
                return Ok(total);
            }
            let err = io::Error::last_os_error();
            match err.raw_os_error() {
                Some(code) if code == libc::EINTR => continue,
                Some(code) if code == libc::EAGAIN || code == libc::EWOULDBLOCK => {
                    return Ok(total);
                }
                _ => return Err(err),
            }
        }
    }

    fn socketpair_files() -> (File, File) {
        let mut fds = [-1; 2];
        assert_eq!(
            unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr()) },
            0
        );
        let left = unsafe { File::from_raw_fd(fds[0]) };
        let right = unsafe { File::from_raw_fd(fds[1]) };
        (left, right)
    }

    fn pipe_files() -> (File, File) {
        let mut fds = [-1; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let read_end = unsafe { File::from_raw_fd(fds[0]) };
        let write_end = unsafe { File::from_raw_fd(fds[1]) };
        (read_end, write_end)
    }
}
