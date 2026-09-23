//! Host-side pinned completion authority and broker-attested root child join.
use oulipoly_kernel_broker::protocol::{self, JoinSpec, Operation};
use oulipoly_state::mailbox::MailboxDb;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::ExitCode;

const REQUIRED_ENV: &str = "OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1";
const CHILD_FD_ENV: &str = "OULIPOLY_KERNEL_CHILD_JOIN_FD_V1";

pub(crate) fn child_entry() -> Option<ExitCode> {
    let fd = std::env::var(CHILD_FD_ENV).ok()?;
    let result = (|| -> Result<ExitCode, String> {
        let fd: i32 = fd.parse().map_err(|_| "invalid child gate FD")?;
        if fd <= 2 || unsafe { libc::getppid() } != 1 {
            return Err("root child is not beneath namespace PID1".into());
        }
        let status = std::fs::read_to_string("/proc/self/status").map_err(|e| e.to_string())?;
        let nested = status
            .lines()
            .find(|line| line.starts_with("NSpid:"))
            .is_some_and(|line| line.split_ascii_whitespace().count() >= 3);
        if !nested {
            return Err("root child has no separate PID namespace".into());
        }
        let mut peer = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut len = std::mem::size_of_val(&peer) as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut peer as *mut libc::ucred).cast(),
                &mut len,
            )
        } != 0
            || len as usize != std::mem::size_of_val(&peer)
            || peer.uid != 0
        {
            return Err("root child gate has no host-root broker peer".into());
        }
        // The gate FD is inherited from the broker fork and is not a textual
        // UUID capability. Its peer is the live root broker process.
        let mut gate = unsafe { UnixStream::from_raw_fd(fd) };
        let mut line = Vec::new();
        loop {
            if line.len() >= 256 {
                return Err("oversized child grant".into());
            }
            let mut byte = [0];
            gate.read_exact(&mut byte).map_err(|e| e.to_string())?;
            if byte == [b'\n'] {
                break;
            }
            line.push(byte[0]);
        }
        let line = String::from_utf8(line).map_err(|_| "invalid child grant")?;
        let fields: Vec<_> = line.split(' ').collect();
        if fields.len() != 4
            || fields[..3]
                .iter()
                .any(|id| uuid::Uuid::parse_str(id).is_err())
        {
            return Err("invalid child grant identity".into());
        }
        let guardian_pid: i64 = fields[3].parse().map_err(|_| "invalid guardian PID")?;
        let mailbox = MailboxDb::open_read_only(&MailboxDb::default_path()?)?;
        let owner = mailbox
            .completion_continuation_owner()?
            .ok_or("missing child owner")?;
        if owner.protocol != oulipoly_state::completion_continuation::PROTOCOL
            || owner.domain_id != fields[1]
            || owner.supervisor_authority_id != fields[2]
            || owner.guardian_identity.pid != guardian_pid
            || mailbox
                .completion_owner_kernel_root_id(&owner.owner_generation)?
                .as_deref()
                != Some(fields[0])
        {
            return Err("child join does not match durable owner".into());
        }
        // The guardian's host PID is not a namespace-local PID. Ask the host
        // broker to verify the connected socket and both live incarnations.
        // This happens while the child still holds its one-use gate.
        let owner_socket = UnixStream::connect(&owner.endpoint).map_err(|e| e.to_string())?;
        #[cfg(feature = "age319-private-broker-fixture")]
        if std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1").is_some() {
            use oulipoly_kernel_broker::protocol::{
                ProcessWitness, SourceScope, SourceSocketWitness,
            };
            let mut changed = owner.clone();
            changed.guardian_identity.starttime_ticks += 1;
            if crate::completion_owner::verify_kernel_owner_socket(
                fields[0],
                &changed,
                &owner_socket,
            )
            .is_ok()
            {
                return Err("private fixture accepted changed guardian incarnation".into());
            }
            let (wrong_socket, _other_end) = UnixStream::pair().map_err(|e| e.to_string())?;
            if crate::completion_owner::verify_kernel_owner_socket(fields[0], &owner, &wrong_socket)
                .is_ok()
            {
                return Err("private fixture accepted unrelated owner socket".into());
            }
            let source = oulipoly_state::pid_identity::read_current_process_identity()?;
            let mut local_peer = libc::ucred {
                pid: 0,
                uid: 0,
                gid: 0,
            };
            let mut local_peer_len = std::mem::size_of_val(&local_peer) as libc::socklen_t;
            if unsafe {
                libc::getsockopt(
                    owner_socket.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_PEERCRED,
                    (&mut local_peer as *mut libc::ucred).cast(),
                    &mut local_peer_len,
                )
            } != 0
                || local_peer_len as usize != std::mem::size_of_val(&local_peer)
                || i64::from(local_peer.pid) == owner.guardian_identity.pid
            {
                return Err("private fixture did not exercise PID-domain mismatch".into());
            }
            let witness = SourceSocketWitness {
                root_id: fields[0].to_owned(),
                domain_id: owner.domain_id.clone(),
                supervisor_id: owner.supervisor_authority_id.clone(),
                guardian: ProcessWitness {
                    host_pid: i32::try_from(owner.guardian_identity.pid)
                        .map_err(|_| "invalid guardian host PID")?,
                    boot_id: owner.guardian_identity.boot_id.clone(),
                    starttime_ticks: u64::try_from(owner.guardian_identity.starttime_ticks)
                        .map_err(|_| "invalid guardian starttime")?,
                },
                source: ProcessWitness {
                    host_pid: i32::try_from(source.os_pid)
                        .map_err(|_| "invalid source host PID")?,
                    boot_id: source.os_boot_id,
                    starttime_ticks: u64::try_from(source.os_pid_starttime_ticks)
                        .map_err(|_| "invalid source starttime")?,
                },
                scope: SourceScope::Root,
            };
            let broker = PathBuf::from(
                std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1")
                    .ok_or("missing private broker socket")?,
            );
            let attest = |witness: &SourceSocketWitness, socket: &UnixStream| {
                protocol::verify_source_socket_at(&broker, witness, socket.as_raw_fd())
            };
            let mut stale = witness.clone();
            stale.source.starttime_ticks += 1;
            if attest(&stale, &owner_socket).is_ok() {
                return Err("private fixture accepted stale source".into());
            }
            stale.source.starttime_ticks = witness.source.starttime_ticks;
            stale.guardian.starttime_ticks += 1;
            if attest(&stale, &owner_socket).is_ok() {
                return Err("private fixture accepted stale guardian".into());
            }
            if attest(&witness, &wrong_socket).is_ok() {
                return Err("private fixture accepted wrong guardian socket".into());
            }
            stale.guardian.starttime_ticks = witness.guardian.starttime_ticks;
            stale.scope = SourceScope::Nested {
                parent_work_id: "sibling-work".into(),
            };
            if attest(&stale, &owner_socket).is_ok() {
                return Err("private fixture accepted unrelated work scope".into());
            }
            stale.scope = SourceScope::CancelOutside {
                work_id: "sibling-work".into(),
            };
            if attest(&stale, &owner_socket).is_ok() {
                return Err("private fixture accepted in-root outside cancellation".into());
            }
            stale.scope = SourceScope::Root;
            stale.root_id = uuid::Uuid::new_v4().to_string();
            if attest(&stale, &owner_socket).is_ok() {
                return Err("private fixture accepted sibling root".into());
            }
            attest(&witness, &owner_socket).map_err(|e| e.to_string())?;
            attest(&witness, &owner_socket).map_err(|e| e.to_string())?;
        }
        crate::completion_owner::verify_kernel_owner_socket(fields[0], &owner, &owner_socket)?;
        let observer_domain = oulipoly_state::pid_identity::procfs_observer_domain()?;
        unsafe {
            std::env::remove_var(CHILD_FD_ENV);
            std::env::set_var(crate::completion_owner::ENDPOINT_ENV, owner.endpoint);
            std::env::set_var(crate::completion_owner::EXPECTED_KERNEL_ROOT_ENV, fields[0]);
            std::env::set_var(
                oulipoly_state::pid_identity::PROCFS_OBSERVER_DOMAIN_ENV,
                observer_domain,
            );
        }
        drop(gate);
        #[cfg(feature = "age319-private-broker-fixture")]
        if std::env::args().nth(1).as_deref() == Some("__age319-private-join-only-v1") {
            crate::completion_owner::join_private_accepted_work_fixture()?;
            return Ok(ExitCode::SUCCESS);
        }
        #[cfg(feature = "age319-private-broker-fixture")]
        if std::env::args().nth(1).as_deref() == Some("__age319-private-bash-work-v1") {
            return private_bash_work();
        }
        Ok(crate::process_entrypoint())
    })();
    Some(match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("OULIPOLY_KERNEL_CHILD_JOIN_GAP={error}");
            ExitCode::FAILURE
        }
    })
}

