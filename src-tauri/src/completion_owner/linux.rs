use super::*;
use oulipoly_state::completion_continuation::{PROTOCOL, SourceProcessIdentity};
use oulipoly_state::mailbox::{CompletionDomainOwner, MailboxDb};
use oulipoly_state::pid_identity::{PidIdentityDb, read_live_process_identity};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;
#[path = "control.rs"]
mod control;
use control::{ControlService, JoinRefusal, JoinRequest, RefusalReason};
#[path = "context_leases.rs"]
mod context_leases;
use context_leases::ContextLeases;

pub(super) fn identity(pid: i64) -> Result<SourceProcessIdentity, String> {
    let identity = read_live_process_identity(pid)?.ok_or("process identity disappeared")?;
    Ok(SourceProcessIdentity {
        pid,
        boot_id: identity.os_boot_id,
        starttime_ticks: identity.os_pid_starttime_ticks,
    })
}

pub(super) fn require_owner(domain_id: &str) -> Result<CompletionDomainOwner, String> {
    let endpoint =
        std::env::var_os(ENDPOINT_ENV).ok_or("native continuation endpoint was not inherited")?;
    let owner = hello(Path::new(&endpoint))?;
    if owner.protocol != PROTOCOL || owner.domain_id != domain_id {
        return Err("completion owner domain/protocol conflict".into());
    }
    let mailbox = MailboxDb::open_existing_native_authority(&MailboxDb::default_path()?)?;
    let current = mailbox
        .completion_continuation_owner()?
        .ok_or("completion owner is no longer running")?;
    // Discovery/handshake only: source admission rechecks live ownership under
    // the State-then-sidecar fence; this result is not an admission capability.
    if current.domain_id != owner.domain_id
        || current.supervisor_authority_id != owner.supervisor_authority_id
        || current.owner_generation != owner.owner_generation
        || current.endpoint != owner.endpoint
        || current.guardian_identity != owner.guardian_identity
        || current.driver_identity != owner.driver_identity
    {
        return Err("completion owner handshake does not match current authority".into());
    }
    Ok(owner)
}

fn hello(endpoint: &Path) -> Result<CompletionDomainOwner, String> {
    let mut socket = UnixStream::connect(endpoint).map_err(|e| e.to_string())?;
    let peer = peer_pid(&socket)?;
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| e.to_string())?;
    socket
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| e.to_string())?;
    socket.write_all(b"hello\n").map_err(|e| e.to_string())?;
    let mut response = Vec::new();
    socket
        .take(8193)
        .read_to_end(&mut response)
        .map_err(|e| e.to_string())?;
    if response.len() > 8192 {
        return Err("oversized completion owner hello".into());
    }
    let owner: CompletionDomainOwner =
        serde_json::from_slice(&response).map_err(|e| e.to_string())?;
    if owner.guardian_identity != identity(peer)?
        || owner.driver_identity != identity(owner.driver_identity.pid)?
    {
        return Err("completion hello process identity mismatch".into());
    }
    Ok(owner)
}

// A live native context prevents idle retirement before its provider can admit
// a source. CLOEXEC keeps this lease out of provider/workload descendants.
static NATIVE_CONTEXT: std::sync::OnceLock<UnixStream> = std::sync::OnceLock::new();
fn retain_context(socket: UnixStream) -> Result<(), String> {
    NATIVE_CONTEXT
        .set(socket)
        .map_err(|_| "native completion context already joined".into())
}
fn join(endpoint: &Path, expected: &CompletionDomainOwner) -> Result<(), String> {
    retain_context(connect_context(endpoint, expected)?)
}

