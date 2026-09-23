//! Opt-in host-root service. No Runner code invokes this binary yet.
use oulipoly_kernel_broker::identity::{
    Classification, PeerIdentity, PinnedProcess, boot_id, classify_peer,
};
use oulipoly_kernel_broker::registry::{RootRecord, RootRegistry};
use std::ffi::{CStr, CString};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;

const SOCKET: &str = "/run/oulipoly-kernel-broker/control.sock";
const STATE: &str = "/var/lib/oulipoly-kernel-broker";
const RUNNER: &str = "/usr/local/libexec/oulipoly/oulipoly-agent-runner";

fn checked_root_path(path: &Path, directory: bool) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(io::Error::other("nonabsolute installed path"));
    }
    let mut current = path;
    loop {
        let meta = fs::symlink_metadata(current)?;
        if meta.uid() != 0 || meta.mode() & 0o022 != 0 || meta.file_type().is_symlink() {
            return Err(io::Error::other("untrusted installed path"));
        }
        if current == path
            && (if directory {
                !meta.is_dir()
            } else {
                !meta.is_file()
            })
        {
            return Err(io::Error::other("wrong installed file type"));
        }
        if current == Path::new("/") {
            break;
        }
        current = current
            .parent()
            .ok_or_else(|| io::Error::other("bad path"))?;
    }
    Ok(())
}

fn recv_request(stream: &mut UnixStream) -> io::Result<(u8, libc::ucred, PinnedProcess)> {
    let fd = stream.as_raw_fd();
    let one: libc::c_int = 1;
    if unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PASSCRED,
            &one as *const _ as *const _,
            std::mem::size_of_val(&one) as _,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut original = std::mem::MaybeUninit::<libc::ucred>::uninit();
    let mut original_len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            original.as_mut_ptr() as *mut _,
            &mut original_len,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    if original_len as usize != std::mem::size_of::<libc::ucred>() {
        return Err(io::Error::other("bad peer credentials"));
    }
    let original = unsafe { original.assume_init() };
    let process = PinnedProcess::open(original.pid)?;
    // A queued request from a process that died before accept cannot answer a
    // fresh challenge. The pidfd/starttime pinned before the request must stay
    // live through classification and dispatch.
    let challenge = *uuid::Uuid::new_v4().as_bytes();
    stream.write_all(&challenge)?;
    let mut request = [0u8; 17];
    let mut iov = libc::iovec {
        iov_base: request.as_mut_ptr().cast(),
        iov_len: request.len(),
    };
    let mut control = [0u8; 128];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = control.len();
    let read = unsafe { libc::recvmsg(fd, &mut msg, libc::MSG_CMSG_CLOEXEC) };
    if read != request.len() as isize
        || request[1..] != challenge
        || msg.msg_flags & (libc::MSG_CTRUNC | libc::MSG_TRUNC) != 0
    {
        return Err(io::Error::other("invalid challenged request"));
    }
    let mut credentials = None;
    let mut cmsg = unsafe { libc::CMSG_FIRSTHDR(&msg) };
    while !cmsg.is_null() {
        let header = unsafe { &*cmsg };
        if header.cmsg_level == libc::SOL_SOCKET && header.cmsg_type == libc::SCM_CREDENTIALS {
            if credentials.is_some() {
                return Err(io::Error::other("duplicate credentials"));
            }
            credentials = Some(unsafe { *(libc::CMSG_DATA(cmsg) as *const libc::ucred) });
        }
        cmsg = unsafe { libc::CMSG_NXTHDR(&msg, cmsg) };
    }
    let credentials =
        credentials.ok_or_else(|| io::Error::other("missing per-request credentials"))?;
    if (credentials.pid, credentials.uid, credentials.gid)
        != (original.pid, original.uid, original.gid)
    {
        return Err(io::Error::other("transferred/inherited socket sender"));
    }
    process.verify()?;
    Ok((request[0], credentials, process))
}

fn peer_from_request(stream: &mut UnixStream) -> io::Result<(u8, PeerIdentity)> {
    let (operation, credentials, process) = recv_request(stream)?;
    Ok((
        operation,
        PeerIdentity {
            uid: credentials.uid,
            gid: credentials.gid,
            process,
        },
    ))
}

