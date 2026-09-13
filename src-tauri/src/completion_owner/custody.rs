//! A gated, independently reaping per-operation custodian. Logical acceptance,
//! result integration and actual child-tree drain remain different observations.
use oulipoly_state::completion_continuation::{
    AdmittedSourceBinding, MAX_REGISTRATION_BYTES, open_source_file, read_source_file, sha256,
};
use oulipoly_state::mailbox::{ContinuationAttempt, MailboxDb};
use std::io::{Read, Seek, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::UnixStream;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

pub(super) const ADOPTER_ARG: &str = "__completion-continuation-adopter-v2";

pub(super) const CUSTODIAN_ARG: &str = "__completion-continuation-custodian-v2";

#[derive(serde::Serialize, serde::Deserialize)]
enum LaunchRecipe {
    Source(AdmittedSourceBinding),
    Native {
        args: Vec<Vec<u8>>,
        environment: Vec<(Vec<u8>, Option<Vec<u8>>)>,
        directory: Option<Vec<u8>>,
    },
}
#[derive(serde::Serialize, serde::Deserialize)]
struct CustodianRequest {
    path: std::path::PathBuf,
    attempt: ContinuationAttempt,
    recipe: LaunchRecipe,
}

pub(super) fn spawn_source(
    path: &Path,
    attempt: &ContinuationAttempt,
    binding: &AdmittedSourceBinding,
) -> Result<i64, String> {
    spawn(path, attempt, LaunchRecipe::Source(binding.clone()))
}

pub(super) fn spawn_activation<F>(
    path: &Path,
    attempt: &ContinuationAttempt,
    command: F,
) -> Result<i64, String>
where
    F: FnOnce() -> Result<Command, String>,
{
    use std::os::unix::ffi::OsStrExt;
    let command = command()?;
    let recipe = LaunchRecipe::Native {
        args: command.get_args().map(|v| v.as_bytes().to_vec()).collect(),
        environment: command
            .get_envs()
            .map(|(k, v)| (k.as_bytes().to_vec(), v.map(|v| v.as_bytes().to_vec())))
            .collect(),
        directory: command
            .get_current_dir()
            .map(|v| v.as_os_str().as_bytes().to_vec()),
    };
    spawn(path, attempt, recipe)
}

fn launch_command(
    recipe: &LaunchRecipe,
    attempt: &ContinuationAttempt,
) -> Result<std::process::Child, String> {
    use std::os::unix::ffi::OsStringExt;
    let mut command = match recipe {
        LaunchRecipe::Source(binding) => source_command(binding, attempt)?,
        LaunchRecipe::Native {
            args,
            environment,
            directory,
        } => {
            // The custodian re-executed the exact current runner image. Native
            // launch uses that image, never an installed-path lookup.
            let mut command = Command::new("/proc/self/exe");
            command.args(args.iter().cloned().map(std::ffi::OsString::from_vec));
            for (key, value) in environment {
                let key = std::ffi::OsString::from_vec(key.clone());
                if let Some(value) = value {
                    command.env(key, std::ffi::OsString::from_vec(value.clone()));
                } else {
                    command.env_remove(key);
                }
            }
            if let Some(directory) = directory {
                command.current_dir(std::ffi::OsString::from_vec(directory.clone()));
            }
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            command
        }
    };
    let directory = Path::new(&attempt.result_path)
        .parent()
        .ok_or("attempt result has no parent")?;
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let (out, err) = if matches!(recipe, LaunchRecipe::Source(_)) {
        ("stdout.json", "stderr.log")
    } else {
        ("launcher.stdout", "launcher.stderr")
    };
    command
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(directory.join(out)).map_err(|e| e.to_string())?)
        .stderr(std::fs::File::create(directory.join(err)).map_err(|e| e.to_string())?);
    command.spawn().map_err(|e| e.to_string())
}

