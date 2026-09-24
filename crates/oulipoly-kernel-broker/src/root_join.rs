//! One-use root child placement. The host guardian remains outside this PID
//! namespace. PID1 is a reaper and retains the root after the broker exits.
use oulipoly_kernel_broker::entry_registry::EntryRegistry;
use oulipoly_kernel_broker::entry_registry::ProcessStamp;
use oulipoly_kernel_broker::identity::{PeerIdentity, PinnedProcess};
use oulipoly_kernel_broker::protocol::JoinSpec;
use oulipoly_kernel_broker::registry::{RootRecord, RootRegistry};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

const GATE_ENV: &str = "OULIPOLY_KERNEL_CHILD_JOIN_FD_V1";

/// The broker owns this gate after J has durably consumed the entry and bound
/// the exact child. Dropping it sends EOF to the child pre-exec read, which
/// fails closed. v30 retains this object across the gate write and commit.
pub(super) struct HeldRootJoin {
    gate: UnixStream,
    root_id: String,
    grant: String,
    entry: PinnedProcess,
    guardian: PinnedProcess,
    root_init: PinnedProcess,
    child: PinnedProcess,
    release_attempted: bool,
    release_id: Option<String>,
    #[cfg(feature = "age319-private-broker-fixture")]
    launch_args: Vec<String>,
}

impl HeldRootJoin {
    pub(super) fn release_id(&self) -> Option<&str> {
        self.release_id.as_deref()
    }

    pub(super) fn handoff_intent_ready(&self) -> bool {
        // Production currently admits only help/diagnostics in
        // supported_entry_args. Neither is a Bash invocation. A real
        // production handoff must wait for a paired descriptor route.
        #[cfg(feature = "age319-private-broker-fixture")]
        if super::private_fixture() && self.launch_args == ["__age319-private-bash-work-v1"] {
            return true;
        }
        false
    }

    /// A partial write can open the physical gate. Consume the attempt before
    /// writing so failure cannot be retried or followed by a State commit.
    pub(super) fn write_v30_gate(
        &mut self,
        source_generation: &str,
        owner_generation: &str,
    ) -> io::Result<()> {
        if self.release_attempted {
            return Err(io::Error::other("held gate release already attempted"));
        }
        self.actors()?;
        self.release_attempted = true;
        #[cfg(feature = "age319-private-broker-fixture")]
        if super::private_fixture()
            && std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_FAIL_GATE_WRITE_V1").is_some()
        {
            self.gate.shutdown(std::net::Shutdown::Write)?;
        }
        let grant = format!("Rv30 {source_generation} {owner_generation} {}", self.grant);
        self.gate.write_all(grant.as_bytes())
    }

    pub(super) fn record_release(&mut self, release_id: String) {
        self.release_id = Some(release_id);
    }

    pub(super) fn root_id(&self) -> &str {
        &self.root_id
    }

    pub(super) fn actors(&self) -> io::Result<[ProcessStamp; 4]> {
        self.entry.verify()?;
        self.guardian.verify()?;
        self.root_init.verify()?;
        self.child.verify()?;
        if !self.guardian.direct_child_of(&self.entry)?
            || !self.child.direct_child_of(&self.root_init)?
            || !self.child.in_namespace(self.root_init.namespace())?
        {
            return Err(io::Error::other("held join actor incarnation changed"));
        }
        Ok([
            ProcessStamp::from(&self.entry),
            ProcessStamp::from(&self.guardian),
            ProcessStamp::from(&self.root_init),
            ProcessStamp::from(&self.child),
        ])
    }

    fn release_legacy(mut self) -> io::Result<String> {
        self.entry.verify()?;
        self.guardian.verify()?;
        self.root_init.verify()?;
        self.child.verify()?;
        if !self.guardian.direct_child_of(&self.entry)?
            || !self.child.direct_child_of(&self.root_init)?
            || !self.child.in_namespace(self.root_init.namespace())?
        {
            return Err(io::Error::other("held join actor incarnation changed"));
        }
        self.gate.write_all(b"R")?;
        self.gate.write_all(self.grant.as_bytes())?;
        Ok(format!("joined {} {}\n", self.root_id, self.child.host_pid))
    }
}

