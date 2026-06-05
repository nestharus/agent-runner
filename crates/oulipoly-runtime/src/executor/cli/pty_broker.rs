//! Unix PTY broker for live interactive sessions.

use super::spawn_identity::{SpawnIdentityContext, record_child_identity};
use super::terminal_signal::{InteractiveSignalGuard, exit_code_from_status};
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
    let Ok(dir) = control_socket_dir() else {
        return false;
    };
    if !path.starts_with(&dir) {
        return false;
    }
    fs::remove_file(path).is_ok()
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
    let winsize = terminal_winsize(real_tty.fd())
        .map_err(|err| format!("Failed to read terminal window size: {err}"))?;
    let pty = PtyPair::open(&winsize, &real_tty.original)?;
    let control = ControlSocket::bind_for(context)?;
    configure_child_pty(&mut cmd, &pty)?;
    let mut child = cmd
        .spawn()
        .map_err(|err| format!("Failed to spawn '{}': {err}", provider.command))?;
    let recorded_context = control.as_ref().and_then(|control| {
        context.map(|context| context.with_pty_control_path(control.path_string()))
    });
    record_child_identity(child.id(), recorded_context.as_ref().or(context));
    let mut idle_guard = SessionRuntimeIdleGuard::new(context);
    drop(pty.slave);

    let signal_guard = InteractiveSignalGuard::install_process_group(child.id())?;
    let mut raw_tty = real_tty.into_raw_mode()?;
    let status = relay_until_exit(&mut raw_tty, &pty.master, control.as_ref(), &mut child)?;
    idle_guard.exit_code = Some(exit_code_from_status(&status));
    drop(signal_guard);
    Ok(status)
}

fn validate_client_payload(bytes: &[u8]) -> Result<(), PtyControlClientError> {
    if bytes.is_empty() {
        return Err(PtyControlClientError {
            kind: PtyControlClientErrorKind::EmptyPayload,
            message: "empty payload".to_string(),
        });
    }
    if bytes.len() > CONTROL_MAX_PAYLOAD_BYTES {
        return Err(PtyControlClientError {
            kind: PtyControlClientErrorKind::Oversize,
            message: format!("payload exceeds {CONTROL_MAX_PAYLOAD_BYTES} bytes"),
        });
    }
    Ok(())
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
    let mut header = [0_u8; 12];
    stream.read_exact(&mut header).map_err(protocol_error)?;
    if &header[..4] != CONTROL_MAGIC || header[4] != CONTROL_VERSION {
        return Err(PtyControlClientError {
            kind: PtyControlClientErrorKind::Protocol,
            message: "bad response header".to_string(),
        });
    }
    let status = header[5];
    if header[6] != 0 || header[7] != 0 {
        return Err(PtyControlClientError {
            kind: PtyControlClientErrorKind::Protocol,
            message: "bad response reserved bytes".to_string(),
        });
    }
    let length = u32::from_be_bytes([header[8], header[9], header[10], header[11]]) as usize;
    if length > CONTROL_MAX_PAYLOAD_BYTES {
        return Err(PtyControlClientError {
            kind: PtyControlClientErrorKind::Protocol,
            message: "oversized response".to_string(),
        });
    }
    let mut message = vec![0_u8; length];
    stream.read_exact(&mut message).map_err(protocol_error)?;
    let message = String::from_utf8(message).map_err(|err| PtyControlClientError {
        kind: PtyControlClientErrorKind::Protocol,
        message: err.to_string(),
    })?;
    Ok(PtyControlResponse {
        ack: status == 0,
        message,
    })
}

fn write_inject_frame(stream: &mut UnixStream, payload: &[u8]) -> io::Result<()> {
    let mut header = [0_u8; 12];
    header[..4].copy_from_slice(CONTROL_MAGIC);
    header[4] = CONTROL_VERSION;
    header[5] = CONTROL_OP_INJECT;
    header[8..12].copy_from_slice(&(payload.len() as u32).to_be_bytes());
    stream.write_all(&header)?;
    stream.write_all(payload)
}

struct PtyPair {
    master: File,
    slave: File,
}

impl PtyPair {
    fn open(winsize: &libc::winsize, termios: &libc::termios) -> Result<Self, String> {
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
            return Err(format!(
                "Failed to allocate PTY: {}",
                io::Error::last_os_error()
            ));
        }
        let master = unsafe { File::from_raw_fd(master_fd) };
        let slave = unsafe { File::from_raw_fd(slave_fd) };
        Ok(Self { master, slave })
    }
}

