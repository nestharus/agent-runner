//! Private, feature-gated CLI execution. This is deliberately separate from
//! production L: no installed State or service route calls this module.

const PRIVATE_INSTALLED_REAP_POLL: std::time::Duration = std::time::Duration::from_millis(200);

use oulipoly_kernel_broker::entry_registry::ProcessStamp;
use oulipoly_kernel_broker::identity::{PeerIdentity, PinnedProcess};
use oulipoly_kernel_broker::installed_launch::{self, EntryKind, InstalledLaunchSpec};
use oulipoly_kernel_broker::protocol::supported_offline_entry_args;
use serde::{Deserialize, Serialize};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

const RECORD_PROTOCOL: &str = "oulipoly-private-installed-exec/v1";
static CANCELLED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    protocol: String,
    generation: String,
    request_id: String,
    uid: u32,
    entry: ProcessStamp,
    guardian: Option<ProcessStamp>,
    init: Option<ProcessStamp>,
    child: Option<ProcessStamp>,
    terminal: Option<String>,
}

pub(super) struct LaunchLedger {
    directory: PathBuf,
}

impl LaunchLedger {
    pub(super) fn open(state: &Path) -> io::Result<Self> {
        let directory = state.join("private-launches");
        if !directory.exists() {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new().mode(0o700).create(&directory)?;
        }
        let ledger = Self { directory };
        for item in fs::read_dir(&ledger.directory)? {
            let item = item?;
            let name = item.file_name();
            let name = name.to_string_lossy();
            if !name.ends_with(".json") || !item.file_type()?.is_file() {
                return Err(io::Error::other("unrecognized private launch record"));
            }
            let record: Record = serde_json::from_slice(&fs::read(item.path())?)?;
            if record.protocol != RECORD_PROTOCOL
                || format!("{}.json", record.request_id) != name
                || uuid::Uuid::parse_str(&record.request_id).is_err()
            {
                return Err(io::Error::other("invalid private launch record"));
            }
        }
        Ok(ledger)
    }

    fn path(&self, request_id: &str) -> PathBuf {
        self.directory.join(format!("{request_id}.json"))
    }

    fn reserve(&self, record: &Record) -> io::Result<()> {
        for item in fs::read_dir(&self.directory)? {
            let current: Record = serde_json::from_slice(&fs::read(item?.path())?)?;
            if current.request_id == record.request_id
                || !current
                    .terminal
                    .as_deref()
                    .is_some_and(|line| line.starts_with("exit ") && line.contains(" drained "))
            {
                return Err(io::Error::other("private launch duplicate or unsettled"));
            }
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(self.path(&record.request_id))?;
        serde_json::to_writer(&mut file, record)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        File::open(&self.directory)?.sync_all()
    }

    fn replace(&self, record: &Record) -> io::Result<()> {
        let temp = self
            .directory
            .join(format!(".{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temp)?;
            serde_json::to_writer(&mut file, record)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(&temp, self.path(&record.request_id))?;
            File::open(&self.directory)?.sync_all()
        })();
        if result.is_err() {
            let _ = fs::remove_file(temp);
        }
        result
    }

    fn read(&self, request_id: &str) -> io::Result<Record> {
        uuid::Uuid::parse_str(request_id).map_err(|_| io::Error::other("invalid request ID"))?;
        serde_json::from_slice(&fs::read(self.path(request_id))?).map_err(Into::into)
    }

    pub(super) fn status(
        &self,
        request_id: &str,
        generation: &str,
        uid: u32,
        cancel: bool,
    ) -> io::Result<String> {
        let record = self.read(request_id)?;
        if record.uid != uid || record.generation != generation {
            return Err(io::Error::other("private launch owner mismatch"));
        }
        if let Some(terminal) = record.terminal {
            return Ok(terminal);
        }
        let Some(stamp) = record.init else {
            return Ok(format!("uncertain {request_id}\n"));
        };
        let init = match PinnedProcess::open(stamp.host_pid) {
            Ok(init) => init,
            Err(_) => return Ok(format!("uncertain {request_id}\n")),
        };
        if ProcessStamp::from(&init) != stamp || !init.is_namespace_init()? {
            return Ok(format!("uncertain {request_id}\n"));
        }
        if cancel {
            init.signal(libc::SIGUSR1)?;
            Ok(format!("cancel-requested {request_id}\n"))
        } else {
            Ok(format!("running {request_id}\n"))
        }
    }
}

fn close_other_descriptors(keep: &[RawFd]) -> io::Result<()> {
    let mut discard = Vec::new();
    for item in fs::read_dir("/proc/self/fd")? {
        let fd = item?
            .file_name()
            .to_string_lossy()
            .parse::<RawFd>()
            .unwrap_or(-1);
        if fd > 2 && !keep.contains(&fd) {
            discard.push(fd);
        }
    }
    for fd in discard {
        unsafe { libc::close(fd) };
    }
    Ok(())
}

extern "C" fn request_cancel(_: libc::c_int) {
    CANCELLED.store(true, Ordering::Relaxed);
}

struct InitContext {
    spec: InstalledLaunchSpec,
    descriptors: Vec<File>,
    image: File,
    gate: UnixStream,
    status: UnixStream,
    child_control: UnixStream,
    child_gate: UnixStream,
    uid: u32,
    gid: u32,
    gui_groups: Vec<libc::gid_t>,
}

fn environment<'a>(spec: &'a InstalledLaunchSpec, name: &[u8]) -> Option<&'a OsStr> {
    spec.environment
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| OsStr::from_bytes(value))
}