fn connect_context(
    endpoint: &Path,
    expected: &CompletionDomainOwner,
) -> Result<UnixStream, String> {
    let mut socket = UnixStream::connect(endpoint).map_err(|e| e.to_string())?;
    let peer = peer_pid(&socket)?;
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| e.to_string())?;
    socket
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| e.to_string())?;
    socket.write_all(b"join!\n").map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    for _ in 0..8193 {
        let mut byte = [0];
        socket.read_exact(&mut byte).map_err(|e| e.to_string())?;
        if byte == [b'\n'] {
            break;
        }
        bytes.push(byte[0]);
    }
    if bytes.len() > 8192 {
        return Err("oversized completion join".into());
    }
    // Authenticate the peer before interpreting a negative. A refusal is not
    // rollback/no-side-effect proof and never authorizes replay or fresh launch.
    let peer_identity = identity(peer)?;
    if peer_identity != expected.guardian_identity {
        return Err("completion join peer conflict".into());
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if value.get("completion_join_refusal").is_some() {
        let refusal: JoinRefusal =
            serde_json::from_value(value).map_err(|_| "invalid completion join refusal")?;
        if refusal.guardian_identity != peer_identity
            || refusal.protocol != PROTOCOL
            || refusal.protocol != expected.protocol
            || refusal.domain_id != expected.domain_id
            || refusal.owner_generation != expected.owner_generation
            || expected.endpoint != endpoint.to_string_lossy()
        {
            return Err("completion join refusal identity conflict".into());
        }
        let category = match refusal.completion_join_refusal {
            RefusalReason::QueueFull => "queue full",
            RefusalReason::Retiring => "retirement pause",
            RefusalReason::Persistence => "persistence failure",
        };
        return Err(format!(
            "completion join refused: {category}; admission outcome uncertain; no replay authorized"
        ));
    }
    let owner: CompletionDomainOwner = serde_json::from_value(value).map_err(|e| e.to_string())?;
    if owner.guardian_identity != identity(peer)? || owner.endpoint != endpoint.to_string_lossy() {
        return Err("completion join peer conflict".into());
    }
    Ok(socket)
}

pub(super) fn bootstrap() -> Result<(), super::BootstrapError> {
    if std::env::var_os(ENDPOINT_ENV).is_some() {
        // Wake-producing entry joins existing authority; read/ACK never bootstrap.
        let mailbox = MailboxDb::open_existing_native_authority(&MailboxDb::default_path()?)?;
        let domain = mailbox
            .completion_continuation_domain()?
            .ok_or("inherited endpoint has no native domain")?;
        let owner = require_owner(&domain)?;
        join(Path::new(&owner.endpoint), &owner)?;
        return Ok(());
    }
    validate_independent_entry()?;
    let path = MailboxDb::default_path()?;
    // State first: interruption before the additive sidecar upgrade leaves a
    // resumable pair, not a running recovery owner against a partial transition.
    // Both opens must succeed before election/join or any recovery fork.
    // The domain-keyed owner election cannot exist until the second database
    // has an identity. Serialize independent bootstrap at its stable path first;
    // this does not replace deployment's all-writers-stopped prerequisite.
    let directory = path.parent().ok_or("sidecar parent absent")?;
    std::fs::create_dir_all(directory).map_err(|error| {
        format!(
            "Failed to create state directory {}: {error}",
            directory.display()
        )
    })?;
    let transition = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path.with_extension("completion-bootstrap.lock"))
        .map_err(|e| e.to_string())?;
    <std::fs::File as fs4::FileExt>::lock(&transition).map_err(|e| e.to_string())?;
    let state =
        oulipoly_state::StateDb::open_default_with_error().map_err(super::BootstrapError::State)?;
    let mailbox = MailboxDb::open_completion_continuation_domain(&path)?;
    let domain = mailbox
        .completion_continuation_domain()?
        .ok_or("missing native domain")?;
    if state.has_legacy_completion_admissions()? {
        eprintln!(
            "unsupported_legacy_source_recovery: legacy admissions have no v2 recovery binding; retained legacy notify/repair and mailbox delivery remain available. This is not a pending-work count or permission to clear debt."
        );
    }
    drop(state);
    drop(mailbox);
    drop(transition);
    let directory = PathBuf::from("/tmp")
        .join(format!("oulipoly-completion-{}-{domain}", unsafe {
            libc::geteuid()
        }));
    match std::fs::DirBuilder::new().mode(0o700).create(&directory) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.to_string().into()),
    }
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(&directory).map_err(|e| e.to_string())?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err("completion election directory is not private".into());
    }
    let endpoint = directory.join("owner.sock");
    // Admission and retirement share this gate. Hold it through startup or
    // hello + join + authority validation, not through provider execution.
    let admission = admission_gate(&endpoint)?;
    <std::fs::File as fs4::FileExt>::lock(&admission).map_err(|e| e.to_string())?;
    let election = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(directory.join("election.lock"))
        .map_err(|e| e.to_string())?;
    match <std::fs::File as fs4::FileExt>::try_lock(&election) {
        Ok(()) => {}
        Err(fs4::TryLockError::WouldBlock) => {
            let owner = hello(&endpoint)?;
            if owner.domain_id != domain {
                return Err("completion election domain conflict".into());
            }
            join(&endpoint, &owner)?;
            unsafe { std::env::set_var(ENDPOINT_ENV, &endpoint) };
            require_owner(&domain)?;
            return Ok(());
        }
        Err(e) => return Err(e.to_string().into()),
    }
    let (mut ready, announce) = UnixStream::pair().map_err(|e| e.to_string())?;
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(std::io::Error::last_os_error().to_string().into());
    }
    if pid == 0 {
        drop(ready);
        let code = guardian(&path, &endpoint, &domain, election, Some(announce))
            .map(|()| 0)
            .unwrap_or(70);
        unsafe { libc::_exit(code) }
    }
    drop(announce);
    drop(election); // close only; never LOCK_UN the child's open description.
    // Readiness is a correctness barrier, not a latency policy. Migration and
    // bounded recovery can legitimately exceed five seconds on a large retained
    // store. A hard socket timeout previously surfaced Linux EAGAIN as apparent
    // process exhaustion while the healthy guardian continued in the background.
    // Wait for exact readiness or channel closure; individual database and IPC
    // operations retain their own typed contention/failure bounds.
    await_guardian_ready(&mut ready, pid)?;
    unsafe { std::env::set_var(ENDPOINT_ENV, &endpoint) };
    require_owner(&domain)?;
    retain_context(ready)?;
    Ok(())
}