struct InitContext {
    spec: JoinSpec,
    descriptors: [File; 5],
    image: File,
    control: UnixStream,
    gate: UnixStream,
    uid: u32,
    gid: u32,
    groups: Vec<libc::gid_t>,
    held_v30: bool,
}

fn validate(spec: &JoinSpec, descriptors: &[File; 5]) -> io::Result<()> {
    let authority: serde_json::Value = serde_json::from_str(&spec.root_authority)?;
    if authority["protocol"] != "root-authority-v1"
        || authority["root_id"] != spec.root_id
        || authority["domain_id"] != spec.domain_id
        || authority["supervisor_authority_id"] != spec.supervisor_id
        || authority["guardian_identity"]["pid"] != spec.guardian_pid
        || !authority["capability"].as_str().is_some_and(|capability| {
            capability.len() == 64 && capability.bytes().all(|b| b.is_ascii_hexdigit())
        })
    {
        return Err(io::Error::other("root join capability binding conflict"));
    }
    for id in [&spec.root_id, &spec.domain_id, &spec.supervisor_id] {
        uuid::Uuid::parse_str(id).map_err(|_| io::Error::other("invalid join identity"))?;
    }
    if spec.guardian_pid <= 0
        || spec.args.is_empty()
        || spec.args.len() > 256
        || !oulipoly_kernel_broker::protocol::supported_entry_args(&spec.args)
        || spec.args.iter().any(|v| v.contains('\0'))
        || spec.environment.len() > 512
    {
        return Err(io::Error::other("unsupported CLI invocation"));
    }
    let mut names = HashSet::new();
    for (key, value) in &spec.environment {
        if key.is_empty()
            || key.contains(['=', '\0'])
            || value.contains('\0')
            || !names.insert(key)
            || key.starts_with("LD_")
            || key.starts_with("DYLD_")
            || key.starts_with("OULIPOLY_KERNEL_")
            || matches!(
                key.as_str(),
                "GLIBC_TUNABLES"
                    | "GCONV_PATH"
                    | "OULIPOLY_COMPLETION_ENDPOINT"
                    | "OULIPOLY_ROOT_AUTHORITY_V1"
                    | "OULIPOLY_ORIGINAL_WORK_REQUIRED_V1"
            )
        {
            return Err(io::Error::other(format!(
                "unsafe or duplicate join environment key: {key}"
            )));
        }
    }
    for (index, file) in descriptors.iter().enumerate() {
        let fd = file.as_raw_fd();
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(fd, &mut stat) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let kind = stat.st_mode & libc::S_IFMT;
        match index {
            0..=2 => {
                if unsafe { libc::isatty(fd) } != 0
                    || !matches!(kind, libc::S_IFREG | libc::S_IFIFO | libc::S_IFCHR)
                {
                    return Err(io::Error::other("TTY or unsupported standard descriptor"));
                }
                let mode = unsafe { libc::fcntl(fd, libc::F_GETFL) };
                if mode < 0
                    || (index == 0 && mode & libc::O_ACCMODE == libc::O_WRONLY)
                    || (index != 0 && mode & libc::O_ACCMODE == libc::O_RDONLY)
                {
                    return Err(io::Error::other("wrong standard descriptor direction"));
                }
            }
            3 if kind != libc::S_IFDIR => {
                return Err(io::Error::other("working directory is not a directory"));
            }
            4 if kind != libc::S_IFSOCK => {
                return Err(io::Error::other("invalid completion socket"));
            }
            _ => {}
        }
    }
    Ok(())
}

fn close_other_descriptors(keep: &[RawFd]) -> io::Result<()> {
    let mut discard = Vec::new();
    for entry in fs::read_dir("/proc/self/fd")? {
        let fd: RawFd = entry?.file_name().to_string_lossy().parse().unwrap_or(-1);
        if fd > 2 && !keep.contains(&fd) {
            discard.push(fd);
        }
    }
    for fd in discard {
        unsafe { libc::close(fd) };
    }
    Ok(())
}

extern "C" fn init_start(pointer: *mut libc::c_void) -> libc::c_int {
    let context = unsafe { Box::from_raw(pointer.cast::<InitContext>()) };
    if run_init(*context).is_ok() { 0 } else { 70 }
}

