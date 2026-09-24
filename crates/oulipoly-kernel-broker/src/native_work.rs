//! Broker-owned, one-use native continuation K. Nothing below the worker's
//! execution gate may run until the exact retained State attach is committed.

const CANCELLATION_ESCALATION_DELAY: std::time::Duration = std::time::Duration::from_secs(2);
const PID1_REAP_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(20);
#[cfg(feature = "age319-private-broker-fixture")]
const PRIVATE_NATIVE_GATE_WAIT: std::time::Duration = std::time::Duration::from_secs(20);
#[cfg(feature = "age319-private-broker-fixture")]
const PRIVATE_NATIVE_GATE_POLL: std::time::Duration = std::time::Duration::from_millis(20);

const HASH_OUTPUT_BUFFER_BYTES: usize = 64 * 1024;
const CAPTURE_OUTPUT_BUFFER_BYTES: usize = 8192;
const LAUNCH_READ_BUFFER_BYTES: usize = 64 * 1024;

use super::work_launch;
use oulipoly_kernel_broker::accepted_grant::{GrantRegistry, NativeGrantRecord};
use oulipoly_kernel_broker::entry_registry::ProcessStamp;
use oulipoly_kernel_broker::identity::{PeerIdentity, PinnedProcess, observed_incarnation_gone};
use oulipoly_kernel_broker::registry::RootRegistry;
use oulipoly_kernel_broker::work_registry::WorkRegistry;
use oulipoly_state::completion_continuation::SourceProcessIdentity;
use oulipoly_state::mailbox::{
    BrokerNativeAttachEvidence, BrokerNativeKernelQEvidence, BrokerSidecar,
    NATIVE_KERNEL_Q_PROTOCOL, NATIVE_ROOT_WORKER_ENTRY, NATIVE_WORKER_ATTACH_PROTOCOL,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

static CANCEL: AtomicBool = AtomicBool::new(false);
extern "C" fn request_cancel(_: libc::c_int) {
    CANCEL.store(true, Ordering::Relaxed);
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeTerminal {
    version: u32,
    grant_id: String,
    attempt_id: String,
    work_incarnation: String,
    init_host_pid: i32,
    worker_local_pid: i32,
    worker_wait_status: i32,
    physical_tree_drained: bool,
    cancellation_observed: bool,
    output: Vec<OutputEvidence>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativePid1Wait {
    version: u32,
    grant_id: String,
    attempt_id: String,
    pid1_parent_namespace_pid: i32,
    wait_status: i32,
    reaped: bool,
}

fn write_pid1_wait(context: &InitContext, pid: i32, status: i32) -> io::Result<()> {
    let name = std::ffi::CString::new(format!("{}.native-pid1-wait.json", context.grant_id))
        .map_err(|_| io::Error::other("native PID1 wait filename"))?;
    let fd = unsafe {
        libc::openat(
            context.terminal_dir.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0o600,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut file = unsafe { File::from_raw_fd(fd) };
    serde_json::to_writer(
        &mut file,
        &NativePid1Wait {
            version: 1,
            grant_id: context.grant_id.clone(),
            attempt_id: context.attempt_id.clone(),
            pid1_parent_namespace_pid: pid,
            wait_status: status,
            reaped: true,
        },
    )?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    context.terminal_dir.sync_all()
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OutputEvidence {
    name: String,
    device: u64,
    inode: u64,
    bytes: u64,
    sha256: String,
}

fn hash_output(path: &Path, name: &str) -> io::Result<Option<OutputEvidence>> {
    let mut file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let before = file.metadata()?;
    if !before.is_file() {
        return Err(io::Error::other("native output is not regular"));
    }
    let mut hash = Sha256::new();
    let mut count = 0u64;
    let mut buffer = [0u8; HASH_OUTPUT_BUFFER_BYTES];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
        count = count
            .checked_add(read as u64)
            .ok_or_else(|| io::Error::other("native output length overflow"))?;
    }
    let after = file.metadata()?;
    if before.dev() != after.dev()
        || before.ino() != after.ino()
        || before.len() != after.len()
        || count != before.len()
    {
        return Err(io::Error::other("native output changed during PID1 read"));
    }
    Ok(Some(OutputEvidence {
        name: name.into(),
        device: before.dev(),
        inode: before.ino(),
        bytes: count,
        sha256: format!("{:x}", hash.finalize()),
    }))
}

fn capture_output(request: &File) -> io::Result<Vec<OutputEvidence>> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; CAPTURE_OUTPUT_BUFFER_BYTES];
    let mut offset = 0;
    loop {
        let read = request.read_at(&mut buffer, offset)?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
        offset += read as u64;
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    let result = value
        .get("attempt")
        .and_then(|attempt| attempt.get("result_path"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| io::Error::other("native output result path absent"))?;
    let result = Path::new(result);
    if !result.is_absolute() {
        return Err(io::Error::other("native output path is relative"));
    }
    let directory = result
        .parent()
        .ok_or_else(|| io::Error::other("native output directory absent"))?;
    let mut output = Vec::new();
    if let Some(receipt) = hash_output(result, "result")? {
        output.push(receipt);
    }
    for name in [
        "launcher.stdout",
        "launcher.stderr",
        "stdout.json",
        "stderr.log",
    ] {
        if let Some(receipt) = hash_output(&directory.join(name), name)? {
            output.push(receipt);
        }
    }
    Ok(output)
}

struct InitContext {
    image: File,
    request: File,
    terminal_dir: File,
    control: UnixStream,
    gate: UnixStream,
    uid: u32,
    gid: u32,
    groups: Vec<libc::gid_t>,
    grant_id: String,
    attempt_id: String,
}

extern "C" fn init_start(pointer: *mut libc::c_void) -> libc::c_int {
    let context = unsafe { Box::from_raw(pointer.cast::<InitContext>()) };
    if run_init(*context).is_ok() { 0 } else { 70 }
}

fn run_init(mut context: InitContext) -> io::Result<()> {
    work_launch::close_other_descriptors(&[
        context.image.as_raw_fd(),
        context.request.as_raw_fd(),
        context.terminal_dir.as_raw_fd(),
        context.control.as_raw_fd(),
        context.gate.as_raw_fd(),
    ])?;
    if unsafe { libc::getpid() } != 1
        || unsafe { libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) } != 0
        || unsafe { libc::prctl(libc::PR_GET_SECCOMP, 0, 0, 0, 0) } != 0
    {
        return Err(io::Error::other("native PID1 lost host sudo semantics"));
    }
    CANCEL.store(false, Ordering::Relaxed);
    if unsafe { libc::signal(libc::SIGUSR1, request_cancel as libc::sighandler_t) } == libc::SIG_ERR
    {
        return Err(io::Error::last_os_error());
    }
    context.control.write_all(b"I")?;
    let mut release = [0u8; 41];
    context.control.read_exact(&mut release)?;
    if release[0] != b'P' {
        return Err(io::Error::other("native PID1 persistence gate refused"));
    }
    let incarnation = std::str::from_utf8(&release[1..37])
        .map_err(|_| io::Error::other("native work incarnation encoding"))?
        .to_owned();
    uuid::Uuid::parse_str(&incarnation).map_err(|_| io::Error::other("native work incarnation"))?;
    let host_pid = i32::from_ne_bytes(release[37..41].try_into().unwrap());
    let (mut cancel_for_init, cancel_for_worker) = UnixStream::pair()?;
    let image_path = format!("/proc/self/fd/{}", context.image.as_raw_fd());
    let mut command = Command::new(image_path);
    command
        .arg(NATIVE_ROOT_WORKER_ENTRY)
        .arg(context.gate.as_raw_fd().to_string())
        .arg(context.request.as_raw_fd().to_string())
        .arg(cancel_for_worker.as_raw_fd().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let gate_fd = context.gate.as_raw_fd();
    let request_fd = context.request.as_raw_fd();
    let cancel_fd = cancel_for_worker.as_raw_fd();
    let control_fd = context.control.as_raw_fd();
    let uid = context.uid;
    let gid = context.gid;
    let groups = context.groups;
    let fixture = super::private_fixture();
    unsafe {
        command.pre_exec(move || {
            if libc::setsid() < 0
                || (!fixture && libc::setgroups(groups.len(), groups.as_ptr()) != 0)
                || libc::setresgid(gid, gid, gid) != 0
                || libc::setresuid(uid, uid, uid) != 0
            {
                return Err(io::Error::last_os_error());
            }
            if libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) != 0
                || libc::prctl(libc::PR_GET_SECCOMP, 0, 0, 0, 0) != 0
            {
                return Err(io::Error::other(
                    "native worker inherited NNP or seccomp restriction",
                ));
            }
            for fd in [gate_fd, request_fd, cancel_fd] {
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
                return Err(io::Error::other("native worker pre-exec gate refused"));
            }
            Ok(())
        });
    }
    let worker = command.spawn()?;
    let worker_local_pid = worker.id() as i32;
    drop(context.gate); // Broker death now produces EOF at the worker's execution gate.
    drop(cancel_for_worker);
    let _ = context.control.write_all(b"E");
    drop(context.control);
    let mut worker_wait = None;
    let mut cancellation_started = None;
    loop {
        if CANCEL.load(Ordering::Relaxed) {
            let started = *cancellation_started.get_or_insert_with(Instant::now);
            let signal = if started.elapsed() >= CANCELLATION_ESCALATION_DELAY {
                libc::SIGKILL
            } else {
                libc::SIGTERM
            };
            work_launch::signal_work_members(signal)?;
            let _ = cancel_for_init.write_all(b"native_pid1_cancel\n");
        }
        let mut status = 0;
        let reaped = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
        if reaped == worker_local_pid {
            worker_wait = Some(status);
        }
        if reaped > 0 {
            continue;
        }
        if reaped == 0 {
            std::thread::sleep(PID1_REAP_POLL_INTERVAL);
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
    let receipt = NativeTerminal {
        version: 1,
        grant_id: context.grant_id,
        attempt_id: context.attempt_id,
        work_incarnation: incarnation.clone(),
        init_host_pid: host_pid,
        worker_local_pid,
        worker_wait_status: worker_wait
            .ok_or_else(|| io::Error::other("native worker wait absent"))?,
        physical_tree_drained: true,
        cancellation_observed: cancellation_started.is_some(),
        output: capture_output(&context.request)?,
    };
    let name = std::ffi::CString::new(format!("{incarnation}.native.json"))
        .map_err(|_| io::Error::other("native terminal name"))?;
    let fd = unsafe {
        libc::openat(
            context.terminal_dir.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0o600,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut file = unsafe { File::from_raw_fd(fd) };
    serde_json::to_writer(&mut file, &receipt)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    context.terminal_dir.sync_all()
}

fn create_init(
    parent_namespace: &File,
    mut context: InitContext,
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
        let child = unsafe {
            libc::clone(
                init_start,
                top.cast(),
                libc::CLONE_NEWPID | libc::SIGCHLD,
                pointer.cast(),
            )
        };
        let reaper_context = unsafe { Box::from_raw(pointer) };
        if child < 0 {
            unsafe { libc::_exit(70) };
        }
        // This persistent parent outlives a broker crash. Keeping the
        // broker's inherited listener would make a lost control socket appear
        // connectable while no process can accept requests on it.
        if work_launch::close_other_descriptors(&[reaper_context.terminal_dir.as_raw_fd()]).is_err()
        {
            unsafe { libc::_exit(70) };
        }
        // The intermediate process remains the actual parent of nested
        // PID1 across broker restart. Only its waitpid result can fill Q's
        // PID1 status; a /proc disappearance alone is insufficient.
        let mut status = 0;
        let waited = unsafe { libc::waitpid(child, &mut status, 0) };
        let recorded = waited == child && write_pid1_wait(&reaper_context, child, status).is_ok();
        unsafe { libc::_exit(if recorded { 0 } else { 70 }) };
    }
    drop(context);
    let mut status = 0;
    if unsafe { libc::waitpid(pid, &mut status, 0) } != pid
        || !libc::WIFEXITED(status)
        || libc::WEXITSTATUS(status) != 0
    {
        return Err(io::Error::other("native namespace helper failed"));
    }
    let credential = work_launch::child_credential(&broker_control, b'I')?;
    if credential.uid != 0 || credential.pid <= 0 {
        return Err(io::Error::other("native PID1 identity refused"));
    }
    Ok((credential.pid, broker_control, broker_gate))
}

fn sealed_request(request: &File, expected_digest: &str) -> io::Result<File> {
    let before = request.metadata()?;
    if !before.is_file() || before.len() > 4 * 1024 * 1024 {
        return Err(io::Error::other("native request changed before K"));
    }
    let mut bytes = vec![0; before.len() as usize];
    request.read_exact_at(&mut bytes, 0)?;
    let after = request.metadata()?;
    if before.len() != after.len()
        || before.dev() != after.dev()
        || before.ino() != after.ino()
        || format!("{:x}", Sha256::digest(&bytes)) != expected_digest
    {
        return Err(io::Error::other("native request bytes changed before K"));
    }
    let name = c"oulipoly-native-request";
    let fd =
        unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_ALLOW_SEALING | libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut file = unsafe { File::from_raw_fd(fd) };
    file.write_all(&bytes)?;
    file.flush()?;
    let seals = libc::F_SEAL_SEAL | libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_WRITE;
    if unsafe { libc::fcntl(fd, libc::F_ADD_SEALS, seals) } < 0 {
        return Err(io::Error::last_os_error());
    }
    file.seek(SeekFrom::Start(0))?;
    Ok(file)
}

fn identity(process: &PinnedProcess) -> SourceProcessIdentity {
    SourceProcessIdentity {
        pid: i64::from(process.host_pid),
        boot_id: process.boot_id.clone(),
        starttime_ticks: process.starttime_ticks as i64,
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "separate broker grant, process, and State authorities"
)]
pub(super) fn launch(
    verified: &NativeGrantRecord,
    request: File,
    peer: &PeerIdentity,
    runner_image: &File,
    roots: &RootRegistry,
    works: &mut WorkRegistry,
    grants: &mut GrantRegistry,
    sidecar: &mut BrokerSidecar,
    broker_incarnation: &str,
    terminal_dir: &Path,
) -> io::Result<String> {
    // Neither a nested PID namespace nor exec can clear inherited filters.
    // Reject before consuming the grant; PID1 and the held child recheck.
    if unsafe { libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) } != 0
        || unsafe { libc::prctl(libc::PR_GET_SECCOMP, 0, 0, 0, 0) } != 0
    {
        return Err(io::Error::other(
            "native launch inherited NNP or seccomp restriction",
        ));
    }
    // The request is copied from the exact descriptor verified by t into a
    // sealed memfd. Its named inode can no longer change the worker's recipe.
    let request = sealed_request(&request, &verified.custodian_request_sha256)?;
    let root = roots
        .live_roots()
        .find(|root| root.record.root_id == verified.root_id)
        .ok_or_else(|| io::Error::other("native root absent"))?;
    root.init.verify()?;
    peer.process.verify()?;
    let namespace = root.init.namespace().try_clone()?;
    let image = runner_image.try_clone()?;
    let digest = {
        let mut hash = Sha256::new();
        let mut buffer = [0u8; LAUNCH_READ_BUFFER_BYTES];
        let mut offset = 0;
        loop {
            let count = image.read_at(&mut buffer, offset)?;
            if count == 0 {
                break;
            }
            hash.update(&buffer[..count]);
            offset += count as u64;
        }
        format!("{:x}", hash.finalize())
    };
    // This fsynced spend is the point of no retry. Every following failure is
    // unknown physical debt, never permission to run another worker.
    let spent = grants.consume_native_v30(verified)?;
    let placeholder = UnixStream::pair()?;
    let context = InitContext {
        image,
        request,
        terminal_dir: File::open(terminal_dir)?,
        control: placeholder.0,
        gate: placeholder.1,
        uid: peer.uid,
        gid: peer.gid,
        groups: peer.process.supplementary_groups()?,
        grant_id: spent.grant_id.clone(),
        attempt_id: spent.attempt_id.clone(),
    };
    let (init_pid, mut control, mut gate) = create_init(&namespace, context)?;
    let init = PinnedProcess::open(init_pid)?;
    let record = works.insert_prepared_granted(
        roots,
        &spent.root_id,
        &spent.attempt_id,
        &spent.grant_id,
        None,
        init_pid,
    )?;
    let mut persisted = Vec::with_capacity(41);
    persisted.push(b'P');
    persisted.extend_from_slice(record.work_incarnation.as_bytes());
    persisted.extend_from_slice(&init_pid.to_ne_bytes());
    control.write_all(&persisted)?;
    let credential = work_launch::child_credential(&control, b'C')?;
    let worker = PinnedProcess::open(credential.pid)?;
    if credential.uid != peer.uid
        || credential.gid != peer.gid
        || !worker.direct_child_of(&init)?
        || !worker.in_namespace(init.namespace())?
    {
        return Err(io::Error::other("native held worker identity mismatch"));
    }
    init.verify()?;
    worker.verify()?;
    let attach_digest = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(
            &spent.grant_id,
            &spent.attempt_id,
            &record.work_incarnation,
            init_pid,
            credential.pid,
            broker_incarnation,
            &digest,
        ))?)
    );
    let evidence = BrokerNativeAttachEvidence {
        protocol: NATIVE_WORKER_ATTACH_PROTOCOL.into(),
        gate_held_before_release: true,
        attempt_id: spent.attempt_id.clone(),
        grant_id: spent.grant_id.clone(),
        kernel_root_id: spent.root_id.clone(),
        work_id: spent.attempt_id.clone(),
        work_incarnation_id: record.work_incarnation.clone(),
        broker_incarnation_id: broker_incarnation.into(),
        worker_entrypoint: NATIVE_ROOT_WORKER_ENTRY.into(),
        runner_image_sha256: digest,
        worker_identity: identity(&worker),
        pid1_identity: identity(&init),
        work_pid_namespace_inode: i64::try_from(record.pidns_ino)
            .map_err(|_| io::Error::other("native namespace inode overflow"))?,
        attach_receipt_sha256: attach_digest,
    };
    sidecar
        .attach_exact_native_worker_v30(
            spent
                .source_generation
                .as_deref()
                .ok_or_else(|| io::Error::other("native source generation absent"))?,
            &spent.attempt_id,
            &spent.grant_id,
            &spent.root_id,
            &evidence,
        )
        .map_err(io::Error::other)?;
    #[cfg(feature = "age319-private-broker-fixture")]
    if super::private_fixture()
        && let Some(directory) = std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_NATIVE_GATE_V1")
    {
        let directory = Path::new(&directory);
        std::fs::write(directory.join("native-attached"), spent.grant_id.as_bytes())?;
        let deadline = Instant::now() + PRIVATE_NATIVE_GATE_WAIT;
        while !directory.join("native-release").exists() {
            if Instant::now() >= deadline {
                return Err(io::Error::other("private native execution gate expired"));
            }
            std::thread::sleep(PRIVATE_NATIVE_GATE_POLL);
        }
    }
    // R lets pre-exec finish; the worker's own gate remains held until E.
    gate.write_all(b"R")?;
    let mut executed = [0u8; 1];
    control.read_exact(&mut executed)?;
    if executed != [b'E'] {
        return Err(io::Error::other("native worker exec unacknowledged"));
    }
    gate.write_all(&[1])?;
    Ok(format!(
        "launched-native-v30 {} {} {}\n",
        record.work_incarnation, init_pid, worker.host_pid
    ))
}

fn bound_work<'a>(
    grant_id: &str,
    peer: &PeerIdentity,
    host_namespace: &File,
    runner_image: &File,
    grants: &'a GrantRegistry,
    works: &'a WorkRegistry,
    sidecar: &BrokerSidecar,
) -> io::Result<(
    &'a NativeGrantRecord,
    &'a oulipoly_kernel_broker::work_registry::WorkRecord,
)> {
    if !peer.process.in_namespace(host_namespace)?
        || !peer.process.same_executable_as(runner_image)?
    {
        return Err(io::Error::other(
            "native observer is not installed host Runner",
        ));
    }
    let grant = grants
        .native_records()
        .iter()
        .find(|grant| {
            grant.grant_id == grant_id
                && grant.version == 5
                && grant.state == "consumed"
                && grant.guardian == ProcessStamp::from(&peer.process)
                && grant.owner_uid == peer.uid
                && grant.source_generation.as_deref() == Some(sidecar.source_generation())
        })
        .ok_or_else(|| io::Error::other("native consumed grant authority absent"))?;
    let work = works
        .live_works()
        .map(|work| &work.record)
        .chain(works.debt_records())
        .find(|work| {
            work.accepted_grant_id.as_deref() == Some(grant_id)
                && work.root_id == grant.root_id
                && work.work_id == grant.attempt_id
        })
        .ok_or_else(|| io::Error::other("native K spent without durable work record"))?;
    let attached = sidecar
        .read_native_worker_attach_v30(
            grant.source_generation.as_deref().unwrap(),
            &grant.attempt_id,
            grant_id,
        )
        .map_err(io::Error::other)?;
    if attached.as_ref().is_none_or(|row| {
        row.evidence.work_incarnation_id != work.work_incarnation
            || row.evidence.pid1_identity.pid != i64::from(work.init_host_pid)
            || row.evidence.pid1_identity.starttime_ticks != work.init_starttime_ticks as i64
    }) {
        return Err(io::Error::other(
            "native held State attach absent or changed",
        ));
    }
    Ok((grant, work))
}

#[expect(
    clippy::too_many_arguments,
    reason = "independent broker and State evidence"
)]
pub(super) fn observe(
    grant_id: &str,
    peer: &PeerIdentity,
    host_namespace: &File,
    runner_image: &File,
    grants: &GrantRegistry,
    works: &WorkRegistry,
    sidecar: &mut BrokerSidecar,
    broker_incarnation: &str,
    terminal_dir: &Path,
) -> io::Result<String> {
    let (grant, work) = bound_work(
        grant_id,
        peer,
        host_namespace,
        runner_image,
        grants,
        works,
        sidecar,
    )?;
    settle_ready(grant, work, sidecar, broker_incarnation, terminal_dir)
}