fn private_gui_session(spec: &InstalledLaunchSpec, uid: u32) -> io::Result<()> {
    if !spec.args.is_empty() {
        return Err(io::Error::other("private GUI argv refused"));
    }
    let runtime = environment(spec, b"XDG_RUNTIME_DIR")
        .ok_or_else(|| io::Error::other("private GUI runtime directory missing"))?;
    let runtime = Path::new(runtime);
    let metadata = fs::symlink_metadata(runtime)?;
    if !runtime.is_absolute()
        || !metadata.file_type().is_dir()
        || metadata.uid() != uid
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(io::Error::other(
            "private GUI runtime directory owner/mode mismatch",
        ));
    }
    let mut display = false;
    if let Some(wayland) = environment(spec, b"WAYLAND_DISPLAY") {
        let name = wayland.as_bytes();
        if name.is_empty() || name.contains(&b'/') || name == b"." || name == b".." {
            return Err(io::Error::other("private GUI Wayland display name refused"));
        }
        private_gui_socket(&runtime.join(wayland), uid, false)?;
        display = true;
    }
    if let Some(x11) = environment(spec, b"DISPLAY") {
        let bytes = x11.as_bytes();
        let number = bytes
            .strip_prefix(b":")
            .and_then(|rest| rest.split(|b| *b == b'.').next());
        let Some(number) = number.filter(|n| !n.is_empty() && n.iter().all(u8::is_ascii_digit))
        else {
            return Err(io::Error::other("private GUI X11 display form refused"));
        };
        let socket = PathBuf::from("/tmp/.X11-unix")
            .join(OsStr::from_bytes(&[b"X".as_slice(), number].concat()));
        private_gui_socket(&socket, uid, true)?;
        display = true;
    }
    if !display {
        return Err(io::Error::other("private GUI display missing"));
    }
    if let Some(address) = environment(spec, b"DBUS_SESSION_BUS_ADDRESS") {
        let path = address
            .as_bytes()
            .strip_prefix(b"unix:path=")
            .ok_or_else(|| io::Error::other("private GUI DBus address refused"))?;
        let path = Path::new(OsStr::from_bytes(path));
        if path.parent() != Some(runtime) {
            return Err(io::Error::other(
                "private GUI DBus path outside runtime directory",
            ));
        }
        private_gui_socket(path, uid, false)?;
    }
    if let Some(authority) = environment(spec, b"XAUTHORITY") {
        let path = Path::new(authority);
        let metadata = fs::symlink_metadata(path)?;
        if !path.is_absolute() || !metadata.file_type().is_file() || metadata.uid() != uid {
            return Err(io::Error::other(
                "private GUI Xauthority owner/type mismatch",
            ));
        }
    }
    Ok(())
}