struct RealTerminal {
    file: File,
    original: libc::termios,
}

impl RealTerminal {
    fn open() -> Result<Self, String> {
        let file = open_real_terminal().map_err(|err| format!("Failed to open /dev/tty: {err}"))?;
        let original = terminal_attrs(file.as_raw_fd())
            .map_err(|err| format!("Failed to read terminal attributes: {err}"))?;
        Ok(Self { file, original })
    }

    fn fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }

    fn into_raw_mode(self) -> Result<RawTerminalGuard, String> {
        let mut raw = self.original;
        unsafe { libc::cfmakeraw(&mut raw) };
        set_terminal_attrs(self.fd(), &raw)
            .map_err(|err| format!("Failed to enter terminal raw mode: {err}"))?;
        Ok(RawTerminalGuard { inner: self })
    }
}

struct RawTerminalGuard {
    inner: RealTerminal,
}

impl RawTerminalGuard {
    fn fd(&self) -> RawFd {
        self.inner.fd()
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
    use std::os::unix::process::CommandExt;

    let stdin = pty
        .slave
        .try_clone()
        .map_err(|err| format!("Failed to clone PTY slave for stdin: {err}"))?;
    let stdout = pty
        .slave
        .try_clone()
        .map_err(|err| format!("Failed to clone PTY slave for stdout: {err}"))?;
    let stderr = pty
        .slave
        .try_clone()
        .map_err(|err| format!("Failed to clone PTY slave for stderr: {err}"))?;
    let slave_fd = pty.slave.as_raw_fd();
    let master_fd = pty.master.as_raw_fd();
    cmd.stdin(Stdio::from(stdin));
    cmd.stdout(Stdio::from(stdout));
    cmd.stderr(Stdio::from(stderr));
    unsafe {
        cmd.pre_exec(move || {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            if libc::ioctl(slave_fd, libc::TIOCSCTTY, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            let pid = libc::getpid();
            if libc::tcsetpgrp(slave_fd, pid) == -1 {
                return Err(io::Error::last_os_error());
            }
            if master_fd > 2 {
                libc::close(master_fd);
            }
            if slave_fd > 2 {
                libc::close(slave_fd);
            }
            Ok(())
        });
    }
    Ok(())
}

struct ControlSocket {
    listener: UnixListener,
    path: PathBuf,
    owned_dir: PathBuf,
}

impl ControlSocket {
    fn bind_for(context: Option<&SpawnIdentityContext>) -> Result<Option<Self>, String> {
        let Some(context) = context else {
            return Ok(None);
        };
        let Some(session_id) = context.session_id() else {
            return Ok(None);
        };
        let (dir, path) = control_socket_location(session_id, context.invocation_uuid())?;
        create_private_dir(&dir)?;
        unlink_stale_or_refuse_active(&path, &dir)?;
        let previous_umask = unsafe { libc::umask(0o077) };
        let listener = UnixListener::bind(&path).map_err(|err| {
            format!(
                "Failed to bind PTY control socket {}: {err}",
                path.display()
            )
        });
        unsafe { libc::umask(previous_umask) };
        let listener = listener?;
        fs::set_permissions(&path, Permissions::from_mode(0o600)).map_err(|err| {
            format!(
                "Failed to chmod PTY control socket {}: {err}",
                path.display()
            )
        })?;
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

impl Drop for ControlSocket {
    fn drop(&mut self) {
        unlink_owned_socket(&self.path, &self.owned_dir);
    }
}

fn control_socket_dir() -> Result<PathBuf, String> {
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        return Ok(PathBuf::from(runtime).join("oulipoly-agent-runner/pty"));
    }
    if let Some(state) = std::env::var_os("XDG_STATE_HOME") {
        return Ok(PathBuf::from(state).join("oulipoly-agent-runner/runtime/pty"));
    }
    let base =
        dirs::data_dir().ok_or_else(|| "Failed to resolve user data directory".to_string())?;
    Ok(base.join("oulipoly-agent-runner/runtime/pty"))
}

fn create_private_dir(dir: &Path) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|err| {
        format!(
            "Failed to create PTY control socket directory {}: {err}",
            dir.display()
        )
    })?;
    fs::set_permissions(dir, Permissions::from_mode(0o700)).map_err(|err| {
        format!(
            "Failed to chmod PTY control socket directory {}: {err}",
            dir.display()
        )
    })
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
    if path.as_os_str().as_bytes().len() < UNIX_SOCKET_PATH_LIMIT {
        return (dir, path);
    }
    let fallback = short_control_socket_dir();
    (
        fallback.clone(),
        fallback.join(format!(
            "{}.sock",
            stable_socket_hash(session_id, invocation_uuid)
        )),
    )
}

fn control_socket_path(dir: &Path, session_id: &str, invocation_uuid: &str) -> PathBuf {
    let basename = format!(
        "{}.{}.sock",
        short_component(session_id),
        short_component(invocation_uuid)
    );
    let candidate = dir.join(&basename);
    if candidate.as_os_str().as_bytes().len() < UNIX_SOCKET_PATH_LIMIT {
        return candidate;
    }
    dir.join(format!(
        "{}.sock",
        stable_socket_hash(session_id, invocation_uuid)
    ))
}

fn short_control_socket_dir() -> PathBuf {
    let uid = unsafe { libc::geteuid() };
    PathBuf::from("/tmp").join(format!("oulipoly-agent-runner-pty-{uid}"))
}

fn short_component(value: &str) -> String {
    let sanitized = value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
        .take(12)
        .collect::<String>();
    if sanitized.is_empty() {
        "session".to_string()
    } else {
        sanitized
    }
}

fn stable_socket_hash(session_id: &str, invocation_uuid: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(session_id.as_bytes());
    hasher.update([0]);
    hasher.update(invocation_uuid.as_bytes());
    let digest = hasher.finalize();
    digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn unlink_stale_or_refuse_active(path: &Path, owned_dir: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    if UnixStream::connect(path).is_ok() {
        return Err(format!(
            "PTY control socket already active at {}",
            path.display()
        ));
    }
    unlink_owned_socket(path, owned_dir);
    Ok(())
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
        status = child
            .try_wait()
            .map_err(|err| format!("Failed to poll interactive child: {err}"))?;
    }
    drain_pty_output(master.as_raw_fd(), real_tty.fd(), &mut buffer)?;
    Ok(status.expect("status checked above"))
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
    let mut fds = vec![
        libc::pollfd {
            fd: real_fd,
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: master_fd,
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    if let Some(fd) = control_fd {
        fds.push(libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        });
    }
    let rc = unsafe {
        libc::poll(
            fds.as_mut_ptr(),
            fds.len() as libc::nfds_t,
            RELAY_POLL_TIMEOUT_MS,
        )
    };
    if rc < 0 {
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EINTR) {
            return Ok(ReadyFds::default());
        }
        return Err(format!("Failed to poll PTY relay fds: {err}"));
    }
    Ok(ReadyFds {
        real_input: readable(fds[0].revents),
        pty_output: readable(fds[1].revents),
        control: fds.get(2).is_some_and(|fd| readable(fd.revents)),
    })
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
            write_all_fd(master_fd, &buffer[..n])
                .map_err(|err| format!("Failed to write user input to PTY: {err}"))
        }
        Err(err) => Err(format!("Failed to read user terminal input: {err}")),
    }
}