fn settle_ready(
    grant: &NativeGrantRecord,
    work: &oulipoly_kernel_broker::work_registry::WorkRecord,
    sidecar: &mut BrokerSidecar,
    broker_incarnation: &str,
    terminal_dir: &Path,
) -> io::Result<String> {
    let path = terminal_dir.join(format!("{}.native.json", work.work_incarnation));
    let (receipt, terminal_sha256) = match File::open(&path) {
        Ok(mut file) => {
            if !file.metadata()?.is_file() || file.metadata()?.len() > 4096 {
                return Ok(format!(
                    "native-unknown {} invalid-terminal-file\n",
                    work.work_incarnation
                ));
            }
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            match serde_json::from_slice::<NativeTerminal>(&bytes) {
                Ok(receipt) => (receipt, format!("{:x}", Sha256::digest(&bytes))),
                Err(_) => {
                    return Ok(format!(
                        "native-unknown {} invalid-terminal\n",
                        work.work_incarnation
                    ));
                }
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let gone = observed_incarnation_gone(
                work.init_host_pid,
                &work.boot_id,
                work.init_starttime_ticks,
                (work.pidns_dev, work.pidns_ino),
            )?;
            return Ok(format!(
                "{} {}\n",
                if gone {
                    "native-unknown terminal-absent-after-PID1-loss"
                } else {
                    "native-terminal-pending"
                },
                work.work_incarnation
            ));
        }
        Err(error) => return Err(error),
    };
    if receipt.version != 1
        || receipt.grant_id != grant.grant_id
        || receipt.attempt_id != grant.attempt_id
        || receipt.work_incarnation != work.work_incarnation
        || receipt.init_host_pid != work.init_host_pid
        || receipt.worker_local_pid <= 1
        || !receipt.physical_tree_drained
    {
        return Ok(format!(
            "native-unknown {} terminal-conflict\n",
            work.work_incarnation
        ));
    }
    if receipt.cancellation_observed {
        let cancel_path =
            terminal_dir.join(format!("{}.native.cancel.json", work.work_incarnation));
        let cancel: serde_json::Value = match File::open(cancel_path)
            .and_then(|file| serde_json::from_reader(file).map_err(io::Error::other))
        {
            Ok(value) => value,
            Err(_) => {
                return Ok(format!(
                    "native-unknown {} cancellation-intent-absent\n",
                    work.work_incarnation
                ));
            }
        };
        if cancel["protocol"] != "native-cancel-intent-v1"
            || cancel["grant_id"] != grant.grant_id
            || cancel["attempt_id"] != grant.attempt_id
            || cancel["work_incarnation"] != work.work_incarnation
            || cancel["pid1_host_pid"] != work.init_host_pid
            || cancel["pid1_starttime_ticks"] != work.init_starttime_ticks
        {
            return Ok(format!(
                "native-unknown {} cancellation-intent-conflict\n",
                work.work_incarnation
            ));
        }
    }
    let gone = observed_incarnation_gone(
        work.init_host_pid,
        &work.boot_id,
        work.init_starttime_ticks,
        (work.pidns_dev, work.pidns_ino),
    )?;
    if !gone {
        return Ok(format!(
            "native-terminal-pending {} PID1-live\n",
            work.work_incarnation
        ));
    }
    let wait_path = terminal_dir.join(format!("{}.native-pid1-wait.json", grant.grant_id));
    let mut wait_file = match File::open(wait_path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(format!(
                "native-q-pending {} PID1-wait-absent\n",
                work.work_incarnation
            ));
        }
        Err(error) => return Err(error),
    };
    if !wait_file.metadata()?.is_file() || wait_file.metadata()?.len() > 4096 {
        return Ok(format!(
            "native-unknown {} invalid-PID1-wait\n",
            work.work_incarnation
        ));
    }
    let mut wait_bytes = Vec::new();
    wait_file.read_to_end(&mut wait_bytes)?;
    let wait: NativePid1Wait = match serde_json::from_slice(&wait_bytes) {
        Ok(wait) => wait,
        Err(_) => {
            return Ok(format!(
                "native-unknown {} invalid-PID1-wait\n",
                work.work_incarnation
            ));
        }
    };
    if wait.version != 1
        || wait.grant_id != grant.grant_id
        || wait.attempt_id != grant.attempt_id
        || wait.pid1_parent_namespace_pid <= 0
        || !wait.reaped
        || !libc::WIFEXITED(wait.wait_status)
        || libc::WEXITSTATUS(wait.wait_status) != 0
    {
        return Ok(format!(
            "native-unknown {} PID1-wait-conflict\n",
            work.work_incarnation
        ));
    }
    let attached = sidecar
        .read_native_worker_attach_v30(
            grant.source_generation.as_deref().unwrap(),
            &grant.attempt_id,
            &grant.grant_id,
        )
        .map_err(io::Error::other)?
        .ok_or_else(|| io::Error::other("native Q attach absent"))?;
    let q = BrokerNativeKernelQEvidence {
        protocol: NATIVE_KERNEL_Q_PROTOCOL.into(),
        attempt_id: grant.attempt_id.clone(),
        grant_id: grant.grant_id.clone(),
        kernel_root_id: grant.root_id.clone(),
        work_id: work.work_id.clone(),
        work_incarnation_id: work.work_incarnation.clone(),
        observing_broker_incarnation_id: broker_incarnation.into(),
        worker_identity: attached.evidence.worker_identity.clone(),
        pid1_identity: attached.evidence.pid1_identity.clone(),
        work_pid_namespace_inode: attached.evidence.work_pid_namespace_inode,
        pid1_wait_status: i64::from(wait.wait_status),
        pid1_reaped: true,
        remaining_work_processes: 0,
        terminal_receipt_sha256: terminal_sha256.clone(),
    };
    sidecar
        .settle_exact_native_q_v30(grant.source_generation.as_deref().unwrap(), &q)
        .map_err(io::Error::other)?;
    Ok(format!(
        "native-q-settled {} {} {} {} {} {}\n",
        work.work_incarnation,
        receipt.worker_local_pid,
        receipt.worker_wait_status,
        receipt.cancellation_observed,
        terminal_sha256,
        receipt.output.len()
    ))
}

