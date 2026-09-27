//! Broker-only one-use source launch. The wire caller selects no registration,
//! listener, path, image or grant ID: the retained sidecar does that while the
//! exact v30 driver and guardian are still live.
use oulipoly_kernel_broker::entry_registry::{EntryRegistry, ProcessStamp};
use oulipoly_kernel_broker::identity::{PeerIdentity, PinnedProcess};
use oulipoly_kernel_broker::registry::RootRegistry;
use oulipoly_kernel_broker::source_candidate::SourcePinnedCandidate;
use oulipoly_kernel_broker::source_physical::{
    SourcePhysicalRegistry, install_source_pid1_cancel_handler, reap_source_pid1_with_pipes,
};
use oulipoly_state::mailbox::{BrokerSidecar, CompletionDomainOwner};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

struct InitContext {
    grant_id: String,
    directory: PathBuf,
    image: File,
    environment: BTreeMap<String, String>,
    registration_path: PathBuf,
    confirmation_path: PathBuf,
    stdout_file: File,
    stderr_file: File,
    control: UnixStream,
    gate: UnixStream,
    owner_uid: u32,
    owner_gid: u32,
    groups: Vec<libc::gid_t>,
}

fn ensure_host_sudo_context() -> io::Result<()> {
    if unsafe { libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) } != 0
        || unsafe { libc::prctl(libc::PR_GET_SECCOMP, 0, 0, 0, 0) } != 0
    {
        return Err(io::Error::other(
            "source launch has NNP or seccomp restriction",
        ));
    }
    let last: u32 = fs::read_to_string("/proc/sys/kernel/cap_last_cap")?
        .trim()
        .parse()
        .map_err(|_| io::Error::other("invalid kernel capability range"))?;
    for cap in 0..=last {
        if unsafe { libc::prctl(libc::PR_CAPBSET_READ, cap as libc::c_ulong, 0, 0, 0) } != 1 {
            return Err(io::Error::other(
                "source launch capability bounding set restricted",
            ));
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
    match run_init(*context) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("source PID1 incomplete: {error}");
            70
        }
    }
}

fn run_init(context: InitContext) -> io::Result<()> {
    let InitContext {
        grant_id,
        directory,
        image,
        environment,
        registration_path,
        confirmation_path,
        stdout_file,
        stderr_file,
        mut control,
        gate,
        owner_uid,
        owner_gid,
        groups,
    } = context;
    close_other_descriptors(&[
        image.as_raw_fd(),
        stdout_file.as_raw_fd(),
        stderr_file.as_raw_fd(),
        control.as_raw_fd(),
        gate.as_raw_fd(),
    ])?;
    if unsafe { libc::getpid() } != 1 || ensure_host_sudo_context().is_err() {
        return Err(io::Error::other(
            "source PID1 lost unrestricted host sudo semantics",
        ));
    }
    install_source_pid1_cancel_handler()?;
    control.write_all(b"I")?;
    let mut permit = [0];
    control.read_exact(&mut permit)?;
    if permit != [b'P'] {
        return Err(io::Error::other("source PID1 persistence gate refused"));
    }
    let image_fd = image.as_raw_fd();
    let control_fd = control.as_raw_fd();
    let gate_fd = gate.as_raw_fd();
    let mut command = Command::new(format!("/proc/self/fd/{image_fd}"));
    command
        .env_clear()
        .envs(environment)
        .arg("completion-reconcile-v2")
        .arg("--registration-file")
        .arg(registration_path)
        .arg("--confirmation")
        .arg(confirmation_path)
        .arg("--json")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let fixture = super::private_fixture();
    unsafe {
        command.pre_exec(move || {
            if libc::setsid() < 0
                || (!fixture && libc::setgroups(groups.len(), groups.as_ptr()) != 0)
                || libc::setresgid(owner_gid, owner_gid, owner_gid) != 0
                || libc::setresuid(owner_uid, owner_uid, owner_uid) != 0
                || libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) != 0
                || libc::prctl(libc::PR_GET_SECCOMP, 0, 0, 0, 0) != 0
            {
                return Err(io::Error::last_os_error());
            }
            let flags = libc::fcntl(image_fd, libc::F_GETFD);
            if flags < 0 || libc::fcntl(image_fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::send(control_fd, b"C".as_ptr().cast(), 1, libc::MSG_NOSIGNAL) != 1 {
                return Err(io::Error::last_os_error());
            }
            let mut byte = 0u8;
            if libc::read(gate_fd, (&mut byte as *mut u8).cast(), 1) != 1 || byte != b'R' {
                return Err(io::Error::other("source worker pre-exec gate refused"));
            }
            Ok(())
        });
    }
    let mut worker = command.spawn()?;
    let _ = control.write_all(b"E");
    let stdout = worker
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("source stdout pipe absent"))?;
    let stderr = worker
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("source stderr pipe absent"))?;
    drop(worker);
    drop(gate);
    drop(control);
    drop(image);
    let durable = SourcePhysicalRegistry::open(&directory)?;
    let record = durable
        .records()
        .iter()
        .find(|record| record.grant.grant_id == grant_id)
        .ok_or_else(|| io::Error::other("source physical record absent after gate"))?;
    reap_source_pid1_with_pipes(&directory, record, stdout, stderr, stdout_file, stderr_file)
}

