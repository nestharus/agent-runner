//! One-use accepted-work placement. The broker owns the irreversible grant
//! transition, the nested PID1 and the worker's pre-exec gate. The PID1 writes
//! a terminal receipt only after it has reaped every adopted descendant.
use oulipoly_kernel_broker::accepted_grant::{GrantRecord, GrantRegistry};
use oulipoly_kernel_broker::entry_registry::{EntryRegistry, ProcessStamp};
use oulipoly_kernel_broker::identity::{PeerIdentity, PinnedProcess, observed_incarnation_gone};
use oulipoly_kernel_broker::registry::RootRegistry;
use oulipoly_kernel_broker::work_registry::WorkRegistry;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const EXECUTOR_ARG: &str = "__root-original-work-v1";
static CANCEL_REQUESTED: AtomicBool = AtomicBool::new(false);

extern "C" fn request_cancel(_: libc::c_int) {
    CANCEL_REQUESTED.store(true, Ordering::Relaxed);
}

/// PID1 is inside the accepted work namespace. Linux resolves kill(-1) in
/// the caller's PID namespace, including child namespaces but excluding its
/// parent and siblings. Repeating this during reaping catches late forks and
/// adopted setsid children without process-group or host-PID guesses.
fn signal_work_members(signal: libc::c_int) -> io::Result<()> {
    if unsafe { libc::kill(-1, signal) } != 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(error);
        }
    }
    Ok(())
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TerminalReceipt {
    version: u32,
    work_incarnation: String,
    init_host_pid: i32,
    worker_local_pid: i32,
    worker_wait_status: i32,
    physical_tree_drained: bool,
}