fn checked_user_home(uid: u32) -> io::Result<CString> {
    let entry = unsafe { libc::getpwuid(uid) };
    if entry.is_null() {
        return Err(io::Error::other("unknown UID"));
    }
    let home = unsafe { CStr::from_ptr((*entry).pw_dir) };
    CString::new(home.to_bytes()).map_err(|_| io::Error::other("bad home"))
}

fn write_byte(fd: i32, byte: u8) -> bool {
    unsafe { libc::write(fd, &byte as *const _ as *const _, 1) == 1 }
}
fn read_byte(fd: i32) -> Option<u8> {
    let mut byte = 0u8;
    (unsafe { libc::read(fd, &mut byte as *mut _ as *mut _, 1) } == 1).then_some(byte)
}

fn isolate_stage_fds(control: i32, gate: i32) -> Option<(i32, i32)> {
    let c = unsafe { libc::fcntl(control, libc::F_DUPFD_CLOEXEC, 10) };
    let g = unsafe { libc::fcntl(gate, libc::F_DUPFD_CLOEXEC, 10) };
    if c < 0 || g < 0 {
        return None;
    }
    if unsafe { libc::dup2(c, 3) } < 0 || unsafe { libc::dup2(g, 4) } < 0 {
        return None;
    }
    let null = unsafe { libc::open(c"/dev/null".as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
    if null < 0 {
        return None;
    }
    for fd in 0..=2 {
        if unsafe { libc::dup2(null, fd) } < 0 {
            return None;
        }
    }
    if unsafe { libc::syscall(libc::SYS_close_range, 5u32, u32::MAX, 0u32) } < 0 {
        return None;
    }
    Some((3, 4))
}

fn init_reaper(
    control: i32,
    stage_pipe: i32,
    uid: u32,
    gid: u32,
    groups: &[libc::gid_t],
    home: &CStr,
) -> ! {
    if read_byte(stage_pipe) != Some(b'I') {
        unsafe { libc::_exit(111) }
    }
    unsafe {
        libc::close(stage_pipe);
    }
    let slash = c"/";
    let proc = c"/proc";
    let proc_type = c"proc";
    let setup = unsafe {
        libc::mount(
            std::ptr::null(),
            slash.as_ptr(),
            std::ptr::null(),
            libc::MS_REC | libc::MS_PRIVATE,
            std::ptr::null(),
        ) == 0
            && libc::mount(
                proc_type.as_ptr(),
                proc.as_ptr(),
                proc_type.as_ptr(),
                libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
                std::ptr::null(),
            ) == 0
    };
    if !setup || !write_byte(control, b'R') || read_byte(control) != Some(b'G') {
        unsafe { libc::_exit(112) }
    }
    unsafe {
        libc::close(control);
        libc::close(stage_pipe);
    }
    let child = unsafe { libc::fork() };
    if child < 0 {
        unsafe { libc::_exit(113) }
    }
    if child == 0 {
        if unsafe {
            libc::setgroups(groups.len(), groups.as_ptr()) != 0
                || libc::setresgid(gid, gid, gid) != 0
                || libc::setresuid(uid, uid, uid) != 0
                || libc::chdir(home.as_ptr()) != 0
        } {
            unsafe { libc::_exit(115) }
        }
        let runner = c"/usr/local/libexec/oulipoly/oulipoly-agent-runner";
        let arg0 = c"oulipoly-agent-runner";
        let path = c"PATH=/usr/local/bin:/usr/bin:/bin";
        let home_env = CString::new([b"HOME=".as_slice(), home.to_bytes()].concat()).unwrap();
        let argv = [arg0.as_ptr(), std::ptr::null()];
        let env = [path.as_ptr(), home_env.as_ptr(), std::ptr::null()];
        unsafe {
            libc::execve(runner.as_ptr(), argv.as_ptr(), env.as_ptr());
            libc::_exit(114);
        }
    }
    // PID 1 stays alive indefinitely, reaping adopted descendants. It does not
    // infer work completion from ECHILD or an idle interval.
    loop {
        let mut status = 0;
        let reaped = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
        if reaped <= 0 {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}

fn launch(peer: &PeerIdentity, registry: &mut RootRegistry) -> io::Result<String> {
    checked_root_path(Path::new(RUNNER), false)?;
    let home = checked_user_home(peer.uid)?;
    let groups = peer.process.supplementary_groups()?;
    let mut pair = [0; 2];
    if unsafe {
        libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC,
            0,
            pair.as_mut_ptr(),
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut pipe = [0; 2];
    if unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) } < 0 {
        unsafe {
            libc::close(pair[0]);
            libc::close(pair[1]);
        }
        return Err(io::Error::last_os_error());
    }
    let stage = unsafe { libc::fork() };
    if stage < 0 {
        return Err(io::Error::last_os_error());
    }
    if stage == 0 {
        unsafe {
            libc::close(pair[0]);
            libc::close(pipe[1]);
        }
        let Some((control, stage_gate)) = isolate_stage_fds(pair[1], pipe[0]) else {
            unsafe { libc::_exit(119) }
        };
        if unsafe { libc::unshare(libc::CLONE_NEWNS | libc::CLONE_NEWPID) } < 0 {
            unsafe { libc::_exit(120) }
        }
        let init = unsafe { libc::fork() };
        if init < 0 {
            unsafe { libc::_exit(121) }
        }
        if init == 0 {
            init_reaper(control, stage_gate, peer.uid, peer.gid, &groups, &home);
        }
        unsafe {
            libc::close(stage_gate);
        }
        let bytes = init.to_ne_bytes();
        if unsafe { libc::write(control, bytes.as_ptr().cast(), bytes.len()) }
            != bytes.len() as isize
        {
            unsafe { libc::_exit(122) }
        }
        unsafe { libc::_exit(0) }
    }
    unsafe {
        libc::close(pair[1]);
        libc::close(pipe[0]);
    }
    let mut control = unsafe { UnixStream::from_raw_fd(pair[0]) };
    control.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
    control.set_write_timeout(Some(std::time::Duration::from_secs(10)))?;
    let mut pid_bytes = [0u8; 4];
    let result = (|| {
        control.read_exact(&mut pid_bytes)?;
        let host_pid = i32::from_ne_bytes(pid_bytes);
        let mut status = 0;
        if unsafe { libc::waitpid(stage, &mut status, 0) } != stage
            || !libc::WIFEXITED(status)
            || libc::WEXITSTATUS(status) != 0
        {
            return Err(io::Error::other("namespace stage failed"));
        }
        if !write_byte(pipe[1], b'I') {
            return Err(io::Error::other("init stage gate failed"));
        }
        let mut ready = [0u8; 1];
        control.read_exact(&mut ready)?;
        if ready != [b'R'] {
            return Err(io::Error::other("init setup failed"));
        }
        let init = PinnedProcess::open(host_pid)?;
        if init.pidns_ino == peer.process.pidns_ino || init.boot_id != peer.process.boot_id {
            return Err(io::Error::other("root namespace not distinct"));
        }
        let root_id = uuid::Uuid::new_v4().to_string();
        peer.process.verify()?;
        let record = RootRecord {
            version: 1,
            boot_id: boot_id()?,
            root_id: root_id.clone(),
            owner_uid: peer.uid,
            init_host_pid: host_pid,
            init_starttime_ticks: init.starttime_ticks,
            pidns_dev: init.pidns_dev,
            pidns_ino: init.pidns_ino,
        };
        registry.insert(record)?;
        init.verify()?;
        peer.process.verify()?;
        control.write_all(b"G")?;
        Ok(root_id)
    })();
    unsafe {
        libc::close(pipe[1]);
    }
    let mut status = 0;
    if unsafe { libc::waitpid(stage, &mut status, libc::WNOHANG) } == 0 {
        unsafe {
            libc::kill(stage, libc::SIGKILL);
            libc::waitpid(stage, &mut status, 0);
        }
    }
    result
}

fn serve() -> io::Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(io::Error::other("host root required"));
    }
    if fs::read_link("/proc/self/ns/user")? != fs::read_link("/proc/1/ns/user")? {
        return Err(io::Error::other("initial user namespace required"));
    }
    if fs::read_link("/proc/self/ns/pid")? != fs::read_link("/proc/1/ns/pid")? {
        return Err(io::Error::other("host PID namespace required"));
    }
    checked_root_path(
        Path::new("/usr/local/libexec/oulipoly/oulipoly-kernel-broker"),
        false,
    )?;
    checked_root_path(Path::new(STATE), true)?;
    checked_root_path(Path::new("/run/oulipoly-kernel-broker"), true)?;
    checked_root_path(Path::new(RUNNER), false)?;
    let host_namespace = File::open("/proc/self/ns/pid")?;
    let mut registry = RootRegistry::open(STATE)?;
    if let Ok(meta) = fs::symlink_metadata(SOCKET) {
        if !meta.file_type().is_socket() || meta.uid() != 0 {
            return Err(io::Error::other("unsafe existing socket"));
        }
        fs::remove_file(SOCKET)?;
    }
    let listener = UnixListener::bind(SOCKET)?;
    fs::set_permissions(SOCKET, fs::Permissions::from_mode(0o660))?;
    for incoming in listener.incoming() {
        let Ok(mut stream) = incoming else { continue };
        stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(std::time::Duration::from_secs(5)))?;
        let result = (|| {
            let (operation, peer) = peer_from_request(&mut stream)?;
            match (operation, classify_peer(&peer, &host_namespace, &registry)) {
                (b'C', Classification::Inside(root)) => Ok(format!("inside {root}\n")),
                (b'C', Classification::Outside) => Ok("outside\n".to_owned()),
                (b'C', Classification::Uncertain) => Ok("uncertain\n".to_owned()),
                (b'L', Classification::Outside) if peer.uid >= 1000 => {
                    launch(&peer, &mut registry).map(|id| format!("released {id}\n"))
                }
                (b'L', _) => Err(io::Error::other("root launch denied")),
                _ => Err(io::Error::other("unknown operation")),
            }
        })();
        let response = result.unwrap_or_else(|error| format!("error {error}\n"));
        let _ = stream.write_all(response.as_bytes());
    }
    Ok(())
}