fn child_credential(stream: &UnixStream, expected: u8) -> io::Result<libc::ucred> {
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
    if unsafe { libc::recvmsg(stream.as_raw_fd(), &mut msg, 0) } != 1 || byte != [expected] {
        return Err(io::Error::other("source child never reached identity gate"));
    }
    let header = unsafe { libc::CMSG_FIRSTHDR(&msg) };
    if header.is_null() || unsafe { (*header).cmsg_type } != libc::SCM_CREDENTIALS {
        return Err(io::Error::other("source child credentials absent"));
    }
    Ok(unsafe { *(libc::CMSG_DATA(header) as *const libc::ucred) })
}

fn create_init(
    parent_namespace: &File,
    context: InitContext,
) -> io::Result<(i32, UnixStream, UnixStream)> {
    let (broker_control, init_control) = UnixStream::pair()?;
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
    let mut context = context;
    context.control = init_control;
    context.gate = init_gate;
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(io::Error::last_os_error());
    }
    if pid == 0 {
        drop(broker_control);
        drop(broker_gate);
        if unsafe { libc::setns(parent_namespace.as_raw_fd(), libc::CLONE_NEWPID) } != 0 {
            unsafe { libc::_exit(70) };
        }
        let entered = unsafe { libc::fork() };
        if entered < 0 {
            unsafe { libc::_exit(70) };
        }
        if entered > 0 {
            unsafe { libc::_exit(0) };
        }
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
            unsafe { libc::_exit(70) };
        }
        unsafe { libc::_exit(0) };
    }
    drop(context);
    let mut status = 0;
    let waited = unsafe { libc::waitpid(pid, &mut status, 0) };
    if waited != pid || !libc::WIFEXITED(status) || libc::WEXITSTATUS(status) != 0 {
        return Err(io::Error::other("source namespace helper failed"));
    }
    let credential = child_credential(&broker_control, b'I')?;
    if credential.uid != 0 || credential.pid <= 0 {
        return Err(io::Error::other("source PID1 identity refused"));
    }
    Ok((credential.pid, broker_control, broker_gate))
}