fn await_guardian_ready(ready: &mut UnixStream, pid: i32) -> Result<(), String> {
    let mut ready_byte = [0];
    ready.read_exact(&mut ready_byte).map_err(|error| {
        format!(
            "completion_guardian_startup_failed: guardian pid {pid} closed its ready channel before publishing owner/context readiness: {error}"
        )
    })?;
    if ready_byte != [1] {
        return Err(format!(
            "completion_guardian_startup_failed: guardian pid {pid} published an invalid readiness marker"
        ));
    }
    Ok(())
}

/// Lock ordering: admission -> lifetime election -> State -> sidecar.
/// Retirement takes admission nonblocking while holding election, so an entry
/// joining the running owner cannot deadlock its guardian. Waiting entrants do
/// not hold State/sidecar locks. Kernel lock release, not a DB row or a delay,
/// signals that the retiring custodian has relinquished the election.
fn admission_gate(endpoint: &Path) -> Result<std::fs::File, String> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(endpoint.with_file_name("admission.lock"))
        .map_err(|e| e.to_string())
}

fn try_begin_retirement(
    admission: &std::fs::File,
    close_idle: impl FnOnce() -> bool,
) -> Result<bool, String> {
    match <std::fs::File as fs4::FileExt>::try_lock(admission) {
        Ok(()) => {
            let closing = close_idle();
            if !closing {
                <std::fs::File as fs4::FileExt>::unlock(admission).map_err(|e| e.to_string())?;
            }
            // A successful close retains the gate through physical drain.
            Ok(closing)
        }
        Err(fs4::TryLockError::WouldBlock) => Ok(false),
        Err(e) => Err(e.to_string()),
    }
}