pub fn run() {
    if std::env::args_os().len() != 1 {
        eprintln!("no command-line options accepted");
        std::process::exit(2);
    }
    if let Err(error) = serve() {
        eprintln!("kernel broker: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::thread;

    #[test]
    fn challenged_per_request_credentials_accept_exact_sender() {
        let temp = tempfile::tempdir().unwrap();
        let socket = temp.path().join("socket");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let (op, peer) = peer_from_request(&mut stream).unwrap();
            assert_eq!(op, b'C');
            assert_eq!(peer.process.host_pid, std::process::id() as i32);
        });
        let mut client = UnixStream::connect(&socket).unwrap();
        let mut challenge = [0u8; 16];
        client.read_exact(&mut challenge).unwrap();
        let mut message = [0u8; 17];
        message[0] = b'C';
        message[1..].copy_from_slice(&challenge);
        assert_eq!(
            unsafe { libc::send(client.as_raw_fd(), message.as_ptr().cast(), 17, 0) },
            17
        );
        server.join().unwrap();
    }

    #[test]
    fn challenged_request_rejects_wrong_nonce() {
        let temp = tempfile::tempdir().unwrap();
        let socket = temp.path().join("socket");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            assert!(peer_from_request(&mut stream).is_err());
        });
        let mut client = UnixStream::connect(&socket).unwrap();
        let mut challenge = [0u8; 16];
        client.read_exact(&mut challenge).unwrap();
        let message = [b'C'; 17];
        assert_eq!(
            unsafe { libc::send(client.as_raw_fd(), message.as_ptr().cast(), 17, 0) },
            17
        );
        server.join().unwrap();
    }

    #[test]
    fn inherited_connected_fd_cannot_speak_for_connector() {
        let temp = tempfile::tempdir().unwrap();
        let socket = temp.path().join("socket");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            assert!(peer_from_request(&mut stream).is_err());
        });
        let script = "import os,socket,sys\ns=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM);s.connect(sys.argv[1]);c=s.recv(16);p=os.fork()\nif p==0:\n s.sendall(b'C'+c);os._exit(0)\nos.waitpid(p,0)";
        let output = Command::new("python3")
            .args(["-c", script, socket.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        server.join().unwrap();
    }
}