#[cfg(feature = "age319-private-broker-fixture")]
fn private_bash_work() -> Result<ExitCode, String> {
    crate::completion_owner::join_private_accepted_work_fixture()?;
    let bash =
        std::env::var("AGE319_PRIVATE_BASH_IMAGE").map_err(|_| "private Bash image missing")?;
    let script =
        std::env::var("AGE319_PRIVATE_WORK_SCRIPT").map_err(|_| "private work script missing")?;
    let exit = std::process::Command::new(bash)
        .args(["run", "--delivery", "async", "--", "/bin/sh", "-c", &script])
        .status()
        .map_err(|error| error.to_string())?;
    Ok(ExitCode::from(exit.code().unwrap_or(70) as u8))
}

pub(crate) fn host_entry() -> Option<ExitCode> {
    if std::env::var_os(REQUIRED_ENV).is_none() {
        return None;
    }
    if let Err(error) = supported_host_mode() {
        eprintln!("OULIPOLY_KERNEL_ENTRY_GAP={error}");
        return Some(ExitCode::FAILURE);
    }
    let result = stage_host_entry(
        || {
            // Probe before E: a detached snapshot cannot create or migrate the
            // live databases. Missing State/domain initialization is a separate
            // administrative transition, never part of a failed custody grant.
            let path = MailboxDb::default_path()?;
            let state = oulipoly_state::schema_probe::run_schema_probe()
                .map_err(|e| format!("State preflight refused: {e:?}"))?
                .state_db;
            if !state.exists
                || state.user_version != state.current_schema_version
                || !state.compatible
            {
                return Err("current State domain must be initialized separately".into());
            }
            let domain = MailboxDb::open_read_only(&path)?
                .completion_continuation_domain()?
                .ok_or("native completion domain is absent")?;
            uuid::Uuid::parse_str(&domain).map_err(|_| "invalid native domain ID")?;
            Ok(domain)
        },
        || {
            let response = protocol::request_at(&broker_socket(), Operation::ReserveEntry)
                .map_err(|e| e.to_string())?;
            let id = response
                .strip_prefix("reserved ")
                .and_then(|s| s.strip_suffix('\n'))
                .ok_or_else(|| format!("kernel entry reservation refused: {}", response.trim()))?;
            uuid::Uuid::parse_str(id).map_err(|_| "invalid broker root ID".to_owned())?;
            Ok(id.to_owned())
        },
        bind_host_guardian,
    );
    match result {
        Ok(code) => Some(code),
        Err(error) => {
            eprintln!("OULIPOLY_KERNEL_ENTRY_GAP={error}");
            Some(ExitCode::FAILURE)
        }
    }
}