fn relay_pty_output(master_fd: RawFd, real_fd: RawFd, buffer: &mut [u8]) -> Result<(), String> {
    match read_fd(master_fd, buffer) {
        Ok(0) => Ok(()),
        Ok(n) => write_all_fd(real_fd, &buffer[..n])
            .map_err(|err| format!("Failed to write PTY output to terminal: {err}")),
        Err(err) if is_pty_eof_error(&err) => Ok(()),
        Err(err) => Err(format!("Failed to read PTY output: {err}")),
    }
}

fn drain_pty_output(master_fd: RawFd, real_fd: RawFd, buffer: &mut [u8]) -> Result<(), String> {
    loop {
        let ready = poll_single_fd(master_fd)?;
        if !ready {
            return Ok(());
        }
        match read_fd(master_fd, buffer) {
            Ok(0) => return Ok(()),
            Ok(n) => write_all_fd(real_fd, &buffer[..n])
                .map_err(|err| format!("Failed to drain PTY output to terminal: {err}"))?,
            Err(err) if is_pty_eof_error(&err) => return Ok(()),
            Err(err) => return Err(format!("Failed to drain PTY output: {err}")),
        }
    }
}

fn poll_single_fd(fd: RawFd) -> Result<bool, String> {
    let mut pollfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let rc = unsafe { libc::poll(&mut pollfd, 1, RELAY_POLL_TIMEOUT_MS) };
    if rc == -1 {
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EINTR) {
            return Ok(false);
        }
        return Err(format!("Failed to poll PTY drain fd: {err}"));
    }
    Ok(rc > 0 && readable(pollfd.revents))
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
        if bytes.is_empty() {
            return;
        }
        self.last_user_input_at = Some(Instant::now());
        for byte in bytes {
            match *byte {
                b'\r' | b'\n' | 0x03 | 0x04 | 0x15 => self.at_line_boundary = true,
                _ => self.at_line_boundary = false,
            }
        }
    }

    fn is_safe_to_inject(&self) -> bool {
        self.at_line_boundary
            && self
                .last_user_input_at
                .map(|last| last.elapsed() >= INJECT_DEBOUNCE)
                .unwrap_or(true)
    }
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
        .map_err(|err| format!("Failed to accept PTY control connection: {err}"))?;
    stream
        .set_read_timeout(Some(CONTROL_IO_TIMEOUT))
        .map_err(|err| format!("Failed to set PTY control read timeout: {err}"))?;
    stream
        .set_write_timeout(Some(CONTROL_IO_TIMEOUT))
        .map_err(|err| format!("Failed to set PTY control write timeout: {err}"))?;
    let response = process_control_request(&mut stream, real_fd, master_fd, line_state, buffer);
    let (ack, message) = match response {
        Ok(()) => (true, "ok".to_string()),
        Err(message) => (false, message),
    };
    write_control_response(&mut stream, ack, &message)
        .map_err(|err| format!("Failed to write PTY control response: {err}"))
}