struct InitContext {
    descriptors: [File; 7],
    control: UnixStream,
    gate: UnixStream,
    terminal_dir: File,
    owner_uid: u32,
    owner_gid: u32,
    groups: Vec<libc::gid_t>,
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

fn write_terminal(directory: &File, receipt: &TerminalReceipt) -> io::Result<()> {
    let name = format!("{}.json", receipt.work_incarnation);
    let name = std::ffi::CString::new(name).map_err(|_| io::Error::other("invalid work ID"))?;
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0o600,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut file = unsafe { File::from_raw_fd(fd) };
    serde_json::to_writer(&mut file, receipt)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    directory.sync_all()
}

use std::os::fd::FromRawFd;

fn run_init(context: InitContext) -> io::Result<()> {
    let InitContext {
        descriptors:
            [
                image,
                intent,
                cwd,
                state_dir,
                _accepted,
                control_for_worker,
                capability,
            ],
        mut control,
        gate,
        terminal_dir,
        owner_uid,
        owner_gid,
        groups,
    } = context;
    close_other_descriptors(&[
        image.as_raw_fd(),
        intent.as_raw_fd(),
        cwd.as_raw_fd(),
        state_dir.as_raw_fd(),
        _accepted.as_raw_fd(),
        control_for_worker.as_raw_fd(),
        capability.as_raw_fd(),
        control.as_raw_fd(),
        gate.as_raw_fd(),
        terminal_dir.as_raw_fd(),
    ])?;
    if unsafe { libc::getpid() } != 1
        || unsafe { libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) } != 0
    {
        return Err(io::Error::other("work PID1 lost host sudo semantics"));
    }
    CANCEL_REQUESTED.store(false, Ordering::Relaxed);
    if unsafe { libc::signal(libc::SIGUSR1, request_cancel as libc::sighandler_t) } == libc::SIG_ERR
    {
        return Err(io::Error::last_os_error());
    }
    control.write_all(b"I")?;
    let mut release = [0u8; 41];
    control.read_exact(&mut release)?;
    if release[0] != b'P' {
        return Err(io::Error::other("work persistence gate refused"));
    }
    let incarnation = std::str::from_utf8(&release[1..37])
        .map_err(|_| io::Error::other("invalid work incarnation"))?
        .to_owned();
    uuid::Uuid::parse_str(&incarnation)
        .map_err(|_| io::Error::other("invalid work incarnation"))?;
    let init_host_pid = i32::from_ne_bytes(release[37..41].try_into().unwrap());
    if unsafe { libc::fchdir(cwd.as_raw_fd()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let image_path = format!("/proc/self/fd/{}", image.as_raw_fd());
    let mut command = Command::new(image_path);
    command
        .arg(EXECUTOR_ARG)
        .arg(intent.as_raw_fd().to_string())
        .arg(cwd.as_raw_fd().to_string())
        .arg(state_dir.as_raw_fd().to_string())
        .arg(control_for_worker.as_raw_fd().to_string())
        .arg(capability.as_raw_fd().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let worker_fds = [
        intent.as_raw_fd(),
        cwd.as_raw_fd(),
        state_dir.as_raw_fd(),
        control_for_worker.as_raw_fd(),
        capability.as_raw_fd(),
    ];
    let control_fd = control.as_raw_fd();
    let gate_fd = gate.as_raw_fd();
    let fixture = super::private_fixture();
    unsafe {
        command.pre_exec(move || {
            if libc::setsid() < 0
                || (!fixture && libc::setgroups(groups.len(), groups.as_ptr()) != 0)
                || libc::setresgid(owner_gid, owner_gid, owner_gid) != 0
                || libc::setresuid(owner_uid, owner_uid, owner_uid) != 0
                || libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) != 0
            {
                return Err(io::Error::last_os_error());
            }
            for fd in worker_fds {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            if libc::send(control_fd, b"C".as_ptr().cast(), 1, libc::MSG_NOSIGNAL) != 1 {
                return Err(io::Error::last_os_error());
            }
            let mut byte = 0u8;
            if libc::read(gate_fd, (&mut byte as *mut u8).cast(), 1) != 1 || byte != b'R' {
                return Err(io::Error::other("worker pre-exec gate refused"));
            }
            Ok(())
        });
    }
    let worker = command.spawn()?;
    // Rust's spawn handshake returns only after a successful execve. A lost
    // broker must not stop PID1 from reaping and recording this work.
    let _ = control.write_all(b"E");
    let worker_local_pid = worker.id() as i32;
    drop(gate);
    drop(control);
    drop(image);
    let mut worker_wait_status = None;
    let mut cancellation_started = None;
    loop {
        if CANCEL_REQUESTED.load(Ordering::Relaxed) {
            let started = *cancellation_started.get_or_insert_with(Instant::now);
            let signal = if started.elapsed() >= Duration::from_secs(2) {
                libc::SIGKILL
            } else {
                libc::SIGTERM
            };
            signal_work_members(signal)?;
        }
        let mut status = 0;
        let reaped = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
        if reaped == worker.id() as i32 {
            worker_wait_status = Some(status);
        }
        if reaped > 0 {
            continue;
        }
        if reaped == 0 {
            std::thread::sleep(Duration::from_millis(20));
            continue;
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        if error.raw_os_error() != Some(libc::ECHILD) {
            return Err(error);
        }
        break;
    }
    write_terminal(
        &terminal_dir,
        &TerminalReceipt {
            version: 1,
            work_incarnation: incarnation,
            init_host_pid,
            worker_local_pid,
            worker_wait_status: worker_wait_status
                .ok_or_else(|| io::Error::other("worker wait absent"))?,
            physical_tree_drained: true,
        },
    )
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
        return Err(io::Error::other("child never reached identity gate"));
    }
    let header = unsafe { libc::CMSG_FIRSTHDR(&msg) };
    if header.is_null() || unsafe { (*header).cmsg_type } != libc::SCM_CREDENTIALS {
        return Err(io::Error::other("worker credentials absent"));
    }
    Ok(unsafe { *(libc::CMSG_DATA(header) as *const libc::ucred) })
}

fn validate_auxiliary(control: &File, capability: &File, peer: &PeerIdentity) -> io::Result<()> {
    if !control.metadata()?.file_type().is_socket() || !capability.metadata()?.file_type().is_fifo()
    {
        return Err(io::Error::other(
            "invalid worker control/capability descriptors",
        ));
    }
    let mut origin: libc::ucred = unsafe { std::mem::zeroed() };
    let mut length = std::mem::size_of_val(&origin) as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            control.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut origin as *mut libc::ucred).cast(),
            &mut length,
        )
    } != 0
        || length as usize != std::mem::size_of_val(&origin)
        || (origin.pid, origin.uid, origin.gid) != (peer.process.host_pid, peer.uid, peer.gid)
    {
        return Err(io::Error::other(
            "worker control was not created by guardian",
        ));
    }
    Ok(())
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
        // The setns call changes pid_for_children only. Fork once to become
        // an actual member of the causal parent before CLONE_NEWPID.
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
    // The parent must not retain PID1's socket ends. If either helper or
    // clone fails, recvmsg must see EOF and leave consumed grant debt.
    drop(context);
    let mut status = 0;
    let waited = unsafe { libc::waitpid(pid, &mut status, 0) };
    if waited != pid || !libc::WIFEXITED(status) || libc::WEXITSTATUS(status) != 0 {
        return Err(io::Error::other("work namespace helper failed"));
    }
    let credential = child_credential(&broker_control, b'I')?;
    if credential.uid != 0 || credential.pid <= 0 {
        return Err(io::Error::other("work PID1 identity refused"));
    }
    Ok((credential.pid, broker_control, broker_gate))
}

