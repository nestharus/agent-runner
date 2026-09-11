//! Crash-independent launch custody. A private monitor exists before launch
//! authority is issued. Only that monitor can attest that every inherited launch
//! endpoint closed and every started subreaper reported ECHILD.
//!
//! Linux only: other platforms fail closed rather than infer tree cessation.
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::{cell::RefCell, io};

pub fn proof_path(database: &Path, generation: &str) -> PathBuf {
    database
        .with_extension("starting-custody-v1")
        .join(generation)
}

pub fn is_quiescent(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    if !file
        .metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() == 1)
    {
        return false;
    }
    let mut bytes = [0; 2];
    file.read(&mut bytes)
        .is_ok_and(|n| n == 1 && bytes[0] == b'Q')
}

#[derive(Debug)]
pub struct LaunchCustody {
    path: PathBuf,
    #[cfg(target_os = "linux")]
    endpoint: Mutex<Option<Arc<std::os::fd::OwnedFd>>>,
    #[cfg(all(test, target_os = "linux"))]
    monitor_pid: i32,
}
impl PartialEq for LaunchCustody {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
    }
}
impl Eq for LaunchCustody {}

thread_local! {
    static CURRENT: RefCell<Option<Arc<LaunchCustody>>> = const { RefCell::new(None) };
}

/// Scoped to the synchronous operation caller, not process-global environment.
/// All provider operations (including describe and policy) use this scope.
pub struct LaunchScope(
    Option<Arc<LaunchCustody>>,
    std::marker::PhantomData<std::rc::Rc<()>>,
);
impl LaunchScope {
    pub fn enter(custody: Option<Arc<LaunchCustody>>) -> Self {
        Self(
            CURRENT.with(|current| current.replace(custody)),
            std::marker::PhantomData,
        )
    }
}
impl Drop for LaunchScope {
    fn drop(&mut self) {
        CURRENT.with(|current| current.replace(self.0.take()));
    }
}

pub fn configure_current(command: &mut Command) -> io::Result<()> {
    CURRENT.with(|current| match current.borrow().as_ref() {
        Some(custody) => custody.configure(command),
        None => Ok(()),
    })
}

impl LaunchCustody {
    pub fn start(path: PathBuf) -> io::Result<Self> {
        #[cfg(target_os = "linux")]
        {
            linux::start(path)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = path;
            Err(io::Error::other("independent launch custody unsupported"))
        }
    }

    /// Install after terminal/process-group setup, before workload-only filters.
    /// The returned std Child is a status proxy in the workload group. A separate
    /// subreaper outside that kill group owns the actual executable and tree.
    /// Describe/policy proxies are never published as the Running runtime child.
    pub fn configure(&self, command: &mut Command) -> io::Result<()> {
        #[cfg(target_os = "linux")]
        {
            linux::configure(self, command)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = command;
            Err(io::Error::other("independent launch custody unsupported"))
        }
    }

    /// Revoke future launch authority; already configured commands retain their
    /// own inherited endpoints until dropped and their trees are accounted for.
    pub fn seal(&self) {
        #[cfg(target_os = "linux")]
        {
            self.endpoint
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take();
        }
    }

    pub fn quiescent(&self) -> bool {
        is_quiescent(&self.path)
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::fs::OpenOptions;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::os::unix::{fs::OpenOptionsExt, process::CommandExt};

    /// Fork semantics without libc/pthread atfork callbacks: children below
    /// execute syscall-only bookkeeping or return into Command's exec path.
    /// With no sharing flags, a null child stack is the fork-style clone ABI;
    /// all optional tid/TLS arguments are null on the supported Linux ABIs.
    unsafe fn fork_process() -> libc::pid_t {
        unsafe {
            libc::syscall(
                libc::SYS_clone,
                libc::SIGCHLD as libc::c_ulong,
                0usize,
                0usize,
                0usize,
                0usize,
            ) as libc::pid_t
        }
    }

    // Parent-side ownership only. This guard is never constructed in a raw
    // child. Readiness/thread-start failures kill and reap the still-owned M;
    // successful setup transfers its consuming wait to one dedicated thread.
    struct MonitorOwner(Option<libc::pid_t>);
    impl MonitorOwner {
        fn wait(mut self) {
            if let Some(pid) = self.0.take() {
                let mut status = 0;
                unsafe {
                    wait_exact(pid, &mut status);
                }
            }
        }
    }
    impl Drop for MonitorOwner {
        fn drop(&mut self) {
            let Some(pid) = self.0.take() else {
                return;
            };
            let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
            loop {
                let rc = unsafe {
                    libc::waitid(
                        libc::P_PID,
                        pid as libc::id_t,
                        info.as_mut_ptr(),
                        libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                    )
                };
                if rc == 0 {
                    // The retained owned child pins the numeric signal target.
                    unsafe {
                        libc::kill(pid, libc::SIGKILL);
                    }
                    let mut status = 0;
                    unsafe {
                        wait_exact(pid, &mut status);
                    }
                    break;
                }
                if io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                    break;
                }
            }
        }
    }

