//! One-use, process-bound control channel for a newly admitted installed L.
//! The descriptor is inherited from Broker; its number in the environment is
//! only a locator and never grants authority by itself.
use crate::entry_registry::ProcessStamp;
use crate::identity::{PeerIdentity, PinnedProcess};
use crate::installed_launch::InstalledLaunchSpec;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Stdio};

pub const FD_ENV: &str = "OULIPOLY_KERNEL_CONNECTED_CONTROL_FD_V1";

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantMessage {
    pub protocol: String,
    pub pair_generation: String,
    pub source_generation: String,
    pub request_id: String,
    pub root_id: String,
}

pub struct ControlGrant {
    pub message: GrantMessage,
    pub process: ProcessStamp,
    channel: UnixStream,
    child: Child,
    e_accepted: bool,
}

impl ControlGrant {
    pub fn spawn(
        spec: &InstalledLaunchSpec,
        descriptors: &[File],
        launcher: &PeerIdentity,
        image: &File,
        source_generation: &str,
        root_id: &str,
        fixture_paths: Option<(&std::path::Path, &std::path::Path)>,
    ) -> io::Result<Self> {
        let (mut broker, control) = UnixStream::pair()?;
        // Command's stdio setup can replace descriptors 0..2 before
        // pre_exec. Keep every control/path descriptor above that range.
        let control_fd = unsafe { libc::fcntl(control.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
        if control_fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let control = unsafe { UnixStream::from_raw_fd(control_fd) };
        let image_fd = unsafe { libc::fcntl(image.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
        if image_fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let pinned_image = unsafe { File::from_raw_fd(image_fd) };
        let cwd = descriptors
            .last()
            .ok_or_else(|| io::Error::other("launch cwd absent"))?;
        let cwd_fd = unsafe { libc::fcntl(cwd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
        if cwd_fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let pinned_cwd = unsafe { File::from_raw_fd(cwd_fd) };
        let one: libc::c_int = 1;
        if unsafe {
            libc::setsockopt(
                broker.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PASSCRED,
                (&one as *const libc::c_int).cast(),
                std::mem::size_of_val(&one) as _,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut command = Command::new(format!("/proc/self/fd/{}", pinned_image.as_raw_fd()));
        command.args(spec.args.iter().cloned().map(OsString::from_vec));
        command.env_clear().envs(
            spec.environment
                .iter()
                .map(|(key, value)| (OsStr::from_bytes(key), OsStr::from_bytes(value))),
        );
        command.env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1");
        command.env(FD_ENV, control.as_raw_fd().to_string());
        if let Some((socket, manifest)) = fixture_paths {
            command.env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", socket);
            command.env("OULIPOLY_KERNEL_BROKER_FIXTURE_PAIR_V1", manifest);
        }
        let mut cursor = 0;
        for (index, present) in spec.stdio_present.iter().copied().enumerate() {
            let stdio = if present {
                let fd = descriptors[cursor].try_clone()?;
                cursor += 1;
                Stdio::from(fd)
            } else {
                Stdio::null()
            };
            match index {
                0 => {
                    command.stdin(stdio);
                }
                1 => {
                    command.stdout(stdio);
                }
                _ => {
                    command.stderr(stdio);
                }
            }
        }
        let cwd_fd = pinned_cwd.as_raw_fd();
        let control_fd = control.as_raw_fd();
        let present = spec.stdio_present;
        let uid = launcher.uid;
        let gid = launcher.gid;
        let private_root_mapped = std::fs::read_to_string("/proc/self/uid_map")
            .ok()
            .is_some_and(|map| {
                let mut fields = map.split_ascii_whitespace();
                fields.next() == Some("0")
                    && fields.next().is_some_and(|host_uid| host_uid != "0")
                    && fields.next() == Some("1")
            });
        unsafe {
            command.pre_exec(move || {
                let current_uid = libc::geteuid();
                let current_gid = libc::getegid();
                if libc::fchdir(cwd_fd) != 0
                    || (current_uid == 0
                        && !private_root_mapped
                        && libc::setgroups(0, std::ptr::null()) != 0)
                    || (current_uid != 0 && (uid != current_uid || gid != current_gid))
                    || libc::setresgid(gid, gid, gid) != 0
                    || libc::setresuid(uid, uid, uid) != 0
                    || libc::fcntl(control_fd, libc::F_SETFD, 0) != 0
                {
                    return Err(io::Error::last_os_error());
                }
                for (index, exists) in present.into_iter().enumerate() {
                    if !exists {
                        libc::close(index as i32);
                    }
                }
                Ok(())
            });
        }
        let mut child = command.spawn()?;
        drop(control);
        let prepared = (|| -> io::Result<(ProcessStamp, GrantMessage)> {
            let process = PinnedProcess::open(child.id() as i32)?;
            let broker_process = PinnedProcess::open(std::process::id() as i32)?;
            if !process.same_executable_as(image)? || !process.direct_child_of(&broker_process)? {
                return Err(io::Error::other(
                    "control Runner did not retain Broker parent/image",
                ));
            }
            let message = GrantMessage {
                protocol: "installed-connected-control-v1".into(),
                pair_generation: spec.generation.clone(),
                source_generation: source_generation.into(),
                request_id: spec.request_id.clone(),
                root_id: root_id.into(),
            };
            let mut bytes = serde_json::to_vec(&message)?;
            bytes.push(b'\n');
            broker.write_all(&bytes)?;
            Ok((ProcessStamp::from(&process), message))
        })();
        let (process, message) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                drop(broker);
                let _ = child.kill();
                // Never hold the serving loop for an uncertain child wait.
                let _ = std::thread::Builder::new()
                    .name("failed-connected-control-reaper".into())
                    .spawn(move || {
                        let _ = child.wait();
                    });
                return Err(error);
            }
        };
        Ok(Self {
            message,
            process,
            channel: broker,
            child,
            e_accepted: false,
        })
    }

    pub fn mark_e_accepted(&mut self) {
        self.e_accepted = true;
    }

    pub fn e_accepted(&self) -> bool {
        self.e_accepted
    }

    /// E is the sole consumer. A read from the connected channel must carry
    /// the spawned process's kernel credentials, not merely its advertised PID.
    pub fn consume(&mut self, peer: &PeerIdentity) -> io::Result<String> {
        if self.child.try_wait()?.is_some()
            || peer.uid != self.child_uid()?
            || ProcessStamp::from(&peer.process) != self.process
        {
            return Err(io::Error::other("connected control process changed"));
        }
        peer.process.verify()?;
        let mut byte = [0u8; 1];
        let mut iov = libc::iovec {
            iov_base: byte.as_mut_ptr().cast(),
            iov_len: 1,
        };
        let mut control = [0u8; 128];
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = control.len();
        let read = unsafe { libc::recvmsg(self.channel.as_raw_fd(), &mut msg, libc::MSG_DONTWAIT) };
        if read != 1 || byte != [b'R'] || msg.msg_flags & libc::MSG_CTRUNC != 0 {
            return Err(io::Error::other("connected control proof absent"));
        }
        let mut cmsg = unsafe { libc::CMSG_FIRSTHDR(&msg) };
        let mut authenticated = false;
        while !cmsg.is_null() {
            let header = unsafe { &*cmsg };
            if header.cmsg_level == libc::SOL_SOCKET && header.cmsg_type == libc::SCM_CREDENTIALS {
                let credential = unsafe { *(libc::CMSG_DATA(cmsg) as *const libc::ucred) };
                authenticated = credential.pid == peer.process.host_pid
                    && credential.uid == peer.uid
                    && credential.gid == peer.gid;
            }
            cmsg = unsafe { libc::CMSG_NXTHDR(&msg, cmsg) };
        }
        if !authenticated {
            return Err(io::Error::other("connected control sender changed"));
        }
        Ok(self.message.root_id.clone())
    }

    fn child_uid(&self) -> io::Result<u32> {
        crate::identity::host_proc_uid(self.process.host_pid)
    }

    /// Return the kernel wait result for this exact direct child. The caller
    /// must retain the grant until that result has been recorded durably.
    pub fn reap_if_done(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry_registry::EntryRegistry;
    use crate::installed_launch::{EntryKind, PROTOCOL};
    use std::io::Read;
    use std::os::fd::FromRawFd;

    #[test]
    fn connected_child() {
        if std::env::var_os("AGE319_CONNECTED_CHILD_TEST").is_none() {
            return;
        }
        let fd: i32 = std::env::var(FD_ENV).unwrap().parse().unwrap();
        let mut socket = unsafe { UnixStream::from_raw_fd(fd) };
        let mut bytes = Vec::new();
        loop {
            let mut byte = [0];
            socket.read_exact(&mut byte).unwrap();
            if byte == [b'\n'] {
                break;
            }
            bytes.push(byte[0]);
        }
        let message: GrantMessage = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(message.protocol, "installed-connected-control-v1");
        socket.write_all(b"R").unwrap();
        let mut release = [0];
        socket.read_exact(&mut release).unwrap();
        assert_eq!(release, [b'X']);
    }

    #[test]
    fn inherited_socket_authenticates_child_and_exact_e_is_one_use() {
        let pair = uuid::Uuid::new_v4().to_string();
        let source = uuid::Uuid::new_v4().to_string();
        let request = uuid::Uuid::new_v4().to_string();
        let root = uuid::Uuid::new_v4().to_string();
        let launcher = PeerIdentity {
            uid: unsafe { libc::geteuid() },
            gid: unsafe { libc::getegid() },
            process: PinnedProcess::open(std::process::id() as i32).unwrap(),
        };
        let spec = InstalledLaunchSpec {
            protocol: PROTOCOL.into(),
            generation: pair.clone(),
            request_id: request.clone(),
            kind: EntryKind::Cli,
            args: vec![
                b"--exact".to_vec(),
                b"connected_control::tests::connected_child".to_vec(),
            ],
            environment: vec![(b"AGE319_CONNECTED_CHILD_TEST".to_vec(), b"1".to_vec())],
            stdio_present: [false; 3],
        };
        let image = File::open(std::env::current_exe().unwrap()).unwrap();
        let cwd = File::open(".").unwrap();
        let mut grant =
            ControlGrant::spawn(&spec, &[cwd], &launcher, &image, &source, &root, None).unwrap();
        assert_eq!(grant.message.request_id, request);
        assert_eq!(grant.message.pair_generation, pair);
        assert_eq!(grant.message.source_generation, source);
        assert!(grant.consume(&launcher).is_err());
        let child_peer = PeerIdentity {
            uid: launcher.uid,
            gid: launcher.gid,
            process: PinnedProcess::open(grant.process.host_pid).unwrap(),
        };
        let directory = tempfile::tempdir().unwrap();
        let mut entries = EntryRegistry::open(directory.path()).unwrap();
        let mut ready = libc::pollfd {
            fd: grant.channel.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        assert_eq!(unsafe { libc::poll(&mut ready, 1, 5000) }, 1);
        assert_eq!(grant.consume(&child_peer).unwrap(), root);
        assert_eq!(
            entries
                .reserve_exact(child_peer.uid, &child_peer.process, &root)
                .unwrap(),
            root
        );
        assert!(grant.consume(&child_peer).is_err());
        assert!(
            entries
                .reserve_exact(child_peer.uid, &child_peer.process, &root)
                .is_err()
        );
        grant.channel.write_all(b"X").unwrap();
        assert!(grant.child.wait().unwrap().success());
    }
}