fn spawn(path: &Path, attempt: &ContinuationAttempt, recipe: LaunchRecipe) -> Result<i64, String> {
    // This retained request is intent only. Database acceptance still precedes
    // fork and the launch gate; an unadmitted request cannot launch anything.
    let request = CustodianRequest {
        path: path.into(),
        attempt: attempt.clone(),
        recipe,
    };
    let request_path = Path::new(&attempt.result_path).with_file_name("custodian-request.json");
    #[cfg(feature = "age360-fault-fixtures")]
    if matches!(request.recipe, LaunchRecipe::Source(_)) {
        oulipoly_state::completion_continuation::age360_fault_barrier(
            "source-request-before-write",
        );
    }
    durable_write(
        &request_path,
        &serde_json::to_vec(&request).map_err(|e| e.to_string())?,
    )?;
    let request_file = std::fs::File::open(&request_path).map_err(|e| e.to_string())?;
    let fork_gate = Path::new(&attempt.result_path).with_file_name("adopter-fork-gate.json");
    let driver = super::linux::identity(i64::from(std::process::id()))?;
    let mut gate_file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&fork_gate)
        .map_err(|e| e.to_string())?;
    gate_file
        .write_all(
            &serde_json::to_vec(&serde_json::json!({
                "attempt_id":attempt.attempt_id,"driver":driver,"admitted":false
            }))
            .map_err(|e| e.to_string())?,
        )
        .and_then(|()| gate_file.sync_all())
        .map_err(|e| e.to_string())?;
    std::fs::File::open(fork_gate.parent().ok_or("fork gate parent absent")?)
        .and_then(|dir| dir.sync_all())
        .map_err(|e| e.to_string())?;
    drop(gate_file);
    MailboxDb::open(path)?.accept_continuation_attempt(attempt)?;
    let (mut release, gate) = match birth_channel() {
        Ok(pair) => pair,
        Err(e) => {
            let reason = format!("custodian gate creation failed: {e}");
            MailboxDb::open(path)?.record_continuation_never_forked(attempt, &reason)?;
            return Err(reason);
        }
    };
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        let reason = format!("custodian fork failed: {}", std::io::Error::last_os_error());
        MailboxDb::open(path)?.record_continuation_never_forked(attempt, &reason)?;
        return Err(reason);
    }
    if pid == 0 {
        drop(release);
        let gate_fd = gate.as_raw_fd();
        let request_fd = request_file.as_raw_fd();
        super::linux::close_except(&[gate_fd, request_fd]);
        for fd in [gate_fd, request_fd] {
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
            if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0
            {
                unsafe { libc::_exit(70) }
            }
        }
        // No SQLite operation occurs in this fork image. Inherited SQLite
        // process-global WAL bookkeeping is discarded by exec, not reused.
        let _error = Command::new("/proc/self/exe")
            .arg(ADOPTER_ARG)
            .arg(gate_fd.to_string())
            .arg(request_fd.to_string())
            .exec();
        unsafe { libc::_exit(70) }
    }
    drop(gate);
    drop(request_file);
    let adopter = super::linux::identity(i64::from(pid))?;
    let mut worker = [0; BIRTH_PACKET_BYTES];
    let mut custodian = None;
    let mut announced = None;
    let result = (|| {
        read_ac_announcement(path, &driver, &mut release, &mut worker)
            .map_err(|e| e.to_string())?;
        announced = Some(decode_birth(&worker)?);
        custodian = Some(birth_identity(announced.as_ref().unwrap())?);
        #[cfg(feature = "age360-fault-fixtures")]
        oulipoly_state::completion_continuation::age360_fault_barrier("attempt-before-attachment");
        let identity = custodian.as_ref().ok_or("birth identity absent")?;
        let mut mailbox = MailboxDb::open(path)?;
        mailbox.attach_continuation_custodian_with_adopter(attempt, identity, Some(&adopter))?;
        mailbox.advance_continuation_attempt(attempt, 3, "accepted", "starting", identity)?;
        release.write_all(&[1]).map_err(|e| e.to_string())
    })();
    if let Err(error) = result {
        // Preserve the original read/attachment/wait obligation on EVERY unwind,
        // including guardian loss before EOF and an attachment precommit error.
        // Retry can attach for custody only; it can never send an execution grant.
        PENDING_UNRELEASED.with_borrow_mut(|pending| {
            pending.push(PendingUnreleased {
                path: path.into(),
                attempt: attempt.clone(),
                adopter,
                driver,
                socket: Some(release),
                custodian,
                announced,
            })
        });
        retry_unreleased();
        return Err(error);
    }
    Ok(i64::from(pid))
}

/// Birth and grant are small whole records. Packet boundaries prevent a child
/// dying during an announcement from splicing a partial PID with the adopter's
/// later terminal announcement. Each record carries the original incarnation,
/// not a number that could be resolved against a later process.
const BIRTH_PACKET_BYTES: usize = 512;
fn encode_birth(
    identity: &oulipoly_state::completion_continuation::SourceProcessIdentity,
) -> Result<[u8; BIRTH_PACKET_BYTES], String> {
    let value = serde_json::to_vec(identity).map_err(|e| e.to_string())?;
    if value.len() >= BIRTH_PACKET_BYTES {
        return Err("birth identity exceeds packet".into());
    }
    let mut packet = [0; BIRTH_PACKET_BYTES];
    packet[..value.len()].copy_from_slice(&value);
    Ok(packet)
}
fn decode_birth(
    packet: &[u8; BIRTH_PACKET_BYTES],
) -> Result<oulipoly_state::completion_continuation::SourceProcessIdentity, String> {
    let end = packet
        .iter()
        .position(|b| *b == 0)
        .ok_or("unterminated birth identity")?;
    if packet[end..].iter().any(|b| *b != 0) {
        return Err("invalid birth padding".into());
    }
    serde_json::from_slice(&packet[..end]).map_err(|e| e.to_string())
}
fn birth_channel() -> std::io::Result<(UnixStream, UnixStream)> {
    use std::os::fd::FromRawFd;
    let mut fds = [-1; 2];
    if unsafe {
        libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
            0,
            fds.as_mut_ptr(),
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe {
        (
            UnixStream::from_raw_fd(fds[0]),
            UnixStream::from_raw_fd(fds[1]),
        )
    })
}