    pub(super) fn start(path: PathBuf) -> io::Result<LaunchCustody> {
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("missing custody directory"))?;
        std::fs::create_dir_all(parent)?;
        // Never reuse or replace a generation's proof inode. No GC/unlink path.
        let proof = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)?;
        proof.sync_all()?;
        std::fs::File::open(parent)?.sync_all()?;
        let mut sockets = [-1; 2];
        if unsafe {
            libc::socketpair(
                libc::AF_UNIX,
                libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
                0,
                sockets.as_mut_ptr(),
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        let client = unsafe { OwnedFd::from_raw_fd(sockets[0]) };
        let server = unsafe { OwnedFd::from_raw_fd(sockets[1]) };
        let pid = unsafe { fork_process() };
        if pid < 0 {
            return Err(io::Error::last_os_error());
        }
        if pid == 0 {
            // Syscall-only after fork; setsid detaches cancellation scope, not
            // parenthood. M has no parent-death signal and survives creator exit.
            unsafe {
                monitor(server.as_raw_fd(), proof.as_raw_fd());
            }
        }
        let owner = MonitorOwner(Some(pid));
        drop(server);
        // Readiness is bounded even if the detached child cannot initialize.
        let mut pollfd = libc::pollfd {
            fd: client.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        if unsafe { libc::poll(&mut pollfd, 1, 5000) } <= 0 {
            return Err(io::Error::other("launch custodian readiness timeout"));
        }
        let mut byte = 0u8;
        if unsafe { libc::recv(client.as_raw_fd(), (&mut byte as *mut u8).cast(), 1, 0) } != 1
            || byte != b'R'
        {
            return Err(io::Error::other("launch custodian not ready"));
        }
        std::thread::Builder::new()
            .name("launch-custody-wait".into())
            .spawn(move || owner.wait())?;
        Ok(LaunchCustody {
            path,
            endpoint: Mutex::new(Some(Arc::new(client))),
            #[cfg(test)]
            monitor_pid: pid,
        })
    }

    pub(super) fn configure(custody: &LaunchCustody, command: &mut Command) -> io::Result<()> {
        let endpoint = custody.endpoint.lock().unwrap_or_else(|e| e.into_inner());
        let endpoint = endpoint
            .as_ref()
            .ok_or_else(|| io::Error::other("launch custody sealed"))?;
        // A Command needs a retained reference, not a new descriptor. Fork
        // already duplicates the descriptor table. This avoids EMFILE during
        // configuration after Starting and reduces per-command FD pressure.
        let inherited = Arc::clone(endpoint);
        unsafe {
            command.pre_exec(move || {
                let fd = inherited.as_raw_fd();
                // Disable the old creator-thread PDEATHSIG before any workload
                // can exist. This child becomes an independently living reaper.
                if libc::prctl(libc::PR_SET_PDEATHSIG, 0, 0, 0, 0) != 0
                    || libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) != 0
                {
                    return Err(io::Error::last_os_error());
                }
                reset_handlers();
                libc::signal(libc::SIGCHLD, libc::SIG_DFL);
                let proxy = libc::getpid();
                if libc::getpgrp() != proxy && libc::setpgid(0, 0) != 0 {
                    return Err(io::Error::last_os_error());
                }
                let mut status_pair = [-1; 2];
                if libc::socketpair(
                    libc::AF_UNIX,
                    libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
                    0,
                    status_pair.as_mut_ptr(),
                ) != 0
                {
                    return Err(io::Error::last_os_error());
                }
                let custodian = fork_process();
                if custodian < 0 {
                    libc::close(status_pair[0]);
                    libc::close(status_pair[1]);
                    return Err(io::Error::last_os_error());
                }
                if custodian > 0 {
                    libc::close(status_pair[1]);
                    proxy_status(status_pair[0], custodian, fd);
                }
                libc::close(status_pair[0]);
                // Keep the actual tree custodian OUTSIDE the workload's kill
                // group. Existing killpg cleanup may kill the std Child proxy
                // and its workload, but not the owner of quiescence evidence.
                if libc::setpgid(0, 0) != 0
                    || libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) != 0
                {
                    libc::_exit(125);
                }
                if !send(fd, b'B') {
                    libc::_exit(125);
                }
                let workload = fork_process();
                if workload < 0 {
                    send(fd, b'D');
                    libc::_exit(125);
                }
                if workload == 0 {
                    libc::close(fd);
                    libc::close(status_pair[1]);
                    if libc::setpgid(0, proxy) != 0 {
                        return Err(io::Error::last_os_error());
                    }
                    return Ok(());
                }
                reap_tree(fd, workload, status_pair[1]);
            });
        }
        Ok(())
    }

    unsafe fn send(fd: RawFd, byte: u8) -> bool {
        loop {
            let rc = unsafe { libc::send(fd, (&byte as *const u8).cast(), 1, libc::MSG_NOSIGNAL) };
            if rc == 1 {
                return true;
            }
            if unsafe { *libc::__errno_location() } != libc::EINTR {
                return false;
            }
        }
    }

    // close_range preserves precisely the two protocol descriptors, including
    // closing the std spawn error-pipe copy. The workload retains that pipe
    // until exec, so normal exec errors still reach Command::spawn.
    unsafe fn reset_handlers() {
        unsafe {
            for signal in 1..=64 {
                if signal == libc::SIGKILL || signal == libc::SIGSTOP {
                    continue;
                }
                let mut action: libc::sigaction = std::mem::zeroed();
                if libc::sigaction(signal, std::ptr::null(), &mut action) == 0
                    && action.sa_sigaction != libc::SIG_IGN
                {
                    libc::signal(signal, libc::SIG_DFL);
                }
            }
        }
    }

    unsafe fn close_except(a: RawFd, b: RawFd) {
        let low = a.min(b) as u32;
        let high = a.max(b) as u32;
        unsafe {
            if low > 0 && libc::syscall(libc::SYS_close_range, 0u32, low - 1, 0u32) != 0 {
                libc::_exit(125);
            }
            if high > low + 1 && libc::syscall(libc::SYS_close_range, low + 1, high - 1, 0u32) != 0
            {
                libc::_exit(125);
            }
            if libc::syscall(libc::SYS_close_range, high + 1, u32::MAX, 0u32) != 0 {
                libc::_exit(125);
            }
        }
    }

    unsafe fn monitor(fd: RawFd, proof: RawFd) -> ! {
        unsafe {
            reset_handlers();
            close_except(fd, proof);
            if libc::setsid() < 0 {
                libc::_exit(125);
            }
            if !send(fd, b'R') {
                libc::_exit(125);
            }
            let mut debt: u64 = 0;
            loop {
                let mut byte = 0u8;
                let rc = libc::recv(fd, (&mut byte as *mut u8).cast(), 1, 0);
                if rc == 0 {
                    if debt == 0 {
                        let byte = b'Q';
                        if libc::pwrite(proof, (&byte as *const u8).cast(), 1, 0) == 1 {
                            libc::fsync(proof);
                        }
                    }
                    libc::_exit(0);
                }
                if rc < 0 {
                    if *libc::__errno_location() == libc::EINTR {
                        continue;
                    }
                    libc::_exit(125);
                }
                match byte {
                    b'B' if debt < u64::MAX => debt += 1,
                    b'D' if debt > 0 => debt -= 1,
                    _ => libc::_exit(125),
                }
            }
        }
    }

    unsafe fn wait_exact(pid: libc::pid_t, status: &mut i32) {
        unsafe {
            while libc::waitpid(pid, status, 0) < 0 {
                if *libc::__errno_location() != libc::EINTR {
                    *status = 125 << 8;
                    break;
                }
            }
        }
    }

    unsafe fn proxy_status(fd: RawFd, custodian: libc::pid_t, launch_fd: RawFd) -> ! {
        unsafe {
            // A stopped proxy also retains the launch endpoint until actual exit.
            close_except(fd, launch_fd);
            let mut status: i32 = 125 << 8;
            loop {
                let rc = libc::recv(fd, (&mut status as *mut i32).cast(), 4, 0);
                if rc == 4 {
                    break;
                }
                if rc < 0 && *libc::__errno_location() == libc::EINTR {
                    continue;
                }
                let mut custody_status = 0;
                wait_exact(custodian, &mut custody_status);
                libc::_exit(125);
            }
            let mut custody_status = 0;
            wait_exact(custodian, &mut custody_status);
            if libc::WIFEXITED(status) {
                libc::_exit(libc::WEXITSTATUS(status));
            }
            let signal = libc::WTERMSIG(status);
            libc::signal(signal, libc::SIG_DFL);
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            libc::sigaddset(&mut set, signal);
            libc::sigprocmask(libc::SIG_UNBLOCK, &set, std::ptr::null_mut());
            libc::kill(libc::getpid(), signal);
            libc::_exit(128 + signal);
        }
    }

    unsafe fn reap_tree(fd: RawFd, workload: libc::pid_t, status_fd: RawFd) -> ! {
        unsafe {
            close_except(fd, status_fd);
            let mut root_status = None;
            loop {
                let mut status = 0;
                let pid = libc::waitpid(-1, &mut status, 0);
                if pid == workload {
                    root_status = Some(status);
                }
                if pid > 0 {
                    continue;
                }
                if *libc::__errno_location() == libc::EINTR {
                    continue;
                }
                if *libc::__errno_location() != libc::ECHILD {
                    libc::_exit(125);
                }
                // ECHILD is proof only here: this dedicated subreaper was in
                // place before fork and never transfers/adopts away descendants.
                if root_status.is_none() || !send(fd, b'D') {
                    libc::_exit(125);
                }
                // Retain the endpoint through actual custodian exit as well.
                let status = root_status.unwrap_or(125 << 8);
                loop {
                    let rc = libc::send(
                        status_fd,
                        (&status as *const i32).cast(),
                        4,
                        libc::MSG_NOSIGNAL,
                    );
                    if rc >= 0 || *libc::__errno_location() != libc::EINTR {
                        break;
                    }
                }
                libc::_exit(0);
            }
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Stdio};
    use std::time::{Duration, Instant};

    struct Fixture {
        root: PathBuf,
        creator: Option<Child>,
        stopped_proxy: Option<i32>,
    }
    impl Fixture {
        fn new(phase: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "custody-{}-{}-{}",
                std::process::id(),
                phase,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir(&root).unwrap();
            let creator = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "launch_custody::tests::private_creator",
                    "--nocapture",
                ])
                .env_clear()
                .env("HOME", &root)
                .env("XDG_CONFIG_HOME", &root)
                .env("XDG_DATA_HOME", &root)
                .env("CUSTODY_PRIVATE_ROOT", &root)
                .env("CUSTODY_PRIVATE_PHASE", phase)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            Self {
                root,
                creator: Some(creator),
                stopped_proxy: None,
            }
        }
        fn wait_ready(&self) {
            eventually(|| self.root.join("ready").exists());
        }
        fn crash_unreaped(&self) {
            let pid = self.creator.as_ref().unwrap().id() as i32;
            assert_eq!(unsafe { libc::kill(pid, libc::SIGKILL) }, 0);
            eventually(|| {
                let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
                stat.rsplit_once(") ").unwrap().1.starts_with("Z ")
            });
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if let Some(pid) = self.stopped_proxy.take() {
                unsafe {
                    libc::kill(pid, libc::SIGCONT);
                }
            }
            if let Some(mut child) = self.creator.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            // Fixture descendants are finite (<= 1s), never arbitrary host PIDs.
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    fn eventually(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(
                Instant::now() < deadline,
                "private custody fixture deadline"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn private_creator() {
        let Some(root) = std::env::var_os("CUSTODY_PRIVATE_ROOT").map(PathBuf::from) else {
            return;
        };
        let phase = std::env::var("CUSTODY_PRIVATE_PHASE").unwrap();
        if phase == "deny_detach" {
            // This filter is installed only in the private fixture process.
            let mut filter = [
                libc::sock_filter {
                    code: 0x20,
                    jt: 0,
                    jf: 0,
                    k: 0,
                },
                libc::sock_filter {
                    code: 0x15,
                    jt: 0,
                    jf: 1,
                    k: libc::SYS_setsid as u32,
                },
                libc::sock_filter {
                    code: 0x06,
                    jt: 0,
                    jf: 0,
                    k: 0x00050000 | libc::EPERM as u32,
                },
                libc::sock_filter {
                    code: 0x06,
                    jt: 0,
                    jf: 0,
                    k: 0x7fff0000,
                },
            ];
            let program = libc::sock_fprog {
                len: filter.len() as u16,
                filter: filter.as_mut_ptr(),
            };
            assert_eq!(
                unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) },
                0
            );
            assert_eq!(unsafe { libc::prctl(libc::PR_SET_SECCOMP, 2, &program) }, 0);
            assert!(LaunchCustody::start(root.join("proof")).is_err());
            std::fs::write(root.join("ready"), b"rejected").unwrap();
            return;
        }
        let custody = LaunchCustody::start(root.join("proof")).unwrap();
        if phase == "after_spawn" || phase == "stopped_preexec" {
            use std::os::fd::AsRawFd;
            let mut command = Command::new("/bin/sh");
            command
                .arg("-c")
                .arg("/usr/bin/setsid /bin/sh -c 'echo ready > descendant; /bin/sleep 1' & exit 7")
                .current_dir(&root)
                .env_clear()
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .process_group(0);
            if phase == "stopped_preexec" {
                let barrier = std::fs::File::create(root.join("gated")).unwrap();
                unsafe {
                    command.pre_exec(move || {
                        let pid = libc::getpid();
                        libc::write(barrier.as_raw_fd(), (&pid as *const i32).cast(), 4);
                        libc::kill(pid, libc::SIGSTOP);
                        Ok(())
                    });
                }
            }
            custody.configure(&mut command).unwrap();
            let _owned = command.spawn().unwrap();
            std::fs::write(root.join("reaper"), _owned.id().to_string()).unwrap();
            drop(command);
            eventually(|| root.join("descendant").exists());
        }
        std::fs::write(root.join("ready"), b"ready").unwrap();
        // Parent kills this creator and deliberately keeps it unreaped.
        std::thread::sleep(Duration::from_secs(10));
        panic!("fixture creator was not crashed");
    }

    #[test]
    fn denied_monitor_detachment_never_grants_launch_authority() {
        let mut fixture = Fixture::new("deny_detach");
        fixture.wait_ready();
        eventually(|| {
            fixture
                .creator
                .as_mut()
                .unwrap()
                .try_wait()
                .unwrap()
                .is_some()
        });
        assert!(fixture.creator.as_mut().unwrap().wait().unwrap().success());
        assert!(!is_quiescent(&fixture.root.join("proof")));
    }

    #[test]
    fn creator_crash_before_spawn_certifies_quiescence_while_unreaped() {
        let fixture = Fixture::new("before_spawn");
        fixture.wait_ready();
        assert!(!is_quiescent(&fixture.root.join("proof")));
        fixture.crash_unreaped();
        eventually(|| is_quiescent(&fixture.root.join("proof")));
    }

    #[test]
    fn unpublished_tree_survives_creator_crash_and_blocks_until_escaped_descendant_exits() {
        let fixture = Fixture::new("after_spawn");
        fixture.wait_ready();
        fixture.crash_unreaped();
        assert!(!is_quiescent(&fixture.root.join("proof")));
        eventually(|| is_quiescent(&fixture.root.join("proof")));
    }

    #[test]
    fn stopped_unpublished_preexec_child_keeps_custody_after_creator_crash() {
        let fixture = Fixture::new("stopped_preexec");
        eventually(|| std::fs::metadata(fixture.root.join("gated")).is_ok_and(|m| m.len() == 4));
        let bytes = std::fs::read(fixture.root.join("gated")).unwrap();
        let pid = i32::from_ne_bytes(bytes.try_into().unwrap());
        fixture.crash_unreaped();
        assert!(!is_quiescent(&fixture.root.join("proof")));
        // A stopped child cannot shed its inherited custody endpoint. Resume
        // the exact still-stopped fixture; no signal after its identity releases.
        assert_eq!(unsafe { libc::kill(pid, libc::SIGCONT) }, 0);
        eventually(|| is_quiescent(&fixture.root.join("proof")));
    }

    #[test]
    fn stopped_status_proxy_retains_custody_after_actual_workload_exit() {
        let mut fixture = Fixture::new("after_spawn");
        fixture.wait_ready();
        let proxy: i32 = std::fs::read_to_string(fixture.root.join("reaper"))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(unsafe { libc::kill(proxy, libc::SIGSTOP) }, 0);
        fixture.stopped_proxy = Some(proxy);
        fixture.crash_unreaped();
        std::thread::sleep(Duration::from_millis(1200));
        let withheld = !is_quiescent(&fixture.root.join("proof"));
        assert_eq!(unsafe { libc::kill(proxy, libc::SIGCONT) }, 0);
        fixture.stopped_proxy = None;
        eventually(|| is_quiescent(&fixture.root.join("proof")));
        assert!(
            withheld,
            "an unreleased status proxy lost its inherited endpoint"
        );
    }

    #[test]
    fn hard_kill_of_workload_group_cannot_kill_the_independent_tree_custodian() {
        let fixture = Fixture::new("after_spawn");
        fixture.wait_ready();
        let proxy: i32 = std::fs::read_to_string(fixture.root.join("reaper"))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(unsafe { libc::killpg(proxy, libc::SIGKILL) }, 0);
        fixture.crash_unreaped();
        assert!(!is_quiescent(&fixture.root.join("proof")));
        // The escaped finite descendant remains real/live outside the killed
        // group; the separate custodian still adopts and waits it.
        eventually(|| is_quiescent(&fixture.root.join("proof")));
    }

    #[test]
    fn lost_tree_reaper_is_unknown_not_creator_death_proof() {
        let fixture = Fixture::new("after_spawn");
        fixture.wait_ready();
        let pid: i32 = std::fs::read_to_string(fixture.root.join("reaper"))
            .unwrap()
            .parse()
            .unwrap();
        let children = std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children")).unwrap();
        let custodian: i32 = children.split_whitespace().next().unwrap().parse().unwrap();
        assert_eq!(unsafe { libc::kill(custodian, libc::SIGKILL) }, 0);
        fixture.crash_unreaped();
        std::thread::sleep(Duration::from_millis(1200));
        assert!(!is_quiescent(&fixture.root.join("proof")));
        eventually(|| !std::path::Path::new(&format!("/proc/{custodian}")).exists());
    }

    #[test]
    fn completed_monitor_is_reaped_while_its_creator_remains_alive() {
        let root = std::env::temp_dir().join(format!("custody-wait-owner-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let custody = LaunchCustody::start(root.join("proof")).unwrap();
        let pid = custody.monitor_pid;
        custody.seal();
        eventually(|| custody.quiescent());
        eventually(|| !std::path::Path::new(&format!("/proc/{pid}")).exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sealed_authority_rejects_new_commands_and_preserves_exit_status() {
        let root = std::env::temp_dir().join(format!("custody-status-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let custody = LaunchCustody::start(root.join("proof")).unwrap();
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exit 7"]).env_clear();
        custody.configure(&mut command).unwrap();
        let mut child = command.spawn().unwrap();
        drop(command);
        custody.seal();
        assert!(custody.configure(&mut Command::new("/bin/true")).is_err());
        eventually(|| child.try_wait().unwrap().is_some());
        assert_eq!(child.wait().unwrap().code(), Some(7));
        eventually(|| custody.quiescent());
        std::fs::remove_dir_all(root).unwrap();
    }
}