fn guardian(
    path: &Path,
    endpoint: &Path,
    domain: &str,
    election: std::fs::File,
    mut announce: Option<UnixStream>,
) -> Result<(), String> {
    if unsafe { libc::setsid() } < 0
        || unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } < 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let _ = std::fs::remove_file(endpoint); // lifetime election is held; no live peer is replaced.
    let listener = UnixListener::bind(endpoint).map_err(|e| e.to_string())?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    redirect_stdio()?;
    let mut retained = vec![listener.as_raw_fd(), election.as_raw_fd()];
    if let Some(socket) = &announce {
        retained.push(socket.as_raw_fd());
    }
    retained.extend(super::custody::pending_birth_fds());
    close_except(&retained);
    let (mut owner, driver_channel) = start_driver(path, endpoint, domain, listener.as_raw_fd())?;
    let mut root_supervisor = super::root_supervisor::RootSupervisor::new(path, driver_channel)?;
    let mut contexts = ContextLeases::inherit(path)?;
    if let Some(mut socket) = announce.take() {
        let context = identity(peer_pid(&socket)?)?;
        MailboxDb::open(path)?.retain_completion_context(&context)?;
        socket.write_all(&[1]).map_err(|e| e.to_string())?;
        socket.set_nonblocking(true).map_err(|e| e.to_string())?;
        contexts.retain_local(context, socket);
    }
    // Open a distinct description after close_except: never reuse the
    // bootstrap parent's inherited flock description.
    let admission = admission_gate(endpoint)?;
    let mut closing = false;
    let mut control = ControlService::start(&listener, &owner)?;
    let mut pending = Vec::new();
    loop {
        // This guardian is the only process-tree authority.  It accepts launch
        // proposals, owns worker children, observes cancellation, reaps exact
        // terminal identities, and integrates retained results before generic
        // descendant cleanup can consume a wait status.
        root_supervisor.tick(&owner)?;
        if !closing {
            append_pending(
                &mut pending,
                control.recover_finished(&listener, &owner)?,
                &owner,
            );
        }
        loop {
            let mut status = 0;
            // This child has independent, exact custody from start_driver; an
            // unrelated unread birth must not hide its terminal wait.
            let driver_pid =
                if identity(owner.driver_identity.pid).as_ref() == Ok(&owner.driver_identity) {
                    unsafe {
                        libc::waitpid(owner.driver_identity.pid as i32, &mut status, libc::WNOHANG)
                    }
                } else {
                    0
                };
            let pid = if driver_pid > 0 {
                driver_pid
            } else {
                super::custody::reap_unprotected(&mut status)
            };
            if pid <= 0 {
                if closing
                    && pid < 0
                    && std::io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD)
                {
                    let _ = std::fs::remove_file(endpoint);
                    // Release the lifetime election only after actual ECHILD,
                    // and before waking entrants blocked on the admission gate.
                    control.stop()?;
                    drop(listener);
                    drop(election);
                    drop(admission);
                    return Ok(());
                }
                break;
            }
            // A waited descendant is not automatically attributable drain for
            // an activation whose custodian was lost. Its durable row remains.
            if !closing && i64::from(pid) == owner.driver_identity.pid {
                // No control thread (or copied thread locks) may cross fork.
                append_pending(&mut pending, control.stop()?, &owner);
                let (replacement, driver_channel) =
                    start_driver(path, endpoint, domain, listener.as_raw_fd())?;
                owner = replacement;
                root_supervisor.replace_driver(driver_channel)?;
                control = ControlService::start(&listener, &owner)?;
            }
        }
        if pending.is_empty() {
            append_pending(&mut pending, control.pending(), &owner);
        }
        retain_pending_contexts(path, &owner, &mut contexts, &mut pending);
        // Errors leave the group owned for the next pass; never a release ACK.
        let _ = contexts.release_disconnected(path);
        if !closing
            && contexts.is_empty()
            && pending.is_empty()
            && root_supervisor.is_empty()
            && !retained_native_context(path).unwrap_or(true)
        {
            // Hold admission before pausing joins: an entrant already holding
            // this gate must finish hello/join, not be rejected by retirement.
            closing = try_begin_retirement(&admission, || {
                close_idle_owner(&owner, &control, &mut pending)
            })?;
            if closing {
                control.close()?;
            } else {
                control.resume()?;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn close_idle_owner(
    owner: &CompletionDomainOwner,
    control: &ControlService,
    pending: &mut Vec<JoinRequest>,
) -> bool {
    // The barrier exposes every earlier join without obstructing hello during
    // the State close. A failed reader cannot supply permission to retire.
    if control.pause().is_err() {
        return false;
    }
    append_pending(pending, control.pending(), owner);
    if !pending.is_empty() {
        return false;
    }
    oulipoly_state::StateDb::open_default()
        .and_then(|state| state.close_idle_completion_continuation_owner(owner))
        .unwrap_or(false)
}

// Both the channel and the writer backlog are bounded. A refused request has
// not been persisted or acknowledged. Preserve earlier accepted ordering when
// a driver replacement transfers the old reader's queued sockets.
fn append_pending(
    pending: &mut Vec<JoinRequest>,
    requests: Vec<JoinRequest>,
    owner: &CompletionDomainOwner,
) {
    for mut request in requests {
        if pending.len() < control::PENDING_LIMIT {
            pending.push(request);
        } else {
            JoinRefusal::new(owner, RefusalReason::QueueFull).send(&mut request.socket);
        }
    }
}

fn retain_pending_contexts(
    path: &Path,
    owner: &CompletionDomainOwner,
    contexts: &mut ContextLeases,
    pending: &mut Vec<JoinRequest>,
) {
    for request in pending.drain(..pending.len().min(8)) {
        retain_pending_context(path, owner, contexts, request);
    }
}

fn retain_pending_context(
    path: &Path,
    owner: &CompletionDomainOwner,
    contexts: &mut ContextLeases,
    mut request: JoinRequest,
) {
    if contexts.admit(path, &request.context).is_err() {
        JoinRefusal::new(owner, RefusalReason::Persistence).send(&mut request.socket);
        return;
    }
    // Persistence preceded the reply. Even a lost reply is owned locally;
    // grouping sockets by incarnation prevents one release deleting another.
    let _ = serde_json::to_writer(&mut request.socket, owner)
        .map_err(|e| e.to_string())
        .and_then(|()| request.socket.write_all(b"\n").map_err(|e| e.to_string()));
    let _ = request.socket.set_nonblocking(true);
    contexts.retain_local(request.context, request.socket);
}

/// A lost socket owner does not erase a live native entry's ability to admit
/// later work. Process identity only settles this *admission lease*, never an
/// activation or original-workload tree obligation.
fn retained_native_context(path: &Path) -> Result<bool, String> {
    let db = MailboxDb::open(path)?;
    let mut live = false;
    for context in db.completion_contexts()? {
        // The residual sweep must use the same positive expiry evidence as
        // inherited groups; optional absence is not permission to retire.
        if !context_leases::incarnation_expired(&context) {
            live = true;
        } else {
            db.release_completion_context(&context)?;
        }
    }
    Ok(live)
}

fn start_driver(
    path: &Path,
    endpoint: &Path,
    domain: &str,
    listener: RawFd,
) -> Result<(CompletionDomainOwner, UnixStream), String> {
    let (mut release, gate) = UnixStream::pair().map_err(|e| e.to_string())?;
    let executable = std::ffi::CString::new("/proc/self/exe").expect("static path has no NUL");
    let arg0 = std::ffi::CString::new("oulipoly-agent-runner").expect("static argv has no NUL");
    let entry = std::ffi::CString::new(super::driver::DRIVER_ARG).expect("static argv has no NUL");
    let path_arg = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| "completion driver path contains NUL")?;
    let fd_arg = std::ffi::CString::new(gate.as_raw_fd().to_string())
        .expect("numeric descriptor has no NUL");
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if pid == 0 {
        drop(release);
        unsafe { libc::close(listener) };
        let _ = super::custody::pending_birth_fds();
        close_except(&[gate.as_raw_fd()]);
        let flags = unsafe { libc::fcntl(gate.as_raw_fd(), libc::F_GETFD) };
        if flags < 0
            || unsafe { libc::fcntl(gate.as_raw_fd(), libc::F_SETFD, flags & !libc::FD_CLOEXEC) }
                < 0
        {
            unsafe { libc::_exit(70) }
        }
        let argv = [
            arg0.as_ptr(),
            entry.as_ptr(),
            path_arg.as_ptr(),
            fd_arg.as_ptr(),
            std::ptr::null(),
        ];
        unsafe {
            libc::execv(executable.as_ptr(), argv.as_ptr());
            libc::_exit(70)
        }
    }
    drop(gate);
    let guardian_identity = identity(i64::from(std::process::id()))?;
    // Driver replacement beneath the same living guardian stays in the same
    // process-tree authority. A later independent guardian always mints a new
    // root and publication explicitly adopts only unresolved predecessor debt.
    let supervisor_authority_id = MailboxDb::open(path)?
        .completion_continuation_owner()?
        .filter(|current| current.guardian_identity == guardian_identity)
        .map(|current| current.supervisor_authority_id)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let owner = CompletionDomainOwner {
        protocol: PROTOCOL.into(),
        domain_id: domain.into(),
        supervisor_authority_id,
        owner_generation: uuid::Uuid::new_v4().to_string(),
        guardian_identity,
        driver_identity: identity(i64::from(pid))?,
        endpoint: endpoint.to_string_lossy().into_owned(),
    };
    MailboxDb::open(path)?.publish_completion_continuation_owner(&owner)?;
    serde_json::to_writer(&mut release, &owner).map_err(|e| e.to_string())?;
    release.write_all(b"\n").map_err(|e| e.to_string())?;
    Ok((owner, release))
}

pub(super) fn read_driver_owner(socket: &mut UnixStream) -> Result<CompletionDomainOwner, String> {
    let mut bytes = Vec::new();
    loop {
        if bytes.len() >= 8192 {
            return Err("oversized root supervisor driver owner frame".into());
        }
        let mut byte = [0];
        socket
            .read_exact(&mut byte)
            .map_err(|error| error.to_string())?;
        if byte == [b'\n'] {
            break;
        }
        bytes.push(byte[0]);
    }
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

fn validate_independent_entry() -> Result<(), String> {
    let current_ns = std::fs::read_link("/proc/self/ns/net").map_err(|e| e.to_string())?;
    let sidecar = PidIdentityDb::default_path()?;
    let identities = if sidecar.exists() {
        Some(PidIdentityDb::open_read_only(&sidecar)?)
    } else {
        None
    };
    let mut pid = unsafe { libc::getppid() };
    while pid > 1 {
        let stat =
            std::fs::read_to_string(format!("/proc/{pid}/stat")).map_err(|e| e.to_string())?;
        let tail = stat.rsplit_once(')').ok_or("invalid ancestor stat")?.1;
        let parent: i32 = tail
            .split_whitespace()
            .nth(1)
            .ok_or("missing ancestor ppid")?
            .parse()
            .map_err(|_| "invalid ancestor ppid")?;
        use std::os::unix::fs::MetadataExt;
        let same_uid = std::fs::metadata(format!("/proc/{pid}"))
            .map(|m| m.uid() == unsafe { libc::geteuid() })
            .unwrap_or(false);
        if same_uid
            && std::fs::read_link(format!("/proc/{pid}/ns/net"))
                .ok()
                .as_ref()
                == Some(&current_ns)
        {
            let actual = read_live_process_identity(i64::from(pid))?
                .ok_or("ancestor changed during admission")?;
            if let Some(db) = identities.as_ref()
                && db.lookup_by_identity(&actual)?.is_some()
            {
                return Err("managed native ancestor has no inherited completion owner".into());
            }
            let image =
                std::fs::read_link(format!("/proc/{pid}/exe")).map_err(|e| e.to_string())?;
            let image = image.to_string_lossy();
            let environment =
                std::fs::read(format!("/proc/{pid}/environ")).map_err(|e| e.to_string())?;
            let marked = environment.split(|b| *b == 0).any(|entry| {
                entry.starts_with(b"OULIPOLY_COMPLETION_REGISTRATION_AUTHORITY=")
                    || entry.starts_with(b"AGENT_BASH_OWNER_INVOCATION_UUID=")
            });
            if image.contains("agent-bash") || marked {
                return Err(
                    "managed Bash/provider ancestor has no inherited completion owner".into(),
                );
            }
            if read_live_process_identity(i64::from(pid))?.as_ref() != Some(&actual) {
                return Err("ancestor changed during admission".into());
            }
        }
        if parent == pid {
            return Err("cyclic ancestor identity".into());
        }
        pid = parent;
    }
    Ok(())
}

fn peer_pid(socket: &UnixStream) -> Result<i64, String> {
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut credentials as *mut _ as *mut libc::c_void,
            &mut len,
        )
    };
    if result != 0 || credentials.uid != unsafe { libc::geteuid() } {
        return Err("completion peer is not same UID".into());
    }
    Ok(i64::from(credentials.pid))
}

pub(super) fn close_except(keep: &[RawFd]) {
    if let Ok(entries) = std::fs::read_dir("/proc/self/fd") {
        let descriptors: Vec<i32> = entries
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().to_str()?.parse().ok())
            .collect();
        for fd in descriptors {
            if fd > 2 && !keep.contains(&fd) {
                unsafe { libc::close(fd) };
            }
        }
    }
}
fn redirect_stdio() -> Result<(), String> {
    let null = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/null")
        .map_err(|e| e.to_string())?;
    for fd in 0..3 {
        if unsafe { libc::dup2(null.as_raw_fd(), fd) } < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "linux_admission_tests.rs"]
mod admission_tests;

#[cfg(test)]
#[path = "linux_identity_tests.rs"]
mod identity_tests;