fn write_confirmation(
    candidate: &SourcePinnedCandidate,
    path: &Path,
    registration_digest: &str,
    owner_uid: u32,
) -> io::Result<()> {
    let value = serde_json::json!({
        "protocol": candidate.source.protocol,
        "domain_id": candidate.source.domain_id,
        "source_id": candidate.source.source_id,
        "handle": candidate.source.handle,
        "registration_id": candidate.source.registration_id,
        "registration_digest": registration_digest,
        "status": "exact_committed",
        "authority": "completion_only",
        "registration_committed": true,
    });
    // Serialize before creating a name so malformed material has no effect.
    let bytes = serde_json::to_vec(&value)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o400)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    if unsafe { libc::fchown(file.as_raw_fd(), owner_uid, u32::MAX) } != 0 {
        return Err(io::Error::last_os_error());
    }
    file.write_all(&bytes)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    File::open(&candidate.source.handle_dir)?.sync_all()?;
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "independent State, root and physical authorities"
)]
pub(super) fn launch(
    root_id: &str,
    owner: &CompletionDomainOwner,
    peer: &PeerIdentity,
    roots: &RootRegistry,
    entries: &EntryRegistry,
    sidecar: &mut BrokerSidecar,
    physical: &mut SourcePhysicalRegistry,
    directory: &Path,
) -> io::Result<String> {
    ensure_host_sudo_context()?;
    let root = roots
        .live_roots()
        .find(|root| root.record.root_id == root_id)
        .ok_or_else(|| io::Error::other("source root disappeared"))?;
    let entry = entries
        .record(root_id)
        .ok_or_else(|| io::Error::other("source entry absent"))?;
    if root.record.owner_uid != peer.uid
        || entry.owner_uid != peer.uid
        || entry.prepared_driver.as_ref() != Some(&ProcessStamp::from(&peer.process))
        || entry.domain_id.as_deref() != Some(&owner.domain_id)
        || entry.supervisor_authority_id.as_deref() != Some(&owner.supervisor_authority_id)
        || owner.driver_identity.pid != i64::from(peer.process.host_pid)
    {
        return Err(io::Error::other("source owner/driver mismatch"));
    }
    let guardian = PinnedProcess::open(
        entry
            .guardian
            .as_ref()
            .ok_or_else(|| io::Error::other("guardian absent"))?
            .host_pid,
    )?;
    if entry.joined_child.is_none() {
        return Err(io::Error::other("joined child witness absent"));
    }
    let material = sidecar
        .read_reserved_source_material(sidecar.source_generation(), root_id, owner)
        .map_err(io::Error::other)?;
    let grant_id = material.grant.grant_id.clone();
    let mut candidate =
        SourcePinnedCandidate::pin(&material, peer.uid).map_err(io::Error::other)?;
    candidate.verify_at_use().map_err(io::Error::other)?;
    let (stdout_file, stderr_file) = physical.prepare_outputs(&grant_id)?;
    let confirmation_path = Path::new(&candidate.source.handle_dir)
        .join(format!("{grant_id}.registration-confirmation-v2.json"));
    let placeholder = UnixStream::pair()?;
    let context = InitContext {
        grant_id: grant_id.clone(),
        directory: directory.into(),
        image: candidate.image.try_clone()?,
        environment: candidate.environment.clone(),
        registration_path: candidate.registration_path.clone(),
        confirmation_path: confirmation_path.clone(),
        stdout_file,
        stderr_file,
        control: placeholder.0,
        gate: placeholder.1,
        owner_uid: peer.uid,
        owner_gid: peer.gid,
        groups: peer.process.supplementary_groups()?,
    };
    let (pid, mut control, mut gate) = create_init(root.init.namespace(), context)?;
    let pid1 = PinnedProcess::open(pid)?;
    if !pid1.is_namespace_init()? || !pid1.direct_child_of(&root.init)? {
        return Err(io::Error::other("source PID1 lineage changed"));
    }
    control.write_all(b"P")?;
    let credentials = child_credential(&control, b'C')?;
    let worker = PinnedProcess::open(credentials.pid)?;
    if credentials.uid != peer.uid
        || credentials.gid != peer.gid
        || !worker.direct_child_of(&pid1)?
        || !worker.in_namespace(pid1.namespace())?
    {
        return Err(io::Error::other("source held worker lineage changed"));
    }
    candidate.verify_at_use().map_err(io::Error::other)?;
    let consumed = sidecar
        .consume_reserved_source_effect_grant(&material, owner)
        .map_err(io::Error::other)?;
    let record = physical.insert_held(
        consumed,
        &root.record,
        entry,
        &root.init,
        &guardian,
        &peer.process,
        &pid1,
        &worker,
        worker.namespace_pid()?,
    )?;
    candidate.verify_at_use().map_err(io::Error::other)?;
    write_confirmation(
        &candidate,
        &confirmation_path,
        &material.grant.candidate.registration_digest,
        peer.uid,
    )?;
    candidate.verify_at_use().map_err(io::Error::other)?;
    gate.write_all(b"R")?;
    let mut executed = [0];
    control.read_exact(&mut executed)?;
    if executed != [b'E'] {
        return Err(io::Error::other("source exec acknowledgement absent"));
    }
    Ok(format!(
        "source-held {} {} {}\n",
        record.grant.grant_id, pid1.host_pid, worker.host_pid
    ))
}