#[expect(
    clippy::too_many_arguments,
    reason = "grant, registry, and descriptor authorities are independent"
)]
pub(super) fn launch(
    grant_id: &str,
    descriptors: [File; 7],
    peer: &PeerIdentity,
    host_namespace: &File,
    runner_image: &File,
    roots: &RootRegistry,
    works: &mut WorkRegistry,
    entries: &EntryRegistry,
    grants: &mut GrantRegistry,
    terminal_dir: &Path,
) -> io::Result<String> {
    uuid::Uuid::parse_str(grant_id).map_err(|_| io::Error::other("invalid grant ID"))?;
    let [
        image,
        intent,
        cwd,
        state_dir,
        accepted,
        worker_control,
        capability,
    ] = &descriptors;
    let grant =
        grants.validate_launch_artifacts(grant_id, image, intent, cwd, state_dir, accepted)?;
    validate_auxiliary(worker_control, capability, peer)?;
    if !peer.process.same_executable_as(runner_image)?
        || !peer.process.in_namespace(host_namespace)?
        || unsafe { libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) } != 0
    {
        return Err(io::Error::other(
            "work launch lost host guardian or sudo semantics",
        ));
    }
    let source_pid = i32::try_from(grant.initiator.pid)
        .map_err(|_| io::Error::other("invalid accepted source PID"))?;
    let source = PinnedProcess::open(source_pid)?;
    if source.boot_id != grant.initiator.boot_id
        || source.starttime_ticks
            != u64::try_from(grant.initiator.starttime_ticks)
                .map_err(|_| io::Error::other("invalid accepted source starttime"))?
        || !source.same_executable_as(image)?
    {
        return Err(io::Error::other(
            "accepted source image changed before launch",
        ));
    }
    let parent_namespace = if let Some(parent) = &grant.parent_work_incarnation {
        works
            .live_works()
            .find(|work| {
                work.record.work_incarnation == *parent && work.record.root_id == grant.root_id
            })
            .ok_or_else(|| io::Error::other("causal parent work absent"))?
            .init
            .namespace()
            .try_clone()?
    } else {
        roots
            .live_roots()
            .find(|root| root.record.root_id == grant.root_id)
            .ok_or_else(|| io::Error::other("root namespace absent"))?
            .init
            .namespace()
            .try_clone()?
    };
    // No fork or worker side effect can precede the one-use fsynced consume.
    let consumed: GrantRecord = grants.consume(
        grant_id,
        roots,
        entries,
        works,
        peer,
        host_namespace,
        runner_image,
    )?;
    let placeholder = UnixStream::pair()?;
    let context = InitContext {
        descriptors,
        control: placeholder.0,
        gate: placeholder.1,
        terminal_dir: File::open(terminal_dir)?,
        owner_uid: peer.uid,
        owner_gid: peer.gid,
        groups: peer.process.supplementary_groups()?,
    };
    let (init_pid, mut control, mut gate) = create_init(&parent_namespace, context)?;
    let init = PinnedProcess::open(init_pid)?;
    // A failed insert leaves consumed grant debt. PID1 is still held at its
    // persistence gate and will exit when its socket closes.
    let record = works.insert_prepared_granted(
        roots,
        &consumed.root_id,
        &consumed.work_id,
        &consumed.grant_id,
        consumed.parent_work_incarnation.as_deref(),
        init_pid,
    )?;
    let mut release = Vec::with_capacity(41);
    release.push(b'P');
    release.extend_from_slice(record.work_incarnation.as_bytes());
    release.extend_from_slice(&init_pid.to_ne_bytes());
    control.write_all(&release)?;
    let credentials = child_credential(&control, b'C')?;
    let worker = PinnedProcess::open(credentials.pid)?;
    if credentials.uid != peer.uid
        || credentials.gid != peer.gid
        || !worker.direct_child_of(&init)?
        || !worker.in_namespace(init.namespace())?
    {
        return Err(io::Error::other(
            "accepted worker pre-exec identity mismatch",
        ));
    }
    init.verify()?;
    peer.process.verify()?;
    gate.write_all(b"R")?;
    let mut executed = [0u8; 1];
    control.read_exact(&mut executed)?;
    if executed != [b'E'] {
        return Err(io::Error::other(
            "accepted worker exec was not acknowledged",
        ));
    }
    Ok(format!(
        "launched-work {} {} {}\n",
        record.work_incarnation, init_pid, worker.host_pid
    ))
}