fn read_ac_announcement(
    path: &Path,
    driver: &oulipoly_state::completion_continuation::SourceProcessIdentity,
    socket: &mut UnixStream,
    bytes: &mut [u8],
) -> std::io::Result<()> {
    let owner = MailboxDb::open(path)
        .and_then(|db| db.completion_continuation_owner())
        .map_err(std::io::Error::other)?;
    let guardian = owner
        .filter(|o| o.driver_identity == *driver)
        .map(|o| o.guardian_identity.pid);
    socket.set_nonblocking(true)?;
    let mut offset = 0;
    while offset < bytes.len() {
        if guardian.is_some_and(|pid| i64::from(unsafe { libc::getppid() }) != pid) {
            return Err(std::io::Error::other(
                "original driver must succeed lost guardian",
            ));
        }
        match socket.read(&mut bytes[offset..]) {
            Ok(0) => return Err(std::io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => offset += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                let mut fd = libc::pollfd {
                    fd: socket.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                // Responsiveness interval, never a workload/announcement deadline.
                if unsafe { libc::poll(&mut fd, 1, 100) } < 0 {
                    let e = std::io::Error::last_os_error();
                    if e.kind() != std::io::ErrorKind::Interrupted {
                        return Err(e);
                    }
                }
            }
            Err(e) => return Err(e),
        }
    }
    socket.set_nonblocking(false)
}

/// The original announcement endpoint is inherited by AC until it announces
/// itself. EOF before a complete identity, joined to the original adopter wait
/// and this driver's unsent execution grant, proves no effects were released.
/// It does NOT prove no process was forked (a gated AC might also have died).
fn retain_unreleased_adopter_loss(
    path: &Path,
    attempt: &ContinuationAttempt,
    adopter: &oulipoly_state::completion_continuation::SourceProcessIdentity,
    driver: &oulipoly_state::completion_continuation::SourceProcessIdentity,
) -> Result<(), String> {
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    if unsafe {
        libc::waitid(
            libc::P_PID,
            adopter.pid as u32,
            &mut info,
            libc::WEXITED | libc::WNOWAIT | libc::WNOHANG,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if unsafe { info.si_pid() } == 0 {
        return Err("original adopter is not yet waitable".into());
    }
    if super::linux::identity(adopter.pid)? != *adopter {
        return Err("waited adopter identity changed".into());
    }
    let gate_path = Path::new(&attempt.result_path).with_file_name("adopter-fork-gate.json");
    let gate: serde_json::Value = oulipoly_provider::custody::durable::read_json(&gate_path)?;
    if gate["attempt_id"] != attempt.attempt_id
        || gate["driver"] != serde_json::to_value(driver).map_err(|e| e.to_string())?
        || !gate["admitted"].is_boolean()
    {
        return Err("adopter loss has no original execution gate".into());
    }
    let receipt = serde_json::json!({"attempt_id":attempt.attempt_id,"driver":driver,
        "adopter":adopter,"observation":"waitid_wnowait","si_code":info.si_code,
        "si_status":unsafe { info.si_status() },"original_ac_fork_gate":gate,
        "announcement":"eof_before_identity","execution_grant":"not_sent",
        "classification":"original_unreleased_boundary_closed"});
    let bytes = serde_json::to_vec(&receipt).map_err(|e| e.to_string())?;
    durable_write(
        &Path::new(&attempt.result_path).with_file_name("unreleased-announcement-result.json"),
        &bytes,
    )?;
    MailboxDb::open(path)?
        .record_continuation_unreleased_before_announcement(attempt, &receipt.to_string())?;
    // The driver reaper consumes the wait only after this retained integration.
    Ok(())
}

// Keep the original announced incarnation across returning lookup errors. A
// current numeric PID lookup can confirm it, never replace its provenance.
fn birth_identity(
    announced: &oulipoly_state::completion_continuation::SourceProcessIdentity,
) -> Result<oulipoly_state::completion_continuation::SourceProcessIdentity, String> {
    #[cfg(feature = "age360-fault-fixtures")]
    {
        oulipoly_state::completion_continuation::age360_fault_barrier("birth-pid-consumed");
        if let (Some(root), Some(parent)) = (
            std::env::var_os("AGE360_FAULT_ROOT"),
            std::env::var_os("AGE360_FAULT_PARENT_NET"),
        ) && std::fs::read_link("/proc/self/ns/net")
            .ok()
            .is_some_and(|net| net.as_os_str() != parent)
            && std::path::Path::new(&root)
                .join("birth-identity-read-obstructed")
                .exists()
        {
            // An actual returning read error at the lookup boundary, scoped to
            // this driver only; peers' identity acquisition remains untouched.
            std::fs::read_to_string(
                std::path::Path::new(&root).join("birth-identity-read-obstructed"),
            )
            .map_err(|e| {
                oulipoly_state::completion_continuation::age360_fault_barrier(
                    "birth-identity-lookup-failed",
                );
                e.to_string()
            })?;
        }
    }
    let current = super::linux::identity(announced.pid)?;
    if current != *announced {
        return Err("original birth incarnation changed".into());
    }
    Ok(current)
}

struct PendingUnreleased {
    path: std::path::PathBuf,
    attempt: ContinuationAttempt,
    adopter: oulipoly_state::completion_continuation::SourceProcessIdentity,
    driver: oulipoly_state::completion_continuation::SourceProcessIdentity,
    socket: Option<UnixStream>,
    announced: Option<oulipoly_state::completion_continuation::SourceProcessIdentity>,
    custodian: Option<oulipoly_state::completion_continuation::SourceProcessIdentity>,
}
thread_local! {
    static PENDING_UNRELEASED: std::cell::RefCell<Vec<PendingUnreleased>> = const { std::cell::RefCell::new(Vec::new()) };
}

// Only the original process owns these descriptors and kernel waits. Discard
// fork copies BEFORE close_except or descriptor reuse, not on a later reap.
pub(super) fn pending_birth_fds() -> Vec<i32> {
    let current = i64::from(std::process::id());
    PENDING_UNRELEASED.with_borrow_mut(|pending| {
        pending.retain(|p| p.driver.pid == current);
        pending
            .iter()
            .filter_map(|p| p.socket.as_ref().map(AsRawFd::as_raw_fd))
            .collect()
    })
}

impl PendingUnreleased {
    fn retry(&mut self) -> Result<(), String> {
        if self.custodian.is_none()
            && self.announced.is_none()
            && let Some(socket) = &mut self.socket
        {
            socket.set_nonblocking(true).map_err(|e| e.to_string())?;
            let mut bytes = [0; BIRTH_PACKET_BYTES];
            match socket.read(&mut bytes) {
                Ok(0) => self.socket = None, // actual original EOF, not parent loss
                Ok(BIRTH_PACKET_BYTES) => {
                    self.announced = Some(decode_birth(&bytes)?);
                }
                Ok(_) => return Err("incomplete birth packet".into()),
                Err(e) => return Err(e.to_string()),
            }
        }
        if self.custodian.is_none()
            && let Some(announced) = &self.announced
        {
            self.custodian = Some(birth_identity(announced)?);
        }
        if let Some(custodian) = &self.custodian {
            // Attachment is original-driver testimony, including after promotion.
            // It does not revive the old generation's starting/grant authority.
            MailboxDb::open(&self.path)?.attach_original_continuation_custody(
                &self.attempt,
                &self.driver,
                custodian,
                &self.adopter,
            )?;
            self.socket = None; // close unsent grant; original adopter owns drain
            return Ok(());
        }
        retain_unreleased_adopter_loss(&self.path, &self.attempt, &self.adopter, &self.driver)
    }
}

/// One attempt per pending owner per OUTER reap pass, not per reaped child.
/// Returning storage errors do not cause a retry-until-success loop.
pub(super) fn retry_unreleased() {
    let _ = pending_birth_fds();
    PENDING_UNRELEASED.with_borrow_mut(|pending| {
        pending.retain_mut(|p| p.retry().is_err());
    });
}

/// Shared by CD and its guardian succession: retry original evidence first,
/// then reap unprotected exact child PIDs. Enumerating candidates is not drain
/// evidence; only waitpid/waitid supply terminal/ECHILD observations.
pub(super) fn reap_unprotected(status: &mut i32) -> i32 {
    #[cfg(feature = "age360-fault-fixtures")]
    if PENDING_UNRELEASED.with_borrow(|pending| pending.iter().any(|p| p.announced.is_none())) {
        oulipoly_state::completion_continuation::age360_fault_barrier("birth-unread-before-reap");
    }
    // Until the original birth is consumed, a newly adopted child can belong to
    // this pending operation. Do not consume an unidentified original wait in
    // the gap between a WouldBlock read and peer announcement/death. This is
    // retention only, never an ECHILD or drain observation.
    if PENDING_UNRELEASED.with_borrow(|pending| pending.iter().any(|p| p.announced.is_none())) {
        return 0;
    }
    let protected = PENDING_UNRELEASED.with_borrow(|pending| {
        pending
            .iter()
            .flat_map(|p| [Some(p.adopter.pid), p.announced.as_ref().map(|a| a.pid)])
            .flatten()
            .collect::<Vec<_>>()
    });
    // A consumed PID still names the original unreaped child. If its adopter
    // dies, this original subreaper inherits it; do not consume that incarnation
    // while identity lookup or attachment remains pending.
    if protected.is_empty() {
        return unsafe { libc::waitpid(-1, status, libc::WNOHANG) };
    }
    let Ok(children) = std::fs::read_to_string("/proc/thread-self/children") else {
        return 0;
    };
    for pid in children
        .split_whitespace()
        .filter_map(|v| v.parse::<i32>().ok())
    {
        if protected.contains(&i64::from(pid)) {
            continue;
        }
        let waited = unsafe { libc::waitpid(pid, status, libc::WNOHANG) };
        if waited > 0 {
            return waited;
        }
    }
    // A protected wait still exists. Never turn enumeration into ECHILD.
    0
}

pub(super) fn entry() -> Result<(), String> {
    use std::os::fd::FromRawFd;
    let fd = |index| -> Result<i32, String> {
        std::env::args()
            .nth(index)
            .ok_or("missing custodian descriptor")?
            .parse()
            .map_err(|_| "invalid custodian descriptor".into())
    };
    let gate_fd = fd(2)?;
    let request_fd = fd(3)?;
    if gate_fd < 3 || request_fd < 3 || gate_fd == request_fd {
        return Err("invalid custodian descriptor identity".into());
    }
    let mut gate = unsafe { UnixStream::from_raw_fd(gate_fd) };
    let mut file = unsafe { std::fs::File::from_raw_fd(request_fd) };
    let mut bytes = Vec::new();
    (&mut file)
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("custodian request too large".into());
    }
    let request: CustodianRequest = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if std::env::args().nth(1).as_deref() == Some(ADOPTER_ARG) {
        file.rewind().map_err(|e| e.to_string())?;
        return adopt(request, gate, file);
    }
    drop(file);
    let path = &request.path;
    let attempt = &request.attempt;
    if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let identity = super::linux::identity(i64::from(std::process::id()))?;
    let mut byte = [0];
    if gate.read_exact(&mut byte).is_err() || byte != [1] {
        let receipt=serde_json::json!({"attempt_id":attempt.attempt_id,"custodian":identity,"gate":"unreleased_eof","owned_children":"ECHILD"}).to_string();
        if owned_tree_empty()? {
            persist_result_until_retained(Path::new(&attempt.result_path), receipt.as_bytes());
            loop {
                if MailboxDb::open(path)
                    .and_then(|mut db| {
                        db.cancel_unreleased_continuation_gate(attempt, &identity, &receipt)
                    })
                    .is_ok()
                {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        return Ok(());
    }
    drop(gate);
    let launch = MailboxDb::open(path)
        .and_then(|db| db.require_continuation_launch_gate(attempt, &identity))
        .and_then(|()| launch_command(&request.recipe, attempt));
    let (mut child, spawn_error) = match launch {
        Ok(child) => (Some(child), None),
        Err(error) => (None, Some(error)),
    };
    let spawn_failed = child.is_none();
    let mut root_status = None;
    let mut root_wait_status = None;
    let mut cancellation: Option<(String, std::time::Instant)> = None;
    let observations = activation_observer(path, attempt);
    loop {
        if cancellation.is_none()
            && let Some(identity) =
                latest_activation_observation(&observations).and_then(|v| v.cancellation)
        {
            cancellation = Some((identity, std::time::Instant::now()));
        }
        if let Some((_, started)) = &cancellation {
            let signal =
                if started.elapsed() >= oulipoly_core::launch_custody::TERMINATION_GRACE_PERIOD {
                    libc::SIGKILL
                } else {
                    libc::SIGTERM
                };
            // All direct children belong to this exact activation AC. The
            // original Bash source tree is never in this custody boundary.
            let _ = oulipoly_core::launch_custody::signal_owned_children(signal);
        }
        // Persist each waitable incarnation before consuming it. A surviving
        // original adopter can complete the same record after AC loss.
        let (waited, status, _) = reap_adopted_child(Some((attempt, &identity)))?;
        if child.as_ref().is_some_and(|p| p.id() as i32 == waited) {
            let exit = std::process::ExitStatus::from_raw(status);
            if cancellation.is_none()
                && attempt.operation == "activation"
                && matches!(exit.signal(), Some(libc::SIGTERM | libc::SIGINT))
            {
                cancellation = Some((
                    format!("native_launcher_wait_signal:{}", exit.signal().unwrap()),
                    std::time::Instant::now(),
                ));
            }
            root_status = exit.code();
            root_wait_status = Some(status);
            child = None;
        }
        if waited < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ECHILD) {
                if child.is_some() {
                    return Err("original launcher wait missing".into());
                }
                break;
            }
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error.to_string());
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let classification = if attempt.operation == "source_recovery" && !spawn_failed {
        match classify_source_reply(attempt) {
            Ok(value) => value,
            Err(error) => {
                serde_json::json!({"classification":"uncertain_response","error":error})
            }
        }
    } else {
        serde_json::json!({"classification":"native_wait_result"})
    };
    let receipt=serde_json::json!({"attempt_id":attempt.attempt_id,"custodian":identity,"root_exit_code":root_status,"root_wait_status":root_wait_status,"spawn_failed":spawn_failed,"spawn_error":spawn_error,"accepted_cancellation":cancellation.as_ref().map(|v|&v.0),"response":classification,"owned_children":"ECHILD","result_retained":true}).to_string();
    // Persist the complete wait/drain result before its DB integration.
    persist_result_until_retained(Path::new(&attempt.result_path), receipt.as_bytes());
    #[cfg(feature = "age360-fault-fixtures")]
    oulipoly_state::completion_continuation::age360_fault_barrier("ac-result-retained");
    loop {
        let result = MailboxDb::open(path).and_then(|mut mailbox| {
            if spawn_failed {
                mailbox.cancel_unreleased_continuation_gate(attempt, &identity, &receipt)
            } else {
                mailbox.discharge_continuation_attempt(attempt, &identity, &receipt)
            }
        });
        match result {
            Ok(()) => break,
            Err(error) => {
                // Persist uncertainty separately; a retained physical
                // receipt is not a successful database integration.
                let _ = durable_write(
                    &Path::new(&attempt.result_path).with_file_name("integration-error.txt"),
                    error.as_bytes(),
                );
            }
        }
        // No children remain, but retained result integration is still
        // owed. There is no provider/workload silence deadline.
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

fn source_command(
    binding: &AdmittedSourceBinding,
    attempt: &ContinuationAttempt,
) -> Result<Command, String> {
    let source = binding.registration()?;
    let directory = Path::new(&source.handle_dir);
    let registration = read_source_file(
        directory,
        &source.registration_relative,
        MAX_REGISTRATION_BYTES,
    )?;
    if registration != binding.registration_bytes() {
        return Err("recovery source incarnation conflict".into());
    }
    let environment = read_source_file(
        directory,
        "delivery-helper-environment.json",
        MAX_REGISTRATION_BYTES,
    )?;
    if sha256(&environment) != source.recovery.environment_sha256 {
        return Err("recovery environment digest conflict".into());
    }
    let environment: std::collections::BTreeMap<String, String> =
        serde_json::from_slice(&environment).map_err(|e| e.to_string())?;
    let relative = Path::new(&source.recovery.path)
        .strip_prefix(directory)
        .map_err(|e| e.to_string())?
        .to_str()
        .ok_or("non UTF-8 recovery path")?;
    let mut executable = open_source_file(directory, relative, 256 * 1024 * 1024)?;
    let metadata = executable.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > 256 * 1024 * 1024 {
        return Err("recovery image is not a bounded regular file".into());
    }
    let mut hasher = sha2::Sha256::new();
    use sha2::Digest;
    let mut buffer = [0; 65536];
    loop {
        let count = executable.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    if format!("{:x}", hasher.finalize()) != source.recovery.sha256 {
        return Err("recovery executable digest conflict".into());
    }
    let confirmation =
        Path::new(&attempt.result_path).with_file_name("registration-confirmation-v2.json");
    let mut receipt = crate::commands::notify_continuation::response(binding, "exact_committed")?;
    receipt["authority"] = "completion_only".into();
    receipt["registration_committed"] = true.into();
    durable_write(
        &confirmation,
        serde_json::to_string(&receipt)
            .map_err(|e| e.to_string())?
            .as_bytes(),
    )?;
    let descriptor = executable.as_raw_fd();
    // Command owns the pinned descriptor via pre_exec closure through exec.
    let mut command = Command::new(format!("/proc/self/fd/{descriptor}"));
    command
        .env_clear()
        .envs(environment)
        .arg("completion-reconcile-v2")
        .arg("--registration-file")
        .arg(directory.join(&source.registration_relative))
        .arg("--confirmation")
        .arg(confirmation)
        .arg("--json");
    unsafe {
        command.pre_exec(move || {
            let fd = executable.as_raw_fd();
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(command)
}

pub(super) fn owned_tree_empty() -> Result<bool, String> {
    loop {
        let pid = unsafe { libc::waitpid(-1, std::ptr::null_mut(), libc::WNOHANG) };
        if pid > 0 {
            continue;
        }
        if pid == 0 {
            return Ok(false);
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ECHILD) {
            return Ok(true);
        }
        if error.kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        return Err(error.to_string());
    }
}

pub(super) fn durable_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("durable result has no directory")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)
        .map_err(|e| e.to_string())?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|e| e.to_string())?;
    std::fs::rename(&temp, path).map_err(|e| e.to_string())?;
    std::fs::File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())
}

fn classify_source_reply(attempt: &ContinuationAttempt) -> Result<serde_json::Value, String> {
    let directory = Path::new(&attempt.result_path)
        .parent()
        .ok_or("attempt result directory absent")?;
    let bytes = read_source_file(directory, "stdout.json", MAX_REGISTRATION_BYTES)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    // This native custody lane already has State writer authority. Admission
    // identity must be published even though the response remains an observation,
    // not acceptance or ACK. Original admitted bindings are immutable.
    let state = oulipoly_state::StateDb::open_default()?;
    let binding = state
        .admitted_completion_continuations()?
        .into_iter()
        .find(|binding| {
            binding.registration().ok().is_some_and(|source| {
                Some(source.registration_id) == attempt.source_registration_id
            })
        })
        .ok_or("source reply has no retained admission")?;
    let expected = serde_json::to_value(binding.identity()?).map_err(|e| e.to_string())?;
    for (key, expected) in expected.as_object().ok_or("invalid source identity")? {
        if value.get(key) != Some(expected) {
            return Err(format!("source reply identity conflict at {key}"));
        }
    }
    match value["status"].as_str() {
        Some("source_ready" | "source_output_missing") => {
            let evidence =
                oulipoly_state::completion_continuation::VerifiedCompletion::from_source_files(
                    &binding,
                )?;
            evidence.validate_source_reply(&value)?;
        }
        Some("pending" | "conflict" | "unavailable") => {}
        _ => return Err("source reply missing/unsupported status".into()),
    }
    Ok(
        serde_json::json!({"classification":"structured_source_response","stdout_sha256":sha256(&bytes),"stdout_byte_len":bytes.len(),"reply":value}),
    )
}

fn persist_result_until_retained(path: &Path, bytes: &[u8]) {
    while durable_write(path, bytes).is_err() {
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[derive(Default)]
struct ActivationObservation {
    launcher: Option<oulipoly_state::completion_continuation::SourceProcessIdentity>,
    cancellation: Option<String>,
}

/// Database work cannot block physical reaping or lose a wait result. The
/// observer owns ordinary writer connections in a separate thread, not snapshot
/// helper children intermingled with the custodian's wait set. One latest-value
/// slot bounds transport memory; persisted cancellation is queried until read.
fn activation_observer(
    path: &Path,
    attempt: &ContinuationAttempt,
) -> Option<std::sync::mpsc::Receiver<ActivationObservation>> {
    if attempt.operation != "activation" {
        return None;
    }
    let path = path.to_path_buf();
    let attempt = attempt.clone();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || observe_activation_database(path, attempt, sender));
    Some(receiver)
}

fn observe_activation_database(
    path: std::path::PathBuf,
    attempt: ContinuationAttempt,
    sender: std::sync::mpsc::SyncSender<ActivationObservation>,
) {
    loop {
        #[cfg(feature = "age360-fault-fixtures")]
        oulipoly_state::completion_continuation::age360_fault_barrier("activation-observation");
        let observation = read_activation_observation(&path, &attempt).unwrap_or_default();
        if matches!(
            sender.try_send(observation),
            Err(std::sync::mpsc::TrySendError::Disconnected(_))
        ) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn read_activation_observation(
    path: &Path,
    attempt: &ContinuationAttempt,
) -> Result<ActivationObservation, String> {
    let mailbox = MailboxDb::open(path)?;
    let launcher = mailbox.continuation_launcher_identity(attempt)?;
    let Some((generation, invocation)) = mailbox.continuation_runtime_identity(attempt)? else {
        return Ok(ActivationObservation {
            launcher,
            cancellation: None,
        });
    };
    let cancellation = read_activation_cancellation(&generation, &invocation)
        .ok()
        .flatten();
    Ok(ActivationObservation {
        launcher,
        cancellation,
    })
}

fn read_activation_cancellation(
    generation: &str,
    invocation: &str,
) -> Result<Option<String>, String> {
    let state = oulipoly_state::StateDb::open_default()?;
    use rusqlite::OptionalExtension;
    state.connection().query_row("SELECT l.logical_launch_id || ':' || l.cancel_requested_at FROM provider_launch_attempts a JOIN provider_logical_launches l ON l.logical_launch_id=a.logical_launch_id WHERE a.runtime_generation_uuid=?1 AND a.invocation_uuid=?2 AND l.cancel_requested_at IS NOT NULL",rusqlite::params![generation,invocation],|r|r.get(0)).optional().map_err(|e|e.to_string())
}

fn latest_activation_observation(
    receiver: &Option<std::sync::mpsc::Receiver<ActivationObservation>>,
) -> Option<ActivationObservation> {
    receiver.as_ref()?.try_iter().last()
}

/// An original per-attempt adopting boundary exists before AC starts. This is
/// not a replacement's empty tree: every launched descendant stays beneath it
/// on AC loss, independently of domain guardian/driver replacement.
fn adopt(
    request: CustodianRequest,
    mut gate: UnixStream,
    file: std::fs::File,
) -> Result<(), String> {
    if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    // Announcement and launch release have distinct peer lifetimes. AC keeps
    // the CD/adopter endpoint only until its own birth announcement, never
    // while waiting for release. Thus adopter death cannot hide a surviving AC
    // or keep both CD and AC waiting on endpoints owned by each other.
    let (mut ac_release, ac_gate) = UnixStream::pair().map_err(|e| e.to_string())?;
    #[cfg(feature = "age360-fault-fixtures")]
    oulipoly_state::completion_continuation::age360_fault_barrier("adopter-before-ac-fork");
    let gate_path =
        Path::new(&request.attempt.result_path).with_file_name("adopter-fork-gate.json");
    let mut fork_gate: serde_json::Value =
        oulipoly_provider::custody::durable::read_json(&gate_path)?;
    if fork_gate["attempt_id"] != request.attempt.attempt_id || fork_gate["admitted"] != false {
        return Err("original AC-fork admission conflict".into());
    }
    fork_gate["admitted"] = true.into();
    durable_write(
        &gate_path,
        &serde_json::to_vec(&fork_gate).map_err(|e| e.to_string())?,
    )?;
    #[cfg(feature = "age360-fault-fixtures")]
    oulipoly_state::completion_continuation::age360_fault_barrier(
        "adopter-admitted-before-ac-fork",
    );
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if pid == 0 {
        // Keep the original endpoint through the real fork/birth announcement.
        // A surviving AC announces even if its adopter dies in the fork window;
        // before announcement it cannot reach exec or read any execution grant.
        drop(ac_release);
        #[cfg(feature = "age360-fault-fixtures")]
        oulipoly_state::completion_continuation::age360_fault_barrier("ac-created-before-announce");
        // Receiver loss withdraws the only grant; it must not kill the original
        // self-receipt producer. Exec still reaches the unreleased/ECHILD path.
        // A returning identity-read error must not drop the only announcement
        // while this same AC stays alive waiting for its execution gate.
        let packet = loop {
            if let Ok(identity) = super::linux::identity(i64::from(unsafe { libc::getpid() })) {
                break encode_birth(&identity)?;
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        let _ = gate.write_all(&packet);
        #[cfg(feature = "age360-fault-fixtures")]
        oulipoly_state::completion_continuation::age360_fault_barrier("ac-announced-before-exec");
        drop(gate);
        let gate_fd = ac_gate.as_raw_fd();
        let flags = unsafe { libc::fcntl(gate_fd, libc::F_GETFD) };
        if flags < 0
            || unsafe { libc::fcntl(gate_fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0
        {
            unsafe { libc::_exit(70) }
        }
        let _error = Command::new("/proc/self/exe")
            .arg(CUSTODIAN_ARG)
            .arg(gate_fd.to_string())
            .arg(file.as_raw_fd().to_string())
            .exec();
        unsafe { libc::_exit(70) }
    }
    #[cfg(feature = "age360-fault-fixtures")]
    oulipoly_state::completion_continuation::age360_fault_barrier("adopter-after-ac-fork");
    drop(ac_gate);
    let custodian = super::linux::identity(i64::from(pid))?;
    let adopter = super::linux::identity(i64::from(std::process::id()))?;
    #[cfg(feature = "age360-fault-fixtures")]
    oulipoly_state::completion_continuation::age360_fault_barrier("adopter-before-ac-announce");
    // Relay only the actual original driver's grant after attachment. EOF or
    // adopter death closes AC's separate gate; AC then retains its own exact
    // unreleased/ECHILD receipt. Neither a new owner nor a timeout grants work.
    let mut grant = [0];
    if read_original_grant(&mut gate, pid, &custodian, &mut grant).is_ok() && grant == [1] {
        #[cfg(feature = "age360-fault-fixtures")]
        oulipoly_state::completion_continuation::age360_fault_barrier("adopter-before-ac-release");
        let _ = ac_release.write_all(&grant);
    }
    drop(ac_release);
    drop(gate);
    drop(file);
    let path = &request.path;
    let attempt = &request.attempt;
    let mut custodian_wait = None;
    let mut cancellation: Option<(String, std::time::Instant)> = None;
    let mut launcher = None;
    let observations = activation_observer(path, attempt);
    let mut signal_waits = Vec::new();
    loop {
        if let Some(observation) = latest_activation_observation(&observations) {
            launcher = observation.launcher.or(launcher);
            if cancellation.is_none() {
                cancellation = observation
                    .cancellation
                    .map(|id| (id, std::time::Instant::now()));
            }
        }
        if cancellation.is_none()
            && let Some(expected) = &launcher
            && let Some((_, status)) = signal_waits
                .iter()
                .find(|(identity, _)| identity == expected)
        {
            cancellation = Some((
                format!(
                    "adopted_native_launcher_wait_signal:{}",
                    libc::WTERMSIG(*status)
                ),
                std::time::Instant::now(),
            ));
        }
        // The original fork child stays pinned by our exclusive wait ownership.
        // Its terminal observation permits signaling adopted children even when
        // identity acquisition or durable journaling prevents consuming its wait.
        // Neither this observation nor signaling supplies a drain receipt.
        if (custodian_wait.is_some() || original_child_is_waitable(pid))
            && let Some((_, started)) = &cancellation
        {
            let signal =
                if started.elapsed() >= oulipoly_core::launch_custody::TERMINATION_GRACE_PERIOD {
                    libc::SIGKILL
                } else {
                    libc::SIGTERM
                };
            let _ = oulipoly_core::launch_custody::signal_owned_children(signal);
        }
        let (waited, status, wait_identity) = reap_adopted_child(Some((attempt, &adopter)))?;
        if waited == pid {
            custodian_wait = Some(status);
        }
        if let Some(identity) = wait_identity
            && libc::WIFSIGNALED(status)
            && matches!(libc::WTERMSIG(status), libc::SIGTERM | libc::SIGINT)
        {
            // Retain exact waits even when the DB identity/cancellation read is
            // unavailable at this instant. Later observation may join them.
            signal_waits.push((identity, status));
            #[cfg(feature = "age360-fault-fixtures")]
            oulipoly_state::completion_continuation::age360_fault_barrier("adopted-terminal-wait");
        }
        if waited < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ECHILD) {
                break;
            }
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error.to_string());
            }
        }
        if waited <= 0 {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    // Preserve the original receipt when AC already retained its actual result.
    if replay_result(path, attempt).is_ok() {
        return Ok(());
    }
    let receipt = serde_json::json!({"attempt_id":attempt.attempt_id,"custodian":custodian,"adopter":adopter,"custodian_wait_status":custodian_wait.ok_or("original custodian was not waited")?,"owned_children":"ECHILD","accepted_cancellation":cancellation.map(|v|v.0),"classification":"original_adopting_boundary_drained"}).to_string();
    let result_path = Path::new(&attempt.result_path).with_file_name("adopting-result.json");
    persist_result_until_retained(&result_path, receipt.as_bytes());
    loop {
        // An AC-integrated result wins over the later enclosing-boundary receipt.
        if !MailboxDb::open(path)?
            .pending_continuation_attempts()?
            .iter()
            .any(|v| v.attempt_id == attempt.attempt_id)
        {
            return Ok(());
        }
        if MailboxDb::open(path)
            .and_then(|mut db| db.discharge_adopted_continuation_attempt(attempt, &receipt))
            .is_ok()
        {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A living unannounced AC retains birth authority. Only its original parent,
/// observing that exact child with WNOWAIT, may relay its terminal identity.
/// The wait stays unconsumed until the ordinary adopting journal retains it.
fn read_original_grant(
    gate: &mut UnixStream,
    pid: i32,
    custodian: &oulipoly_state::completion_continuation::SourceProcessIdentity,
    grant: &mut [u8; 1],
) -> Result<(), String> {
    gate.set_nonblocking(true).map_err(|e| e.to_string())?;
    let mut terminal_announced = false;
    loop {
        match gate.read(grant) {
            Ok(1) => return Ok(()),
            Ok(_) => return Err("original grant endpoint closed".into()),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e.to_string()),
        }
        if !terminal_announced && exact_child_terminal(pid, custodian)? {
            // At most one extra original-incarnation packet, after AC can no
            // longer write. If AC announced, CD has its first complete identity;
            // this cannot change attachment or manufacture an execution grant.
            gate.set_nonblocking(false).map_err(|e| e.to_string())?;
            gate.write_all(&encode_birth(custodian)?)
                .map_err(|e| e.to_string())?;
            gate.set_nonblocking(true).map_err(|e| e.to_string())?;
            terminal_announced = true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn exact_child_terminal(
    pid: i32,
    expected: &oulipoly_state::completion_continuation::SourceProcessIdentity,
) -> Result<bool, String> {
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    if unsafe {
        libc::waitid(
            libc::P_PID,
            pid as u32,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if unsafe { info.si_pid() } == 0 {
        return Ok(false);
    }
    if super::linux::identity(i64::from(pid))? != *expected {
        return Err("original terminal child incarnation conflict".into());
    }
    Ok(true)
}

/// Observe only the original, still-owned fork child. Errors/absence are not
/// death evidence; WNOWAIT preserves the ordinary identity/journal/reap path.
fn original_child_is_waitable(pid: i32) -> bool {
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    result == 0 && unsafe { info.si_pid() } == pid
}

/// Inspect the waitable child before consuming its PID, preserving incarnation
/// identity even if the launcher binding cannot currently be read from SQLite.
fn reap_adopted_child(
    journal: Option<(
        &ContinuationAttempt,
        &oulipoly_state::completion_continuation::SourceProcessIdentity,
    )>,
) -> Result<
    (
        i32,
        i32,
        Option<oulipoly_state::completion_continuation::SourceProcessIdentity>,
    ),
    String,
> {
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let result = unsafe {
        libc::waitid(
            libc::P_ALL,
            0,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result < 0 {
        return Ok((-1, 0, None));
    }
    let pid = unsafe { info.si_pid() };
    if pid == 0 {
        return Ok((0, 0, None));
    }
    #[cfg(feature = "age360-fault-fixtures")]
    oulipoly_state::completion_continuation::age360_fault_barrier("adopted-before-identity");
    let identity = match super::linux::identity(i64::from(pid)) {
        Ok(identity) => identity,
        Err(_) => {
            // WNOWAIT left this exact incarnation owned and unreaped. A
            // returning read failure cannot transfer the original wait duty.
            #[cfg(feature = "age360-fault-fixtures")]
            oulipoly_state::completion_continuation::age360_fault_barrier(
                "adopted-identity-failed",
            );
            return Ok((0, 0, None));
        }
    };
    if let Some((attempt, owner)) = journal {
        let status = if info.si_code == libc::CLD_EXITED {
            (unsafe { info.si_status() }) << 8
        } else {
            (unsafe { info.si_status() })
                | if info.si_code == libc::CLD_DUMPED {
                    128
                } else {
                    0
                }
        };
        let file = Path::new(&attempt.result_path)
            .with_file_name("owned-waits")
            .join(format!(
                "{}-{}-{}.json",
                identity.pid, identity.starttime_ticks, owner.pid
            ));
        // No aggregation at ECHILD. The receipt binds original owner, operation
        // boundary, incarnation and an actual WNOWAIT terminal observation.
        if durable_write(
            &file,
            &serde_json::to_vec(&serde_json::json!({
                "attempt_id": attempt.attempt_id, "owner": owner, "process": identity,
                "status": status, "observation": "waitid_wnowait"
            }))
            .map_err(|e| e.to_string())?,
        )
        .is_err()
        {
            // Preserve the waitable child and original owner on retention
            // failure. The caller continues TERM/KILL escalation each turn;
            // storage failure must not kill both original custody boundaries.
            return Ok((0, 0, None));
        }
    }
    let mut status = 0;
    let waited = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
    Ok((waited, status, Some(identity)))
}

/// Replay only retained wait/drain evidence joined by the DB to its original
/// producer. A lost process with no receipt is not a replayable result.
pub(super) fn replay_result(path: &Path, attempt: &ContinuationAttempt) -> Result<(), String> {
    let result = Path::new(&attempt.result_path);
    let directory = result.parent().ok_or("result parent absent")?;
    if let Ok(bytes) = read_source_file(
        directory,
        "unreleased-announcement-result.json",
        MAX_REGISTRATION_BYTES,
    ) {
        let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        let driver = super::linux::identity(i64::from(std::process::id()))?;
        if value["attempt_id"] != attempt.attempt_id
            || value["driver"] != serde_json::to_value(driver).map_err(|e| e.to_string())?
            || value["observation"] != "waitid_wnowait"
            || value["announcement"] != "eof_before_identity"
            || value["execution_grant"] != "not_sent"
            || value["classification"] != "original_unreleased_boundary_closed"
        {
            return Err("unreleased announcement replay original owner/evidence conflict".into());
        }
        return MailboxDb::open(path)?
            .record_continuation_unreleased_before_announcement(attempt, &value.to_string());
    }
    let primary = read_source_file(directory, "result.json", MAX_REGISTRATION_BYTES);
    if let Ok(bytes) = primary {
        let receipt = std::str::from_utf8(&bytes).map_err(|e| e.to_string())?;
        let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if value["attempt_id"] != attempt.attempt_id || value["owned_children"] != "ECHILD" {
            return Err("retained result evidence conflict".into());
        }
        let custodian =
            serde_json::from_value(value["custodian"].clone()).map_err(|e| e.to_string())?;
        let mut db = MailboxDb::open(path)?;
        return if value["gate"] == "unreleased_eof" || value["spawn_failed"] == true {
            db.cancel_unreleased_continuation_gate(attempt, &custodian, receipt)
        } else if value["result_retained"] == true && value["root_wait_status"].as_i64().is_some() {
            db.discharge_continuation_attempt(attempt, &custodian, receipt)
        } else {
            Err("retained result lacks launch/wait evidence".into())
        };
    }
    let bytes = read_source_file(directory, "adopting-result.json", MAX_REGISTRATION_BYTES)?;
    MailboxDb::open(path)?.discharge_adopted_continuation_attempt(
        attempt,
        std::str::from_utf8(&bytes).map_err(|e| e.to_string())?,
    )
}
