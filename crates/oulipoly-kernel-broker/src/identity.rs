use crate::registry::RootRegistry;
use std::ffi::CString;
use std::fs::{self, File};
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::MetadataExt;
use std::sync::OnceLock;

// The serving broker installs a fresh detached procfs before reading any
// durable identity. Replacing the pathname /proc in a shared mount namespace
// cannot redirect reads through this CLOEXEC fd. This does not protect the fd
// from a host-root workload with access to broker memory or /proc/<broker>/fd.
// Non-serving library tests retain a plain /proc view.
static HOST_PROC: OnceLock<File> = OnceLock::new();

pub fn install_detached_host_proc() -> io::Result<()> {
    if HOST_PROC.get().is_some() {
        return Err(io::Error::other("host proc observer already installed"));
    }
    let name = CString::new("proc").unwrap();
    let context = unsafe { libc::syscall(libc::SYS_fsopen, name.as_ptr(), 1u32) as i32 };
    if context < 0 {
        return Err(io::Error::last_os_error());
    }
    let context = unsafe { File::from_raw_fd(context) };
    // FSCONFIG_CMD_CREATE. A new procfs superblock is bound to this broker's
    // current PID namespace; no mountpoint in the workload is consulted.
    if unsafe { libc::syscall(libc::SYS_fsconfig, context.as_raw_fd(), 6u32, 0, 0, 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mount = unsafe { libc::syscall(libc::SYS_fsmount, context.as_raw_fd(), 1u32, 0u32) as i32 };
    if mount < 0 {
        return Err(io::Error::last_os_error());
    }
    let mount = unsafe { File::from_raw_fd(mount) };
    let stat = proc_read_from(&mount, "self/stat")?;
    let pid = stat
        .split_ascii_whitespace()
        .next()
        .and_then(|field| field.parse::<i32>().ok());
    if pid != Some(unsafe { libc::getpid() }) {
        return Err(io::Error::other(
            "detached procfs is not the broker PID observer",
        ));
    }
    HOST_PROC
        .set(mount)
        .map_err(|_| io::Error::other("host proc observer changed"))
}

fn proc_file_from(root: &File, relative: &str) -> io::Result<File> {
    let name = CString::new(relative).map_err(|_| io::Error::other("invalid proc path"))?;
    let fd = unsafe {
        libc::openat(
            root.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn proc_read_from(root: &File, relative: &str) -> io::Result<String> {
    let mut content = String::new();
    proc_file_from(root, relative)?.read_to_string(&mut content)?;
    Ok(content)
}

pub fn host_proc_file(relative: &str) -> io::Result<File> {
    if let Some(root) = HOST_PROC.get() {
        proc_file_from(root, relative)
    } else {
        File::open(format!("/proc/{relative}"))
    }
}

fn host_proc_read(relative: &str) -> io::Result<String> {
    if let Some(root) = HOST_PROC.get() {
        proc_read_from(root, relative)
    } else {
        fs::read_to_string(format!("/proc/{relative}"))
    }
}

pub fn host_proc_uid(pid: i32) -> io::Result<u32> {
    Ok(host_proc_file(&format!("{pid}"))?.metadata()?.uid())
}

#[derive(Debug, PartialEq, Eq)]
pub enum Classification {
    Inside(String),
    Outside,
    Uncertain,
}

#[derive(Debug)]
pub struct PinnedProcess {
    pub host_pid: i32,
    pub boot_id: String,
    pub starttime_ticks: u64,
    pub pidns_dev: u64,
    pub pidns_ino: u64,
    pidfd: File,
    pidns: File,
}

fn proc_starttime(pid: i32) -> io::Result<(u64, u8)> {
    let stat = host_proc_read(&format!("{pid}/stat"))?;
    // comm is parenthesized and may itself contain spaces and parentheses.
    let tail = stat
        .rsplit_once(") ")
        .ok_or_else(|| io::Error::other("malformed proc stat"))?
        .1;
    let fields: Vec<_> = tail.split_ascii_whitespace().collect();
    let state = fields
        .first()
        .ok_or_else(|| io::Error::other("missing proc state"))?;
    let starttime = fields
        .get(19)
        .ok_or_else(|| io::Error::other("missing starttime"))?
        .parse()
        .map_err(|_| io::Error::other("bad starttime"))?;
    Ok((starttime, state.as_bytes()[0]))
}

pub fn boot_id() -> io::Result<String> {
    Ok(host_proc_read("sys/kernel/random/boot_id")?
        .trim()
        .to_owned())
}

fn namespace_identity(file: &File) -> io::Result<(u64, u64)> {
    let stat = file.metadata()?;
    Ok((stat.dev(), stat.ino()))
}

impl PinnedProcess {
    /// Require an exact live parent at both sides of the proc parent read.
    /// This binds the host guardian fork to the entry incarnation, rather than
    /// accepting an inherited root ID from an unrelated sibling process.
    pub fn direct_child_of(&self, parent: &PinnedProcess) -> io::Result<bool> {
        self.verify()?;
        parent.verify()?;
        let stat = host_proc_read(&format!("{}/stat", self.host_pid))?;
        let tail = stat
            .rsplit_once(") ")
            .ok_or_else(|| io::Error::other("malformed proc stat"))?
            .1;
        let ppid: i32 = tail
            .split_ascii_whitespace()
            .nth(1)
            .ok_or_else(|| io::Error::other("missing parent PID"))?
            .parse()
            .map_err(|_| io::Error::other("bad parent PID"))?;
        self.verify()?;
        parent.verify()?;
        Ok(ppid == parent.host_pid)
    }

    pub fn same_executable_as(&self, installed: &File) -> io::Result<bool> {
        self.verify()?;
        let image = host_proc_file(&format!("{}/exe", self.host_pid))?;
        let actual = image.metadata()?;
        let expected = installed.metadata()?;
        self.verify()?;
        Ok((actual.dev(), actual.ino()) == (expected.dev(), expected.ino()))
    }

    pub fn open(host_pid: i32) -> io::Result<Self> {
        if host_pid <= 0 {
            return Err(io::Error::other("invalid host PID"));
        }
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, host_pid, 0) as i32 };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let pidfd = unsafe { File::from_raw_fd(fd) };
        let boot_id = boot_id()?;
        let (starttime_ticks, state) = proc_starttime(host_pid)?;
        if state == b'Z' || state == b'X' {
            return Err(io::Error::other("dead peer"));
        }
        let pidns = host_proc_file(&format!("{host_pid}/ns/pid"))?;
        let (pidns_dev, pidns_ino) = namespace_identity(&pidns)?;
        let process = Self {
            host_pid,
            boot_id,
            starttime_ticks,
            pidns_dev,
            pidns_ino,
            pidfd,
            pidns,
        };
        process.verify()?;
        Ok(process)
    }

    pub fn verify(&self) -> io::Result<()> {
        if boot_id()? != self.boot_id {
            return Err(io::Error::other("boot changed"));
        }
        let (starttime, state) = proc_starttime(self.host_pid)?;
        if starttime != self.starttime_ticks || state == b'Z' || state == b'X' {
            return Err(io::Error::other("process incarnation changed"));
        }
        let current = host_proc_file(&format!("{}/ns/pid", self.host_pid))?;
        if namespace_identity(&current)? != (self.pidns_dev, self.pidns_ino) {
            return Err(io::Error::other("PID namespace changed"));
        }
        let mut pollfd = libc::pollfd {
            fd: self.pidfd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let rc = unsafe { libc::poll(&mut pollfd, 1, 0) };
        if rc != 0 {
            return Err(io::Error::other("pidfd exited or poll failed"));
        }
        Ok(())
    }

    pub fn namespace(&self) -> &File {
        &self.pidns
    }

    /// A root launch may originate only from the broker's host PID namespace.
    /// `Outside` also includes unrelated child namespaces, which are not an
    /// entry authority. Reverify the pinned process on both sides of the
    /// namespace comparison so a dead/reused connector is never accepted.
    pub fn in_namespace(&self, namespace: &File) -> io::Result<bool> {
        self.verify()?;
        let matches = namespace_identity(namespace)? == (self.pidns_dev, self.pidns_ino);
        self.verify()?;
        Ok(matches)
    }

    pub fn is_namespace_init(&self) -> io::Result<bool> {
        self.verify()?;
        let status = host_proc_read(&format!("{}/status", self.host_pid))?;
        let line = status
            .lines()
            .find(|line| line.starts_with("NSpid:"))
            .ok_or_else(|| io::Error::other("missing NSpid"))?;
        let local = line
            .split_ascii_whitespace()
            .last()
            .ok_or_else(|| io::Error::other("empty NSpid"))?;
        self.verify()?;
        Ok(local == "1")
    }

    pub fn supplementary_groups(&self) -> io::Result<Vec<libc::gid_t>> {
        self.verify()?;
        let status = host_proc_read(&format!("{}/status", self.host_pid))?;
        let line = status
            .lines()
            .find(|line| line.starts_with("Groups:"))
            .ok_or_else(|| io::Error::other("missing Groups"))?;
        let groups: Vec<libc::gid_t> = line
            .split_ascii_whitespace()
            .skip(1)
            .map(|group| {
                group
                    .parse()
                    .map_err(|_| io::Error::other("bad supplementary group"))
            })
            .collect::<io::Result<_>>()?;
        if groups.len() > 256 {
            return Err(io::Error::other("too many supplementary groups"));
        }
        self.verify()?;
        Ok(groups)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supplementary_groups_come_from_pinned_kernel_peer() {
        let peer = PinnedProcess::open(std::process::id() as i32).unwrap();
        let mut actual = peer.supplementary_groups().unwrap();
        let count = unsafe { libc::getgroups(0, std::ptr::null_mut()) };
        assert!(count >= 0);
        let mut expected = vec![0 as libc::gid_t; count as usize];
        assert_eq!(
            unsafe { libc::getgroups(count, expected.as_mut_ptr()) },
            count
        );
        actual.sort_unstable();
        expected.sort_unstable();
        assert_eq!(actual, expected);
    }
}

#[derive(Debug)]
pub struct PeerIdentity {
    pub uid: u32,
    pub gid: u32,
    pub process: PinnedProcess,
}

pub fn classify_peer(
    peer: &PeerIdentity,
    host_namespace: &File,
    registry: &RootRegistry,
) -> Classification {
    if peer.process.verify().is_err() || registry.has_debt() {
        return Classification::Uncertain;
    }
    let mut namespace = match peer.process.namespace().try_clone() {
        Ok(fd) => fd,
        Err(_) => return Classification::Uncertain,
    };
    let host = match namespace_identity(host_namespace) {
        Ok(id) => id,
        Err(_) => return Classification::Uncertain,
    };
    for _ in 0..64 {
        let current = match namespace_identity(&namespace) {
            Ok(id) => id,
            Err(_) => return Classification::Uncertain,
        };
        let matches: Vec<_> = registry
            .live_roots()
            .filter(|r| (r.record.pidns_dev, r.record.pidns_ino) == current)
            .collect();
        if matches.len() > 1 {
            return Classification::Uncertain;
        }
        if let Some(root) = matches.first() {
            if root.init.verify().is_err() || peer.process.verify().is_err() {
                return Classification::Uncertain;
            }
            return Classification::Inside(root.record.root_id.clone());
        }
        if current == host {
            return if peer.process.verify().is_ok() {
                Classification::Outside
            } else {
                Classification::Uncertain
            };
        }
        let fd = unsafe { libc::ioctl(namespace.as_raw_fd(), libc::NS_GET_PARENT) };
        if fd < 0 {
            return Classification::Uncertain;
        }
        namespace = unsafe { File::from_raw_fd(fd) };
    }
    Classification::Uncertain
}