fn run_init(context: InitContext) -> io::Result<()> {
    let InitContext {
        spec,
        descriptors: [stdin, stdout, stderr, cwd, status],
        image,
        mut control,
        gate,
        uid,
        gid,
        groups,
        held_v30,
    } = context;
    close_other_descriptors(&[
        stdin.as_raw_fd(),
        stdout.as_raw_fd(),
        stderr.as_raw_fd(),
        cwd.as_raw_fd(),
        status.as_raw_fd(),
        image.as_raw_fd(),
        control.as_raw_fd(),
        gate.as_raw_fd(),
    ])?;
    if unsafe { libc::getpid() } != 1 {
        return Err(io::Error::other("root init is not namespace PID1"));
    }
    let mut release = [0u8; 1];
    control.read_exact(&mut release)?;
    if release != [b'P'] {
        return Err(io::Error::other("root persistence gate refused"));
    }
    if unsafe { libc::fchdir(cwd.as_raw_fd()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    drop(cwd);
    let image_path = format!("/proc/self/fd/{}", image.as_raw_fd());
    let mut command = Command::new(image_path);
    command
        .args(&spec.args)
        .env_clear()
        .envs(spec.environment.iter().map(|(key, value)| (key, value)))
        .env("OULIPOLY_ROOT_AUTHORITY_V1", &spec.root_authority)
        .env(GATE_ENV, gate.as_raw_fd().to_string())
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    #[cfg(feature = "age319-private-broker-fixture")]
    if held_v30 && super::private_fixture() {
        command.env("OULIPOLY_KERNEL_V30_PRIVATE_CHILD_V1", "1");
        if let Some(directory) = std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1") {
            command.env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", directory);
        }
    }
    #[cfg(not(feature = "age319-private-broker-fixture"))]
    let _ = held_v30;
    #[cfg(feature = "age319-private-broker-fixture")]
    if super::private_fixture() {
        let path = std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1")
            .ok_or_else(|| io::Error::other("private broker socket absent"))?;
        command.env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", path);
    }
    let control_fd = control.as_raw_fd();
    let gate_fd = gate.as_raw_fd();
    let fixture = super::private_fixture();
    unsafe {
        command.pre_exec(move || {
            if (!fixture && libc::setgroups(groups.len(), groups.as_ptr()) != 0)
                || libc::setresgid(gid, gid, gid) != 0
                || libc::setresuid(uid, uid, uid) != 0
            {
                return Err(io::Error::last_os_error());
            }
            if libc::fcntl(gate_fd, libc::F_SETFD, 0) != 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::send(control_fd, b"C".as_ptr().cast(), 1, libc::MSG_NOSIGNAL) != 1 {
                return Err(io::Error::last_os_error());
            }
            let mut byte = 0u8;
            if libc::read(gate_fd, (&mut byte as *mut u8).cast(), 1) != 1 || byte != b'R' {
                return Err(io::Error::other("child pre-exec gate refused"));
            }
            Ok(())
        });
    }
    let child = command.spawn()?;
    let original_pid = child.id() as i32;
    drop(gate);
    drop(control);
    drop(image);
    // Reap every child, including adopted descendants that finish while the
    // original Runner is still live. The original exit is reported once.
    let mut original_reported = false;
    let mut status = Some(status);
    loop {
        let mut child_status = 0;
        let reaped = unsafe {
            libc::waitpid(
                -1,
                &mut child_status,
                if original_reported { libc::WNOHANG } else { 0 },
            )
        };
        if reaped == original_pid && !original_reported {
            let code = if libc::WIFEXITED(child_status) {
                libc::WEXITSTATUS(child_status)
            } else if libc::WIFSIGNALED(child_status) {
                128 + libc::WTERMSIG(child_status)
            } else {
                1
            };
            if let Some(mut receipt) = status.take() {
                receipt.write_all(format!("exit {code}\n").as_bytes())?;
            }
            original_reported = true;
        }
        if reaped > 0
            || reaped < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted
        {
            continue;
        }
        unsafe { libc::pause() };
    }
}

fn child_credential(stream: &UnixStream) -> io::Result<libc::ucred> {
    let mut byte = [0u8; 1];
    let mut iov = libc::iovec {
        iov_base: byte.as_mut_ptr().cast(),
        iov_len: 1,
    };
    let mut control = [0u8; 64];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = control.len();
    if unsafe { libc::recvmsg(stream.as_raw_fd(), &mut msg, 0) } != 1 || byte != [b'C'] {
        return Err(io::Error::other("root child never reached pre-exec gate"));
    }
    let header = unsafe { libc::CMSG_FIRSTHDR(&msg) };
    if header.is_null() || unsafe { (*header).cmsg_type } != libc::SCM_CREDENTIALS {
        return Err(io::Error::other("missing root child credentials"));
    }
    Ok(unsafe { *(libc::CMSG_DATA(header) as *const libc::ucred) })
}

pub(super) fn launch(
    spec: JoinSpec,
    descriptors: [File; 5],
    peer: &PeerIdentity,
    image: &File,
    roots: &mut RootRegistry,
    entries: &mut EntryRegistry,
) -> io::Result<String> {
    hold(spec, descriptors, peer, image, roots, entries, false)?.release_legacy()
}

/// Return only after the exact joined child has been fsynced while its
/// pre-exec gate is still held by this broker. v30 keeps the handle in the
/// serving broker through a one-use gate write and committed State release.
pub(super) fn hold(
    spec: JoinSpec,
    descriptors: [File; 5],
    peer: &PeerIdentity,
    image: &File,
    roots: &mut RootRegistry,
    entries: &mut EntryRegistry,
    _held_v30: bool,
) -> io::Result<HeldRootJoin> {
    validate(&spec, &descriptors)?;
    let mut receipt_peer = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut receipt_len = std::mem::size_of_val(&receipt_peer) as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            descriptors[4].as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut receipt_peer as *mut libc::ucred).cast(),
            &mut receipt_len,
        )
    } != 0
        || receipt_len as usize != std::mem::size_of_val(&receipt_peer)
        || (receipt_peer.pid, receipt_peer.uid, receipt_peer.gid)
            != (peer.process.host_pid, peer.uid, peer.gid)
    {
        return Err(io::Error::other(
            "completion receipt is not the original entry socket",
        ));
    }
    if (peer.uid < 1000 && !super::private_fixture())
        || !peer.process.same_executable_as(image)?
        || roots.has_debt()
    {
        return Err(io::Error::other("untrusted root join entry"));
    }
    // This fsynced transition precedes any namespace fork. It is never reset
    // on timeout, connection loss or broker restart.
    entries.consume_join(
        &spec.root_id,
        &spec.domain_id,
        &spec.supervisor_id,
        spec.guardian_pid,
        peer.uid,
        &peer.process,
    )?;
    let (mut broker_control, init_control) = UnixStream::pair()?;
    let (broker_gate, init_gate) = UnixStream::pair()?;
    let one: libc::c_int = 1;
    if unsafe {
        libc::setsockopt(
            broker_control.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PASSCRED,
            (&one as *const libc::c_int).cast(),
            std::mem::size_of_val(&one) as _,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    #[cfg(feature = "age319-private-broker-fixture")]
    let launch_args = spec.args.clone();
    let context = Box::new(InitContext {
        spec,
        descriptors,
        image: image.try_clone()?,
        control: init_control,
        gate: init_gate,
        uid: peer.uid,
        gid: peer.gid,
        groups: peer.process.supplementary_groups()?,
        held_v30: _held_v30,
    });
    let root_id = context.spec.root_id.clone();
    let domain = context.spec.domain_id.clone();
    let supervisor = context.spec.supervisor_id.clone();
    let guardian_pid = context.spec.guardian_pid;
    let pointer = Box::into_raw(context);
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
    // Parent and PID1 have independent address spaces after clone.
    unsafe {
        drop(Box::from_raw(pointer));
    }
    if init_pid < 0 {
        return Err(io::Error::last_os_error());
    }
    let init = PinnedProcess::open(init_pid)?;
    if !init.is_namespace_init()? || init.in_namespace(peer.process.namespace())? {
        return Err(io::Error::other("root PID namespace did not form"));
    }
    roots.insert(RootRecord {
        version: 1,
        boot_id: init.boot_id.clone(),
        root_id: root_id.clone(),
        owner_uid: peer.uid,
        init_host_pid: init_pid,
        init_starttime_ticks: init.starttime_ticks,
        pidns_dev: init.pidns_dev,
        pidns_ino: init.pidns_ino,
    })?;
    broker_control.write_all(b"P")?;
    let credentials = child_credential(&broker_control)?;
    let child = PinnedProcess::open(credentials.pid)?;
    if credentials.uid != peer.uid
        || credentials.gid != peer.gid
        || !child.direct_child_of(&init)?
        || !child.in_namespace(init.namespace())?
        || entries
            .bound_entry(&root_id, peer.uid, &peer.process)?
            .guardian
            .as_ref()
            .map(|g| g.host_pid)
            != Some(guardian_pid)
    {
        return Err(io::Error::other("root child pre-exec identity mismatch"));
    }
    // Persist the exact child incarnation before the executable can cross the
    // gate. A failed write leaves consumed-join debt, never a loose UUID grant.
    entries.bind_joined_child(&root_id, peer.uid, &peer.process, &child)?;
    #[cfg(feature = "age319-private-broker-fixture")]
    if !_held_v30
        && super::private_fixture()
        && let Some(directory) = std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1")
    {
        let directory = std::path::PathBuf::from(directory);
        fs::write(directory.join("ready"), child.host_pid.to_string())?;
        while !directory.join("release").exists() {
            init.verify()?;
            child.verify()?;
            peer.process.verify()?;
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    let guardian = PinnedProcess::open(guardian_pid)?;
    if entries
        .record(&root_id)
        .and_then(|record| record.guardian.as_ref())
        != Some(&oulipoly_kernel_broker::entry_registry::ProcessStamp::from(
            &guardian,
        ))
    {
        return Err(io::Error::other("held join guardian changed"));
    }
    Ok(HeldRootJoin {
        gate: broker_gate,
        grant: format!("{root_id} {domain} {supervisor} {guardian_pid}\n"),
        root_id,
        entry: PinnedProcess::open(peer.process.host_pid)?,
        guardian,
        root_init: init,
        child,
        release_attempted: false,
        release_id: None,
        #[cfg(feature = "age319-private-broker-fixture")]
        launch_args,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::{FromRawFd, IntoRawFd};

    fn descriptors() -> [File; 5] {
        let input = File::open("/dev/null").unwrap();
        let output = File::options().write(true).open("/dev/null").unwrap();
        let error = File::options().write(true).open("/dev/null").unwrap();
        let cwd = File::open(".").unwrap();
        let (_entry, receipt) = UnixStream::pair().unwrap();
        [input, output, error, cwd, unsafe {
            File::from_raw_fd(receipt.into_raw_fd())
        }]
    }

    fn spec() -> JoinSpec {
        let root_id = uuid::Uuid::new_v4().to_string();
        let domain_id = uuid::Uuid::new_v4().to_string();
        let supervisor_id = uuid::Uuid::new_v4().to_string();
        JoinSpec {
            root_authority: serde_json::json!({
                "protocol": "root-authority-v1",
                "root_id": root_id,
                "domain_id": domain_id,
                "supervisor_authority_id": supervisor_id,
                "guardian_identity": {"pid": 42},
                "capability": "a".repeat(64),
            })
            .to_string(),
            root_id,
            domain_id,
            supervisor_id,
            guardian_pid: 42,
            args: vec!["--help".into()],
            environment: vec![("HOME".into(), "/tmp".into())],
        }
    }

    #[test]
    fn only_validated_pipe_or_file_cli_descriptors_pass() {
        validate(&spec(), &descriptors()).unwrap();
        let mut pty_master = -1;
        let mut pty_slave = -1;
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut pty_master,
                    &mut pty_slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            },
            0
        );
        let mut files = descriptors();
        files[1] = unsafe { File::from_raw_fd(pty_slave) };
        assert!(
            validate(&spec(), &files)
                .unwrap_err()
                .to_string()
                .contains("TTY")
        );
        unsafe {
            libc::close(pty_master);
        }
    }

    #[test]
    fn gui_and_loader_environment_refuse_before_consumption() {
        let mut gui = spec();
        gui.args.clear();
        assert!(validate(&gui, &descriptors()).is_err());
        let mut loader = spec();
        loader
            .environment
            .push(("LD_PRELOAD".into(), "x.so".into()));
        assert!(validate(&loader, &descriptors()).is_err());
    }
}