fn private_gui_socket(path: &Path, uid: u32, allow_root: bool) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != uid && !(allow_root && metadata.uid() == 0)
    {
        return Err(io::Error::other(
            "private GUI display/session socket owner/type mismatch",
        ));
    }
    Ok(())
}

extern "C" fn init_start(pointer: *mut libc::c_void) -> libc::c_int {
    let context = unsafe { Box::from_raw(pointer.cast::<InitContext>()) };
    if run_init(*context).is_ok() { 0 } else { 70 }
}

fn run_init(context: InitContext) -> io::Result<()> {
    if unsafe { libc::getpid() } != 1
        || unsafe { libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) } != 0
    {
        return Err(io::Error::other("private root PID1 lost sudo semantics"));
    }
    let InitContext {
        spec,
        descriptors,
        image,
        mut gate,
        mut status,
        child_control,
        child_gate,
        uid,
        gid,
        gui_groups,
    } = context;
    let mut keep = vec![
        image.as_raw_fd(),
        gate.as_raw_fd(),
        status.as_raw_fd(),
        child_control.as_raw_fd(),
        child_gate.as_raw_fd(),
    ];
    keep.extend(descriptors.iter().map(AsRawFd::as_raw_fd));
    close_other_descriptors(&keep)?;
    let mut release = [0u8; 1];
    gate.read_exact(&mut release)?;
    if release != [b'R'] {
        return Err(io::Error::other("private root release refused"));
    }
    drop(gate);
    CANCELLED.store(false, Ordering::Relaxed);
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = request_cancel as libc::sighandler_t;
    unsafe { libc::sigemptyset(&mut action.sa_mask) };
    if unsafe { libc::sigaction(libc::SIGUSR1, &action, std::ptr::null_mut()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let cwd = descriptors
        .last()
        .ok_or_else(|| io::Error::other("private cwd missing"))?;
    if unsafe { libc::fchdir(cwd.as_raw_fd()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut command = Command::new(format!("/proc/self/fd/{}", image.as_raw_fd()));
    if spec.kind == EntryKind::Gui {
        command.args(["__age319-private-installed-probe-v1", "gui"]);
    } else {
        command.args(spec.args.iter().cloned().map(OsString::from_vec));
    }
    command.env_clear();
    command.envs(
        spec.environment
            .iter()
            .map(|(key, value)| (OsStr::from_bytes(key), OsStr::from_bytes(value))),
    );
    if spec.kind == EntryKind::Gui {
        command.env(
            "OULIPOLY_AGE319_PRIVATE_GUI_STDIO_V1",
            spec.stdio_present
                .iter()
                .map(|present| if *present { '1' } else { '0' })
                .collect::<String>(),
        );
    }
    let mut cursor = 0;
    for (index, present) in spec.stdio_present.into_iter().enumerate() {
        if present {
            let file = descriptors[cursor].try_clone()?;
            match index {
                0 => {
                    command.stdin(Stdio::from(file));
                }
                1 => {
                    command.stdout(Stdio::from(file));
                }
                _ => {
                    command.stderr(Stdio::from(file));
                }
            }
            cursor += 1;
        } else {
            match index {
                0 => {
                    command.stdin(Stdio::null());
                }
                1 => {
                    command.stdout(Stdio::null());
                }
                _ => {
                    command.stderr(Stdio::null());
                }
            }
        }
    }
    let present = spec.stdio_present;
    let gui = spec.kind == EntryKind::Gui;
    let control_fd = child_control.as_raw_fd();
    let child_gate_fd = child_gate.as_raw_fd();
    unsafe {
        command.pre_exec(move || {
            // The broker observed these on the pinned launcher process. Apply
            // them while still privileged, before dropping to its UID/GID.
            if gui && libc::setgroups(gui_groups.len(), gui_groups.as_ptr()) != 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::setsid() < 0
                || libc::setresgid(gid, gid, gid) != 0
                || libc::setresuid(uid, uid, uid) != 0
            {
                return Err(io::Error::last_os_error());
            }
            for (index, exists) in present.into_iter().enumerate() {
                if !exists {
                    libc::close(index as i32);
                }
                let observed = libc::fcntl(index as i32, libc::F_GETFD) >= 0;
                if observed != exists {
                    return Err(io::Error::other("private Runner stdio presence mismatch"));
                }
            }
            let tty = (0..=2).find(|fd| libc::isatty(*fd) == 1);
            if let Some(fd) = tty {
                if libc::ioctl(fd, libc::TIOCSCTTY, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            if libc::send(control_fd, b"C".as_ptr().cast(), 1, libc::MSG_NOSIGNAL) != 1 {
                return Err(io::Error::last_os_error());
            }
            let mut release = 0u8;
            if libc::read(child_gate_fd, (&mut release as *mut u8).cast(), 1) != 1
                || release != b'R'
            {
                return Err(io::Error::other("private Runner child gate refused"));
            }
            Ok(())
        });
    }
    let child = command.spawn()?;
    let original_pid = child.id() as i32;
    drop(child);
    drop(descriptors);
    drop(image);
    drop(child_control);
    drop(child_gate);
    let mut reported = false;
    loop {
        if CANCELLED.swap(false, Ordering::Relaxed) {
            unsafe { libc::kill(-1, libc::SIGTERM) };
            std::thread::sleep(PRIVATE_INSTALLED_REAP_POLL);
            unsafe { libc::kill(-1, libc::SIGKILL) };
        }
        let mut child_status = 0;
        let reaped = unsafe { libc::waitpid(-1, &mut child_status, 0) };
        if reaped == original_pid && !reported {
            let code = if libc::WIFEXITED(child_status) {
                libc::WEXITSTATUS(child_status)
            } else if libc::WIFSIGNALED(child_status) {
                128 + libc::WTERMSIG(child_status)
            } else {
                70
            };
            status.write_all(format!("exit {code}\n").as_bytes())?;
            reported = true;
        }
        if reaped > 0
            || reaped < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted
        {
            continue;
        }
        if reaped < 0 && io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD) {
            return Ok(());
        }
        return Err(io::Error::last_os_error());
    }
}

fn guardian(
    record: Record,
    directory: PathBuf,
    context: InitContext,
    mut client: UnixStream,
    mut broker: UnixStream,
) -> io::Result<()> {
    let (mut guardian_status, init_status) = UnixStream::pair()?;
    let mut context = context;
    context.status = init_status;
    let mut keep = vec![
        client.as_raw_fd(),
        broker.as_raw_fd(),
        guardian_status.as_raw_fd(),
        context.image.as_raw_fd(),
        context.gate.as_raw_fd(),
        context.status.as_raw_fd(),
        context.child_control.as_raw_fd(),
        context.child_gate.as_raw_fd(),
    ];
    keep.extend(context.descriptors.iter().map(AsRawFd::as_raw_fd));
    close_other_descriptors(&keep)?;
    let pointer = Box::into_raw(Box::new(context));
    let mut stack = vec![0u8; 1024 * 1024];
    let top = unsafe { stack.as_mut_ptr().add(stack.len()) };
    let init_pid = unsafe {
        libc::clone(
            init_start,
            top.cast(),
            libc::CLONE_NEWPID | libc::SIGCHLD,
            pointer.cast(),
        )
    };
    unsafe {
        drop(Box::from_raw(pointer));
    }
    if init_pid < 0 {
        broker.write_all(&(-1_i32).to_ne_bytes())?;
        return Err(io::Error::last_os_error());
    }
    broker.write_all(&init_pid.to_ne_bytes())?;
    drop(broker);
    let mut init_status = 0;
    let waited = unsafe { libc::waitpid(init_pid, &mut init_status, 0) };
    let mut reported = String::new();
    guardian_status.read_to_string(&mut reported)?;
    let terminal = if waited == init_pid
        && libc::WIFEXITED(init_status)
        && libc::WEXITSTATUS(init_status) == 0
        && reported.starts_with("exit ")
        && reported.ends_with('\n')
    {
        format!("{} drained {}\n", reported.trim_end(), record.request_id)
    } else {
        format!("uncertain {}\n", record.request_id)
    };
    let ledger = LaunchLedger { directory };
    let mut latest = ledger.read(&record.request_id)?;
    latest.terminal = Some(terminal.clone());
    ledger.replace(&latest)?;
    let _ = client.write_all(terminal.as_bytes());
    Ok(())
}

fn child_credential(stream: &UnixStream) -> io::Result<libc::ucred> {
    let mut byte = [0u8; 1];
    let mut iov = libc::iovec {
        iov_base: byte.as_mut_ptr().cast(),
        iov_len: 1,
    };
    let mut control = [0u8; 64];
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = control.len();
    if unsafe { libc::recvmsg(stream.as_raw_fd(), &mut message, 0) } != 1 || byte != [b'C'] {
        return Err(io::Error::other("private Runner never reached child gate"));
    }
    let header = unsafe { libc::CMSG_FIRSTHDR(&message) };
    if header.is_null()
        || unsafe {
            (*header).cmsg_level != libc::SOL_SOCKET || (*header).cmsg_type != libc::SCM_CREDENTIALS
        }
    {
        return Err(io::Error::other("private Runner child credentials missing"));
    }
    Ok(unsafe { *(libc::CMSG_DATA(header) as *const libc::ucred) })
}

#[expect(
    clippy::too_many_arguments,
    reason = "entry, host identity, image, and reply are separate authorities"
)]
pub(super) fn launch(
    ledger: &LaunchLedger,
    spec: InstalledLaunchSpec,
    descriptors: Vec<File>,
    peer: &PeerIdentity,
    host_namespace: &File,
    image: &File,
    client: UnixStream,
) -> io::Result<()> {
    installed_launch::validate(&spec, &installed_launch::files_as_raw(&descriptors))?;
    if !peer.process.in_namespace(host_namespace)? {
        return Err(io::Error::other("private host entry refused"));
    }
    if spec.kind == EntryKind::Gui {
        if peer.uid != 0 && image.metadata()?.permissions().mode() & 0o6000 != 0 {
            return Err(io::Error::other(
                "private GUI fixed image privilege bits refused",
            ));
        }
        private_gui_session(&spec, peer.uid)?;
    } else {
        let args = spec
            .args
            .iter()
            .map(|arg| String::from_utf8(arg.clone()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| io::Error::other("private CLI mode requires UTF-8 args"))?;
        let private_probe = super::private_fixture()
            && (matches!(args.as_slice(), [first, second]
                if first == "__age319-private-installed-probe-v1"
                    && matches!(second.as_str(), "tty" | "setuid" | "sleep"))
                || matches!(args.as_slice(), [first, second, marker]
                    if first == "__age319-private-installed-probe-v1"
                        && second == "ambient"
                        && marker.starts_with('/')));
        if !supported_offline_entry_args(&args) && !private_probe {
            return Err(io::Error::other("private CLI mode refused"));
        }
    }
    if unsafe { libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) } != 0 {
        return Err(io::Error::other("private broker lost sudo semantics"));
    }
    // The submitted frame carries no group list. The pinned launcher is the
    // only authority for a GUI's actual session/newgrp supplementary groups.
    // A failed or ambiguous proc observation refuses launch before reserve.
    let gui_groups = if spec.kind == EntryKind::Gui {
        peer.process.supplementary_groups()?
    } else {
        Vec::new()
    };
    let record = Record {
        protocol: RECORD_PROTOCOL.into(),
        generation: spec.generation.clone(),
        request_id: spec.request_id.clone(),
        uid: peer.uid,
        entry: ProcessStamp::from(&peer.process),
        guardian: None,
        init: None,
        child: None,
        terminal: None,
    };
    ledger.reserve(&record)?;
    let (mut broker, guardian_control) = UnixStream::pair()?;
    let (broker_gate, init_gate) = UnixStream::pair()?;
    let (broker_child_control, init_child_control) = UnixStream::pair()?;
    let (mut broker_child_gate, init_child_gate) = UnixStream::pair()?;
    let one: libc::c_int = 1;
    if unsafe {
        libc::setsockopt(
            broker_child_control.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PASSCRED,
            (&one as *const libc::c_int).cast(),
            std::mem::size_of_val(&one) as _,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    let (unused_status, init_status) = UnixStream::pair()?;
    drop(unused_status);
    let context = InitContext {
        spec,
        descriptors,
        image: image.try_clone()?,
        gate: init_gate,
        status: init_status,
        child_control: init_child_control,
        child_gate: init_child_gate,
        uid: peer.uid,
        gid: peer.gid,
        gui_groups,
    };
    let guardian_pid = unsafe { libc::fork() };
    if guardian_pid < 0 {
        return Err(io::Error::last_os_error());
    }
    if guardian_pid == 0 {
        drop(broker);
        drop(broker_gate);
        drop(broker_child_control);
        drop(broker_child_gate);
        let outcome = guardian(
            record,
            ledger.directory.clone(),
            context,
            client,
            guardian_control,
        );
        unsafe { libc::_exit(if outcome.is_ok() { 0 } else { 70 }) }
    }
    drop(guardian_control);
    drop(context);
    drop(client);
    // After the fork, the guardian alone owns the terminal reply. If any
    // broker-side identity/write step fails, drop both gates and let it write
    // an uncertain result after PID1 exits; never send a competing response.
    let post_fork = (|| -> io::Result<()> {
        let mut pid_bytes = [0u8; 4];
        broker.read_exact(&mut pid_bytes)?;
        let init_pid = i32::from_ne_bytes(pid_bytes);
        if init_pid <= 0 {
            return Err(io::Error::other("private root PID1 creation failed"));
        }
        let guardian = PinnedProcess::open(guardian_pid)?;
        let init = PinnedProcess::open(init_pid)?;
        if !guardian.direct_child_of(&PinnedProcess::open(unsafe { libc::getpid() })?)?
            || !init.direct_child_of(&guardian)?
            || !init.is_namespace_init()?
            || init.in_namespace(host_namespace)?
        {
            return Err(io::Error::other(
                "private root guardian/PID1 identity mismatch",
            ));
        }
        let mut bound = ledger.read(&record.request_id)?;
        bound.guardian = Some(ProcessStamp::from(&guardian));
        bound.init = Some(ProcessStamp::from(&init));
        ledger.replace(&bound)?;
        init.verify()?;
        guardian.verify()?;
        broker_gate.try_clone()?.write_all(b"R")?;
        let credential = child_credential(&broker_child_control)?;
        let child = PinnedProcess::open(credential.pid)?;
        if credential.uid != peer.uid
            || credential.gid != peer.gid
            || !child.direct_child_of(&init)?
            || !child.in_namespace(init.namespace())?
        {
            return Err(io::Error::other("private Runner child identity mismatch"));
        }
        bound.child = Some(ProcessStamp::from(&child));
        ledger.replace(&bound)?;
        init.verify()?;
        child.verify()?;
        broker_child_gate.write_all(b"R")?;
        Ok(())
    })();
    if let Err(error) = post_fork {
        eprintln!(
            "private installed launch {} uncertain: {error}",
            record.request_id
        );
    }
    Ok(())
}
