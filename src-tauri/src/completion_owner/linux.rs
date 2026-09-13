use super::*;
use oulipoly_state::completion_continuation::{PROTOCOL, SourceProcessIdentity};
use oulipoly_state::mailbox::{CompletionDomainOwner, MailboxDb};
use oulipoly_state::pid_identity::{PidIdentityDb, read_live_process_identity};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

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
    let mailbox = MailboxDb::open_read_only(&MailboxDb::default_path()?)?;
    let current = mailbox
        .completion_continuation_owner()?
        .ok_or("completion owner is no longer running")?;
    if current.owner_generation != owner.owner_generation
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
fn join(endpoint: &Path) -> Result<(), String> {
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
    let owner: CompletionDomainOwner = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if owner.guardian_identity != identity(peer)? || owner.endpoint != endpoint.to_string_lossy() {
        return Err("completion join peer conflict".into());
    }
    retain_context(socket)
}

pub(super) fn bootstrap() -> Result<(), String> {
    if std::env::var_os(ENDPOINT_ENV).is_some() {
        // Admission checks availability; read/ACK do not bootstrap at all.
        let mailbox = MailboxDb::open_read_only(&MailboxDb::default_path()?)?;
        let domain = mailbox
            .completion_continuation_domain()?
            .ok_or("inherited endpoint has no native domain")?;
        let owner = require_owner(&domain)?;
        join(Path::new(&owner.endpoint))?;
        return Ok(());
    }
    validate_independent_entry()?;
    let path = MailboxDb::default_path()?;
    if path.exists() {
        let probe = MailboxDb::open_read_only(&path)?;
        if probe.completion_continuation_domain()?.is_none() {
            // Existing legacy operation remains usable. New v2 registration is
            // refused locally rather than silently installing a new writer lane.
            return Ok(());
        }
    }
    let mailbox = MailboxDb::open_completion_continuation_domain(&path)?;
    let domain = mailbox
        .completion_continuation_domain()?
        .ok_or("missing fresh native domain")?;
    drop(mailbox);
    let directory = PathBuf::from("/tmp")
        .join(format!("oulipoly-completion-{}-{domain}", unsafe {
            libc::geteuid()
        }));
    match std::fs::DirBuilder::new().mode(0o700).create(&directory) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.to_string()),
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
            join(&endpoint)?;
            unsafe { std::env::set_var(ENDPOINT_ENV, &endpoint) };
            return Ok(());
        }
        Err(e) => return Err(e.to_string()),
    }
    // Finish the normal State migration once, under independent bootstrap
    // election, before CD and the provider entry can open it concurrently.
    // This does not alter legacy-domain admission or read-only commands.
    drop(oulipoly_state::StateDb::open_default()?);
    let (mut ready, announce) = UnixStream::pair().map_err(|e| e.to_string())?;
    ready
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if pid == 0 {
        drop(ready);
        let code = guardian(&path, &endpoint, &domain, election, announce)
            .map(|()| 0)
            .unwrap_or(70);
        unsafe { libc::_exit(code) }
    }
    drop(announce);
    drop(election); // close only; never LOCK_UN the child's open description.
    let mut ready_byte = [0];
    ready
        .read_exact(&mut ready_byte)
        .map_err(|e| e.to_string())?;
    if ready_byte != [1] {
        return Err("completion guardian startup failed".into());
    }
    unsafe { std::env::set_var(ENDPOINT_ENV, &endpoint) };
    require_owner(&domain)?;
    retain_context(ready)?;
    Ok(())
}

fn guardian(
    path: &Path,
    endpoint: &Path,
    domain: &str,
    election: std::fs::File,
    mut announce: UnixStream,
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
    close_except(&[
        listener.as_raw_fd(),
        election.as_raw_fd(),
        announce.as_raw_fd(),
    ]);
    let mut owner = start_driver(
        path,
        endpoint,
        domain,
        election.as_raw_fd(),
        listener.as_raw_fd(),
    )?;
    announce.write_all(&[1]).map_err(|e| e.to_string())?;
    announce.set_nonblocking(true).map_err(|e| e.to_string())?;
    let mut contexts = vec![announce];
    let mut closing = false;
    loop {
        loop {
            let mut status = 0;
            let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
            if pid <= 0 {
                if closing
                    && pid < 0
                    && std::io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD)
                {
                    let _ = std::fs::remove_file(endpoint);
                    return Ok(());
                }
                break;
            }
            // A waited descendant is not automatically attributable drain for
            // an activation whose custodian was lost. Its durable row remains.
            if !closing && i64::from(pid) == owner.driver_identity.pid {
                owner = start_driver(
                    path,
                    endpoint,
                    domain,
                    election.as_raw_fd(),
                    listener.as_raw_fd(),
                )?;
            }
        }
        for _ in 0..8 {
            match listener.accept() {
                Ok((mut socket, _)) => {
                    if peer_pid(&socket).is_err() {
                        continue;
                    }
                    let _ = socket.set_read_timeout(Some(Duration::from_millis(50)));
                    let _ = socket.set_write_timeout(Some(Duration::from_millis(50)));
                    let mut request = [0; 6];
                    if socket.read_exact(&mut request).is_ok() && !closing {
                        if request == *b"hello\n" {
                            let _ = serde_json::to_writer(&mut socket, &owner);
                        } else if request == *b"join!\n"
                            && serde_json::to_writer(&mut socket, &owner).is_ok()
                            && socket.write_all(b"\n").is_ok()
                            && socket.set_nonblocking(true).is_ok()
                        {
                            contexts.push(socket);
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
        contexts.retain_mut(|socket| {
            let mut byte = [0];
            matches!(socket.read(&mut byte), Err(e) if matches!(e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted))
        });
        if !closing && contexts.is_empty() {
            // A missing/unreadable State DB is uncertainty, not no obligations.
            if let Ok(state) = oulipoly_state::StateDb::open_default() {
                closing = state
                    .close_idle_completion_continuation_owner(&owner)
                    .unwrap_or(false);
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn start_driver(
    path: &Path,
    endpoint: &Path,
    domain: &str,
    election: RawFd,
    listener: RawFd,
) -> Result<CompletionDomainOwner, String> {
    let (mut release, mut gate) = UnixStream::pair().map_err(|e| e.to_string())?;
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if pid == 0 {
        drop(release);
        unsafe { libc::close(listener) };
        close_except(&[election, gate.as_raw_fd()]);
        let mut bytes = Vec::new();
        let result = gate
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())
            .and_then(|_| {
                let owner: CompletionDomainOwner =
                    serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                unsafe { std::env::set_var(ENDPOINT_ENV, &owner.endpoint) };
                super::driver::run(path, &owner, election)
            });
        unsafe { libc::_exit(if result.is_ok() { 0 } else { 70 }) }
    }
    drop(gate);
    let owner = CompletionDomainOwner {
        protocol: PROTOCOL.into(),
        domain_id: domain.into(),
        owner_generation: uuid::Uuid::new_v4().to_string(),
        guardian_identity: identity(i64::from(std::process::id()))?,
        driver_identity: identity(i64::from(pid))?,
        endpoint: endpoint.to_string_lossy().into_owned(),
    };
    MailboxDb::open(path)?.publish_completion_continuation_owner(&owner)?;
    serde_json::to_writer(&mut release, &owner).map_err(|e| e.to_string())?;
    drop(release);
    Ok(owner)
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