/// A broker restart can finish exact physical Q without the old guardian or
/// its lost response. Prepared/spent-without-work records stay unknown debt.
pub(super) fn reconcile_after_restart(
    grants: &GrantRegistry,
    works: &WorkRegistry,
    sidecar: &mut BrokerSidecar,
    broker_incarnation: &str,
    terminal_dir: &Path,
    settled_cache: &mut HashSet<String>,
) {
    let source_generation = sidecar.source_generation().to_owned();
    for grant in grants.native_records().iter().filter(|grant| {
        grant.version == 5
            && grant.state == "consumed"
            && grant.source_generation.as_deref() == Some(source_generation.as_str())
    }) {
        if settled_cache.contains(&grant.grant_id) {
            continue;
        }
        let Some(work) = works
            .live_works()
            .map(|live| &live.record)
            .chain(works.debt_records())
            .find(|work| {
                work.accepted_grant_id.as_deref() == Some(grant.grant_id.as_str())
                    && work.root_id == grant.root_id
                    && work.work_id == grant.attempt_id
            })
        else {
            continue;
        };
        let attached = sidecar.read_native_worker_attach_v30(
            grant.source_generation.as_deref().unwrap(),
            &grant.attempt_id,
            &grant.grant_id,
        );
        let Ok(Some(attached)) = attached else {
            continue;
        };
        if attached.evidence.work_incarnation_id != work.work_incarnation
            || attached.evidence.pid1_identity.pid != i64::from(work.init_host_pid)
            || attached.evidence.pid1_identity.starttime_ticks != work.init_starttime_ticks as i64
        {
            continue;
        }
        match sidecar.native_q_settled_v30(&source_generation, &grant.attempt_id, &grant.grant_id) {
            Ok(true) => {
                settled_cache.insert(grant.grant_id.clone());
                continue;
            }
            Ok(false) => {}
            Err(error) => {
                eprintln!(
                    "native Q readback retained debt for {}: {error}",
                    grant.grant_id
                );
                continue;
            }
        }
        match settle_ready(grant, work, sidecar, broker_incarnation, terminal_dir) {
            Ok(status) if status.starts_with("native-q-settled ") => {
                settled_cache.insert(grant.grant_id.clone());
            }
            Ok(_) => {}
            Err(error) => eprintln!(
                "native K/Q reconciliation retained unknown debt for {}: {error}",
                grant.grant_id
            ),
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "independent broker and State evidence"
)]
pub(super) fn cancel(
    grant_id: &str,
    peer: &PeerIdentity,
    host_namespace: &File,
    runner_image: &File,
    grants: &GrantRegistry,
    works: &WorkRegistry,
    sidecar: &BrokerSidecar,
    terminal_dir: &Path,
) -> io::Result<String> {
    let (grant, work) = bound_work(
        grant_id,
        peer,
        host_namespace,
        runner_image,
        grants,
        works,
        sidecar,
    )?;
    let live = works
        .live_works()
        .find(|live| live.record.work_incarnation == work.work_incarnation)
        .ok_or_else(|| io::Error::other("native PID1 already absent; inspect terminal"))?;
    live.init.verify()?;
    let path = terminal_dir.join(format!("{}.native.cancel.json", work.work_incarnation));
    let intent = serde_json::json!({
        "protocol": "native-cancel-intent-v1", "grant_id": grant.grant_id,
        "attempt_id": grant.attempt_id, "work_incarnation": work.work_incarnation,
        "pid1_host_pid": work.init_host_pid, "pid1_starttime_ticks": work.init_starttime_ticks,
    });
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
    {
        Ok(mut file) => {
            serde_json::to_writer(&mut file, &intent)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            File::open(terminal_dir)?.sync_all()?;
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let existing: serde_json::Value = serde_json::from_reader(File::open(&path)?)?;
            if existing != intent {
                return Err(io::Error::other("native cancellation intent changed"));
            }
        }
        Err(error) => return Err(error),
    }
    live.init.signal(libc::SIGUSR1)?;
    Ok(format!(
        "native-cancel-signalled {}\n",
        work.work_incarnation
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    #[test]
    fn private_native_pid1_holds_worker_then_reaps_long_adopted_descendant() {
        if std::env::var_os("AGE319_NATIVE_K_INNER").is_none() {
            let Some(runner) = std::env::var_os("OULIPOLY_AGE319_RUNNER_IMAGE") else {
                return;
            };
            let output = Command::new("unshare")
                .args(["-Urpfm", "--mount-proc"])
                .arg(std::env::current_exe().unwrap())
                .args(["--exact", "linux_main::native_work::tests::private_native_pid1_holds_worker_then_reaps_long_adopted_descendant", "--nocapture"])
                .env("AGE319_NATIVE_K_INNER", "1")
                .env("OULIPOLY_AGE319_RUNNER_IMAGE", runner)
                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", "/tmp/private-native-k-control")
                .output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
            return;
        }
        let temporary = tempfile::tempdir().unwrap();
        let marker = temporary.path().join("one-effect");
        let result = temporary.path().join("result.json");
        let request_path = temporary.path().join("request.json");
        let grant_id = uuid::Uuid::new_v4().to_string();
        let attempt_id = uuid::Uuid::new_v4().to_string();
        let request = serde_json::json!({
            "path": request_path,
            "attempt": {
                "attempt_id": attempt_id, "owner_generation": uuid::Uuid::new_v4().to_string(),
                "operation": "activation", "request_sha256": "a".repeat(64),
                "source_registration_id": null, "source_listener_revision": null,
                "session_id": null, "claim_token": null, "result_path": result,
            },
            "recipe": {"Native": {
                "args": [b"__age319-private-installed-probe-v1".to_vec(), b"ambient".to_vec(), marker.as_os_str().as_encoded_bytes().to_vec()],
                "environment": [], "directory": null,
            }},
        });
        let bytes = serde_json::to_vec(&request).unwrap();
        fs::write(&request_path, &bytes).unwrap();
        let request = sealed_request(
            &File::open(&request_path).unwrap(),
            &format!("{:x}", Sha256::digest(&bytes)),
        )
        .unwrap();
        let image = File::open(std::env::var("OULIPOLY_AGE319_RUNNER_IMAGE").unwrap()).unwrap();
        let placeholder = UnixStream::pair().unwrap();
        let context = InitContext {
            image,
            request,
            terminal_dir: File::open(temporary.path()).unwrap(),
            control: placeholder.0,
            gate: placeholder.1,
            uid: 0,
            gid: 0,
            groups: Vec::new(),
            grant_id: grant_id.clone(),
            attempt_id: attempt_id.clone(),
        };
        let (init_pid, mut control, mut gate) =
            create_init(&File::open("/proc/self/ns/pid").unwrap(), context).unwrap();
        struct Kill(i32);
        impl Drop for Kill {
            fn drop(&mut self) {
                unsafe {
                    libc::kill(self.0, libc::SIGKILL);
                }
            }
        }
        let _cleanup = Kill(init_pid);
        std::thread::sleep(Duration::from_millis(250));
        assert!(!marker.exists(), "effect before PID1 persistence gate");
        let incarnation = uuid::Uuid::new_v4().to_string();
        let mut persisted = vec![b'P'];
        persisted.extend_from_slice(incarnation.as_bytes());
        persisted.extend_from_slice(&init_pid.to_ne_bytes());
        control.write_all(&persisted).unwrap();
        let worker = work_launch::child_credential(&control, b'C').unwrap();
        assert!(worker.pid > 0);
        assert!(!marker.exists(), "effect before worker pre-exec gate");
        gate.write_all(b"R").unwrap();
        let mut executed = [0];
        control.read_exact(&mut executed).unwrap();
        assert_eq!(executed, [b'E']);
        assert!(!marker.exists(), "effect before worker execution gate");
        gate.write_all(&[1]).unwrap();
        let until = Instant::now() + Duration::from_secs(15);
        while !marker.exists() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            marker.exists(),
            "native worker effect absent after K release"
        );
        // The released worker and its adopted descendant survive broker
        // socket loss; a later observer still has exact PID1 receipts.
        drop(control);
        drop(gate);
        std::thread::sleep(Duration::from_secs(6));
        assert!(
            !temporary
                .path()
                .join(format!("{incarnation}.native.json"))
                .exists(),
            "adopted descendant drained before cancellation"
        );
        unsafe {
            libc::kill(init_pid, libc::SIGUSR1);
        }
        let terminal = temporary.path().join(format!("{incarnation}.native.json"));
        let wait = temporary
            .path()
            .join(format!("{grant_id}.native-pid1-wait.json"));
        let until = Instant::now() + Duration::from_secs(15);
        while (!terminal.exists() || !wait.exists()) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        let terminal: NativeTerminal =
            serde_json::from_slice(&fs::read(terminal).unwrap()).unwrap();
        let wait: NativePid1Wait = serde_json::from_slice(&fs::read(wait).unwrap()).unwrap();
        assert!(terminal.physical_tree_drained && terminal.cancellation_observed);
        assert!(
            terminal
                .output
                .iter()
                .any(|output| output.name == "launcher.stdout")
        );
        assert!(wait.reaped && libc::WIFEXITED(wait.wait_status));
        assert_eq!(libc::WEXITSTATUS(wait.wait_status), 0);
    }
}