pub(super) fn observe(
    grant_id: &str,
    peer: &PeerIdentity,
    host_namespace: &File,
    runner_image: &File,
    works: &WorkRegistry,
    grants: &GrantRegistry,
    terminal_dir: &Path,
) -> io::Result<String> {
    if !peer.process.in_namespace(host_namespace)?
        || !peer.process.same_executable_as(runner_image)?
    {
        return Err(io::Error::other("work observer is not host guardian"));
    }
    let grant = grants
        .records()
        .iter()
        .find(|grant| {
            grant.grant_id == grant_id
                && grant.consumed
                && grant.guardian == ProcessStamp::from(&peer.process)
                && grant.owner_uid == peer.uid
        })
        .ok_or_else(|| io::Error::other("work observation authority absent"))?;
    let record = works
        .live_works()
        .map(|work| &work.record)
        .chain(works.debt_records())
        .find(|record| {
            record.accepted_grant_id.as_deref() == Some(grant_id)
                && record.root_id == grant.root_id
                && record.work_id == grant.work_id
        });
    let Some(record) = record else {
        return Ok(format!(
            "work-uncertain {grant_id} no-durable-work-record\n"
        ));
    };
    let name = std::ffi::CString::new(format!("{}.json", record.work_incarnation))
        .map_err(|_| io::Error::other("invalid work incarnation"))?;
    let directory = File::open(terminal_dir)?;
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    let receipt = if fd < 0 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::NotFound {
            None
        } else {
            return Ok(format!(
                "work-uncertain {} receipt-read-failed\n",
                record.work_incarnation
            ));
        }
    } else {
        let file = unsafe { File::from_raw_fd(fd) };
        if !file.metadata()?.is_file() || file.metadata()?.len() > 4096 {
            return Ok(format!(
                "work-uncertain {} invalid-receipt-file\n",
                record.work_incarnation
            ));
        }
        match serde_json::from_reader::<_, TerminalReceipt>(file) {
            Ok(receipt) => Some(receipt),
            Err(_) => {
                return Ok(format!(
                    "work-uncertain {} invalid-receipt\n",
                    record.work_incarnation
                ));
            }
        }
    };
    let gone = if let Some(work) = works
        .live_works()
        .find(|work| work.record.work_incarnation == record.work_incarnation)
    {
        work.init.exited()?
    } else {
        observed_incarnation_gone(
            record.init_host_pid,
            &record.boot_id,
            record.init_starttime_ticks,
            (record.pidns_dev, record.pidns_ino),
        )?
    };
    let Some(receipt) = receipt else {
        return Ok(if gone {
            format!(
                "work-uncertain {} missing-terminal-receipt\n",
                record.work_incarnation
            )
        } else {
            format!("work-live {}\n", record.work_incarnation)
        });
    };
    if receipt.version != 1
        || receipt.work_incarnation != record.work_incarnation
        || receipt.init_host_pid != record.init_host_pid
        || receipt.worker_local_pid <= 1
        || !receipt.physical_tree_drained
    {
        return Ok(format!(
            "work-uncertain {} conflicting-terminal-receipt\n",
            record.work_incarnation
        ));
    }
    if !gone {
        return Ok(format!("work-drain-pending {}\n", record.work_incarnation));
    }
    Ok(format!(
        "work-drained {} {} {}\n",
        record.work_incarnation, receipt.worker_local_pid, receipt.worker_wait_status
    ))
}

/// A signal acknowledgement is never interpreted as source completion or
/// physical drain. Only the original bound guardian can request it; the PID1
/// continues reaping and writes the terminal receipt after ECHILD.
pub(super) fn cancel(
    grant_id: &str,
    peer: &PeerIdentity,
    host_namespace: &File,
    runner_image: &File,
    works: &WorkRegistry,
    grants: &GrantRegistry,
) -> io::Result<String> {
    if !peer.process.in_namespace(host_namespace)?
        || !peer.process.same_executable_as(runner_image)?
    {
        return Err(io::Error::other(
            "work cancellation is not from host guardian",
        ));
    }
    let grant = grants
        .records()
        .iter()
        .find(|grant| {
            grant.grant_id == grant_id
                && grant.consumed
                && grant.guardian == ProcessStamp::from(&peer.process)
                && grant.owner_uid == peer.uid
        })
        .ok_or_else(|| io::Error::other("work cancellation authority absent"))?;
    let work = works
        .live_works()
        .find(|work| {
            work.record.accepted_grant_id.as_deref() == Some(grant_id)
                && work.record.root_id == grant.root_id
                && work.record.work_id == grant.work_id
        })
        .ok_or_else(|| io::Error::other("live work namespace absent"))?;
    work.init.signal(libc::SIGUSR1)?;
    Ok(format!(
        "cancel-signalled {}\n",
        work.record.work_incarnation
    ))
}