fn supported_host_mode() -> Result<(), String> {
    let args = std::env::args_os()
        .skip(1)
        .map(|arg| {
            arg.into_string()
                .map_err(|_| "non-UTF8 CLI argument".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    #[cfg(feature = "age319-private-broker-fixture")]
    let private_bash_fixture = args == ["__age319-private-bash-work-v1"]
        && unsafe { libc::geteuid() } == 0
        && std::fs::read_to_string("/proc/self/uid_map")
            .ok()
            .is_some_and(|map| map.split_ascii_whitespace().nth(2) == Some("1"))
        && std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1").is_some();
    #[cfg(not(feature = "age319-private-broker-fixture"))]
    let private_bash_fixture = false;
    if !private_bash_fixture && !protocol::supported_entry_args(&args) {
        return Err(
            "unsupported kernel CLI mode: only help and offline diagnostics are admitted".into(),
        );
    }
    if (0..=2).any(|fd| unsafe { libc::isatty(fd) } != 0) {
        return Err("TTY entry needs a separate descriptor handoff".into());
    }
    Ok(())
}

// This socket override is reachable only in an isolated user namespace where
// UID 0 is not host root. It permits a private unprivileged broker-like fixture
// to run the production bootstrap code without installing a host service.
fn broker_socket() -> PathBuf {
    #[cfg(feature = "age319-private-broker-fixture")]
    if unsafe { libc::geteuid() } == 0
        && std::fs::read_to_string("/proc/self/uid_map")
            .ok()
            .is_some_and(|map| map.split_ascii_whitespace().nth(2) == Some("1"))
        && let Some(path) = std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1")
    {
        return PathBuf::from(path);
    }
    PathBuf::from(protocol::INSTALLED_SOCKET)
}

fn stage_host_entry(
    preflight: impl FnOnce() -> Result<String, String>,
    reserve: impl FnOnce() -> Result<String, String>,
    bind: impl FnOnce(&str, &str, &str) -> Result<ExitCode, String>,
) -> Result<ExitCode, String> {
    let domain = preflight()?;
    uuid::Uuid::parse_str(&domain).map_err(|_| "invalid preflight domain ID")?;
    let root_id = reserve()?;
    uuid::Uuid::parse_str(&root_id).map_err(|_| "invalid reserved root ID")?;
    let supervisor = uuid::Uuid::new_v4().to_string();
    bind(&root_id, &domain, &supervisor)
}

fn bind_host_guardian(root: &str, domain: &str, supervisor: &str) -> Result<ExitCode, String> {
    let broker = broker_socket();
    let (mut parent, mut child) = UnixStream::pair().map_err(|e| e.to_string())?;
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if pid == 0 {
        drop(parent);
        let mut release = [0u8; 1];
        let result = (|| {
            child.read_exact(&mut release).map_err(|e| e.to_string())?;
            if release != [b'P'] {
                return Err("guardian gate refused".into());
            }
            let response = protocol::bind_guardian_at(&broker, root, domain, supervisor)
                .map_err(|e| format!("broker guardian binding failed: {e}"))?;
            child
                .write_all(response.as_bytes())
                .map_err(|e| e.to_string())?;
            // Remain pinned while the entry authenticates durable readback.
            let mut done = [0u8; 1];
            child.read_exact(&mut done).map_err(|e| e.to_string())?;
            if done != [b'X'] {
                return Err("guardian readback incomplete".into());
            }
            unsafe { std::env::remove_var(REQUIRED_ENV) };
            crate::completion_owner::run_pinned_guardian(
                &crate::completion_owner::PinnedGuardian {
                    root_id: root.into(),
                    domain_id: domain.into(),
                    supervisor_authority_id: supervisor.into(),
                },
                child,
            )
        })();
        unsafe { libc::_exit(if result.is_ok() { 0 } else { 70 }) }
    }
    drop(child);
    let result = (|| {
        let prepared = protocol::prepare_guardian_at(&broker, root, pid)
            .map_err(|e| format!("broker guardian prepare failed: {e}"))?;
        if prepared != format!("prepared {root}\n") {
            return Err(format!(
                "broker guardian prepare refused: {}",
                prepared.trim()
            ));
        }
        parent.write_all(b"P").map_err(|e| e.to_string())?;
        let mut bound = Vec::new();
        // The broker response is short; a child that does not provide one
        // keeps this entry blocked, never authorized to bootstrap or dispatch.
        loop {
            if bound.len() >= 256 {
                return Err("oversized guardian bind response".into());
            }
            let mut byte = [0u8; 1];
            parent.read_exact(&mut byte).map_err(|e| e.to_string())?;
            bound.push(byte[0]);
            if byte[0] == b'\n' {
                break;
            }
        }
        if bound != format!("bound {root} {domain} {supervisor}\n").as_bytes() {
            return Err("broker guardian binding refused or mismatched".into());
        }
        let readback = protocol::read_entry_at(&broker, root)
            .map_err(|e| format!("broker entry readback failed: {e}"))?;
        if readback != format!("bound-entry {root} {domain} {supervisor} {pid}\n") {
            return Err("broker durable grant readback mismatch".into());
        }
        parent.write_all(b"X").map_err(|e| e.to_string())?;
        let pin = crate::completion_owner::PinnedGuardian {
            root_id: root.into(),
            domain_id: domain.into(),
            supervisor_authority_id: supervisor.into(),
        };
        let root_authority =
            crate::completion_owner::verify_pinned_owner_ready(&mut parent, &pin, pid)?;
        let readback = protocol::read_entry_at(&broker, root)
            .map_err(|e| format!("broker final owner readback failed: {e}"))?;
        if readback != format!("bound-entry {root} {domain} {supervisor} {pid}\n") {
            return Err("broker final owner readback mismatch".into());
        }
        parent.write_all(b"R").map_err(|e| e.to_string())?;
        join_child(&broker, root, domain, supervisor, pid, root_authority)
    })();
    if result.is_err() {
        let _ = parent.shutdown(std::net::Shutdown::Both);
    }
    drop(parent);
    result
}

fn join_child(
    broker: &std::path::Path,
    root: &str,
    domain: &str,
    supervisor: &str,
    guardian_pid: i32,
    root_authority: String,
) -> Result<ExitCode, String> {
    let args = std::env::args_os()
        .skip(1)
        .map(|arg| {
            arg.into_string()
                .map_err(|_| "non-UTF8 CLI argument".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    if args.is_empty() {
        return Err("GUI entry needs a separate descriptor handoff".into());
    }
    let environment = std::env::vars_os()
        .map(|(key, value)| {
            Ok((
                key.into_string()
                    .map_err(|_| "non-UTF8 environment name".to_owned())?,
                value
                    .into_string()
                    .map_err(|_| "non-UTF8 environment value".to_owned())?,
            ))
        })
        .collect::<Result<Vec<_>, String>>()?
        .into_iter()
        .filter(|(key, _)| !key.starts_with("OULIPOLY_KERNEL_"))
        .collect();
    let cwd = File::open(".").map_err(|e| e.to_string())?;
    let (mut receipt, completion) = UnixStream::pair().map_err(|e| e.to_string())?;
    let spec = JoinSpec {
        root_id: root.into(),
        domain_id: domain.into(),
        supervisor_id: supervisor.into(),
        guardian_pid,
        root_authority,
        args,
        environment,
    };
    let response = protocol::join_at(
        broker,
        &spec,
        [0, 1, 2, cwd.as_raw_fd(), completion.as_raw_fd()],
    )
    .map_err(|e| format!("broker child join uncertain: {e}"))?;
    if !response.starts_with(&format!("joined {root} ")) || !response.ends_with('\n') {
        return Err(format!(
            "broker child join refused or uncertain: {}",
            response.trim()
        ));
    }
    drop(completion);
    let mut line = String::new();
    use std::io::BufRead;
    if std::io::BufReader::new(&mut receipt)
        .read_line(&mut line)
        .map_err(|e| e.to_string())?
        > 32
    {
        return Err("oversized child completion receipt".into());
    }
    let code: u8 = line
        .strip_prefix("exit ")
        .and_then(|s| s.strip_suffix('\n'))
        .ok_or("missing child completion receipt")?
        .parse()
        .map_err(|_| "invalid child exit code")?;
    Ok(ExitCode::from(code))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn pregrant_failure_never_reserves_or_bootstraps() {
        let result = stage_host_entry(
            || Err("missing native domain".into()),
            || panic!("reservation after failed preflight"),
            |_, _, _| panic!("binding after failed preflight"),
        );
        assert!(result.unwrap_err().contains("missing native domain"));
    }

    #[test]
    fn exact_binding_precedes_any_possible_child_handoff() {
        let events = RefCell::new(Vec::new());
        let root = uuid::Uuid::new_v4().to_string();
        let domain = uuid::Uuid::new_v4().to_string();
        let result = stage_host_entry(
            || {
                events.borrow_mut().push("read-only preflight");
                Ok(domain.clone())
            },
            || {
                events.borrow_mut().push("reserve");
                Ok(root.clone())
            },
            |r, d, s| {
                events.borrow_mut().push("durable bind and readback");
                assert_eq!((r, d), (root.as_str(), domain.as_str()));
                uuid::Uuid::parse_str(s).unwrap();
                Ok(ExitCode::SUCCESS)
            },
        );
        assert_eq!(
            events.into_inner(),
            [
                "read-only preflight",
                "reserve",
                "durable bind and readback"
            ]
        );
        assert!(result.is_ok());
    }
}