fn process_control_request(
    stream: &mut UnixStream,
    real_fd: RawFd,
    master_fd: RawFd,
    line_state: &mut InputLineState,
    buffer: &mut [u8],
) -> Result<(), String> {
    validate_peer_uid(stream)?;
    let payload = read_control_request(stream)?;
    wait_until_safe_to_inject(real_fd, master_fd, line_state, buffer)?;
    write_all_fd(master_fd, &payload).map_err(|err| format!("pty_write_failed: {err}"))?;
    write_all_fd(master_fd, b"\n").map_err(|err| format!("pty_submit_failed: {err}"))?;
    line_state.at_line_boundary = true;
    line_state.last_user_input_at = None;
    Ok(())
}

fn read_control_request(stream: &mut UnixStream) -> Result<Vec<u8>, String> {
    let mut header = [0_u8; 12];
    stream
        .read_exact(&mut header)
        .map_err(|err| format!("read_failed: {err}"))?;
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
    let length = u32::from_be_bytes([header[8], header[9], header[10], header[11]]) as usize;
    if length == 0 {
        return Err("empty_payload".to_string());
    }
    if length > CONTROL_MAX_PAYLOAD_BYTES {
        return Err("oversize_frame".to_string());
    }
    let mut payload = vec![0_u8; length];
    stream
        .read_exact(&mut payload)
        .map_err(|err| format!("read_payload_failed: {err}"))?;
    std::str::from_utf8(&payload).map_err(|_| "invalid_utf8".to_string())?;
    Ok(payload)
}

fn wait_until_safe_to_inject(
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
        let ready = poll_relay_fds(real_fd, master_fd, None)?;
        if ready.real_input {
            relay_real_input(real_fd, master_fd, line_state, buffer)?;
        }
        if ready.pty_output {
            relay_pty_output(master_fd, real_fd, buffer)?;
        }
    }
    if line_state.is_safe_to_inject() {
        return Ok(());
    }
    Err("unsafe_mid_line".to_string())
}

fn write_control_response(stream: &mut UnixStream, ack: bool, message: &str) -> io::Result<()> {
    let mut header = [0_u8; 12];
    header[..4].copy_from_slice(CONTROL_MAGIC);
    header[4] = CONTROL_VERSION;
    header[5] = if ack { 0 } else { 1 };
    let bytes = message.as_bytes();
    header[8..12].copy_from_slice(&(bytes.len() as u32).to_be_bytes());
    stream.write_all(&header)?;
    stream.write_all(bytes)
}

#[cfg(target_os = "linux")]
fn validate_peer_uid(stream: &UnixStream) -> Result<(), String> {
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
        return Err(format!("peercred_failed: {}", io::Error::last_os_error()));
    }
    let expected = unsafe { libc::geteuid() };
    if cred.uid != expected {
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
