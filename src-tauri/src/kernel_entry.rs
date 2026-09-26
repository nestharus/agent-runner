//! Host-side pinned completion authority and broker-attested root child join.

#[cfg(feature = "age319-private-broker-fixture")]
const PRIVATE_CHILD_EFFECT_WAIT: std::time::Duration = std::time::Duration::from_secs(20);
#[cfg(feature = "age319-private-broker-fixture")]
const PRIVATE_CHILD_EFFECT_POLL: std::time::Duration = std::time::Duration::from_millis(20);
#[cfg(feature = "age319-private-broker-fixture")]
const PRIVATE_PROVIDER_RESULT_WAIT: std::time::Duration = std::time::Duration::from_secs(20);
#[cfg(feature = "age319-private-broker-fixture")]
const PRIVATE_CAUSAL_BASH_RESULT_WAIT: std::time::Duration = std::time::Duration::from_secs(90);
#[cfg(feature = "age319-private-broker-fixture")]
const PRIVATE_PROVIDER_RESULT_POLL: std::time::Duration = std::time::Duration::from_millis(20);
#[cfg(feature = "age319-private-broker-fixture")]
const PRIVATE_LOST_REPLY_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
#[cfg(feature = "age319-private-broker-fixture")]
const PRIVATE_PREPARED_RELEASE_WAIT: std::time::Duration = std::time::Duration::from_secs(20);
#[cfg(feature = "age319-private-broker-fixture")]
const PRIVATE_PREPARED_FINISH_WAIT: std::time::Duration = std::time::Duration::from_secs(20);
#[cfg(feature = "age319-private-broker-fixture")]
const PRIVATE_PREPARED_GATE_POLL: std::time::Duration = std::time::Duration::from_millis(20);
#[cfg(feature = "age319-private-broker-fixture")]
const PRIVATE_V30_BARRIER_WAIT: std::time::Duration = std::time::Duration::from_secs(20);
#[cfg(feature = "age319-private-broker-fixture")]
const PRIVATE_V30_BARRIER_POLL: std::time::Duration = std::time::Duration::from_millis(20);
#[cfg(feature = "age319-private-broker-fixture")]
const PRIVATE_VERIFIED_OUTPUT_BUFFER_BYTES: usize = 64 * 1024;
#[cfg(feature = "age319-private-broker-fixture")]
const PRIVATE_ACCOUNT_EFFECT_POLL: std::time::Duration = std::time::Duration::from_millis(100);
#[cfg(feature = "age319-private-broker-fixture")]
const PRIVATE_BASH_F_RECEIPT_WAIT: std::time::Duration = std::time::Duration::from_secs(5);
#[cfg(feature = "age319-private-broker-fixture")]
const PRIVATE_BASH_F_RECEIPT_POLL: std::time::Duration = std::time::Duration::from_millis(10);

use oulipoly_kernel_broker::installed_pair::{self, InstalledPair};
use oulipoly_kernel_broker::protocol::{self, EntryRoute, JoinSpec, Operation, StateRoute};
use oulipoly_state::mailbox::MailboxDb;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::ExitCode;

#[cfg(feature = "age319-private-broker-fixture")]
mod root_pty_control;

#[cfg(feature = "age319-private-broker-fixture")]
const PRIVATE_PREPARED_ENTRY: &str = "__age319-private-held-prepared-v30";
#[cfg(feature = "age319-private-broker-fixture")]
const PRIVATE_NORMAL_ENTRY: &str = "__age319-private-normal-v30";

#[cfg(feature = "age319-private-broker-fixture")]
fn private_prepared_mode() -> bool {
    std::env::args().nth(1).as_deref() == Some(PRIVATE_PREPARED_ENTRY)
        && unsafe { libc::geteuid() } == 0
        && std::fs::read_to_string("/proc/self/uid_map")
            .ok()
            .is_some_and(|map| map.split_ascii_whitespace().nth(2) == Some("1"))
        && std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1").is_some()
}

#[cfg(feature = "age319-private-broker-fixture")]
fn private_normal_mode() -> bool {
    std::env::args().nth(1).as_deref().is_some_and(|arg| {
        arg == PRIVATE_NORMAL_ENTRY
            || arg == "__age319-private-bash-work-v1"
            || arg == "__age319-private-root-handoff-v1"
            || (arg == "--model" && std::env::var_os("AGE319_PRIVATE_NORMAL_ROOT_V1").is_some())
            || (arg == "--help" && std::env::var_os("AGE319_PRIVATE_OFFLINE_ROOT_V1").is_some())
    }) && unsafe { libc::geteuid() } == 0
        && std::fs::read_to_string("/proc/self/uid_map")
            .ok()
            .is_some_and(|map| map.split_ascii_whitespace().nth(2) == Some("1"))
        && std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1").is_some()
}

#[cfg(feature = "age319-private-broker-fixture")]
fn private_v30_child_mode() -> bool {
    std::env::var_os("OULIPOLY_KERNEL_V30_PRIVATE_CHILD_V1").is_some()
        && std::env::var_os(CHILD_FD_ENV).is_some()
        && unsafe { libc::geteuid() } == 0
        && std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1").is_some()
}

const REQUIRED_ENV: &str = "OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1";
const CHILD_FD_ENV: &str = "OULIPOLY_KERNEL_CHILD_JOIN_FD_V1";
const INSTALLED_RUNNER: &str = "/usr/local/libexec/oulipoly/oulipoly-agent-runner";

/// The fixed installed image and the explicit kernel entry path must consult
/// broker-owned ingress state before worker, helper, CLI, GUI, or State work.
/// A missing broker is an admission refusal for those supported paths.
pub(crate) fn verify_installed_entry_route() -> Result<(), String> {
    let image = std::env::current_exe()
        .map_err(|error| format!("cannot identify Runner image: {error}"))?;
    if !needs_installed_entry_gate(&image, std::env::var_os(REQUIRED_ENV).is_some()) {
        return Ok(());
    }
    if is_fixed_installed_image(&image) {
        let pair = InstalledPair::load(std::path::Path::new(installed_pair::MANIFEST), true)
            .map_err(|error| format!("installed pair manifest unavailable: {error}"))?;
        pair.verify_image(
            std::path::Path::new(INSTALLED_RUNNER),
            &pair.runner_sha256,
            true,
        )
        .map_err(|error| format!("installed Runner image mismatch: {error}"))?;
        let observation = protocol::observe_installed_pair_at(&broker_socket())
            .map_err(|error| format!("installed pair broker unavailable: {error}"))?;
        require_pair_route(&pair, &observation)?;
        return require_paired_launch_mode(
            std::env::var_os(REQUIRED_ENV).is_some(),
            std::env::var_os(CHILD_FD_ENV).is_some(),
        );
    }
    let route = protocol::observe_entry_gate_at(&broker_socket())
        .map_err(|error| format!("installed broker entry gate unavailable: {error}"))?;
    #[cfg(feature = "age319-private-broker-fixture")]
    if route == EntryRoute::BrokerV30Closed
        && (private_prepared_mode() || private_normal_mode() || private_v30_child_mode())
    {
        return Ok(());
    }
    require_legacy_entry_route(route)
}

fn needs_installed_entry_gate(image: &std::path::Path, explicit_kernel_entry: bool) -> bool {
    explicit_kernel_entry || is_fixed_installed_image(image)
}

fn is_fixed_installed_image(image: &std::path::Path) -> bool {
    image == std::path::Path::new(INSTALLED_RUNNER)
        || image == std::path::Path::new(&format!("{INSTALLED_RUNNER} (deleted)"))
}

fn require_pair_route(
    pair: &InstalledPair,
    observed: &protocol::InstalledPairObservation,
) -> Result<(), String> {
    if pair.version != observed.version || pair.generation != observed.generation {
        return Err("installed broker and Runner generation differ".into());
    }
    require_legacy_entry_route(observed.route)
}

fn require_paired_launch_mode(host_entry: bool, child_entry: bool) -> Result<(), String> {
    if host_entry ^ child_entry {
        Ok(())
    } else {
        Err("paired Runner requires one broker-owned entry path".into())
    }
}

fn require_legacy_entry_route(route: EntryRoute) -> Result<(), String> {
    match route {
        EntryRoute::LegacyOpen => Ok(()),
        EntryRoute::Draining => Err("installed broker entry gate is draining".into()),
        EntryRoute::BrokerV30Closed => Err("installed Runner has no v30 route".into()),
    }
}

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
        if let Some(versioned) = line.strip_prefix("v30 ") {
            return child_v30_entry(versioned, gate);
        }
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
fn private_v30_source_witness(
    evidence: &oulipoly_state::mailbox::BrokerReleaseEvidence,
    gate_dir: &std::path::Path,
) -> Result<(), String> {
    use oulipoly_kernel_broker::protocol::{
        OwnerWitness, PrivateSourceWitnessProbe, ProcessWitness,
    };
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::OpenOptionsExt;

    std::fs::write(gate_dir.join("source-stage"), b"building-registration")
        .map_err(|e| e.to_string())?;

    let request: serde_json::Value = serde_json::from_slice(
        &std::fs::read(gate_dir.join("source-witness-request")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let invocation = request["invocation"]
        .as_str()
        .ok_or("private source invocation absent")?
        .to_owned();
    let session = request["session"]
        .as_str()
        .ok_or("private source session absent")?
        .to_owned();
    let capability = request["capability"]
        .as_str()
        .ok_or("private source capability absent")?
        .to_owned();
    let source_dir = gate_dir.join("spool/ab_source_witness");
    std::fs::create_dir_all(&source_dir).map_err(|e| e.to_string())?;
    let path = source_dir.join("source-registration-v2.json");
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../crates/oulipoly-state/tests/fixtures/age360-paired-wire.json"
    ))
    .map_err(|e| e.to_string())?;
    let mut source: serde_json::Value = serde_json::from_str(
        fixture["registration_bytes_utf8"]
            .as_str()
            .ok_or("registration fixture absent")?,
    )
    .map_err(|e| e.to_string())?;
    source["domain_id"] = evidence.prepared.domain_id.clone().into();
    source["owner_invocation_uuid"] = invocation.clone().into();
    source["owner_session_id"] = session.clone().into();
    source["listeners"][0]["listener_id"] = invocation.clone().into();
    source["listeners"][0]["owner_invocation_uuid"] = invocation.clone().into();
    source["listeners"][0]["session_id"] = session.clone().into();
    source["spool_root"] = source_dir
        .parent()
        .unwrap()
        .to_string_lossy()
        .into_owned()
        .into();
    source["handle"] = "ab_source_witness".into();
    source["handle_dir"] = source_dir.to_string_lossy().into_owned().into();
    source["helper"]["path"] = source_dir
        .join("runner")
        .to_string_lossy()
        .into_owned()
        .into();
    source["recovery"]["path"] = source_dir
        .join("agent-bash")
        .to_string_lossy()
        .into_owned()
        .into();
    source["registering_caller"]["pid"] = i64::from(evidence.prepared.joined_child.host_pid).into();
    source["registering_caller"]["boot_id"] = evidence.prepared.joined_child.boot_id.clone().into();
    source["registering_caller"]["starttime_ticks"] =
        i64::try_from(evidence.prepared.joined_child.starttime_ticks)
            .map_err(|e| e.to_string())?
            .into();
    let bytes = serde_json::to_vec(&source).map_err(|e| e.to_string())?;
    let parsed: oulipoly_state::completion_continuation::SourceRegistration =
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    parsed.validate()?;
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    let registration = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)
        .map_err(|e| e.to_string())?;
    std::fs::write(gate_dir.join("source-stage"), b"connecting-guardian")
        .map_err(|e| e.to_string())?;
    let mut owner_socket =
        UnixStream::connect(&evidence.owner.endpoint).map_err(|e| e.to_string())?;
    std::fs::write(gate_dir.join("source-stage"), b"guardian-connected")
        .map_err(|e| e.to_string())?;
    owner_socket
        .write_all(b"hello\n")
        .map_err(|e| e.to_string())?;
    let mut hello = Vec::new();
    (&mut owner_socket)
        .take(8193)
        .read_to_end(&mut hello)
        .map_err(|e| e.to_string())?;
    let observed: oulipoly_state::mailbox::CompletionDomainOwner =
        serde_json::from_slice(&hello).map_err(|e| e.to_string())?;
    if observed != evidence.owner {
        return Err("private guardian hello changed released owner".into());
    }
    std::fs::write(gate_dir.join("source-stage"), b"guardian-hello-complete")
        .map_err(|e| e.to_string())?;
    let stamp = |process: &oulipoly_state::mailbox::PreparedProcessStamp| ProcessWitness {
        host_pid: process.host_pid,
        boot_id: process.boot_id.clone(),
        starttime_ticks: process.starttime_ticks,
    };
    let witness = OwnerWitness {
        root_id: evidence.prepared.root_id.clone(),
        domain_id: evidence.prepared.domain_id.clone(),
        supervisor_id: evidence.prepared.supervisor_authority_id.clone(),
        guardian: stamp(&evidence.prepared.guardian),
        driver: stamp(&evidence.prepared.driver),
        owner_generation: Some(evidence.prepared.owner_generation.clone()),
        work_id: None,
        owner_session_id: Some(session.clone()),
        owner_invocation_uuid: Some(invocation.clone()),
        registration_authority_sha256: Some(format!("{:x}", Sha256::digest(capability.as_bytes()))),
    };
    let probe = PrivateSourceWitnessProbe {
        owner: witness.clone(),
        source_generation: evidence.prepared.source_generation.clone(),
        owner_generation: evidence.prepared.owner_generation.clone(),
        registration_path: path.clone(),
        registration_len: bytes.len() as u64,
        registration_sha256: format!("{:x}", Sha256::digest(&bytes)),
        owner_session_id: session,
        owner_invocation_uuid: invocation,
        capability,
    };
    let broker = broker_socket();
    let check = |probe: &PrivateSourceWitnessProbe, fd: &File| {
        protocol::private_source_witness_probe_at(
            &broker,
            probe,
            owner_socket.as_raw_fd(),
            fd.as_raw_fd(),
        )
    };
    if protocol::verify_owner_at(&broker, &witness, owner_socket.as_raw_fd()).is_ok() {
        return Err("legacy V opened under v30 cutover".into());
    }
    std::fs::write(gate_dir.join("source-stage"), b"requesting-broker")
        .map_err(|e| e.to_string())?;
    check(&probe, &registration).map_err(|e| format!("combined source witness: {e}"))?;
    std::fs::write(gate_dir.join("source-stage"), b"broker-positive").map_err(|e| e.to_string())?;
    let (wrong_guardian, _peer) = UnixStream::pair().map_err(|e| e.to_string())?;
    if !protocol::private_source_witness_probe_at(
        &broker,
        &probe,
        wrong_guardian.as_raw_fd(),
        registration.as_raw_fd(),
    )
    .is_err_and(|error| {
        error
            .to_string()
            .contains("owner socket peer is not pinned host guardian")
    }) {
        return Err("unconnected guardian socket accepted".into());
    }
    let mut wrong = probe.clone();
    std::fs::write(gate_dir.join("source-stage"), b"other-root").map_err(|e| e.to_string())?;
    wrong.owner.root_id = uuid::Uuid::new_v4().to_string();
    if !check(&wrong, &registration)
        .is_err_and(|error| error.to_string().contains("owner witness root absent"))
    {
        return Err("other root source witness accepted".into());
    }
    wrong = probe.clone();
    std::fs::write(gate_dir.join("source-stage"), b"owner-generation")
        .map_err(|e| e.to_string())?;
    wrong.owner_generation = uuid::Uuid::new_v4().to_string();
    wrong.owner.owner_generation = Some(wrong.owner_generation.clone());
    if !check(&wrong, &registration)
        .is_err_and(|error| error.to_string().contains("broker prepared owner absent"))
    {
        return Err("wrong owner generation accepted".into());
    }
    wrong = probe.clone();
    wrong.source_generation = uuid::Uuid::new_v4().to_string();
    if !check(&wrong, &registration).is_err_and(|error| {
        error
            .to_string()
            .contains("broker prepared source generation changed")
    }) {
        return Err("wrong source generation accepted".into());
    }
    wrong = probe.clone();
    std::fs::write(gate_dir.join("source-stage"), b"wrong-invocation")
        .map_err(|e| e.to_string())?;
    wrong.owner_invocation_uuid = uuid::Uuid::new_v4().to_string();
    wrong.owner.owner_invocation_uuid = Some(wrong.owner_invocation_uuid.clone());
    let mut changed_source = source.clone();
    changed_source["owner_invocation_uuid"] = wrong.owner_invocation_uuid.clone().into();
    changed_source["listeners"][0]["listener_id"] = wrong.owner_invocation_uuid.clone().into();
    changed_source["listeners"][0]["owner_invocation_uuid"] =
        wrong.owner_invocation_uuid.clone().into();
    let changed_bytes = serde_json::to_vec(&changed_source).map_err(|e| e.to_string())?;
    std::fs::write(&path, &changed_bytes).map_err(|e| e.to_string())?;
    wrong.registration_len = changed_bytes.len() as u64;
    wrong.registration_sha256 = format!("{:x}", Sha256::digest(&changed_bytes));
    let wrong_invocation = check(&wrong, &registration);
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    if !wrong_invocation.is_err_and(|error| {
        error
            .to_string()
            .contains("original State invocation absent")
    }) {
        return Err("wrong invocation accepted".into());
    }
    wrong = probe.clone();
    std::fs::write(gate_dir.join("source-stage"), b"wrong-capability")
        .map_err(|e| e.to_string())?;
    wrong.capability = "0".repeat(64);
    wrong.owner.registration_authority_sha256 =
        Some(format!("{:x}", Sha256::digest(wrong.capability.as_bytes())));
    if !check(&wrong, &registration).is_err_and(|error| {
        error
            .to_string()
            .contains("original State invocation/session/capability mismatch")
    }) {
        return Err("wrong capability accepted".into());
    }
    let different = source_dir.join("different.json");
    std::fs::write(gate_dir.join("source-stage"), b"different-fd").map_err(|e| e.to_string())?;
    std::fs::write(&different, &bytes).map_err(|e| e.to_string())?;
    let different_fd = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&different)
        .map_err(|e| e.to_string())?;
    if !check(&probe, &different_fd).is_err_and(|error| {
        error
            .to_string()
            .contains("private registration FD/path identity mismatch")
    }) {
        return Err("copied registration FD accepted".into());
    }
    let fork = unsafe { libc::fork() };
    std::fs::write(gate_dir.join("source-stage"), b"copied-child").map_err(|e| e.to_string())?;
    if fork < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if fork == 0 {
        let refused = check(&probe, &registration).is_err_and(|error| {
            error
                .to_string()
                .contains("owner witness is outside consumed work")
        });
        unsafe { libc::_exit(if refused { 0 } else { 70 }) }
    }
    let mut status = 0;
    if unsafe { libc::waitpid(fork, &mut status, 0) } != fork
        || !libc::WIFEXITED(status)
        || libc::WEXITSTATUS(status) != 0
    {
        return Err("copied child process identity accepted".into());
    }
    std::fs::write(&path, b"changed registration bytes").map_err(|e| e.to_string())?;
    std::fs::write(gate_dir.join("source-stage"), b"changed-bytes").map_err(|e| e.to_string())?;
    if !check(&probe, &registration).is_err_and(|error| {
        error
            .to_string()
            .contains("private registration asserted bytes mismatch")
    }) {
        return Err("changed registration bytes accepted".into());
    }
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    std::fs::write(
        gate_dir.join("source-witness-positive"),
        serde_json::to_vec(&serde_json::json!({
            "root_id": probe.owner.root_id,
            "source_generation": probe.source_generation,
            "owner_generation": probe.owner_generation,
            "registration_path": path,
            "registration_sha256": probe.registration_sha256,
            "registration_len": probe.registration_len,
            "invocation_uuid": probe.owner_invocation_uuid,
            "session_id": probe.owner_session_id,
        }))
        .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + PRIVATE_CHILD_EFFECT_WAIT;
    while !gate_dir.join("source-state-replaced").exists() {
        if std::time::Instant::now() >= deadline {
            return Err("source State replacement gate expired".into());
        }
        std::thread::sleep(PRIVATE_CHILD_EFFECT_POLL);
    }
    if !check(&probe, &registration).is_err_and(|error| {
        error
            .to_string()
            .contains("broker StateDb source identity changed")
    }) {
        return Err("changed original State inode accepted".into());
    }
    std::fs::write(gate_dir.join("source-witness-negatives-done"), b"yes")
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn child_v30_entry(grant: &str, gate: UnixStream) -> Result<ExitCode, String> {
    let fields: Vec<_> = grant.split(' ').collect();
    if fields.len() != 6
        || fields[..5].iter().any(|field| {
            uuid::Uuid::parse_str(field)
                .map(|id| id.to_string() != *field)
                .unwrap_or(true)
        })
    {
        return Err("invalid v30 child grant".into());
    }
    let guardian_pid: i32 = fields[5].parse().map_err(|_| "invalid v30 guardian PID")?;
    let spec = protocol::StateReadSpec {
        protocol: "broker-release-attest-v30".into(),
        source_generation: fields[0].into(),
        owner_generation: fields[1].into(),
        root_id: fields[2].into(),
        attempt_id: None,
    };
    let evidence = protocol::attest_released_child_at(&broker_socket(), &spec)
        .map_err(|e| format!("v30 child release absent: {e}"))?;
    if evidence.prepared.source_generation != fields[0]
        || evidence.prepared.owner_generation != fields[1]
        || evidence.prepared.root_id != fields[2]
        || evidence.prepared.domain_id != fields[3]
        || evidence.prepared.supervisor_authority_id != fields[4]
        || evidence.prepared.guardian.host_pid != guardian_pid
        || evidence.owner.owner_generation != fields[1]
        || evidence.owner.domain_id != fields[3]
        || evidence.owner.supervisor_authority_id != fields[4]
    {
        return Err("v30 child attestation does not match physical gate".into());
    }
    #[cfg(feature = "age319-private-broker-fixture")]
    let private_help = private_v30_child_mode()
        && std::env::var_os("AGE319_PRIVATE_OFFLINE_ROOT_V1").is_some()
        && std::env::args().nth(1).as_deref() == Some("--help")
        && std::env::args().nth(2).is_none();
    #[cfg(feature = "age319-private-broker-fixture")]
    let private_handoff = private_v30_child_mode()
        && (std::env::args().nth(1).as_deref() == Some("__age319-private-root-handoff-v1")
            || private_help
            || (std::env::var_os("AGE319_PRIVATE_NORMAL_ROOT_V1").is_some()
                && std::env::args().nth(1).as_deref() == Some("--model")));
    #[cfg(feature = "age319-private-broker-fixture")]
    let request_handoff = private_handoff || !private_v30_child_mode();
    #[cfg(not(feature = "age319-private-broker-fixture"))]
    let request_handoff = true;
    #[cfg(feature = "age319-private-broker-fixture")]
    let mut private_receipt = None;
    let mut effect_binding = None;
    if request_handoff {
        let fresh_socket = broker_socket().with_file_name("v30.sock");
        #[cfg(feature = "age319-private-broker-fixture")]
        if private_handoff && std::env::var_os("AGE319_PRIVATE_HANDOFF_REPLY_LOSS_V1").is_some() {
            private_drop_fresh_reply(&fresh_socket, b'U', Some(spec_for_handoff(&spec)), None)?;
        }
        let receipt =
            protocol::request_released_fresh_handoff_at(&fresh_socket, spec_for_handoff(&spec))
                .map_err(|e| format!("v30 released-child handoff absent: {e}"))?;
        if receipt.old_release != evidence {
            return Err("v30 handoff release readback conflict".into());
        }
        #[cfg(feature = "age319-private-broker-fixture")]
        if private_handoff && std::env::var_os("AGE319_PRIVATE_HANDOFF_REPLY_LOSS_V1").is_some() {
            private_drop_fresh_reply(&fresh_socket, b'D', None, Some(&receipt.d_key))?;
        }
        let session = protocol::allocate_fresh_v30_session_at(&fresh_socket, &receipt.d_key)
            .map_err(|e| format!("v30 released-child D absent: {e}"))?;
        let readback = protocol::read_fresh_v30_session_at(&fresh_socket, &receipt.d_key)
            .map_err(|e| format!("v30 released-child d absent: {e}"))?;
        if readback.as_ref() != Some(&session) || session.request_id != receipt.d_key {
            return Err("v30 released-child D readback conflict".into());
        }
        #[cfg(feature = "age319-private-broker-fixture")]
        if private_handoff {
            if std::env::var_os("AGE319_PRIVATE_FRESH_PROVIDER_V1").is_none() {
                // The older handoff fixture runs sibling probes. The provider
                // proof keeps this released root free of local forks before K.
                let child = std::env::current_exe().map_err(|e| e.to_string())?;
                let release_json = serde_json::to_string(&spec).map_err(|e| e.to_string())?;
                for (key, value) in [
                    ("AGE319_PRIVATE_HANDOFF_PROBE_SPEC", release_json.as_str()),
                    ("AGE319_PRIVATE_HANDOFF_PROBE_D_KEY", receipt.d_key.as_str()),
                ] {
                    let outcome = std::process::Command::new(&child)
                        .arg("__age319-private-handoff-probe-v1")
                        .env_remove(REQUIRED_ENV)
                        .env_remove(CHILD_FD_ENV)
                        .env_remove("AGE319_PRIVATE_HANDOFF_PROBE_SPEC")
                        .env_remove("AGE319_PRIVATE_HANDOFF_PROBE_D_KEY")
                        .env(key, value)
                        .status()
                        .map_err(|e| e.to_string())?;
                    if outcome.success() {
                        return Err("unregistered later root descendant reused root U/D".into());
                    }
                }
            }
            let status = std::fs::read_to_string("/proc/self/status").map_err(|e| e.to_string())?;
            let status_value = |name: &str| -> Result<u32, String> {
                status
                    .lines()
                    .find_map(|line| line.strip_prefix(name))
                    .and_then(|value| value.trim().parse().ok())
                    .ok_or_else(|| format!("root child status field {name} absent"))
            };
            private_v30_marker(
                "child-handoff",
                &serde_json::json!({
                    "handoff_id": receipt.handoff_id,
                    "d_key": receipt.d_key,
                    "invocation_uuid": receipt.invocation_uuid,
                    "root_work_intent": receipt.root_work_intent,
                    "session_id": session.session_id,
                    "no_new_privs": status_value("NoNewPrivs:")?,
                    "seccomp": status_value("Seccomp:")?,
                }),
            )?;
            private_receipt = Some(receipt.clone());
            if std::env::var_os("AGE319_PRIVATE_BASH_CHILD_V1").is_some() {
                let bash = std::env::var("AGE319_PRIVATE_BASH_IMAGE")
                    .map_err(|_| "private Bash source image absent")?;
                let output = std::process::Command::new(&bash)
                    .arg("__age319-private-admit-child-v1")
                    .output()
                    .map_err(|e| e.to_string())?;
                if output.status.success()
                    || !String::from_utf8_lossy(&output.stderr)
                        .contains("consumed causal parent work grant absent")
                {
                    return Err(format!(
                        "root-only Bash was not refused: {}",
                        String::from_utf8_lossy(&output.stderr)
                    ));
                }
                // A direct child and a grandchild both lack a consumed parent
                // K. Their root membership, image and copied environment do
                // not authorize C.
                let unrelated_marker = PathBuf::from(
                    std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1")
                        .ok_or("private gate directory absent")?,
                )
                .join(format!("unrelated-bash-{}", uuid::Uuid::new_v4()));
                let unrelated = std::process::Command::new("/bin/sh")
                    .args(["-c", "\"$1\" __age319-private-admit-child-v1", "sh", &bash])
                    .env(
                        "AGE319_PRIVATE_BASH_REQUEST_KEY",
                        uuid::Uuid::new_v4().to_string(),
                    )
                    .env("AGE319_PRIVATE_BASH_EFFECT_MARKER", &unrelated_marker)
                    .output()
                    .map_err(|e| e.to_string())?;
                if unrelated.status.success() || unrelated_marker.exists() {
                    return Err("in-root Bash grandchild acquired fresh child admission".into());
                }
                if !String::from_utf8_lossy(&unrelated.stderr)
                    .contains("consumed causal parent work grant absent")
                {
                    return Err(format!(
                        "in-root Bash grandchild refused for unexpected reason: {}",
                        String::from_utf8_lossy(&unrelated.stderr)
                    ));
                }
                private_v30_marker(
                    "bash-child-refused",
                    &serde_json::json!({
                        "direct": String::from_utf8_lossy(&output.stderr),
                        "grandchild": String::from_utf8_lossy(&unrelated.stderr),
                    }),
                )?;
            }
        }
        effect_binding = Some((receipt, session));
    }
    drop(gate);
    #[cfg(feature = "age319-private-broker-fixture")]
    if private_v30_child_mode() {
        let gate_dir = PathBuf::from(
            std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1")
                .ok_or("private v30 gate directory absent")?,
        );
        if gate_dir.join("source-witness-request").exists() {
            private_v30_source_witness(&evidence, &gate_dir)?;
        }
        std::fs::write(
            gate_dir.join("child-attested"),
            evidence.release_id.as_bytes(),
        )
        .map_err(|e| e.to_string())?;
        let deadline = std::time::Instant::now() + PRIVATE_CHILD_EFFECT_WAIT;
        let mut retried = false;
        while !gate_dir.join("child-effect").exists() {
            if !retried && gate_dir.join("child-retry").exists() {
                let original = private_receipt
                    .as_ref()
                    .ok_or("private handoff retry has no original receipt")?;
                let fresh_socket = broker_socket().with_file_name("v30.sock");
                let mut wrong_release = spec_for_handoff(&spec);
                wrong_release.owner_generation = uuid::Uuid::new_v4().to_string();
                if protocol::request_released_fresh_handoff_at(&fresh_socket, wrong_release).is_ok()
                {
                    return Err("stale owner generation obtained a handoff".into());
                }
                let repeated = protocol::request_released_fresh_handoff_at(
                    &fresh_socket,
                    spec_for_handoff(&spec),
                )
                .map_err(|e| format!("v30 handoff retry failed: {e}"))?;
                if repeated != *original {
                    return Err("v30 handoff retry minted another identity".into());
                }
                if protocol::allocate_fresh_v30_session_at(
                    &fresh_socket,
                    &uuid::Uuid::new_v4().to_string(),
                )
                .is_ok()
                {
                    return Err("wrong D key allocated a fresh session".into());
                }
                let allocated =
                    protocol::allocate_fresh_v30_session_at(&fresh_socket, &original.d_key)
                        .map_err(|e| format!("v30 D retry failed: {e}"))?;
                if protocol::read_fresh_v30_session_at(&fresh_socket, &original.d_key)
                    .map_err(|e| e.to_string())?
                    != Some(allocated)
                {
                    return Err("v30 D retry readback changed".into());
                }
                std::fs::write(gate_dir.join("child-retried"), b"yes")
                    .map_err(|e| e.to_string())?;
                retried = true;
            }
            if std::time::Instant::now() >= deadline {
                return Err("private v30 child effect wait expired".into());
            }
            std::thread::sleep(PRIVATE_CHILD_EFFECT_POLL);
        }
        if let Some(original) = private_receipt.as_ref() {
            let current = protocol::request_released_fresh_handoff_at(
                &broker_socket().with_file_name("v30.sock"),
                spec_for_handoff(&spec),
            )
            .map_err(|e| format!("v30 handoff no longer live: {e}"))?;
            if current != *original {
                return Err("v30 released handoff changed before private marker".into());
            }
            let (receipt, session) = effect_binding
                .as_ref()
                .ok_or("private root effect binding absent")?;
            if std::env::var_os("AGE319_PRIVATE_EFFECT_REPLY_LOSS_V1").is_some() {
                private_drop_fresh_reply(
                    &broker_socket().with_file_name("v30.sock"),
                    b'0',
                    None,
                    Some(&receipt.d_key),
                )?;
                let unknown = protocol::observe_fresh_root_effect_at(
                    &broker_socket().with_file_name("v30.sock"),
                    &receipt.d_key,
                )
                .map_err(|e| e.to_string())?;
                if !unknown.is_some_and(|effect| {
                    effect.state == oulipoly_state::mailbox::FreshRootEffectState::Started
                }) {
                    return Err("lost root effect reply did not retain unknown start".into());
                }
                return Err("private root effect start reply lost; execution refused".into());
            }
            if matches!(
                receipt.root_work_intent,
                oulipoly_state::mailbox::FreshRootWorkIntent::NormalCli(_)
            ) {
                prepare_normal_work(receipt, session)?;
                #[cfg(feature = "age319-private-broker-fixture")]
                if std::env::var_os("AGE319_PRIVATE_FRESH_PROVIDER_V1").is_some() {
                    return private_fresh_provider(FreshEntryAuthority { receipt, session });
                }
                return Err("normal provider route held: native K/Q, result and physical custody are absent".into());
            }
            if !private_help {
                begin_root_effect(receipt, session)?;
            }
        } else {
            let current = protocol::attest_released_child_at(&broker_socket(), &spec)
                .map_err(|e| format!("v30 child release no longer live: {e}"))?;
            if current != evidence {
                return Err("v30 child release changed before effect".into());
            }
        }
        if !private_help {
            println!("OULIPOLY_KERNEL_V30_CHILD_EFFECT={}", evidence.release_id);
            if let Some((receipt, session)) = effect_binding.as_ref() {
                return_root_effect(receipt, session, true)?;
            }
            return Ok(ExitCode::SUCCESS);
        }
    }
    let (receipt, session) = effect_binding.ok_or("v30 root U/D binding absent")?;
    if std::env::args().skip(1).collect::<Vec<_>>() != receipt.root_work_intent.arguments() {
        return Err("root entry argv changed after broker release".into());
    }
    if !receipt.root_work_intent.returnable_entry() {
        if matches!(
            receipt.root_work_intent,
            oulipoly_state::mailbox::FreshRootWorkIntent::NormalCli(_)
        ) {
            prepare_normal_work(&receipt, &session)?;
            return Err(
                "normal provider route held: native K/Q, result and physical custody are absent"
                    .into(),
            );
        }
        return Err("root entry intent has no returnable effect/result path".into());
    }
    begin_root_effect(&receipt, &session)?;
    let result = crate::process_entrypoint();
    return_root_effect(&receipt, &session, result == ExitCode::SUCCESS)?;
    Ok(result)
}

fn prepare_normal_work(
    receipt: &oulipoly_state::mailbox::FreshReleasedHandoff,
    session: &oulipoly_state::mailbox::FreshV30Session,
) -> Result<(), String> {
    let socket = broker_socket().with_file_name("v30.sock");
    let preparation = protocol::prepare_fresh_normal_work_at(&socket, &receipt.d_key)
        .or_else(|_| {
            protocol::observe_fresh_normal_work_at(&socket, &receipt.d_key).and_then(|value| {
                value.ok_or_else(|| std::io::Error::other("normal work preparation unknown"))
            })
        })
        .map_err(|e| format!("normal work preparation absent or unknown: {e}"))?;
    if preparation.handoff_id != receipt.handoff_id
        || preparation.invocation_uuid != receipt.invocation_uuid
        || preparation.session_id != session.session_id
        || preparation.intent != receipt.root_work_intent
        || preparation.state != "held"
    {
        return Err("normal work preparation readback conflict".into());
    }
    #[cfg(feature = "age319-private-broker-fixture")]
    if std::env::var_os("AGE319_PRIVATE_NORMAL_ROOT_V1").is_some() {
        let repeated = protocol::prepare_fresh_normal_work_at(&socket, &receipt.d_key)
            .map_err(|e| format!("private normal preparation retry failed: {e}"))?;
        if repeated != preparation {
            return Err("normal work retry minted a second preparation".into());
        }
    }
    Ok(())
}

#[cfg(feature = "age319-private-broker-fixture")]
struct FreshEntryAuthority<'a> {
    receipt: &'a oulipoly_state::mailbox::FreshReleasedHandoff,
    session: &'a oulipoly_state::mailbox::FreshV30Session,
}

#[cfg(feature = "age319-private-broker-fixture")]
struct PrivateFreshBroker<'a> {
    authority: FreshEntryAuthority<'a>,
    grant_id: Option<String>,
    raw_result: Option<oulipoly_runtime::executor::cli::fresh_remote::FreshProviderCompletion>,
}

#[cfg(feature = "age319-private-broker-fixture")]
impl PrivateFreshBroker<'_> {
    fn unknown(&self, grant: Option<&str>, stage: &str, reason: &str) -> String {
        let receipt = self.authority.receipt;
        private_provider_unknown(
            &receipt.d_key,
            &receipt.handoff_id,
            &self.authority.session.session_id,
            grant,
            &broker_socket().with_file_name("v30.sock"),
            stage,
            reason,
        )
    }
}

#[cfg(feature = "age319-private-broker-fixture")]
fn private_provider_unknown(
    d_key: &str,
    handoff_id: &str,
    session_id: &str,
    grant: Option<&str>,
    socket: &std::path::Path,
    stage: &str,
    reason: &str,
) -> String {
    format!(
        "fresh provider unknown: {}",
        serde_json::json!({
            "stage": stage,
            "reason": reason,
            "d_key": d_key,
            "handoff_id": handoff_id,
            "session_id": session_id,
            "grant_id": grant,
            "broker_socket": socket,
            "grant_artifact": format!("v30/fresh-provider/{handoff_id}.fresh-grant.json"),
            "effect_artifact_prefix": grant.map(|id| format!("v30/fresh-provider/{id}.")),
            "automatic_replay": false,
            "caller_retry_duplicate_effect_risk": true,
        })
    )
}

#[cfg(feature = "age319-private-broker-fixture")]
struct PrivatePinnedPlan {
    image: File,
    cwd: File,
    input: File,
    recipe: File,
}

#[cfg(feature = "age319-private-broker-fixture")]
impl PrivatePinnedPlan {
    fn descriptors(&self) -> [std::os::fd::RawFd; 4] {
        [
            self.image.as_raw_fd(),
            self.cwd.as_raw_fd(),
            self.input.as_raw_fd(),
            self.recipe.as_raw_fd(),
        ]
    }
}

#[cfg(feature = "age319-private-broker-fixture")]
fn private_pin_plan(
    plan: &oulipoly_runtime::executor::cli::fresh_remote::FreshProviderPlan,
    role: oulipoly_kernel_broker::protocol::FreshPlanRole,
) -> Result<PrivatePinnedPlan, String> {
    use std::os::unix::fs::OpenOptionsExt;
    let image = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_PATH)
        .open(&plan.executable)
        .map_err(|e| format!("fresh provider image: {e}"))?;
    let cwd = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(&plan.cwd)
        .map_err(|e| format!("fresh provider cwd: {e}"))?;
    let input = private_sealed_bytes(b"fresh-provider-input", &plan.stdin)?;
    let recipe_bytes = serde_json::to_vec(&serde_json::json!({
        "configured_program": plan.configured_program,
        "argv": plan.argv, "env": plan.environment,
        "role": role,
    }))
    .map_err(|e| e.to_string())?;
    let recipe = private_sealed_bytes(b"fresh-provider-recipe", &recipe_bytes)?;
    Ok(PrivatePinnedPlan {
        image,
        cwd,
        input,
        recipe,
    })
}

#[cfg(feature = "age319-private-broker-fixture")]
fn private_interactive_plan(
    model: &oulipoly_config::ModelConfig,
    index: usize,
    cwd: &std::path::Path,
    session_id: &str,
) -> Result<oulipoly_runtime::executor::cli::fresh_remote::FreshProviderPlan, String> {
    let mut plan = oulipoly_runtime::executor::cli::fresh_remote::prepare_fresh_interactive(
        model, index, cwd,
    )?;
    if std::env::var_os("AGE319_PRIVATE_ROOT_PTY_RESIDENT_V1").is_some() {
        if plan
            .environment
            .iter()
            .any(|(key, _)| key == "AGE319_PRIVATE_NATIVE_SESSION")
        {
            return Err("private native session environment already set".into());
        }
        plan.environment
            .push(("AGE319_PRIVATE_NATIVE_SESSION".into(), session_id.into()));
        plan.environment.sort();
    }
    Ok(plan)
}

#[cfg(feature = "age319-private-broker-fixture")]
fn private_resident_registry(
    config_dir: &std::path::Path,
    model: &oulipoly_config::ModelConfig,
) -> Result<oulipoly_runtime::provider_registry::ProviderRegistry, String> {
    let source =
        std::fs::read_to_string(config_dir.join("providers.toml")).map_err(|e| e.to_string())?;
    let providers = oulipoly_config::ProvidersConfig::from_toml(&source)?;
    let data_root = oulipoly_state::paths::data_dir()?;
    oulipoly_runtime::provider_registry::ProviderRegistry::from_configs(
        &[model.clone()],
        &providers,
        oulipoly_runtime::provider_registry::ProviderRegistryOptions::default()
            .with_config_root(config_dir.to_owned())
            .with_data_root(data_root),
    )
    .map_err(|e| e.to_string())
}

#[cfg(feature = "age319-private-broker-fixture")]
fn private_resident_empty_tail(
    registry: &oulipoly_runtime::provider_registry::ProviderRegistry,
    model: &str,
    account: &str,
    instance: &str,
    settings: &str,
    session: &str,
    cwd: &std::path::Path,
) -> Result<serde_json::Value, String> {
    use oulipoly_runtime::session_provider::{
        SessionProviderIdentity, SessionProviderPageCursor, SessionProviderReadPageRequest,
        SessionProviderTurnProjection, read_turn_page,
    };
    use sha2::Digest as _;
    let cancellation = oulipoly_provider::client::CancellationToken::new();
    let observation_nonce = format!("{:x}", sha2::Sha256::digest(session.as_bytes()));
    let page = read_turn_page(SessionProviderReadPageRequest {
        registry,
        identity: SessionProviderIdentity {
            model_name: model.into(),
            provider_name: account.into(),
            provider_instance_id: Some(instance.into()),
            settings_id: settings.into(),
        },
        session_id: session,
        effective_cwd: Some(cwd),
        projection: SessionProviderTurnProjection::UserObservation,
        expected_delivery_nonce: Some(&observation_nonce),
        cursor: SessionProviderPageCursor::Tail,
        expected_page_index: 0,
        expected_turn_sequence: 0,
        max_turns: 64,
        max_response_bytes: 256 * 1024,
        max_source_bytes: 4 * 1024 * 1024,
        max_inline_body_bytes: 16 * 1024,
        cancellation: &cancellation,
        timeout: std::time::Duration::from_secs(5),
    })
    .map_err(|e| e.to_string())?;
    if page.provider_instance_id != instance
        || page.settings_id != settings
        || page.session_id != session
        || !page.snapshot_complete
        || !page.turns.is_empty()
        || page.resume_token.as_deref().is_none_or(str::is_empty)
    {
        return Err("selected native store Tail is not complete and empty".into());
    }
    Ok(serde_json::json!({
        "session_id": page.session_id,
        "provider_instance_id": page.provider_instance_id,
        "settings_id": page.settings_id,
        "snapshot_id": page.snapshot_id,
        "resume_token": page.resume_token,
        "snapshot_complete": page.snapshot_complete,
        "turn_count": page.turns.len(),
        "source_bytes_examined": page.source_bytes_examined,
    }))
}

#[cfg(feature = "age319-private-broker-fixture")]
fn private_resident_fence_pending_f(
    socket: &std::path::Path,
    root_d: &str,
    session_id: &str,
    registry: &oulipoly_runtime::provider_registry::ProviderRegistry,
    model: &str,
    account: &str,
    instance: &str,
    settings: &str,
    cwd: &std::path::Path,
    generation_id: &str,
    gate: &std::path::Path,
    physical_control: Option<&mut root_pty_control::RootPtyControl>,
) -> Result<(), String> {
    use base64::Engine as _;
    use oulipoly_kernel_broker::protocol::FreshRecipientRequest;
    use oulipoly_runtime::session_provider::{
        SessionProviderIdentity, SessionProviderPageCursor, SessionProviderReadPageRequest,
        SessionProviderTurnProjection, read_turn_page,
    };
    use oulipoly_state::mailbox::{
        FreshDeliveryReadback, FreshNativeFObservedTurn, FreshNativeFReceipt, FreshNativeFTransport,
    };
    use sha2::{Digest as _, Sha256};
    let delivery_request_id = uuid::Uuid::new_v4().to_string();
    let request_path = gate.join("interactive-f-request-id");
    let mut durable_key = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&request_path)
        .map_err(|e| e.to_string())?;
    durable_key
        .write_all(delivery_request_id.as_bytes())
        .map_err(|e| e.to_string())?;
    durable_key.sync_all().map_err(|e| e.to_string())?;
    File::open(gate)
        .and_then(|dir| dir.sync_all())
        .map_err(|e| e.to_string())?;
    // A broker restart can publish its main control socket before the fresh
    // recipient listener. Only this no-effect read may wait for readiness.
    let ready_until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        match protocol::fresh_recipient_request_at(
            socket,
            &FreshRecipientRequest::Read {
                delivery_request_id: delivery_request_id.clone(),
            },
        ) {
            Ok(read) if read["kind"] == "readback" && read["grant"].is_null() => break,
            Ok(_) => return Err("original interactive F request key already used".into()),
            Err(_) if std::time::Instant::now() < ready_until => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(error) => {
                return Err(format!(
                    "original interactive F listener unavailable: {error}"
                ));
            }
        }
    }
    let result = protocol::fresh_recipient_request_at(
        socket,
        &FreshRecipientRequest::Submit {
            allocation_request_id: root_d.into(),
            delivery_request_id: delivery_request_id.clone(),
        },
    )
    .map_err(|e| format!("original interactive F submission unknown: {e}"))?;
    if result["kind"] != "delivery" {
        return Err("original interactive F submission reply kind changed".into());
    }
    let submitted: FreshDeliveryReadback = serde_json::from_value(result["grant"].clone())
        .map_err(|e| format!("original interactive F grant absent: {e}"))?;
    let payload = base64::engine::general_purpose::STANDARD
        .decode(
            result["payload_base64"]
                .as_str()
                .ok_or("original interactive F bytes absent")?,
        )
        .map_err(|e| e.to_string())?;
    let payload_value: serde_json::Value =
        serde_json::from_slice(&payload).map_err(|e| e.to_string())?;
    if submitted.session_id != session_id
        || payload_value["protocol"] != "fresh-bash-complete-v30"
        || payload_value["source"]["source_id"] != submitted.source_id
        || payload_value["source"]["attempt_id"] != submitted.attempt_id
    {
        return Err("original interactive F differs from accepted Bash W".into());
    }
    let token = result["grant"]["delivery_token"]
        .as_str()
        .or_else(|| result["delivery_token"].as_str())
        .ok_or("original interactive F delivery token absent")?;
    let read = protocol::fresh_recipient_request_at(
        socket,
        &FreshRecipientRequest::Read {
            delivery_request_id: delivery_request_id.clone(),
        },
    )
    .map_err(|e| e.to_string())?;
    let grant: FreshDeliveryReadback =
        serde_json::from_value(read["grant"].clone()).map_err(|e| e.to_string())?;
    if grant.grant_id != submitted.grant_id
        || grant.session_id != submitted.session_id
        || grant.seq != submitted.seq
        || grant.source_id != submitted.source_id
        || grant.attempt_id != submitted.attempt_id
        || grant.payload_sha256 != submitted.payload_sha256
        || grant.payload_byte_len != submitted.payload_byte_len
    {
        return Err("original interactive F readback changed selected grant".into());
    }
    let preparation_request_id = uuid::Uuid::new_v4().to_string();
    let nonce = uuid::Uuid::new_v4().to_string();
    let prepared = crate::native_f_preparation::prepare_original_recipient_native_f(
        socket,
        session_id,
        &delivery_request_id,
        &grant,
        Some(token),
        Some(
            crate::native_f_preparation::OriginalRecipientNativeFSource {
                registry,
                identity: SessionProviderIdentity {
                    model_name: model.into(),
                    provider_name: account.into(),
                    provider_instance_id: Some(instance.into()),
                    settings_id: settings.into(),
                },
                effective_cwd: cwd,
                runtime_generation_id: generation_id,
                preparation_request_id: &preparation_request_id,
                envelope_nonce: &nonce,
            },
        ),
    )?;
    let fence = crate::native_f_preparation::fence_original_recipient_native_f(socket, &prepared)?;
    let duplicate = protocol::fresh_recipient_request_at(
        socket,
        &FreshRecipientRequest::BeginNativeFSubmission {
            preparation_request_id: preparation_request_id.clone(),
        },
    );
    if duplicate.is_ok() {
        return Err("duplicate native F submission fence accepted".into());
    }
    let readback = protocol::fresh_recipient_request_at(
        socket,
        &FreshRecipientRequest::ReadNativeFSubmission {
            preparation_request_id: preparation_request_id.clone(),
        },
    )
    .map_err(|e| e.to_string())?;
    if readback["fence"] != serde_json::to_value(&fence).map_err(|e| e.to_string())? {
        return Err("native F submission fence readback changed".into());
    }
    std::fs::write(
        gate.join("interactive-f-fenced.json"),
        serde_json::to_vec(&serde_json::json!({
            "delivery_request_id":delivery_request_id,
            "grant":grant,
            "preparation":prepared,
            "fence":fence,
            "duplicate_error":duplicate.unwrap_err().to_string(),
        }))
        .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let Some(control) = physical_control else {
        return Ok(());
    };
    let fault = std::env::var("AGE319_PRIVATE_NATIVE_F_FAULT_V1").unwrap_or_default();
    if fault == "after_fence" {
        private_native_f_fault_gate(gate, &fault)?;
        private_native_f_reattest(socket, control, &prepared, &fence)?;
        return Err("native F fence spent before byte; pending unknown, no replay".into());
    }
    let input = format!("{}\n", prepared.envelope_text);
    let sent = control.send_native_f_once(socket, &fence, input.as_bytes());
    if fault == "after_partial" {
        private_native_f_fault_gate(gate, &fault)?;
        private_native_f_reattest(socket, control, &prepared, &fence)?;
        return Err("native F partial write unknown; pending, no replay".into());
    }
    if std::env::var_os("AGE319_PRIVATE_NATIVE_F_PARTIAL_WRITE_V1").is_some()
        && sent.is_err()
        && control
            .send_native_f_once(socket, &fence, input.as_bytes())
            .is_ok()
    {
        return Err("native F partial write replay accepted".into());
    }
    sent?;
    if fault == "after_write" {
        private_native_f_fault_gate(gate, &fault)?;
        private_native_f_reattest(socket, control, &prepared, &fence)?;
    }
    if control
        .send_native_f_once(socket, &fence, input.as_bytes())
        .is_ok()
    {
        return Err("duplicate native F PTY send accepted".into());
    }
    let transport_answer = protocol::fresh_recipient_request_at(
        socket,
        &FreshRecipientRequest::RecordNativeFTransport {
            preparation_request_id: preparation_request_id.clone(),
        },
    );
    let transport = if transport_answer.is_err() {
        private_native_f_reattest(socket, control, &prepared, &fence)?;
        private_native_f_readback(
            socket,
            control,
            &prepared,
            &fence,
            FreshRecipientRequest::ReadNativeFTransport {
                preparation_request_id: preparation_request_id.clone(),
            },
            "transport",
            "native_f_transport_readback",
        )?
    } else {
        let answer = transport_answer.map_err(|e| e.to_string())?;
        if answer["kind"] != "native_f_transport" {
            return Err("native F transport reply changed; pending".into());
        }
        answer["transport"].clone()
    };
    let transport: FreshNativeFTransport =
        serde_json::from_value(transport).map_err(|e| e.to_string())?;
    if transport.preparation_request_id != preparation_request_id
        || transport.grant_id != fence.grant_id
        || transport.recipient_identity != fence.recipient_identity
        || transport.input_sha256 != fence.input_sha256
        || transport.input_byte_len != fence.input_byte_len
    {
        return Err("native F transport readback changed exact fence; pending".into());
    }
    let identity = SessionProviderIdentity {
        model_name: model.into(),
        provider_name: account.into(),
        provider_instance_id: Some(instance.into()),
        settings_id: settings.into(),
    };
    let observation_nonce = format!("{:x}", Sha256::digest(prepared.envelope_nonce.as_bytes()));
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let page = loop {
        let cancellation = oulipoly_provider::client::CancellationToken::new();
        let page = read_turn_page(SessionProviderReadPageRequest {
            registry,
            identity: identity.clone(),
            session_id,
            effective_cwd: Some(cwd),
            projection: SessionProviderTurnProjection::UserObservation,
            expected_delivery_nonce: Some(&observation_nonce),
            cursor: SessionProviderPageCursor::Beginning {
                after_token: Some(prepared.tail_resume_token.clone()),
            },
            expected_page_index: 0,
            expected_turn_sequence: 0,
            max_turns: 64,
            max_response_bytes: 256 * 1024,
            max_source_bytes: 4 * 1024 * 1024,
            max_inline_body_bytes: 64 * 1024,
            cancellation: &cancellation,
            timeout: std::time::Duration::from_secs(2),
        })
        .map_err(|e| format!("native F provider readback unknown: {e}"))?;
        if !page.turns.is_empty() || std::time::Instant::now() >= until {
            break page;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let turn = page
        .turns
        .first()
        .ok_or("native F provider turn absent; pending")?;
    let body = turn
        .body
        .as_ref()
        .and_then(|chunks| chunks.as_array())
        .and_then(|chunks| chunks.first())
        .and_then(|chunk| chunk["text"].as_str())
        .ok_or("native F provider body absent; pending")?;
    if !turn.canonical_text_digest_verified {
        return Err("native F provider canonical body unverified; pending".into());
    }
    let observed = FreshNativeFObservedTurn {
        provider_instance_id: page.provider_instance_id,
        settings_id: page.settings_id,
        provider_session_id: page.session_id,
        anchor_token: prepared.tail_resume_token.clone(),
        snapshot_id: page.snapshot_id,
        page_digest: page.page_digest,
        page_index: page.page_index,
        page_start_sequence: page.page_start_sequence,
        page_turn_count: page.page_turn_count,
        snapshot_complete: page.snapshot_complete,
        turn_id: turn.turn_id.clone(),
        role: turn.role.clone(),
        nonce: body
            .lines()
            .find_map(|line| line.strip_prefix("nonce: "))
            .unwrap_or_default()
            .into(),
        body: body.into(),
        canonical_text_sha256: turn.canonical_text_sha256.clone().unwrap_or_default(),
    };
    if fault == "after_turn" {
        private_native_f_fault_gate(gate, &fault)?;
        private_native_f_reattest(socket, control, &prepared, &fence)?;
    }
    for changed in ["tail", "turn", "session", "nonce", "body"] {
        let mut wrong = observed.clone();
        match changed {
            "tail" => wrong.anchor_token = "wrong-tail".into(),
            "turn" => wrong.turn_id = uuid::Uuid::new_v4().to_string(),
            "session" => wrong.provider_session_id = uuid::Uuid::new_v4().to_string(),
            "nonce" => wrong.nonce = uuid::Uuid::new_v4().to_string(),
            "body" => wrong.body.push_str(" wrong-body"),
            _ => unreachable!(),
        }
        if protocol::fresh_recipient_request_at(
            socket,
            &FreshRecipientRequest::CertifyNativeFReceipt {
                preparation_request_id: preparation_request_id.clone(),
                observed: wrong,
            },
        )
        .is_ok()
        {
            return Err(format!("native F {changed} negative receipt was accepted"));
        }
    }
    let receipt_answer = protocol::fresh_recipient_request_at(
        socket,
        &FreshRecipientRequest::CertifyNativeFReceipt {
            preparation_request_id: preparation_request_id.clone(),
            observed: observed.clone(),
        },
    );
    let receipt = if receipt_answer.is_err() {
        private_native_f_reattest(socket, control, &prepared, &fence)?;
        private_native_f_readback(
            socket,
            control,
            &prepared,
            &fence,
            FreshRecipientRequest::ReadNativeFReceipt {
                preparation_request_id: preparation_request_id.clone(),
            },
            "receipt",
            "native_f_receipt_readback",
        )?
    } else {
        let answer = receipt_answer.map_err(|e| e.to_string())?;
        if answer["kind"] != "native_f_receipt" {
            return Err("native F receipt reply changed; pending".into());
        }
        answer["receipt"].clone()
    };
    let receipt: FreshNativeFReceipt =
        serde_json::from_value(receipt).map_err(|e| e.to_string())?;
    if receipt.preparation_request_id != preparation_request_id
        || receipt.grant_id != grant.grant_id
        || receipt.recipient_identity != fence.recipient_identity
        || receipt.provider_session_id != prepared.provider_session_id
        || receipt.envelope_sha256 != prepared.envelope_sha256
        || receipt.payload_sha256 != prepared.payload_sha256
        || receipt.observation != observed
    {
        return Err("native F receipt readback changed exact native turn; pending".into());
    }
    if fault == "after_receipt" {
        private_native_f_fault_gate(gate, &fault)?;
        private_native_f_reattest(socket, control, &prepared, &fence)?;
    }
    if protocol::fresh_recipient_request_at(
        socket,
        &FreshRecipientRequest::CertifyNativeFReceipt {
            preparation_request_id: preparation_request_id.clone(),
            observed: observed.clone(),
        },
    )
    .is_ok()
    {
        return Err("duplicate native F receipt accepted".into());
    }
    if protocol::fresh_recipient_request_at(
        socket,
        &FreshRecipientRequest::AcknowledgeNativeFReceipt {
            preparation_request_id: preparation_request_id.clone(),
            delivery_token: uuid::Uuid::new_v4().to_string(),
        },
    )
    .is_ok()
    {
        return Err("native F wrong-token ACK accepted".into());
    }
    let ack_answer = protocol::fresh_recipient_request_at(
        socket,
        &FreshRecipientRequest::AcknowledgeNativeFReceipt {
            preparation_request_id: preparation_request_id.clone(),
            delivery_token: token.into(),
        },
    );
    let ack = if ack_answer.is_err() {
        private_native_f_reattest(socket, control, &prepared, &fence)?;
        private_native_f_readback(
            socket,
            control,
            &prepared,
            &fence,
            FreshRecipientRequest::Read {
                delivery_request_id: delivery_request_id.clone(),
            },
            "grant",
            "readback",
        )?
    } else {
        let answer = ack_answer.map_err(|e| e.to_string())?;
        if answer["kind"] != "native_f_auto_ack" {
            return Err("native F ACK reply changed; pending".into());
        }
        answer["grant"].clone()
    };
    let ack: FreshDeliveryReadback = serde_json::from_value(ack).map_err(|e| e.to_string())?;
    let mut expected_ack = grant.clone();
    expected_ack.phase = "acked".into();
    if ack != expected_ack {
        return Err("native F ACK readback changed exact grant; pending".into());
    }
    let durable_ack: oulipoly_state::mailbox::FreshNativeFAutoAck =
        serde_json::from_value(private_native_f_readback(
            socket,
            control,
            &prepared,
            &fence,
            FreshRecipientRequest::ReadNativeFAutoAck {
                preparation_request_id: preparation_request_id.clone(),
            },
            "ack",
            "native_f_auto_ack_readback",
        )?)
        .map_err(|e| e.to_string())?;
    if durable_ack.grant_id != grant.grant_id
        || durable_ack.preparation_request_id != preparation_request_id
        || durable_ack.delivery_request_id != delivery_request_id
        || durable_ack.delivery_token_sha256 != prepared.delivery_token_sha256
        || durable_ack.session_id != grant.session_id
        || durable_ack.seq != grant.seq
        || durable_ack.source_id != grant.source_id
        || durable_ack.attempt_id != grant.attempt_id
        || durable_ack.recipient_identity_json
            != serde_json::to_string(&fence.recipient_identity).map_err(|e| e.to_string())?
        || durable_ack.payload_sha256 != grant.payload_sha256
        || durable_ack.payload_byte_len != grant.payload_byte_len
        || durable_ack.turn_id != receipt.turn_id
        || durable_ack.basis != "native_f_receipt"
    {
        return Err("native F automatic ACK row changed exact receipt; pending".into());
    }
    if protocol::fresh_recipient_request_at(
        socket,
        &FreshRecipientRequest::AcknowledgeNativeFReceipt {
            preparation_request_id: preparation_request_id.clone(),
            delivery_token: token.into(),
        },
    )
    .is_ok()
    {
        return Err("duplicate native F automatic ACK accepted".into());
    }
    std::fs::write(
        gate.join("interactive-f-physical.json"),
        serde_json::to_vec(&serde_json::json!({
            "transport": transport, "receipt": receipt,
            "ack": ack,
        }))
        .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(feature = "age319-private-broker-fixture")]
fn private_native_f_fault_gate(gate: &std::path::Path, stage: &str) -> Result<(), String> {
    std::fs::write(gate.join("interactive-f-crash-ready"), stage).map_err(|e| e.to_string())?;
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !gate.join("interactive-f-crash-continue").exists() {
        if std::time::Instant::now() >= until {
            return Err(format!(
                "native F {stage} restart gate expired; pending unknown"
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    Ok(())
}

#[cfg(feature = "age319-private-broker-fixture")]
fn private_native_f_reattest(
    socket: &std::path::Path,
    control: &root_pty_control::RootPtyControl,
    prepared: &oulipoly_state::mailbox::FreshNativeFPreparation,
    fence: &oulipoly_state::mailbox::FreshNativeFSubmission,
) -> Result<(), String> {
    use oulipoly_kernel_broker::protocol::{self, FreshRecipientRequest};
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let result: Result<(), String> = (|| {
            control.broker_resident_readback(socket)?;
            let key = prepared.preparation_request_id.clone();
            let read = protocol::fresh_recipient_request_at(
                socket,
                &FreshRecipientRequest::ReadNativeFPreparation {
                    preparation_request_id: key.clone(),
                },
            )
            .map_err(|e| e.to_string())?;
            let actual: oulipoly_state::mailbox::FreshNativeFPreparation =
                serde_json::from_value(read["preparation"].clone())
                    .map_err(|_| "native F preparation absent after crash".to_string())?;
            if read["kind"] != "native_f_preparation_readback" || actual != *prepared {
                return Err(
                    "native F preparation, session, root or source changed after crash".into(),
                );
            }
            let read = protocol::fresh_recipient_request_at(
                socket,
                &FreshRecipientRequest::ReadNativeFSubmission {
                    preparation_request_id: key,
                },
            )
            .map_err(|e| e.to_string())?;
            let actual: oulipoly_state::mailbox::FreshNativeFSubmission =
                serde_json::from_value(read["fence"].clone())
                    .map_err(|_| "native F fence absent after crash".to_string())?;
            if read["kind"] != "native_f_submission_readback" || actual != *fence {
                return Err("native F fence changed after crash; no replay".into());
            }
            let read = protocol::fresh_recipient_request_at(
                socket,
                &FreshRecipientRequest::ReadNativeFReceipt {
                    preparation_request_id: prepared.preparation_request_id.clone(),
                },
            )
            .map_err(|e| e.to_string())?;
            if read["kind"] != "native_f_receipt_readback" {
                return Err("native F receipt readback kind changed".into());
            }
            Ok(())
        })();
        match result {
            Ok(()) => return Ok(()),
            Err(error) if std::time::Instant::now() >= until => {
                return Err(format!(
                    "native F reattestation unknown; pending, no replay: {error}"
                ));
            }
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
}

#[cfg(feature = "age319-private-broker-fixture")]
fn private_native_f_readback(
    socket: &std::path::Path,
    control: &root_pty_control::RootPtyControl,
    prepared: &oulipoly_state::mailbox::FreshNativeFPreparation,
    fence: &oulipoly_state::mailbox::FreshNativeFSubmission,
    request: oulipoly_kernel_broker::protocol::FreshRecipientRequest,
    field: &str,
    kind: &str,
) -> Result<serde_json::Value, String> {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        private_native_f_reattest(socket, control, prepared, fence)?;
        match oulipoly_kernel_broker::protocol::fresh_recipient_request_at(socket, &request) {
            Ok(answer) if answer["kind"] == kind && !answer[field].is_null() => {
                return Ok(answer[field].clone());
            }
            Ok(_) => return Err(format!("native F {field} absent after lost reply; pending")),
            Err(error) if std::time::Instant::now() >= until => {
                return Err(format!(
                    "native F {field} readback unknown; pending: {error}"
                ));
            }
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
}

#[cfg(feature = "age319-private-broker-fixture")]
impl oulipoly_runtime::executor::cli::fresh_remote::FreshProviderBackend
    for PrivateFreshBroker<'_>
{
    fn run_to_physical_q(
        &mut self,
        plan: oulipoly_runtime::executor::cli::fresh_remote::FreshProviderPlan,
    ) -> Result<oulipoly_runtime::executor::cli::fresh_remote::FreshProviderCompletion, String>
    {
        use oulipoly_kernel_broker::protocol;
        let socket = broker_socket().with_file_name("v30.sock");
        // The broker binds the original descriptor's inode and mount before
        // one-use K. Preflight content observations cannot attest later bytes.
        let pinned = private_pin_plan(
            &plan,
            oulipoly_kernel_broker::protocol::FreshPlanRole::Headless,
        )?;
        let submitted = protocol::private_fresh_provider_at(
            &socket,
            &self.authority.receipt.d_key,
            b'5',
            Some(pinned.descriptors()),
        );
        let grant = match submitted {
            Ok(response) => response
                .strip_prefix("fresh-provider-k ")
                .and_then(|s| s.strip_suffix('\n'))
                .ok_or_else(|| self.unknown(None, "K reply", "invalid broker reply"))?
                .to_owned(),
            Err(error) => {
                // K may be consumed. Observe that same D binding; never issue
                // a second K after a lost or ambiguous reply.
                let state = protocol::private_fresh_provider_at(
                    &socket,
                    &self.authority.receipt.d_key,
                    b'9',
                    Some(pinned.descriptors()),
                )
                .map_err(|e| self.unknown(None, "K readback", &format!("{error}; {e}")))?;
                if state.starts_with("fresh-provider-unknown ") {
                    return Err(self.unknown(
                        state.split_whitespace().nth(1),
                        "K readback",
                        &format!(
                            "consumed with unknown physical state: K={error}; observation={state}"
                        ),
                    ));
                }
                if ![
                    "fresh-provider-pending",
                    "fresh-provider-exited",
                    "fresh-provider-drained",
                ]
                .iter()
                .any(|prefix| state.starts_with(&format!("{prefix} ")))
                {
                    return Err(self.unknown(None, "K readback", "invalid broker observation"));
                }
                state
                    .split_whitespace()
                    .nth(1)
                    .ok_or_else(|| self.unknown(None, "K readback", "missing grant"))?
                    .to_owned()
            }
        };
        uuid::Uuid::parse_str(&grant)
            .map_err(|_| self.unknown(Some(&grant), "K reply", "invalid grant"))?;
        self.grant_id = Some(grant.clone());
        let causal = std::env::var_os("AGE319_PRIVATE_PROVIDER_CAUSAL_BASH_V1").is_some();
        let direct_caller = std::env::var_os("AGE319_PRIVATE_CALLER_OUTPUT_V1").is_some();
        let deadline = std::time::Instant::now()
            + if causal {
                PRIVATE_CAUSAL_BASH_RESULT_WAIT
            } else {
                PRIVATE_PROVIDER_RESULT_WAIT
            };
        loop {
            let state = protocol::private_fresh_provider_at(
                &socket,
                &self.authority.receipt.d_key,
                b'6',
                None,
            )
            .map_err(|e| self.unknown(Some(&grant), "provider exit readback", &e.to_string()))?;
            if state.starts_with(&format!("fresh-provider-exited {grant} ")) {
                break;
            }
            if direct_caller && state.starts_with(&format!("fresh-provider-drained {grant} ")) {
                break;
            }
            if state.starts_with("fresh-provider-unknown ") || std::time::Instant::now() >= deadline
            {
                return Err(self.unknown(Some(&grant), "provider exit readback", &state));
            }
            std::thread::sleep(PRIVATE_PROVIDER_RESULT_POLL);
        }
        let gate = std::env::var("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1")
            .map_err(|e| self.unknown(Some(&grant), "fixture cancellation", &e.to_string()))?;
        let mut sibling_checked = !causal;
        let mut recipient_checked =
            std::env::var_os("AGE319_PRIVATE_BASH_RECIPIENT_MODE_V1").is_none();
        while !direct_caller && !std::path::Path::new(&gate).join("provider-cancel").exists() {
            if !recipient_checked
                && std::fs::read(std::path::Path::new(&gate).join("bash-causal-output"))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                    .is_some_and(|report| report["fresh_source_w"]["request_id"].is_string())
            {
                private_bash_recipient_probe(&socket, &self.authority.receipt.d_key, &gate)
                    .map_err(|e| self.unknown(Some(&grant), "fresh F/ACK recipient probe", &e))?;
                recipient_checked = true;
            }
            if !sibling_checked && std::path::Path::new(&gate).join("sibling-request").exists() {
                let sibling_effect = std::path::Path::new(&gate).join("sibling-effect");
                let sibling_bash = std::env::var("AGE319_PRIVATE_BASH_IMAGE")
                    .map_err(|e| self.unknown(Some(&grant), "sibling image", &e.to_string()))?;
                let sibling_request = std::env::var("AGE319_PRIVATE_BASH_REQUEST_KEY")
                    .map_err(|e| self.unknown(Some(&grant), "sibling key", &e.to_string()))?;
                let output = std::process::Command::new("unshare")
                    .args(["--pid", "--fork", "--"])
                    .arg(sibling_bash)
                    .arg("__age319-private-admit-child-v1")
                    .arg(&socket)
                    .arg(sibling_request)
                    .arg(&sibling_effect)
                    .env_clear()
                    .env("PATH", "/usr/bin:/bin")
                    .output()
                    .map_err(|e| self.unknown(Some(&grant), "sibling launch", &e.to_string()))?;
                std::fs::write(
                    std::path::Path::new(&gate).join("sibling-result.json"),
                    serde_json::to_vec(&serde_json::json!({
                        "success": output.status.success(),
                        "stderr": String::from_utf8_lossy(&output.stderr),
                    }))
                    .map_err(|e| self.unknown(Some(&grant), "sibling result", &e.to_string()))?,
                )
                .map_err(|e| self.unknown(Some(&grant), "sibling result", &e.to_string()))?;
                sibling_checked = true;
            }
            if std::time::Instant::now() >= deadline {
                return Err(self.unknown(Some(&grant), "fixture cancellation", "expired"));
            }
            std::thread::sleep(PRIVATE_PROVIDER_RESULT_POLL);
        }
        if !direct_caller {
            protocol::private_fresh_provider_at(&socket, &self.authority.receipt.d_key, b'7', None)
                .map_err(|e| self.unknown(Some(&grant), "cancel reply", &e.to_string()))?;
        }
        loop {
            let state = protocol::private_fresh_provider_at(
                &socket,
                &self.authority.receipt.d_key,
                b'6',
                None,
            )
            .map_err(|e| self.unknown(Some(&grant), "Q readback", &e.to_string()))?;
            if state.starts_with(&format!("fresh-provider-drained {grant} ")) {
                break;
            }
            if state.starts_with("fresh-provider-unknown ") || std::time::Instant::now() >= deadline
            {
                return Err(self.unknown(Some(&grant), "Q readback", &state));
            }
            std::thread::sleep(PRIVATE_PROVIDER_RESULT_POLL);
        }
        let output =
            protocol::private_fresh_provider_output_at(&socket, &self.authority.receipt.d_key)
                .map_err(|e| self.unknown(Some(&grant), "output readback", &e.to_string()))?;
        if output.grant_id != grant
            || (direct_caller && output.cancelled)
            || (!direct_caller && !output.cancelled)
        {
            return Err(self.unknown(Some(&grant), "output readback", "grant/Q mismatch"));
        }
        let stdout =
            private_verified_output(output.stdout, output.stdout_len, &output.stdout_sha256)
                .map_err(|e| self.unknown(Some(&grant), "stdout verification", &e))?;
        let stderr =
            private_verified_output(output.stderr, output.stderr_len, &output.stderr_sha256)
                .map_err(|e| self.unknown(Some(&grant), "stderr verification", &e))?;
        self.raw_result = Some(
            oulipoly_runtime::executor::cli::fresh_remote::FreshProviderCompletion {
                wait_status: output.wait_status,
                stdout: stdout.clone(),
                stderr: stderr.clone(),
            },
        );
        Ok(
            oulipoly_runtime::executor::cli::fresh_remote::FreshProviderCompletion {
                wait_status: output.wait_status,
                stdout,
                stderr,
            },
        )
    }
}

#[cfg(feature = "age319-private-broker-fixture")]
fn private_verified_output(
    mut file: File,
    expected_len: u64,
    expected_hash: &str,
) -> Result<Vec<u8>, String> {
    use sha2::{Digest, Sha256};
    let mut bytes = Vec::new();
    let mut hash = Sha256::new();
    let mut count = 0u64;
    let mut chunk = [0u8; PRIVATE_VERIFIED_OUTPUT_BUFFER_BYTES];
    loop {
        let n = file.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        count = count
            .checked_add(n as u64)
            .ok_or("private output length overflow")?;
        if count > expected_len {
            return Err("private output exceeds broker receipt".into());
        }
        hash.update(&chunk[..n]);
        bytes.extend_from_slice(&chunk[..n]);
    }
    if count != expected_len || format!("{:x}", hash.finalize()) != expected_hash {
        return Err("private output differs from broker receipt".into());
    }
    Ok(bytes)
}

#[cfg(feature = "age319-private-broker-fixture")]
fn private_fresh_provider(authority: FreshEntryAuthority<'_>) -> Result<ExitCode, String> {
    use oulipoly_kernel_broker::protocol::{
        self, FreshAccountEffectKind, FreshAccountEffectRequest, FreshPlanRole, FreshRouteRequest,
    };
    use oulipoly_runtime::executor::cli::fresh_remote::{
        load_fresh_headless_pool, prepare_fresh_headless, run_prepared_fresh_headless,
    };
    let (model_name, provider_pin, prompt) = match &authority.receipt.root_work_intent {
        oulipoly_state::mailbox::FreshRootWorkIntent::NormalCli(args) => match args.as_slice() {
            [flag, model, prompt] if flag == "--model" => (model.as_str(), None, prompt.as_str()),
            [flag, model, pin_flag, pin, prompt]
                if flag == "--model" && pin_flag == "--pin-provider" =>
            {
                (model.as_str(), Some(pin.as_str()), prompt.as_str())
            }
            _ => return Err("fresh provider CLI shape unsupported before K".into()),
        },
        _ => return Err("fresh provider root intent unsupported before K".into()),
    };
    if authority.session.session_id.is_empty() || authority.receipt.d_key.is_empty() {
        return Err("private fresh entry authority incomplete".into());
    }
    let config_dir = oulipoly_state::paths::config_dir()?;
    let pool = load_fresh_headless_pool(&config_dir, model_name)?;
    let config_source = File::open(&config_dir)
        .map_err(|e| format!("fresh config source unavailable before K: {e}"))?;
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let socket = broker_socket().with_file_name("v30.sock");
    let total = pool.model.providers.len();
    let mut prepared = Vec::with_capacity(total);
    let causal = std::env::var_os("AGE319_PRIVATE_PROVIDER_CAUSAL_BASH_V1").is_some();
    for index in 0..total {
        let mut plan = prepare_fresh_headless(&pool.model, index, prompt, &cwd)?;
        if causal {
            plan.plan.argv.extend([
                std::env::var("AGE319_PRIVATE_BASH_IMAGE")
                    .map_err(|_| "private Bash image absent")?,
                socket_for_private_causal_bash().display().to_string(),
                std::env::var("AGE319_PRIVATE_BASH_REQUEST_KEY")
                    .map_err(|_| "private Bash request absent")?,
                std::env::var("AGE319_PRIVATE_BASH_EFFECT_MARKER")
                    .map_err(|_| "private Bash marker absent")?,
                std::env::var("OULIPOLY_DATA_DIR")
                    .map_err(|_| "private Bash data directory absent")?,
            ]);
            if let Some(mode) = std::env::var_os("AGE319_PRIVATE_BASH_ORDINARY_MODE_V1") {
                plan.plan.argv.push(mode.to_string_lossy().into_owned());
            } else if std::env::var_os("AGE319_PRIVATE_BASH_SOURCE_SUCCESS_V1").is_some() {
                plan.plan.argv.push("no-cancel".into());
            } else if std::env::var_os("AGE319_PRIVATE_BASH_ORIGINAL_NOTIFY_V1").is_some() {
                plan.plan.argv.push("notify".into());
            }
        }
        prepared.push(plan);
    }
    for (index, candidate) in prepared.iter().enumerate() {
        let request = FreshRouteRequest {
            protocol_version: 4,
            d_key: authority.receipt.d_key.clone(),
            model: pool.model.name.clone(),
            config_sha256: pool.config_sha256.clone(),
            account: Some(pool.model.providers[index].name.clone()),
            account_identity: Some(
                pool.account_identities[index]
                    .clone()
                    .ok_or("private fresh account requires quota_account_id in providers.toml")?,
            ),
            index: Some(index),
            total,
            pin: provider_pin.map(str::to_owned),
            quota_script: pool.account_effects[index].0.clone(),
            auth_refresh_command: pool.account_effects[index].1.clone(),
            environment_sha256: None,
        };
        let pinned = private_pin_plan(&candidate.plan, FreshPlanRole::Headless)?;
        let [image, cwd_fd, input, recipe] = pinned.descriptors();
        protocol::private_fresh_route_at(
            &socket,
            &request,
            b'h',
            &[image, cwd_fd, input, recipe, config_source.as_raw_fd()],
        )
        .map_err(|e| format!("fresh route candidate refused before K: {e}"))?;
        if std::env::var_os("AGE319_PRIVATE_ROOT_PTY_CONTROL_V1").is_some() {
            let interactive =
                private_interactive_plan(&pool.model, index, &cwd, &authority.session.session_id)?;
            if std::env::var_os("AGE319_PRIVATE_ROOT_PTY_NEGATIVE_V1").is_some() {
                let mut mislabeled = interactive.clone();
                mislabeled.argv = candidate.plan.argv.clone();
                let wrong = private_pin_plan(&mislabeled, FreshPlanRole::Interactive)?;
                if protocol::private_fresh_interactive_route_at(
                    &socket,
                    &request,
                    b'(',
                    &[
                        wrong.image.as_raw_fd(),
                        wrong.cwd.as_raw_fd(),
                        wrong.input.as_raw_fd(),
                        wrong.recipe.as_raw_fd(),
                        config_source.as_raw_fd(),
                    ],
                )
                .is_ok()
                {
                    return Err("headless argv mislabeled as interactive accepted".into());
                }
            }
            let pinned = private_pin_plan(&interactive, FreshPlanRole::Interactive)?;
            protocol::private_fresh_interactive_route_at(
                &socket,
                &request,
                b'(',
                &[
                    pinned.image.as_raw_fd(),
                    pinned.cwd.as_raw_fd(),
                    pinned.input.as_raw_fd(),
                    pinned.recipe.as_raw_fd(),
                    config_source.as_raw_fd(),
                ],
            )
            .map_err(|e| format!("interactive route candidate refused before K: {e}"))?;
        }
    }
    let keyed_v3_mode = std::env::var("AGE319_PRIVATE_JOIN_MODE")
        .ok()
        .as_deref()
        .is_some_and(|mode| mode.starts_with("normal_model_provider_v3_"));
    let mut environment = Vec::new();
    for (key, value) in std::env::vars_os() {
        let key = key
            .into_string()
            .map_err(|_| "fresh effect environment key is not UTF-8")?;
        if oulipoly_runtime::executor::cli::fresh_remote::forbidden_fresh_environment(&key) {
            continue;
        }
        let value = value
            .into_string()
            .map_err(|_| "fresh effect environment value is not UTF-8")?;
        environment.push((key, value));
    }
    environment.sort();
    let environment_sha256 = {
        use sha2::{Digest, Sha256};
        format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&environment).map_err(|e| e.to_string())?)
        )
    };
    let mut quota_receipts = Vec::new();
    let mut auth_receipts = Vec::new();
    // The private manual/route join exercises a second original-root actor
    // against an already settled physical account Q. Route admission itself
    // decides whether that Q is usable; starting QuotaFirst here would spend
    // a duplicate K before the decision could inspect the manual fact.
    let route_mode = std::env::var("AGE319_PRIVATE_JOIN_MODE").unwrap_or_default();
    let manual_route = route_mode.starts_with("normal_model_provider_v3_quota_route_manual");
    let mut shared_quota_receipts = Vec::new();
    for (index, (quota_script, auth_command)) in pool.account_effects.iter().enumerate() {
        if manual_route {
            continue;
        }
        if quota_script.is_none() {
            continue;
        }
        let mut effect = FreshAccountEffectRequest {
            d_key: authority.receipt.d_key.clone(),
            model: pool.model.name.clone(),
            config_sha256: pool.config_sha256.clone(),
            account: pool.model.providers[index].name.clone(),
            index,
            kind: FreshAccountEffectKind::QuotaFirst,
            environment: prepared[index].plan.environment.clone(),
        };
        let shared = if keyed_v3_mode {
            Some(
                protocol::private_shared_quota_at(&socket, &effect)
                    .or_else(|_| protocol::private_shared_quota_at(&socket, &effect))
                    .map_err(|e| format!("v3 shared physical quota readback refused: {e}"))?,
            )
        } else {
            None
        };
        let first = if let Some(reused) = shared.flatten() {
            if reused.state != "drained" {
                return Err("v3 shared physical quota Q not drained".into());
            }
            shared_quota_receipts.push((index, reused.effect_id.clone()));
            reused
        } else {
            private_run_account_effect(&socket, &authority.receipt.handoff_id, &effect)?
        };
        quota_receipts.push((
            effect.clone(),
            first.effect_id.clone(),
            first.outcome.clone(),
        ));
        if first.outcome.as_deref() != Some("valid_windows") && auth_command.is_some() {
            let shared_auth_first = std::env::var("AGE319_PRIVATE_JOIN_MODE").ok().as_deref()
                == Some("normal_model_provider_v3_quota_auth_shared")
                && std::env::var_os("AGE319_PRIVATE_SECOND_RESULT_V1").is_none();
            if shared_auth_first {
                let gate = std::path::PathBuf::from(
                    std::env::var("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1")
                        .map_err(|e| e.to_string())?,
                );
                std::fs::write(gate.join("shared-auth-first-ready"), b"yes")
                    .map_err(|e| e.to_string())?;
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
                while !gate.join("shared-auth-first-go").exists() {
                    if std::time::Instant::now() >= deadline {
                        return Err("shared auth first actor gate timed out".into());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            }
            effect.kind = FreshAccountEffectKind::AuthRefresh;
            let auth = private_run_account_effect(&socket, &authority.receipt.handoff_id, &effect)?;
            if std::env::var("AGE319_PRIVATE_JOIN_MODE")
                .ok()
                .as_deref()
                .is_some_and(|mode| mode.starts_with("normal_model_provider_v3_quota_auth"))
            {
                let mut changed_env = effect.clone();
                changed_env
                    .environment
                    .push(("AGE319_CHANGED_ENV".into(), "1".into()));
                if protocol::private_fresh_account_effect_at(&socket, &changed_env, false).is_ok() {
                    return Err("v3 auth accepted changed environment readback".into());
                }
                let mut changed_account = effect.clone();
                changed_account.account = "different".into();
                if protocol::private_fresh_account_effect_at(&socket, &changed_account, false)
                    .is_ok()
                {
                    return Err("v3 auth accepted changed account readback".into());
                }
                if protocol::private_fresh_account_effect_at(&socket, &effect, true).is_ok() {
                    return Err("v3 auth began a second physical effect".into());
                }
            }
            auth_receipts.push((effect.clone(), auth.effect_id.clone(), auth.outcome.clone()));
            if auth.outcome.as_deref() == Some("refreshed") {
                effect.kind = FreshAccountEffectKind::QuotaRetry;
                if shared_auth_first {
                    let gate = std::path::PathBuf::from(
                        std::env::var("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1")
                            .map_err(|e| e.to_string())?,
                    );
                    std::fs::write(gate.join("shared-auth-first-refreshed"), b"yes")
                        .map_err(|e| e.to_string())?;
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
                    while !gate.join("shared-auth-first-retry-go").exists() {
                        if std::time::Instant::now() >= deadline {
                            return Err("shared auth retry gate timed out".into());
                        }
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                    let mut follow = effect.clone();
                    follow.kind = FreshAccountEffectKind::QuotaFirst;
                    let retry = protocol::private_shared_quota_at(&socket, &follow)
                        .map_err(|e| format!("shared quota retry readback refused: {e}"))?
                        .ok_or("shared quota retry Q absent")?;
                    if retry.state != "drained" || retry.outcome.as_deref() != Some("valid_windows")
                    {
                        return Err("shared quota retry Q invalid".into());
                    }
                    shared_quota_receipts.push((index, retry.effect_id.clone()));
                    auth_receipts.push((effect.clone(), retry.effect_id, retry.outcome));
                } else {
                    let retry = private_run_account_effect(
                        &socket,
                        &authority.receipt.handoff_id,
                        &effect,
                    )?;
                    auth_receipts.push((effect.clone(), retry.effect_id, retry.outcome));
                }
            }
        }
    }
    let request = FreshRouteRequest {
        protocol_version: 4,
        d_key: authority.receipt.d_key.clone(),
        model: pool.model.name.clone(),
        config_sha256: pool.config_sha256.clone(),
        account: None,
        account_identity: None,
        index: None,
        total,
        pin: provider_pin.map(str::to_owned),
        quota_script: None,
        auth_refresh_command: None,
        environment_sha256: Some(environment_sha256),
    };
    let first =
        protocol::private_fresh_route_at(&socket, &request, b'f', &[config_source.as_raw_fd()]);
    let selected = if matches!(
        route_mode.as_str(),
        "normal_model_provider_v3_quota_route_reply_loss"
            | "normal_model_provider_v3_quota_route_manual_route_reply_loss"
            | "normal_model_provider_v3_quota_route_manual_physical_route_reply_loss"
    ) {
        if first.is_ok() {
            return Err("v3 route fixture did not lose first reply".into());
        }
        let gate = std::path::PathBuf::from(
            std::env::var("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1")
                .map_err(|e| e.to_string())?,
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !gate.join("route-restarted").exists() {
            if std::time::Instant::now() >= deadline {
                return Err("v3 route restart fixture timed out".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        protocol::private_fresh_route_at(&socket, &request, b'f', &[config_source.as_raw_fd()])
            .map_err(|e| format!("v3 route exact restart readback refused: {e}"))?
            .ok_or("v3 route exact restart readback absent")?
    } else {
        first
            .map_err(|e| format!("fresh route selection refused before K: {e}"))?
            .ok_or("fresh route selection absent before K")?
    };
    if route_mode.starts_with("normal_model_provider_v3_quota_route") {
        let repeated =
            protocol::private_fresh_route_at(&socket, &request, b'f', &[config_source.as_raw_fd()])
                .map_err(|e| format!("v3 route exact readback refused: {e}"))?
                .ok_or("v3 route exact readback absent")?;
        if repeated != selected {
            return Err("v3 route readback changed selection".into());
        }
        let mut changed_environment = request.clone();
        changed_environment.environment_sha256 = Some("0".repeat(64));
        if protocol::private_fresh_route_at(
            &socket,
            &changed_environment,
            b'f',
            &[config_source.as_raw_fd()],
        )
        .is_ok()
        {
            return Err("v3 route accepted changed environment".into());
        }
        let mut changed_account = request.clone();
        changed_account.account = Some("unused".into());
        changed_account.account_identity = Some("physical-unused".into());
        changed_account.index = Some(0);
        if protocol::private_fresh_route_at(
            &socket,
            &changed_account,
            b'f',
            &[config_source.as_raw_fd()],
        )
        .is_ok()
        {
            return Err("v3 route accepted changed account".into());
        }
        let gate = std::path::PathBuf::from(
            std::env::var("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1")
                .map_err(|e| e.to_string())?,
        );
        let replacement = gate.join("replacement-config");
        std::fs::create_dir_all(replacement.join("models")).map_err(|e| e.to_string())?;
        std::fs::copy(
            config_dir.join("providers.toml"),
            replacement.join("providers.toml"),
        )
        .map_err(|e| e.to_string())?;
        std::fs::copy(
            config_dir.join("models/configured-model.toml"),
            replacement.join("models/configured-model.toml"),
        )
        .map_err(|e| e.to_string())?;
        let replacement_fd = File::open(&replacement).map_err(|e| e.to_string())?;
        if protocol::private_fresh_route_at(&socket, &request, b'f', &[replacement_fd.as_raw_fd()])
            .is_ok()
        {
            return Err("v3 route accepted changed source directory".into());
        }
        std::fs::write(
            gate.join("v3-route-selection.json"),
            serde_json::to_vec(&selected).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    if selected.model != pool.model.name
        || selected.config_sha256 != pool.config_sha256
        || selected.policy_version != "fresh-quota-account-v4"
        || pool
            .account_identities
            .get(selected.index)
            .and_then(Option::as_deref)
            != Some(selected.account_identity.as_str())
        || !selected.eligible_accounts.contains(&selected.account)
        || selected.eligible_accounts.iter().any(|account| {
            !pool
                .model
                .providers
                .iter()
                .any(|member| &member.name == account)
        })
        || pool
            .model
            .providers
            .get(selected.index)
            .is_none_or(|member| member.name != selected.account)
        || pool.account_effects[selected.index].0.is_some()
            != selected.quota_remaining_basis_points.is_some()
    {
        return Err("fresh route readback differs from configured pool before K".into());
    }
    if matches!(
        route_mode.as_str(),
        "normal_model_provider_v3_quota_route_manual_physical_capacity_terminal"
            | "normal_model_provider_v3_quota_route_manual_physical_account_quota_terminal"
    ) {
        let gate = std::path::PathBuf::from(
            std::env::var("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1")
                .map_err(|e| e.to_string())?,
        );
        if gate.join("v3-stop-after-route").exists() {
            std::fs::write(
                gate.join("v3-second-route-selection.json"),
                serde_json::to_vec(&selected).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            return Err("v3 second actor stopped after route before provider K".into());
        }
    }
    let mut selected_plan = prepared.swap_remove(selected.index);
    if route_mode.starts_with("normal_model_provider_v3_quota_route_physical")
        || route_mode.starts_with("normal_model_provider_v3_quota_route_manual_physical")
    {
        let gate = std::path::PathBuf::from(
            std::env::var("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1")
                .map_err(|e| e.to_string())?,
        );
        let expected = serde_json::json!({
            "argv": selected_plan.plan.argv,
            "cwd": selected_plan.plan.cwd,
            "env": selected_plan.plan.environment.iter().cloned().collect::<std::collections::BTreeMap<_, _>>(),
            "account_identity": selected.account_identity,
        });
        std::fs::write(
            gate.join("v3-provider-expected-plan.json"),
            serde_json::to_vec(&expected).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    if route_mode.ends_with("physical_bad_plan") {
        selected_plan
            .plan
            .argv
            .push("--changed-after-selection".into());
    }
    if route_mode.ends_with("physical_bad_actor") {
        let pinned = private_pin_plan(&selected_plan.plan, FreshPlanRole::Headless)?;
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            return Err(format!(
                "v3 bad actor fixture fork: {}",
                std::io::Error::last_os_error()
            ));
        }
        if pid == 0 {
            let refused = protocol::private_fresh_provider_at(
                &socket,
                &authority.receipt.d_key,
                b'5',
                Some(pinned.descriptors()),
            )
            .is_err();
            unsafe { libc::_exit(if refused { 0 } else { 1 }) };
        }
        let mut status = 0;
        if unsafe { libc::waitpid(pid, &mut status, 0) } != pid
            || !libc::WIFEXITED(status)
            || libc::WEXITSTATUS(status) != 0
        {
            return Err("v3 changed actor was admitted to provider K".into());
        }
        return Err("v3 changed actor correctly refused before provider K".into());
    }
    // The interactive decision is independently durable. The headless plan
    // stays with the existing K/Q backend; # does not spend an interactive K.
    let mut selected_interactive = None;
    let mut root_pty = if std::env::var_os("AGE319_PRIVATE_ROOT_PTY_CONTROL_V1").is_some() {
        let interactive = protocol::private_fresh_interactive_route_at(
            &socket,
            &request,
            b')',
            &[config_source.as_raw_fd()],
        )
        .map_err(|e| format!("interactive route selection refused before K: {e}"))?
        .ok_or("interactive route selection absent before K")?;
        let local_interactive = private_interactive_plan(
            &pool.model,
            selected.index,
            &cwd,
            &authority.session.session_id,
        )?;
        let local_pinned = private_pin_plan(&local_interactive, FreshPlanRole::Interactive)?;
        let readback = protocol::private_fresh_interactive_route_at(
            &socket,
            &request,
            b')',
            &[config_source.as_raw_fd()],
        )
        .map_err(|e| format!("interactive route readback refused: {e}"))?
        .ok_or("interactive route readback absent")?;
        if interactive != readback
            || interactive.role != FreshPlanRole::Interactive
            || interactive.model != selected.model
            || interactive.config_sha256 != selected.config_sha256
            || interactive.account != selected.account
            || interactive.index != selected.index
            || interactive.plan_sha256 == selected.plan_sha256
        {
            return Err("interactive selection readback differs from held account".into());
        }
        let selected_request = FreshRouteRequest {
            account: Some(interactive.account.clone()),
            account_identity: Some(selected.account_identity.clone()),
            index: Some(interactive.index),
            quota_script: pool.account_effects[interactive.index].0.clone(),
            auth_refresh_command: pool.account_effects[interactive.index].1.clone(),
            ..request.clone()
        };
        protocol::private_fresh_interactive_route_at(
            &socket,
            &selected_request,
            b'(',
            &[
                local_pinned.image.as_raw_fd(),
                local_pinned.cwd.as_raw_fd(),
                local_pinned.input.as_raw_fd(),
                local_pinned.recipe.as_raw_fd(),
                config_source.as_raw_fd(),
            ],
        )
        .map_err(|e| format!("selected interactive plan changed before #: {e}"))?;
        selected_interactive = Some(interactive.clone());
        let control = root_pty_control::RootPtyControl::offer(
            &socket,
            &authority.receipt.d_key,
            &authority.session.session_id,
            &selected.account,
            &interactive.plan_sha256,
            &std::path::PathBuf::from(
                std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1")
                    .ok_or("private PTY control directory absent")?,
            ),
        )?;
        if std::env::var_os("AGE319_PRIVATE_ROOT_PTY_NEGATIVE_V1").is_some() {
            let wrong_role = private_pin_plan(&local_interactive, FreshPlanRole::Headless)?;
            if control
                .prepare_interactive_k(
                    &socket,
                    [
                        wrong_role.image.as_raw_fd(),
                        wrong_role.cwd.as_raw_fd(),
                        wrong_role.input.as_raw_fd(),
                        wrong_role.recipe.as_raw_fd(),
                        config_source.as_raw_fd(),
                    ],
                )
                .is_ok()
            {
                return Err("headless recipe accepted as interactive K preparation".into());
            }
        }
        control.prepare_interactive_k(
            &socket,
            [
                local_pinned.image.as_raw_fd(),
                local_pinned.cwd.as_raw_fd(),
                local_pinned.input.as_raw_fd(),
                local_pinned.recipe.as_raw_fd(),
                config_source.as_raw_fd(),
            ],
        )?;
        Some(control)
    } else {
        None
    };
    if let Some(control) = &root_pty {
        if std::env::var_os("AGE319_PRIVATE_ROOT_PTY_NEGATIVE_V1").is_some() {
            control.probe_wrong_bindings(&socket, &selected.plan_sha256)?;
        }
        if std::env::var_os("AGE319_PRIVATE_ROOT_PTY_REPLAY_V1").is_some() {
            control.rechallenge(&socket)?;
        }
        if std::env::var_os("AGE319_PRIVATE_ROOT_PTY_RESTART_V1").is_some() {
            let gate = std::path::PathBuf::from(
                std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1")
                    .ok_or("private PTY restart gate absent")?,
            );
            std::fs::write(gate.join("root-pty-ready"), b"challenged")
                .map_err(|e| e.to_string())?;
            let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
            while !gate.join("root-pty-rechallenge").exists() {
                if std::time::Instant::now() >= until {
                    return Err("root PTY broker restart wait expired".into());
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let readback = protocol::private_fresh_interactive_route_at(
                &socket,
                &request,
                b')',
                &[config_source.as_raw_fd()],
            )
            .map_err(|e| format!("interactive selection restart readback refused: {e}"))?;
            if readback.as_ref() != selected_interactive.as_ref() {
                return Err("interactive selection changed after broker restart".into());
            }
            control.rechallenge(&socket)?;
            let interactive = private_interactive_plan(
                &pool.model,
                selected.index,
                &cwd,
                &authority.session.session_id,
            )?;
            let pinned = private_pin_plan(&interactive, FreshPlanRole::Interactive)?;
            control.prepare_interactive_k(
                &socket,
                [
                    pinned.image.as_raw_fd(),
                    pinned.cwd.as_raw_fd(),
                    pinned.input.as_raw_fd(),
                    pinned.recipe.as_raw_fd(),
                    config_source.as_raw_fd(),
                ],
            )?;
        }
    }
    if route_mode.ends_with("physical_source_changed") {
        let gate = std::path::PathBuf::from(
            std::env::var("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1")
                .map_err(|e| e.to_string())?,
        );
        std::fs::write(gate.join("v3-provider-ready"), b"yes").map_err(|e| e.to_string())?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !gate.join("v3-provider-continue").exists() {
            if std::time::Instant::now() >= deadline {
                return Err("v3 provider source gate expired".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    if std::env::var_os("AGE319_PRIVATE_ROOT_PTY_PHYSICAL_V1").is_some() {
        let control = root_pty.as_mut().ok_or("physical PTY control absent")?;
        if std::env::var_os("AGE319_PRIVATE_ROOT_PTY_PHYSICAL_ACTOR_GATE_V1").is_some() {
            let gate = std::path::PathBuf::from(
                std::env::var("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1")
                    .map_err(|e| e.to_string())?,
            );
            std::fs::write(gate.join("physical-before-k"), b"ready").map_err(|e| e.to_string())?;
            let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
            while !gate.join("physical-continue").exists() {
                if std::time::Instant::now() >= until {
                    return Err("physical wrong-actor gate expired".into());
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        let interactive = private_interactive_plan(
            &pool.model,
            selected.index,
            &cwd,
            &authority.session.session_id,
        )?;
        let pinned = private_pin_plan(&interactive, FreshPlanRole::Interactive)?;
        let plan_source = [
            pinned.image.as_raw_fd(),
            pinned.cwd.as_raw_fd(),
            pinned.input.as_raw_fd(),
            pinned.recipe.as_raw_fd(),
            config_source.as_raw_fd(),
        ];
        let resident_mode = std::env::var_os("AGE319_PRIVATE_ROOT_PTY_RESIDENT_V1").is_some();
        let (q, output) = if resident_mode {
            let registry = private_resident_registry(&config_dir, &pool.model)?;
            let endpoint = registry
                .preflight_account(&selected.account)
                .map_err(|e| format!("selected resident adapter unavailable before K: {e}"))?;
            let instance = format!("{}-instance", endpoint.capabilities().provider_id);
            let settings = endpoint
                .settings_id()
                .map_err(|e| e.to_string())?
                .to_owned();
            if !endpoint.capabilities().capabilities.session
                || !endpoint.capabilities().capabilities.session_turn_pages_v1
            {
                return Err("selected resident adapter lacks native pages before K".into());
            }
            let running = control.start_interactive(&socket, plan_source)?;
            if let Ok(failure) = std::env::var("AGE319_PRIVATE_RESIDENT_FAILURE_V1") {
                let gate = std::path::PathBuf::from(
                    std::env::var("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1")
                        .map_err(|e| e.to_string())?,
                );
                let (q, output) = if failure == "stale_provider" {
                    let (q, output) = control.finish_interactive(
                        &socket,
                        running,
                        b"fixture-input-through-pty\n",
                    )?;
                    let refusal = control.probe_resident_refusal(&socket, &failure)?;
                    std::fs::write(gate.join("interactive-resident-refusal"), refusal)
                        .map_err(|e| e.to_string())?;
                    (q, output)
                } else {
                    let refusal = control.probe_resident_refusal(&socket, &failure)?;
                    std::fs::write(gate.join("interactive-resident-refusal"), refusal)
                        .map_err(|e| e.to_string())?;
                    let (q, output) = control.finish_interactive(
                        &socket,
                        running,
                        b"fixture-input-through-pty\n",
                    )?;
                    (q, output)
                };
                (q, output)
            } else {
                let resident = control.register_resident(&socket)?;
                control.set_selected_adapter(&instance, &settings)?;
                let challenge = control.challenge_resident()?;
                if challenge.provider_account != selected.account
                    || resident["resident"]["registration"]["plan_sha256"]
                        != selected_interactive
                            .as_ref()
                            .ok_or("interactive selection missing for resident")?
                            .plan_sha256
                    || challenge.provider_session_id != authority.session.session_id
                    || challenge.provider_instance_id != instance
                    || challenge.settings_id != settings
                    || challenge.generation_id != resident["resident"]["registration"]["grant_id"]
                {
                    return Err("resident socket differs from selected adapter or broker K".into());
                }
                if private_resident_empty_tail(
                    &registry,
                    &pool.model.name,
                    &selected.account,
                    &instance,
                    "wrong-settings",
                    &authority.session.session_id,
                    &cwd,
                )
                .is_ok()
                {
                    return Err("wrong selected adapter settings read native Tail".into());
                }
                if private_resident_empty_tail(
                    &registry,
                    &pool.model.name,
                    &selected.account,
                    &instance,
                    &settings,
                    &uuid::Uuid::new_v4().to_string(),
                    &cwd,
                )
                .is_ok()
                {
                    return Err("wrong native session read selected Tail".into());
                }
                let native = private_resident_empty_tail(
                    &registry,
                    &pool.model.name,
                    &selected.account,
                    &instance,
                    &settings,
                    &authority.session.session_id,
                    &cwd,
                )?;
                let broker_challenged = control.broker_resident_readback(&socket)?;
                if broker_challenged["resident"] != resident["resident"]
                    || broker_challenged["generation"] != resident["generation"]
                    || broker_challenged["socket"]["provider_instance_id"] != instance
                    || broker_challenged["socket"]["settings_id"] != settings
                {
                    return Err("broker resident readback changed after native Tail".into());
                }
                let gate = std::path::PathBuf::from(
                    std::env::var("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1")
                        .map_err(|e| e.to_string())?,
                );
                std::fs::write(
                    gate.join("interactive-resident-readback.json"),
                    serde_json::to_vec(&serde_json::json!({
                        "broker": broker_challenged, "socket": challenge, "native_tail": native,
                    }))
                    .map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?;
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
                while !gate.join("interactive-resident-continue").exists() {
                    if std::time::Instant::now() >= deadline {
                        return Err("resident live readback gate expired".into());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                if std::env::var_os("AGE319_PRIVATE_ROOT_PTY_NATIVE_F_FENCE_V1").is_some() {
                    private_resident_fence_pending_f(
                        &socket,
                        &authority.receipt.d_key,
                        &authority.session.session_id,
                        &registry,
                        &pool.model.name,
                        &selected.account,
                        &instance,
                        &settings,
                        &cwd,
                        challenge.generation_id.as_str(),
                        &gate,
                        if std::env::var_os("AGE319_PRIVATE_ROOT_PTY_NATIVE_F_PHYSICAL_V1")
                            .is_some()
                        {
                            Some(control)
                        } else {
                            None
                        },
                    )?;
                }
                control.finish_interactive(&socket, running, b"fixture-input-through-pty\n")?
            }
        } else {
            control.run_interactive(&socket, plan_source, b"fixture-input-through-pty\n")?
        };
        let gate = std::path::PathBuf::from(
            std::env::var("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1")
                .map_err(|e| e.to_string())?,
        );
        std::fs::write(gate.join("interactive-q-readback"), q).map_err(|e| e.to_string())?;
        std::fs::write(gate.join("interactive-output-readback"), output)
            .map_err(|e| e.to_string())?;
        if resident_mode {
            return Ok(ExitCode::SUCCESS);
        }
    }
    let mut backend = PrivateFreshBroker {
        authority,
        grant_id: None,
        raw_result: None,
    };
    let result = run_prepared_fresh_headless(selected_plan, &mut backend)?;
    if let Some(control) = &root_pty {
        control.ensure_live()?;
    }
    let typed_auth_rejection = oulipoly_runtime::diagnostics::non_quota_failure_diagnosis(
        &result.stderr,
        result.exit_code,
    )
    .is_some_and(|diagnosis| {
        diagnosis.category == oulipoly_runtime::diagnostics::ErrorCategory::AuthExpired
    });
    let mut auth_after_provider_q = false;
    if typed_auth_rejection
        && pool.account_effects[selected.index].1.is_some()
        && quota_receipts.iter().any(|(request, _, outcome)| {
            request.index == selected.index && outcome.as_deref() == Some("valid_windows")
        })
    {
        let mut effect = quota_receipts
            .iter()
            .find(|(request, _, _)| request.index == selected.index)
            .expect("selected healthy quota receipt")
            .0
            .clone();
        effect.kind = FreshAccountEffectKind::AuthRefresh;
        let auth =
            private_run_account_effect(&socket, &backend.authority.receipt.handoff_id, &effect)?;
        auth_receipts.push((effect.clone(), auth.effect_id.clone(), auth.outcome.clone()));
        if auth.outcome.as_deref() == Some("refreshed") {
            effect.kind = FreshAccountEffectKind::QuotaRetry;
            let retry = private_run_account_effect(
                &socket,
                &backend.authority.receipt.handoff_id,
                &effect,
            )?;
            auth_receipts.push((effect, retry.effect_id, retry.outcome));
            auth_after_provider_q = true;
        }
    }
    let mut quota_restart_readback = false;
    if matches!(
        std::env::var("AGE319_PRIVATE_JOIN_MODE").ok().as_deref(),
        Some("normal_model_provider_quota_restart" | "normal_model_provider_auth_restart")
    ) {
        if quota_receipts.is_empty() {
            return Err("fresh quota restart fixture has no effect receipt".into());
        }
        for (effect_request, expected_id, expected_outcome) in
            quota_receipts.into_iter().chain(auth_receipts)
        {
            let after_restart =
                protocol::private_fresh_account_effect_at(&socket, &effect_request, false)
                    .map_err(|e| {
                        format!("fresh quota readback after broker restart failed: {e}")
                    })?;
            if after_restart.effect_id != expected_id || after_restart.outcome != expected_outcome {
                return Err("fresh quota readback after broker restart changed".into());
            }
        }
        quota_restart_readback = true;
    }
    let gate = std::env::var("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1").map_err(|e| {
        backend.unknown(
            backend.grant_id.as_deref(),
            "mapped result witness",
            &e.to_string(),
        )
    })?;
    let witness = serde_json::json!({
        "mapped_after_q": true,
        "exit_code": result.exit_code,
        "stdout": String::from_utf8_lossy(&result.stdout),
        "stderr": result.stderr,
        "provider_index": result.provider_index,
        "model": selected.model,
        "provider": selected.account,
        "route_config_sha256": selected.config_sha256,
        "route_plan_sha256": selected.plan_sha256,
        "route_observed_live": selected.observed_live,
        "route_observed_failures": selected.observed_failures,
        "route_observed_invocations": selected.observed_invocations,
        "quota_restart_readback": quota_restart_readback,
        "shared_quota_receipts": shared_quota_receipts,
        "auth_after_provider_q": auth_after_provider_q,
        "terminal_reason": result.terminal_reason,
    });
    let result_name = if std::env::var_os("AGE319_PRIVATE_SECOND_RESULT_V1").is_some() {
        "provider-runtime-result-second"
    } else {
        "provider-runtime-result"
    };
    std::fs::write(
        std::path::Path::new(&gate).join(result_name),
        serde_json::to_vec(&witness).map_err(|e| {
            backend.unknown(
                backend.grant_id.as_deref(),
                "mapped result witness",
                &e.to_string(),
            )
        })?,
    )
    .map_err(|e| {
        backend.unknown(
            backend.grant_id.as_deref(),
            "mapped result witness",
            &e.to_string(),
        )
    })?;
    if std::env::var_os("AGE319_PRIVATE_ROOT_TERMINAL_V1").is_some() {
        use protocol::FreshRecipientRequest;
        let d_key = backend.authority.receipt.d_key.clone();
        let recorded = protocol::fresh_root_terminal_request_at(
            &socket,
            &FreshRecipientRequest::SettleRootTerminal {
                d_key: d_key.clone(),
            },
        )
        .map_err(|e| format!("private root terminal settle failed: {e}"))?;
        let read = protocol::fresh_root_terminal_request_at(
            &socket,
            &FreshRecipientRequest::ReadRootTerminal {
                d_key: d_key.clone(),
            },
        )
        .map_err(|e| format!("private root terminal readback failed: {e}"))?;
        if recorded != read {
            return Err("private root terminal lost reply readback changed".into());
        }
        let repaired = protocol::fresh_root_terminal_request_at(
            &socket,
            &FreshRecipientRequest::RepairRootTerminal { d_key },
        )
        .map_err(|e| format!("private root terminal repair readback failed: {e}"))?;
        if repaired != read {
            return Err("private root terminal exact repair changed record".into());
        }
        std::fs::write(
            std::path::Path::new(&gate).join("root-terminal-readback.json"),
            serde_json::to_vec(&read).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        if std::env::var_os("AGE319_PRIVATE_CALLER_OUTPUT_V1").is_some() {
            let raw = backend
                .raw_result
                .as_ref()
                .ok_or("broker Q raw result absent")?;
            return private_publish_caller_result(
                &socket,
                &backend.authority.receipt.d_key,
                &read,
                raw,
            );
        }
    }
    // The private mapped result and terminal readback do not activate the
    // ordinary caller publication path.
    Err("private provider runtime result mapped after Q; root terminal publication closed".into())
}

#[cfg(feature = "age319-private-broker-fixture")]
fn private_publish_caller_result(
    socket: &std::path::Path,
    d_key: &str,
    terminal: &oulipoly_state::mailbox::FreshRootTerminalReadback,
    raw: &oulipoly_runtime::executor::cli::fresh_remote::FreshProviderCompletion,
) -> Result<ExitCode, String> {
    use oulipoly_kernel_broker::protocol::FreshRecipientRequest;
    use oulipoly_state::mailbox::FreshRootCallerResult;
    use sha2::{Digest as _, Sha256};
    let execution = terminal
        .execution
        .as_ref()
        .ok_or("caller result terminal execution absent")?;
    if terminal.publication_state != "not_started" {
        return Err(
            "caller result publication already unknown; automatic output replay refused".into(),
        );
    }
    if terminal.execution_state == "unknown" || !terminal.unresolved_child_request_ids.is_empty() {
        return Err("caller result blocked by terminal execution or child C debt".into());
    }
    let offered = FreshRootCallerResult {
        parent_grant_id: execution.parent.grant_id.clone(),
        wait_status: raw.wait_status,
        stdout_sha256: format!("{:x}", Sha256::digest(&raw.stdout)),
        stdout_len: raw.stdout.len() as u64,
        stderr_sha256: format!("{:x}", Sha256::digest(&raw.stderr)),
        stderr_len: raw.stderr.len() as u64,
    };
    if offered.wait_status != execution.parent.wait_status
        || offered.stdout_sha256 != execution.parent.stdout_sha256
        || offered.stdout_len != execution.parent.stdout_len
        || offered.stderr_sha256 != execution.parent.stderr_sha256
        || offered.stderr_len != execution.parent.stderr_len
    {
        return Err("caller result bytes differ from verified parent Q".into());
    }
    if !libc::WIFEXITED(raw.wait_status) {
        return Err(
            "caller result gap: non-exit wait status cannot be represented by CLI exit code".into(),
        );
    }
    let code = libc::WEXITSTATUS(raw.wait_status);
    let code = u8::try_from(code).map_err(|_| "caller result exit code out of range")?;
    let reserved = protocol::fresh_root_terminal_request_at(
        socket,
        &FreshRecipientRequest::BeginRootCallerResult {
            d_key: d_key.to_owned(),
            result: offered,
        },
    )
    .map_err(|e| format!("caller result publication reservation failed: {e}"))?;
    if reserved.publication_state != "unknown"
        || reserved.execution_state == "unknown"
        || !reserved.unresolved_child_request_ids.is_empty()
        || reserved.execution != terminal.execution
        || reserved.actor != terminal.actor
        || reserved.invocation_uuid != terminal.invocation_uuid
        || reserved.d_key != terminal.d_key
    {
        return Err("caller result publication readback changed".into());
    }
    if std::env::var_os("AGE319_PRIVATE_CALLER_LOST_WRITE_V1").is_some() {
        return Err("caller result write lost before bytes; publication remains unknown".into());
    }
    let mut stderr = std::io::stderr().lock();
    let mut stdout = std::io::stdout().lock();
    if std::env::var_os("AGE319_PRIVATE_CALLER_PARTIAL_WRITE_V1").is_some() {
        let mut partial = PrivatePartialCallerWrite {
            inner: &mut stdout,
            remaining: 1,
        };
        return private_write_caller_bytes(&mut partial, &mut stderr, &raw.stdout, &raw.stderr)
            .map(|_| ExitCode::from(code))
            .map_err(|e| {
                format!("caller result write uncertain; publication remains unknown: {e}")
            });
    }
    private_write_caller_bytes(&mut stdout, &mut stderr, &raw.stdout, &raw.stderr)
        .map_err(|e| format!("caller result write uncertain; publication remains unknown: {e}"))?;
    Ok(ExitCode::from(code))
}

#[cfg(feature = "age319-private-broker-fixture")]
struct PrivatePartialCallerWrite<'a, W: Write> {
    inner: &'a mut W,
    remaining: usize,
}

#[cfg(feature = "age319-private-broker-fixture")]
impl<W: Write> Write for PrivatePartialCallerWrite<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.remaining == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "private caller disconnected",
            ));
        }
        let size = bytes.len().min(self.remaining);
        let written = self.inner.write(&bytes[..size])?;
        self.remaining -= written;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(feature = "age319-private-broker-fixture")]
fn private_write_caller_bytes(
    stdout: &mut impl Write,
    stderr: &mut impl Write,
    stdout_bytes: &[u8],
    stderr_bytes: &[u8],
) -> std::io::Result<()> {
    stderr.write_all(stderr_bytes)?;
    stderr.flush()?;
    stdout.write_all(stdout_bytes)?;
    stdout.flush()
}

#[cfg(feature = "age319-private-broker-fixture")]
fn private_run_account_effect(
    socket: &std::path::Path,
    handoff_id: &str,
    request: &oulipoly_kernel_broker::protocol::FreshAccountEffectRequest,
) -> Result<oulipoly_kernel_broker::protocol::FreshAccountEffectReadback, String> {
    use oulipoly_kernel_broker::protocol;
    let restart_probe = matches!(
        std::env::var("AGE319_PRIVATE_JOIN_MODE").ok().as_deref(),
        Some(
            "normal_model_provider_v3_quota_restart"
                | "normal_model_provider_v3_quota_auth_restart"
        )
    );
    let restart_deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let started = match protocol::private_fresh_account_effect_at(socket, request, true) {
        Ok(started) => started,
        Err(_) if restart_probe => loop {
            match protocol::private_fresh_account_effect_at(socket, request, false) {
                Ok(observed) => break observed,
                Err(_) if std::time::Instant::now() < restart_deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(e) => {
                    return Err(private_account_effect_unknown(
                        handoff_id,
                        request,
                        None,
                        "begin/readback",
                        &e.to_string(),
                    ));
                }
            }
        },
        Err(_) => {
            protocol::private_fresh_account_effect_at(socket, request, false).map_err(|e| {
                private_account_effect_unknown(
                    handoff_id,
                    request,
                    None,
                    "begin/readback",
                    &e.to_string(),
                )
            })?
        }
    };
    let mut effect = started;
    while effect.state == "pending" {
        std::thread::sleep(std::time::Duration::from_millis(100));
        effect = match protocol::private_fresh_account_effect_at(socket, request, false) {
            Ok(observed) => observed,
            Err(_) if restart_probe && std::time::Instant::now() < restart_deadline => continue,
            Err(e) => {
                return Err({
                    private_account_effect_unknown(
                        handoff_id,
                        request,
                        Some(&effect),
                        "Q readback",
                        &e.to_string(),
                    )
                });
            }
        };
    }
    if effect.state != "drained" {
        return Err(private_account_effect_unknown(
            handoff_id,
            request,
            Some(&effect),
            "Q readback",
            &effect.state,
        ));
    }
    Ok(effect)
}

#[cfg(feature = "age319-private-broker-fixture")]
fn private_account_effect_unknown(
    handoff_id: &str,
    request: &oulipoly_kernel_broker::protocol::FreshAccountEffectRequest,
    readback: Option<&oulipoly_kernel_broker::protocol::FreshAccountEffectReadback>,
    stage: &str,
    reason: &str,
) -> String {
    use oulipoly_kernel_broker::protocol::FreshAccountEffectKind;
    let kind = match request.kind {
        FreshAccountEffectKind::QuotaFirst => "quota-first",
        FreshAccountEffectKind::AuthRefresh => "auth-refresh",
        FreshAccountEffectKind::QuotaRetry => "quota-retry",
    };
    format!(
        "fresh account effect unknown: {}",
        serde_json::json!({
            "d_key": request.d_key,
            "handoff_id": handoff_id,
            "account": request.account,
            "kind": request.kind,
            "effect_id": readback.map(|effect| &effect.effect_id),
            "artifact": readback.map(|effect| effect.artifact.clone()).unwrap_or_else(||
                format!("v30/fresh-provider/account-effects/{handoff_id}-{}-{kind}", request.index)
            ),
            "stage": stage,
            "reason": reason,
            "automatic_replay": false,
            "caller_retry_duplicate_effect_risk": true,
        })
    )
}

#[cfg(feature = "age319-private-broker-fixture")]
fn private_bash_recipient_probe(
    socket: &std::path::Path,
    root_d: &str,
    gate: &str,
) -> Result<(), String> {
    use base64::Engine as _;
    use protocol::FreshRecipientRequest;
    use sha2::Digest as _;
    let mode = std::env::var("AGE319_PRIVATE_BASH_RECIPIENT_MODE_V1").map_err(|e| e.to_string())?;
    if !matches!(
        mode.as_str(),
        "ack" | "lost_pending" | "prepare_unavailable"
    ) {
        return Err("invalid private Bash recipient mode".into());
    }
    let request_id = std::env::var("AGE319_PRIVATE_BASH_REQUEST_KEY").map_err(|e| e.to_string())?;
    let delivery_request_id = uuid::Uuid::new_v4().to_string();
    let request_path = std::path::Path::new(gate).join("bash-f-request-id");
    let mut request_file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&request_path)
        .map_err(|e| e.to_string())?;
    request_file
        .write_all(delivery_request_id.as_bytes())
        .map_err(|e| e.to_string())?;
    request_file.sync_all().map_err(|e| e.to_string())?;
    File::open(gate)
        .and_then(|directory| directory.sync_all())
        .map_err(|e| e.to_string())?;
    let delivered = if mode == "lost_pending" {
        // Deliberately lose the first F response. Read the same request before
        // explicitly recovering the same bytes; never submit a second F.
        let mut stream = UnixStream::connect(socket).map_err(|e| e.to_string())?;
        let mut challenge = [0u8; 16];
        stream
            .read_exact(&mut challenge)
            .map_err(|e| e.to_string())?;
        let request = FreshRecipientRequest::Submit {
            allocation_request_id: root_d.into(),
            delivery_request_id: delivery_request_id.clone(),
        };
        let mut frame = vec![b'F'];
        frame.extend_from_slice(&challenge);
        frame.extend_from_slice(&serde_json::to_vec(&request).map_err(|e| e.to_string())?);
        stream.write_all(&frame).map_err(|e| e.to_string())?;
        drop(stream);
        let deadline = std::time::Instant::now() + PRIVATE_BASH_F_RECEIPT_WAIT;
        let read = loop {
            let read = protocol::fresh_recipient_request_at(
                socket,
                &FreshRecipientRequest::Read {
                    delivery_request_id: delivery_request_id.clone(),
                },
            )
            .map_err(|e| e.to_string())?;
            if !read["grant"].is_null() {
                break read;
            }
            if std::time::Instant::now() >= deadline {
                return Err("lost F grant absent".into());
            }
            std::thread::sleep(PRIVATE_BASH_F_RECEIPT_POLL);
        };
        if read["grant"].is_null() || read["grant"].get("delivery_token").is_some() {
            return Err("lost F request did not retain exact status-only readback".into());
        }
        let grant = &read["grant"];
        let lookup = protocol::fresh_recipient_request_at(
            socket,
            &FreshRecipientRequest::Lookup {
                lane_id: grant["lane_id"]
                    .as_str()
                    .ok_or("lost F lane absent")?
                    .into(),
                session_id: grant["session_id"]
                    .as_str()
                    .ok_or("lost F session absent")?
                    .into(),
                seq: grant["seq"].as_i64().ok_or("lost F row absent")?,
            },
        )
        .map_err(|e| e.to_string())?;
        serde_json::json!({"kind":"manual_lookup_after_lost_reply", "grant":grant,
            "payload_base64":lookup["payload_base64"]})
    } else {
        protocol::fresh_recipient_request_at(
            socket,
            &FreshRecipientRequest::Submit {
                allocation_request_id: root_d.into(),
                delivery_request_id: delivery_request_id.clone(),
            },
        )
        .map_err(|e| e.to_string())?
    };
    let grant = &delivered["grant"];
    let seq = grant["seq"].as_i64().ok_or("fresh F row absent")?;
    if grant["source_id"].as_str().is_none() || grant["attempt_id"].as_str().is_none() {
        return Err("fresh F exact source/row binding absent".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(
            delivered["payload_base64"]
                .as_str()
                .ok_or("fresh F payload absent")?,
        )
        .map_err(|e| e.to_string())?;
    let payload: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if payload["protocol"] != "fresh-bash-complete-v30"
        || payload["source"]["request_id"] != request_id
        || payload["source"]["source_id"] != grant["source_id"]
        || payload["source"]["attempt_id"] != grant["attempt_id"]
        || payload["stdout_bytes"] != serde_json::json!(b"broker-child-output\n")
    {
        return Err("fresh F payload differs from broker W raw output".into());
    }
    let lookup = protocol::fresh_recipient_request_at(
        socket,
        &FreshRecipientRequest::Lookup {
            lane_id: grant["lane_id"].as_str().ok_or("F lane absent")?.into(),
            session_id: grant["session_id"]
                .as_str()
                .ok_or("F session absent")?
                .into(),
            seq,
        },
    )
    .map_err(|e| e.to_string())?;
    if lookup["payload_base64"] != delivered["payload_base64"] {
        return Err("read-only lookup changed fresh F bytes".into());
    }
    let preparation_refusal = if mode == "prepare_unavailable" {
        let read = protocol::fresh_recipient_request_at(
            socket,
            &FreshRecipientRequest::Read {
                delivery_request_id: delivery_request_id.clone(),
            },
        )
        .map_err(|e| e.to_string())?;
        let exact: oulipoly_state::mailbox::FreshDeliveryReadback =
            serde_json::from_value(read["grant"].clone()).map_err(|e| e.to_string())?;
        if exact.grant_id != grant["grant_id"]
            || exact.session_id != grant["session_id"]
            || exact.seq != seq
            || exact.source_id != grant["source_id"]
            || exact.attempt_id != grant["attempt_id"]
        {
            return Err("native F original recipient readback changed F".into());
        }
        let token = grant["delivery_token"]
            .as_str()
            .ok_or("native F original F token absent")?;
        let wrong_session = crate::native_f_preparation::prepare_original_recipient_native_f(
            socket,
            &uuid::Uuid::new_v4().to_string(),
            &delivery_request_id,
            &exact,
            Some(token),
            None,
        )
        .expect_err("wrong fresh session unexpectedly prepared native F");
        if wrong_session != "native F original recipient grant or fresh session changed" {
            return Err(format!(
                "native F wrong-session refusal changed: {wrong_session}"
            ));
        }
        let lost_token = crate::native_f_preparation::prepare_original_recipient_native_f(
            socket,
            &exact.session_id,
            &delivery_request_id,
            &exact,
            None,
            None,
        )
        .expect_err("lost F token unexpectedly prepared native F");
        if lost_token != "native F delivery token unknown after lost F reply" {
            return Err(format!("native F lost-token refusal changed: {lost_token}"));
        }
        let mut refusal = None;
        for _ in 0..2 {
            let error = crate::native_f_preparation::prepare_original_recipient_native_f(
                socket,
                &exact.session_id,
                &delivery_request_id,
                &exact,
                Some(token),
                None,
            )
            .expect_err("headless private root unexpectedly prepared native F");
            if refusal.as_ref().is_some_and(|previous| previous != &error) {
                return Err("native F unavailable retry changed refusal".into());
            }
            refusal = Some(error);
        }
        refusal
    } else {
        None
    };
    if mode == "ack" {
        if protocol::fresh_recipient_request_at(
            socket,
            &FreshRecipientRequest::Acknowledge {
                grant_id: grant["grant_id"].as_str().ok_or("F grant absent")?.into(),
                delivery_token: uuid::Uuid::new_v4().to_string(),
            },
        )
        .is_ok()
        {
            return Err("wrong fresh F token acknowledged accepted W".into());
        }
        let ack = protocol::fresh_recipient_request_at(
            socket,
            &FreshRecipientRequest::Acknowledge {
                grant_id: grant["grant_id"].as_str().ok_or("F grant absent")?.into(),
                delivery_token: grant["delivery_token"]
                    .as_str()
                    .ok_or("F token absent")?
                    .into(),
            },
        )
        .map_err(|e| e.to_string())?;
        if ack["grant"]["phase"] != "acked" {
            return Err("fresh F recipient assertion not acknowledged".into());
        }
    }
    let read = protocol::fresh_recipient_request_at(
        socket,
        &FreshRecipientRequest::Read {
            delivery_request_id: delivery_request_id.clone(),
        },
    )
    .map_err(|e| e.to_string())?;
    if read["grant"]["grant_id"] != grant["grant_id"]
        || (mode == "ack" && read["grant"]["phase"] != "acked")
        || (mode == "lost_pending"
            && read["grant"]["phase"] != "unknown"
            && read["grant"]["phase"] != "submitted")
    {
        return Err("fresh F/ACK readback mismatch".into());
    }
    std::fs::write(
        std::path::Path::new(gate).join("bash-recipient-output"),
        serde_json::to_vec(
            &serde_json::json!({"mode":mode,"listener_policy":"notify_at_admission",
            "delivery_request_id":delivery_request_id,"grant":grant,"readback":read,
            "native_f_preparation_refusal":preparation_refusal,
            "observed_payload_sha256":format!("{:x}",sha2::Sha256::digest(&bytes))}),
        )
        .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(feature = "age319-private-broker-fixture")]
fn socket_for_private_causal_bash() -> PathBuf {
    broker_socket().with_file_name("v30.sock")
}

#[cfg(feature = "age319-private-broker-fixture")]
fn private_sealed_bytes(name: &'static [u8], bytes: &[u8]) -> Result<File, String> {
    use std::io::Seek;
    let name = std::ffi::CString::new(name).map_err(|e| e.to_string())?;
    let fd =
        unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_ALLOW_SEALING | libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let mut file = unsafe { File::from_raw_fd(fd) };
    file.write_all(bytes).map_err(|e| e.to_string())?;
    let seals = libc::F_SEAL_SEAL | libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_WRITE;
    if unsafe { libc::fcntl(fd, libc::F_ADD_SEALS, seals) } < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    file.rewind().map_err(|e| e.to_string())?;
    Ok(file)
}

fn begin_root_effect(
    receipt: &oulipoly_state::mailbox::FreshReleasedHandoff,
    session: &oulipoly_state::mailbox::FreshV30Session,
) -> Result<(), String> {
    let effect = protocol::begin_fresh_root_effect_at(
        &broker_socket().with_file_name("v30.sock"),
        &receipt.d_key,
    )
    .map_err(|e| format!("v30 root pre-effect start absent: {e}"))?;
    if effect.handoff_id != receipt.handoff_id
        || effect.invocation_uuid != receipt.invocation_uuid
        || effect.session_id != session.session_id
        || effect.intent != receipt.root_work_intent
        || effect.state != oulipoly_state::mailbox::FreshRootEffectState::Started
    {
        return Err("v30 root pre-effect readback conflict".into());
    }
    Ok(())
}

fn return_root_effect(
    receipt: &oulipoly_state::mailbox::FreshReleasedHandoff,
    session: &oulipoly_state::mailbox::FreshV30Session,
    success: bool,
) -> Result<(), String> {
    let socket = broker_socket().with_file_name("v30.sock");
    let expected = if success {
        oulipoly_state::mailbox::FreshRootEffectState::ReturnedSuccess
    } else {
        oulipoly_state::mailbox::FreshRootEffectState::ReturnedFailure
    };
    let effect = protocol::return_fresh_root_effect_at(&socket, &receipt.d_key, success)
        .or_else(|_| {
            protocol::observe_fresh_root_effect_at(&socket, &receipt.d_key)
                .and_then(|value| value.ok_or_else(|| std::io::Error::other("root result unknown")))
        })
        .map_err(|e| format!("v30 root result unknown: {e}"))?;
    if effect.handoff_id != receipt.handoff_id
        || effect.invocation_uuid != receipt.invocation_uuid
        || effect.session_id != session.session_id
        || effect.intent != receipt.root_work_intent
        || effect.state != expected
    {
        return Err("v30 root result readback conflict or unknown".into());
    }
    Ok(())
}

fn spec_for_handoff(spec: &protocol::StateReadSpec) -> protocol::StateReadSpec {
    protocol::StateReadSpec {
        protocol: spec.protocol.clone(),
        source_generation: spec.source_generation.clone(),
        owner_generation: spec.owner_generation.clone(),
        root_id: spec.root_id.clone(),
        attempt_id: None,
    }
}

#[cfg(feature = "age319-private-broker-fixture")]
fn private_drop_fresh_reply(
    socket: &std::path::Path,
    operation: u8,
    release: Option<protocol::StateReadSpec>,
    d_key: Option<&str>,
) -> Result<(), String> {
    let mut stream = UnixStream::connect(socket).map_err(|e| e.to_string())?;
    let mut challenge = [0u8; 16];
    stream
        .read_exact(&mut challenge)
        .map_err(|e| e.to_string())?;
    let mut frame = Vec::from([operation]);
    frame.extend_from_slice(&challenge);
    match operation {
        b'U' => frame.extend_from_slice(
            &serde_json::to_vec(&protocol::FreshChildRequest {
                request_id: String::new(),
                invocation_uuid: String::new(),
                release,
            })
            .map_err(|e| e.to_string())?,
        ),
        b'D' => frame.extend_from_slice(
            uuid::Uuid::parse_str(d_key.ok_or("private D key absent")?)
                .map_err(|e| e.to_string())?
                .as_bytes(),
        ),
        b'0' => frame.extend_from_slice(
            &serde_json::to_vec(&protocol::FreshRootEffectRequest {
                d_key: d_key.ok_or("private root effect D key absent")?.into(),
                success: None,
            })
            .map_err(|e| e.to_string())?,
        ),
        _ => return Err("unsupported private lost-reply operation".into()),
    }
    stream.write_all(&frame).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(PRIVATE_LOST_REPLY_READ_TIMEOUT))
        .map_err(|e| e.to_string())?;
    let mut first = [0u8; 1];
    stream.read_exact(&mut first).map_err(|e| e.to_string())?;
    if first != [b'f'] {
        return Err("private lost-reply operation was refused".into());
    }
    // The broker has committed and started its success reply. Discard the
    // rest so the same child must recover the result with its original key.
    drop(stream);
    Ok(())
}

#[cfg(feature = "age319-private-broker-fixture")]
fn private_bash_work() -> Result<ExitCode, String> {
    crate::completion_owner::join_private_accepted_work_fixture()?;
    use oulipoly_state::pid_identity::{PidIdentityDb, PidIdentityRecord};
    use oulipoly_state::{InvocationStart, ProviderSessionBinding, StateDb};

    // The joined Runner child is the actual owner Bash will inspect through
    // `session of-pid`. Record its procfs-observer incarnation, not getpid():
    // the latter is local to the broker's root PID namespace.
    let invocation_uuid = uuid::Uuid::new_v4().to_string();
    let session_id = format!("age319-paired-{invocation_uuid}");
    let state = StateDb::open(&StateDb::default_path()?).map_err(|e| format!("State: {e:?}"))?;
    let started =
        state.start_invocation_with_completion_registration_authority(&InvocationStart {
            invocation_uuid: invocation_uuid.clone(),
            model_name: "age319-private-bash-work".into(),
            provider_name: "agent-bash".into(),
            provider_index: 0,
            parent_invocation_id: None,
        })?;
    state.bind_invocation_provider_session_start(
        oulipoly_state::InvocationMutationAuthority::Standalone,
        started.invocation_row_id,
        &ProviderSessionBinding {
            provider_session_id: session_id.clone(),
            capture_method: "age319-private-joined-runner",
            resume_input_id: None,
            provider_session_resolved_account: None,
        },
    )?;
    let identity = oulipoly_state::pid_identity::read_current_process_identity()?;
    PidIdentityDb::open(&PidIdentityDb::default_path()?)?.record_identity(PidIdentityRecord {
        identity: &identity,
        os_pgid: None,
        invocation_uuid: &invocation_uuid,
        session_id: Some(&session_id),
        provider_name: Some("agent-bash"),
        model_name: Some("age319-private-bash-work"),
        recorded_at: &chrono::Utc::now().to_rfc3339(),
    })?;
    let parent_marker = started
        .completion_registration_authority
        .invocation_launch_environment(
            &oulipoly_state::invocation_marker::CompositeInvocationId {
                source: "age319-private-joined-runner".into(),
                id: invocation_uuid.clone(),
            },
        )?;
    let bash =
        std::env::var("AGE319_PRIVATE_BASH_IMAGE").map_err(|_| "private Bash image missing")?;
    let script =
        std::env::var("AGE319_PRIVATE_WORK_SCRIPT").map_err(|_| "private work script missing")?;
    let exit = std::process::Command::new(bash)
        .args(["run", "--delivery", "async", "--", "/bin/sh", "-c", &script])
        .env("OULIPOLY_PARENT_INVOCATION", parent_marker)
        .env("AGENT_BASH_OWNER_SESSION_ID", session_id)
        .env("AGENT_BASH_OWNER_INVOCATION_UUID", invocation_uuid)
        .env(
            oulipoly_state::COMPLETION_REGISTRATION_AUTHORITY_ENV,
            started
                .completion_registration_authority
                .process_environment_value(),
        )
        .status()
        .map_err(|error| error.to_string())?;
    Ok(ExitCode::from(exit.code().unwrap_or(70) as u8))
}

pub(crate) fn host_entry() -> Option<ExitCode> {
    #[cfg(feature = "age319-private-broker-fixture")]
    if std::env::args().nth(1).as_deref() == Some("__age319-private-handoff-probe-v1")
        && unsafe { libc::geteuid() } == 0
        && std::fs::read_to_string("/proc/self/uid_map")
            .ok()
            .is_some_and(|map| map.split_ascii_whitespace().nth(2) == Some("1"))
        && std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1").is_some()
    {
        let result = (|| -> Result<(), String> {
            let socket = broker_socket().with_file_name("v30.sock");
            if let Ok(d_key) = std::env::var("AGE319_PRIVATE_HANDOFF_PROBE_D_KEY") {
                protocol::allocate_fresh_v30_session_at(&socket, &d_key)
                    .map_err(|e| e.to_string())?;
            } else {
                let spec: protocol::StateReadSpec = serde_json::from_str(
                    &std::env::var("AGE319_PRIVATE_HANDOFF_PROBE_SPEC")
                        .map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?;
                protocol::request_released_fresh_handoff_at(&socket, spec)
                    .map_err(|e| e.to_string())?;
            }
            Ok(())
        })();
        return Some(if result.is_ok() {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        });
    }
    #[cfg(feature = "age319-private-broker-fixture")]
    if private_prepared_mode() {
        return Some(match private_held_prepared_entry() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("OULIPOLY_KERNEL_PREPARED_GAP={error}");
                ExitCode::FAILURE
            }
        });
    }
    if std::env::var_os(REQUIRED_ENV).is_none() {
        return None;
    }
    if let Err(error) = supported_host_mode() {
        eprintln!("OULIPOLY_KERNEL_ENTRY_GAP={error}");
        return Some(ExitCode::FAILURE);
    }
    let result = match protocol::state_route_at(&broker_socket()) {
        Ok(StateRoute::BrokerOwned { .. }) => v30_host_entry(),
        Err(error) => Err(format!("broker State route unavailable: {error}")),
        Ok(StateRoute::Legacy) => stage_host_entry(
            || {
                match protocol::state_route_at(&broker_socket())
                    .map_err(|e| format!("broker State route unavailable: {e}"))?
                {
                    StateRoute::Legacy => {}
                    StateRoute::BrokerOwned {
                        source_generation, ..
                    } => {
                        return Err(format!(
                            "broker-owned State generation {source_generation} requires v30 entry admission"
                        ));
                    }
                }
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
                    .ok_or_else(|| {
                        format!("kernel entry reservation refused: {}", response.trim())
                    })?;
                uuid::Uuid::parse_str(id).map_err(|_| "invalid broker root ID".to_owned())?;
                Ok(id.to_owned())
            },
            bind_host_guardian,
        ),
    };
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
    let private_entry_fixture = (args == ["__age319-private-bash-work-v1"]
        || args == [PRIVATE_NORMAL_ENTRY])
        && unsafe { libc::geteuid() } == 0
        && std::fs::read_to_string("/proc/self/uid_map")
            .ok()
            .is_some_and(|map| map.split_ascii_whitespace().nth(2) == Some("1"))
        && std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1").is_some();
    #[cfg(not(feature = "age319-private-broker-fixture"))]
    let private_entry_fixture = false;
    if !private_entry_fixture && !protocol::supported_entry_args(&args) {
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

#[cfg(feature = "age319-private-broker-fixture")]
fn private_line(socket: &mut UnixStream) -> Result<String, String> {
    let mut bytes = Vec::new();
    loop {
        if bytes.len() >= 4096 {
            return Err("oversized private prepared frame".into());
        }
        let mut byte = [0u8];
        socket.read_exact(&mut byte).map_err(|e| e.to_string())?;
        if byte == [b'\n'] {
            break;
        }
        bytes.push(byte[0]);
    }
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

#[cfg(feature = "age319-private-broker-fixture")]
fn private_guardian_prepared(
    mut channel: UnixStream,
    broker: &std::path::Path,
    gate_dir: &std::path::Path,
    root: &str,
    domain: &str,
    supervisor: &str,
    source_generation: &str,
) -> Result<(), String> {
    let mut byte = [0u8];
    channel.read_exact(&mut byte).map_err(|e| e.to_string())?;
    if byte != [b'P'] {
        return Err("private guardian prepare gate refused".into());
    }
    let bound = protocol::bind_v30_guardian_at(broker, root, domain, supervisor)
        .map_err(|e| e.to_string())?;
    channel
        .write_all(bound.as_bytes())
        .map_err(|e| e.to_string())?;
    channel.read_exact(&mut byte).map_err(|e| e.to_string())?;
    if byte != [b'X'] {
        return Err("private guardian bind readback refused".into());
    }
    let endpoint = gate_dir.join("pending-owner.sock");
    let pending_endpoint =
        std::os::unix::net::UnixListener::bind(&endpoint).map_err(|e| e.to_string())?;
    let mut source_probe_owner = None;
    let (mut driver_parent, mut driver_child) = UnixStream::pair().map_err(|e| e.to_string())?;
    let driver_pid = unsafe { libc::fork() };
    if driver_pid < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if driver_pid == 0 {
        drop(driver_parent);
        if std::env::var_os("AGE319_PRIVATE_EXEC_DRIVER_ROUTE_V30").is_some() {
            let fd = driver_child.as_raw_fd();
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
            if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0
            {
                unsafe { libc::_exit(70) }
            }
            unsafe { std::env::remove_var(REQUIRED_ENV) };
            let executable = std::ffi::CString::new("/proc/self/exe").unwrap();
            let arg0 = std::ffi::CString::new("oulipoly-agent-runner").unwrap();
            let mode = std::ffi::CString::new(crate::completion_owner::PRIVATE_DRIVER_ARG).unwrap();
            let retired_path = std::path::PathBuf::from(
                std::env::var_os("OULIPOLY_DATA_DIR").expect("private data directory"),
            )
            .join("pid-identity.db");
            let retired =
                std::ffi::CString::new(retired_path.to_string_lossy().as_bytes()).unwrap();
            let fd_arg = std::ffi::CString::new(fd.to_string()).unwrap();
            let root_arg = std::ffi::CString::new(root).unwrap();
            let argv = [
                arg0.as_ptr(),
                mode.as_ptr(),
                retired.as_ptr(),
                fd_arg.as_ptr(),
                root_arg.as_ptr(),
                std::ptr::null(),
            ];
            unsafe {
                libc::execv(executable.as_ptr(), argv.as_ptr());
                libc::_exit(70)
            }
        }
        let result = private_v30_driver_route(&mut driver_child, broker, gate_dir, root);
        if let Err(error) = result {
            eprintln!("OULIPOLY_KERNEL_PRIVATE_DRIVER_GAP={error}");
            unsafe { libc::_exit(70) }
        }
        unsafe { libc::_exit(0) }
    }
    drop(driver_child);
    let owner_generation = uuid::Uuid::new_v4().to_string();
    let proposal = serde_json::json!({
        "driver_pid": driver_pid,
        "owner_generation": owner_generation,
        "endpoint": endpoint.to_string_lossy(),
    });
    channel
        .write_all(proposal.to_string().as_bytes())
        .map_err(|e| e.to_string())?;
    channel.write_all(b"\n").map_err(|e| e.to_string())?;
    channel.read_exact(&mut byte).map_err(|e| e.to_string())?;
    if byte != [b'J'] {
        return Err("private held join refused".into());
    }
    let route = crate::completion_owner::broker_route::V30OwnerRoute::guardian(
        broker,
        root,
        domain,
        supervisor,
        &owner_generation,
    )?;
    let generation = protocol::prepared_generation_at(broker, root).map_err(|e| e.to_string())?;
    if generation != source_generation {
        return Err("broker I/Y source generation changed".into());
    }
    let write = protocol::StateWriteSpec {
        protocol: "broker-prepared-write-v30".into(),
        source_generation: generation.clone(),
        root_id: root.into(),
        owner_generation: owner_generation.clone(),
        action: protocol::StateWriteAction::Prepare {
            driver_pid,
            endpoint: endpoint.to_string_lossy().into_owned(),
        },
    };
    let published = if std::env::var_os("AGE319_PRIVATE_PREPARE_LOST_REPLY_V1").is_some() {
        protocol::prepare_owner_drop_reply_at(broker, &write).map_err(|e| e.to_string())?;
        route.read_prepared(driver_pid, &endpoint)?
    } else {
        route.prepare(driver_pid, &endpoint)?
    };
    if protocol::prepare_owner_at(broker, &write).is_ok() {
        return Err("prepared W replay published twice".into());
    }
    let read = protocol::StateReadSpec {
        protocol: "broker-prepared-read-v30".into(),
        source_generation: generation,
        root_id: root.into(),
        owner_generation,
        attempt_id: None,
    };
    let exact = protocol::read_prepared_owner_at(broker, &read).map_err(|e| e.to_string())?;
    if published != exact
        || exact.domain_id != domain
        || exact.supervisor_authority_id != supervisor
        || exact.guardian.host_pid != unsafe { libc::getpid() }
        || exact.driver.host_pid != driver_pid
    {
        return Err("guardian prepared readback changed".into());
    }
    let mut stale = read;
    stale.source_generation = uuid::Uuid::new_v4().to_string();
    if protocol::read_prepared_owner_at(broker, &stale).is_ok() {
        return Err("stale source generation gained prepared readback".into());
    }
    channel
        .write_all(
            serde_json::to_string(&exact)
                .map_err(|e| e.to_string())?
                .as_bytes(),
        )
        .map_err(|e| e.to_string())?;
    channel.write_all(b"\n").map_err(|e| e.to_string())?;
    if std::env::var_os("AGE319_PRIVATE_RELEASE_V30").is_some() {
        channel.read_exact(&mut byte).map_err(|e| e.to_string())?;
        if byte != [b'L'] {
            return Err("private release instruction refused".into());
        }
        let release = protocol::StateWriteSpec {
            protocol: "broker-held-release-v30".into(),
            source_generation: source_generation.into(),
            root_id: root.into(),
            owner_generation: exact.owner_generation.clone(),
            action: protocol::StateWriteAction::Release,
        };
        let mut stale_release = protocol::StateWriteSpec {
            protocol: release.protocol.clone(),
            source_generation: uuid::Uuid::new_v4().to_string(),
            root_id: release.root_id.clone(),
            owner_generation: release.owner_generation.clone(),
            action: protocol::StateWriteAction::Release,
        };
        if protocol::release_prepared_owner_at(broker, &stale_release).is_ok() {
            return Err("stale release generation accepted".into());
        }
        stale_release.source_generation = release.source_generation.clone();
        stale_release.owner_generation = uuid::Uuid::new_v4().to_string();
        if protocol::release_prepared_owner_at(broker, &stale_release).is_ok() {
            return Err("sibling owner release accepted".into());
        }
        let lost_reply = std::env::var_os("AGE319_PRIVATE_RELEASE_LOST_REPLY_V1").is_some();
        let committed = if lost_reply {
            protocol::release_prepared_owner_drop_reply_at(broker, &release)
                .map_err(|e| e.to_string())?;
            None
        } else {
            Some(route.release(&exact)?)
        };
        if protocol::release_prepared_owner_at(broker, &release).is_ok() {
            return Err("double held release accepted".into());
        }
        let readback = protocol::read_released_owner_at(
            broker,
            &protocol::StateReadSpec {
                protocol: "broker-release-readback-v30".into(),
                source_generation: source_generation.into(),
                root_id: root.into(),
                owner_generation: exact.owner_generation.clone(),
                attempt_id: None,
            },
        )
        .map_err(|e| e.to_string())?;
        if committed
            .as_ref()
            .is_some_and(|committed| committed != &readback)
            || readback.prepared != exact
        {
            return Err("release/readback mismatch".into());
        }
        channel
            .write_all(
                serde_json::to_string(&readback)
                    .map_err(|e| e.to_string())?
                    .as_bytes(),
            )
            .map_err(|e| e.to_string())?;
        channel.write_all(b"\n").map_err(|e| e.to_string())?;
        if gate_dir.join("source-witness-request").exists() {
            source_probe_owner = Some(readback.owner.clone());
        }
        if std::env::var_os("AGE319_PRIVATE_DRIVER_ROUTE_V30").is_some() {
            driver_parent.write_all(b"B").map_err(|e| e.to_string())?;
            serde_json::to_writer(&mut driver_parent, &readback.owner)
                .map_err(|e| e.to_string())?;
            driver_parent.write_all(b"\n").map_err(|e| e.to_string())?;
            let attempt: oulipoly_state::mailbox::ContinuationAttempt =
                serde_json::from_str(&private_line(&mut driver_parent)?)
                    .map_err(|e| e.to_string())?;
            let accepted = route.accept(&readback.owner, &attempt)?;
            if accepted.phase.as_deref() != Some("accepted") || accepted.revision != Some(2) {
                return Err("broker driver proposal was not accepted".into());
            }
            if route.accept(&readback.owner, &attempt).is_ok() {
                return Err("accepted PK replayed".into());
            }
            driver_parent.write_all(b"A").map_err(|e| e.to_string())?;
            driver_parent
                .read_exact(&mut byte)
                .map_err(|e| e.to_string())?;
            if byte != [b'D'] {
                return Err("broker driver did not read accepted PK".into());
            }
            if std::env::var_os("AGE319_PRIVATE_NATIVE_LINEAGE_V30").is_some() {
                let sibling: oulipoly_state::mailbox::ContinuationAttempt =
                    serde_json::from_str(&private_line(&mut driver_parent)?)
                        .map_err(|error| error.to_string())?;
                let sibling_read = route.accept(&readback.owner, &sibling)?;
                if sibling_read.phase.as_deref() != Some("accepted") {
                    return Err("private sibling attempt not accepted".into());
                }
                driver_parent
                    .write_all(b"A")
                    .map_err(|error| error.to_string())?;
                crate::completion_owner::private_native_lineage(
                    &readback.owner,
                    &attempt,
                    &sibling,
                    root,
                    &gate_dir.join("native-work/custodian-request.json"),
                    &gate_dir.join("native-sibling-work/custodian-request.json"),
                    gate_dir,
                )?;
            }
            std::fs::write(
                gate_dir.join("driver-routed"),
                attempt.attempt_id.as_bytes(),
            )
            .map_err(|e| e.to_string())?;
        } else if std::env::var_os("AGE319_PRIVATE_EXEC_DRIVER_ROUTE_V30").is_some() {
            serde_json::to_writer(&mut driver_parent, &readback.owner)
                .map_err(|e| e.to_string())?;
            driver_parent.write_all(b"\n").map_err(|e| e.to_string())?;
        }
    }
    if let Some(owner) = source_probe_owner {
        pending_endpoint
            .set_nonblocking(true)
            .map_err(|e| e.to_string())?;
        std::fs::write(gate_dir.join("source-guardian-stage"), b"accepting")
            .map_err(|e| e.to_string())?;
        channel
            .set_read_timeout(Some(std::time::Duration::from_millis(20)))
            .map_err(|e| e.to_string())?;
        let deadline = std::time::Instant::now() + PRIVATE_PREPARED_FINISH_WAIT;
        loop {
            match channel.read_exact(&mut byte) {
                Ok(()) => break,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(error) => return Err(error.to_string()),
            }
            match pending_endpoint.accept() {
                Ok((mut socket, _)) => {
                    std::fs::write(gate_dir.join("source-guardian-stage"), b"accepted")
                        .map_err(|e| e.to_string())?;
                    socket
                        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                        .map_err(|e| e.to_string())?;
                    socket
                        .set_write_timeout(Some(std::time::Duration::from_secs(5)))
                        .map_err(|e| e.to_string())?;
                    let mut hello = [0u8; 6];
                    socket.read_exact(&mut hello).map_err(|e| e.to_string())?;
                    if &hello != b"hello\n" {
                        return Err("private source guardian accepted non-hello request".into());
                    }
                    socket
                        .write_all(&serde_json::to_vec(&owner).map_err(|e| e.to_string())?)
                        .map_err(|e| e.to_string())?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error.to_string()),
            }
            if std::time::Instant::now() >= deadline {
                return Err("private source guardian hello wait expired".into());
            }
        }
    } else {
        let _ = channel.read_exact(&mut byte);
    }
    let _ = driver_parent.write_all(b"X");
    unsafe { libc::waitpid(driver_pid, std::ptr::null_mut(), 0) };
    Ok(())
}

#[cfg(feature = "age319-private-broker-fixture")]
fn private_native_request(
    gate_dir: &std::path::Path,
    attempt: &oulipoly_state::mailbox::ContinuationAttempt,
    name: &str,
) -> Result<(), String> {
    let work = gate_dir.join(name);
    std::fs::create_dir(&work).map_err(|error| error.to_string())?;
    let marker = gate_dir.join(if name == "native-work" {
        "native-effect"
    } else {
        "native-sibling-effect"
    });
    let args = if std::env::var_os("AGE319_PRIVATE_RECEIPT_HELPER_PROBE_V1").is_some() {
        [
            crate::native_receipt::helper::ARG.as_bytes().to_vec(),
            crate::native_receipt::helper::PRIVATE_BROKER_PROBE_ARG
                .as_bytes()
                .to_vec(),
            marker.as_os_str().as_encoded_bytes().to_vec(),
        ]
    } else {
        [
            b"__age319-private-installed-probe-v1".to_vec(),
            b"provider".to_vec(),
            marker.as_os_str().as_encoded_bytes().to_vec(),
        ]
    };
    let request = serde_json::json!({
        "path": std::path::PathBuf::from(std::env::var("OULIPOLY_DATA_DIR")
            .map_err(|error| error.to_string())?).join("state.db"),
        "attempt": attempt,
        "recipe": {"Native": {"args": args, "environment": [], "directory": null}},
    });
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(work.join("custodian-request.json"))
        .map_err(|error| error.to_string())?;
    serde_json::to_writer(&mut file, &request).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    File::open(&work)
        .and_then(|dir| dir.sync_all())
        .map_err(|error| error.to_string())
}

#[cfg(feature = "age319-private-broker-fixture")]
fn private_v30_driver_route(
    channel: &mut UnixStream,
    broker: &std::path::Path,
    gate_dir: &std::path::Path,
    root: &str,
) -> Result<(), String> {
    let mut instruction = [0u8];
    channel
        .read_exact(&mut instruction)
        .map_err(|e| e.to_string())?;
    if instruction == [b'X'] {
        return Ok(());
    }
    if instruction != [b'B'] || std::env::var_os("AGE319_PRIVATE_DRIVER_ROUTE_V30").is_none() {
        return Err("private driver was released without v30 route".into());
    }
    let owner: oulipoly_state::mailbox::CompletionDomainOwner =
        serde_json::from_str(&private_line(channel)?).map_err(|e| e.to_string())?;
    let route = crate::completion_owner::broker_route::V30OwnerRoute::driver(broker, root, &owner)?;
    let running = route.read_running(&owner, None)?;
    if running.attempt.is_some() {
        return Err("unexpected driver owner attempt".into());
    }
    let spec = protocol::StateReadSpec {
        protocol: "broker-state-read-v1".into(),
        source_generation: running.source_generation.clone(),
        root_id: root.into(),
        owner_generation: owner.owner_generation.clone(),
        attempt_id: None,
    };
    let mut stale = protocol::StateReadSpec {
        source_generation: uuid::Uuid::new_v4().to_string(),
        ..spec
    };
    if protocol::read_state_at(broker, &stale).is_ok() {
        return Err("stale driver source gained R".into());
    }
    stale.source_generation = running.source_generation.clone();
    stale.owner_generation = uuid::Uuid::new_v4().to_string();
    if protocol::read_state_at(broker, &stale).is_ok() {
        return Err("sibling driver owner gained R".into());
    }
    let native = std::env::var_os("AGE319_PRIVATE_NATIVE_LINEAGE_V30").is_some();
    let attempt = oulipoly_state::mailbox::ContinuationAttempt {
        attempt_id: uuid::Uuid::new_v4().to_string(),
        owner_generation: owner.owner_generation.clone(),
        operation: if native { "activation" } else { "transport" }.into(),
        request_sha256: "0".repeat(64),
        source_registration_id: None,
        source_listener_revision: None,
        session_id: native.then(|| "native-session".into()),
        claim_token: native.then(|| "native-claim".into()),
        result_path: gate_dir
            .join(if native {
                "native-work/result.json"
            } else {
                "driver-result.json"
            })
            .to_string_lossy()
            .into_owned(),
    };
    if native {
        private_native_request(gate_dir, &attempt, "native-work")?;
    }
    let reserved = route.reserve(&owner, &attempt)?;
    if route.reserve(&owner, &attempt)? != reserved {
        return Err("driver reservation readback changed".into());
    }
    let replay = protocol::StateWriteSpec {
        protocol: "broker-state-write-v1".into(),
        source_generation: running.source_generation.clone(),
        root_id: root.into(),
        owner_generation: owner.owner_generation.clone(),
        action: protocol::StateWriteAction::Reserve {
            attempt: attempt.clone(),
        },
    };
    if protocol::write_state_at(broker, &replay).is_ok() {
        return Err("broker W reservation replay committed".into());
    }
    serde_json::to_writer(&mut *channel, &attempt).map_err(|e| e.to_string())?;
    channel.write_all(b"\n").map_err(|e| e.to_string())?;
    channel
        .read_exact(&mut instruction)
        .map_err(|e| e.to_string())?;
    if instruction != [b'A'] {
        return Err("private driver acceptance refused".into());
    }
    let accepted = route.read_running(&owner, Some(&attempt.attempt_id))?;
    if accepted.attempt.as_ref() != Some(&attempt)
        || accepted.phase.as_deref() != Some("accepted")
        || accepted.revision != Some(2)
    {
        return Err("private driver accepted PK changed".into());
    }
    if route.revoke(&owner, &attempt).is_ok() {
        return Err("accepted attempt was revoked".into());
    }
    channel.write_all(b"D").map_err(|e| e.to_string())?;
    if native {
        let sibling = oulipoly_state::mailbox::ContinuationAttempt {
            attempt_id: uuid::Uuid::new_v4().to_string(),
            session_id: Some("native-sibling-session".into()),
            claim_token: Some("native-sibling-claim".into()),
            result_path: gate_dir
                .join("native-sibling-work/result.json")
                .to_string_lossy()
                .into_owned(),
            ..attempt.clone()
        };
        private_native_request(gate_dir, &sibling, "native-sibling-work")?;
        route.reserve(&owner, &sibling)?;
        serde_json::to_writer(&mut *channel, &sibling).map_err(|error| error.to_string())?;
        channel
            .write_all(b"\n")
            .map_err(|error| error.to_string())?;
        channel
            .read_exact(&mut instruction)
            .map_err(|error| error.to_string())?;
        if instruction != [b'A'] {
            return Err("private sibling acceptance refused".into());
        }
    }
    channel
        .read_exact(&mut instruction)
        .map_err(|e| e.to_string())?;
    if instruction != [b'X'] {
        return Err("private driver final gate changed".into());
    }
    Ok(())
}

#[cfg(feature = "age319-private-broker-fixture")]
fn private_held_prepared_entry() -> Result<(), String> {
    use oulipoly_state::mailbox::PreparedBrokerOwner;
    let broker = broker_socket();
    let gate_dir = PathBuf::from(
        std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1")
            .ok_or("missing private gate directory")?,
    );
    let StateRoute::BrokerOwned {
        source_generation,
        domain_id,
    } = protocol::state_route_at(&broker).map_err(|e| e.to_string())?
    else {
        return Err("private prepared entry requires broker v30 I".into());
    };
    let reserved =
        protocol::request_at(&broker, Operation::ReserveV30Entry).map_err(|e| e.to_string())?;
    let root = reserved
        .strip_prefix("reserved ")
        .and_then(|s| s.strip_suffix('\n'))
        .ok_or("private v30 E refused")?
        .to_owned();
    let supervisor = uuid::Uuid::new_v4().to_string();
    let (mut parent, child) = UnixStream::pair().map_err(|e| e.to_string())?;
    let guardian_pid = unsafe { libc::fork() };
    if guardian_pid < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if guardian_pid == 0 {
        drop(parent);
        let result = private_guardian_prepared(
            child,
            &broker,
            &gate_dir,
            &root,
            &domain_id,
            &supervisor,
            &source_generation,
        );
        if let Err(error) = result {
            eprintln!("OULIPOLY_KERNEL_PRIVATE_GUARDIAN_GAP={error}");
            unsafe { libc::_exit(70) }
        }
        unsafe { libc::_exit(0) }
    }
    drop(child);
    let result = (|| -> Result<(), String> {
        let prepared = protocol::prepare_v30_guardian_at(&broker, &root, guardian_pid)
            .map_err(|e| e.to_string())?;
        if prepared != format!("prepared {root}\n") {
            return Err("private guardian P mismatch".into());
        }
        parent.write_all(b"P").map_err(|e| e.to_string())?;
        if private_line(&mut parent)? != format!("bound {root} {domain_id} {supervisor}") {
            return Err("private guardian G mismatch".into());
        }
        if protocol::read_v30_entry_at(&broker, &root).map_err(|e| e.to_string())?
            != format!("bound-entry {root} {domain_id} {supervisor} {guardian_pid}\n")
        {
            return Err("private entry A mismatch".into());
        }
        parent.write_all(b"X").map_err(|e| e.to_string())?;
        let proposal: serde_json::Value =
            serde_json::from_str(&private_line(&mut parent)?).map_err(|e| e.to_string())?;
        let owner_generation = proposal["owner_generation"]
            .as_str()
            .ok_or("missing owner generation")?;
        let driver_pid = proposal["driver_pid"]
            .as_i64()
            .ok_or("missing driver PID")?;
        let endpoint = proposal["endpoint"].as_str().ok_or("missing endpoint")?;
        let root_authority = serde_json::json!({
            "protocol": "root-authority-v1",
            "root_id": root,
            "domain_id": domain_id,
            "supervisor_authority_id": supervisor,
            "guardian_identity": {"pid": guardian_pid},
            "capability": format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple()),
        }).to_string();
        let cwd = File::open(".").map_err(|e| e.to_string())?;
        let (_receipt, completion) = UnixStream::pair().map_err(|e| e.to_string())?;
        let join = JoinSpec {
            root_id: root.clone(),
            domain_id: domain_id.clone(),
            supervisor_id: supervisor.clone(),
            guardian_pid,
            root_authority,
            args: vec!["--help".into()],
            environment: vec![],
        };
        let held = protocol::join_held_v30_at(
            &broker,
            &join,
            [0, 1, 2, cwd.as_raw_fd(), completion.as_raw_fd()],
        )
        .map_err(|e| e.to_string())?;
        if !held.starts_with(&format!("held-joined {root} ")) {
            return Err("private J did not return held child".into());
        }
        let replay = protocol::join_held_v30_at(
            &broker,
            &join,
            [0, 1, 2, cwd.as_raw_fd(), completion.as_raw_fd()],
        );
        if replay.is_ok_and(|reply| !reply.starts_with("error ")) {
            return Err("held J replay acquired a second child".into());
        }
        parent.write_all(b"J").map_err(|e| e.to_string())?;
        let guardian_row: PreparedBrokerOwner =
            serde_json::from_str(&private_line(&mut parent)?).map_err(|e| e.to_string())?;
        let read = protocol::StateReadSpec {
            protocol: "broker-prepared-read-v30".into(),
            source_generation: source_generation.clone(),
            root_id: root.clone(),
            owner_generation: owner_generation.into(),
            attempt_id: None,
        };
        let entry_row =
            protocol::read_prepared_owner_at(&broker, &read).map_err(|e| e.to_string())?;
        if entry_row != guardian_row
            || entry_row.entry.host_pid != unsafe { libc::getpid() }
            || entry_row.guardian.host_pid != guardian_pid
            || entry_row.driver.host_pid != i32::try_from(driver_pid).map_err(|e| e.to_string())?
            || entry_row.endpoint != endpoint
        {
            return Err("entry/guardian exact broker readback mismatch".into());
        }
        std::fs::write(
            gate_dir.join("prepared"),
            serde_json::to_vec(&entry_row).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        if std::env::var_os("AGE319_PRIVATE_RELEASE_V30").is_some() {
            let deadline = std::time::Instant::now() + PRIVATE_PREPARED_RELEASE_WAIT;
            while !gate_dir.join("release").exists() {
                if std::time::Instant::now() >= deadline {
                    return Err("private release wait expired".into());
                }
                std::thread::sleep(PRIVATE_PREPARED_GATE_POLL);
            }
            parent.write_all(b"L").map_err(|e| e.to_string())?;
            let committed: oulipoly_state::mailbox::BrokerReleaseEvidence =
                serde_json::from_str(&private_line(&mut parent)?).map_err(|e| e.to_string())?;
            if committed.prepared != entry_row {
                return Err("entry release identity mismatch".into());
            }
            std::fs::write(
                gate_dir.join("released"),
                serde_json::to_vec(&committed).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
        }
        let expect_death = std::env::var_os("AGE319_PRIVATE_EXPECT_GUARDIAN_DEATH_V1").is_some();
        let mut death_refused = false;
        let deadline = std::time::Instant::now() + PRIVATE_PREPARED_FINISH_WAIT;
        while !gate_dir.join("finish").exists() {
            if expect_death && !death_refused && gate_dir.join("guardian-dead").exists() {
                if protocol::read_prepared_owner_at(&broker, &read).is_ok() {
                    return Err("dead guardian retained prepared read authority".into());
                }
                std::fs::write(gate_dir.join("death-refused"), b"yes")
                    .map_err(|e| e.to_string())?;
                death_refused = true;
            }
            if std::time::Instant::now() >= deadline {
                return Err("private prepared fixture wait expired".into());
            }
            std::thread::sleep(PRIVATE_PREPARED_GATE_POLL);
        }
        if expect_death && !death_refused {
            return Err("guardian death was not read back".into());
        }
        Ok(())
    })();
    let _ = parent.write_all(b"X");
    drop(parent);
    unsafe { libc::waitpid(guardian_pid, std::ptr::null_mut(), 0) };
    result?;
    if std::env::var_os("AGE319_PRIVATE_RELEASE_V30").is_some() {
        Ok(())
    } else {
        Err("prepared-only child gate remains closed".into())
    }
}

fn v30_host_entry() -> Result<ExitCode, String> {
    let broker = broker_socket();
    let StateRoute::BrokerOwned {
        source_generation,
        domain_id,
    } = protocol::state_route_at(&broker).map_err(|e| e.to_string())?
    else {
        return Err("v30 entry has no broker-owned source".into());
    };
    let response =
        protocol::request_at(&broker, Operation::ReserveV30Entry).map_err(|e| e.to_string())?;
    let root = response
        .strip_prefix("reserved ")
        .and_then(|s| s.strip_suffix('\n'))
        .ok_or("v30 E did not reserve a root")?
        .to_owned();
    let supervisor = uuid::Uuid::new_v4().to_string();
    let (mut entry, mut guardian) = UnixStream::pair().map_err(|e| e.to_string())?;
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if pid == 0 {
        drop(entry);
        let result = (|| {
            #[cfg(feature = "age319-private-broker-fixture")]
            if std::env::var_os("AGE319_PRIVATE_CALLER_OUTPUT_V1").is_some() {
                use std::os::unix::fs::OpenOptionsExt as _;
                let gate = std::env::var("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1")
                    .map_err(|e| e.to_string())?;
                for (channel, name) in [
                    (libc::STDOUT_FILENO, "caller-control-stdout"),
                    (libc::STDERR_FILENO, "caller-control-stderr"),
                ] {
                    let diagnostic = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(std::path::Path::new(&gate).join(name))
                        .map_err(|e| e.to_string())?;
                    if unsafe { libc::dup2(diagnostic.as_raw_fd(), channel) } != channel {
                        return Err(std::io::Error::last_os_error().to_string());
                    }
                }
            }
            let mut gate = [0];
            guardian.read_exact(&mut gate).map_err(|e| e.to_string())?;
            if gate != [b'P'] {
                return Err("v30 guardian P gate refused".into());
            }
            let bound = protocol::bind_v30_guardian_at(&broker, &root, &domain_id, &supervisor)
                .map_err(|e| e.to_string())?;
            guardian
                .write_all(bound.as_bytes())
                .map_err(|e| e.to_string())?;
            guardian.read_exact(&mut gate).map_err(|e| e.to_string())?;
            if gate != [b'X'] {
                return Err("v30 guardian A gate refused".into());
            }
            unsafe { std::env::remove_var(REQUIRED_ENV) };
            crate::completion_owner::run_pinned_guardian_v30(
                &crate::completion_owner::PinnedGuardian {
                    root_id: root,
                    domain_id,
                    supervisor_authority_id: supervisor,
                },
                guardian,
            )
        })();
        if let Err(error) = &result {
            eprintln!("OULIPOLY_KERNEL_V30_GUARDIAN_GAP={error}");
        }
        unsafe { libc::_exit(if result.is_ok() { 0 } else { 70 }) }
    }
    drop(guardian);
    let result = (|| {
        let prepared =
            protocol::prepare_v30_guardian_at(&broker, &root, pid).map_err(|e| e.to_string())?;
        if prepared != format!("prepared {root}\n") {
            return Err("v30 guardian P readback changed".into());
        }
        entry.write_all(b"P").map_err(|e| e.to_string())?;
        if read_v30_frame(&mut entry)? != format!("bound {root} {domain_id} {supervisor}") {
            return Err("v30 guardian G changed".into());
        }
        if protocol::read_v30_entry_at(&broker, &root).map_err(|e| e.to_string())?
            != format!("bound-entry {root} {domain_id} {supervisor} {pid}\n")
        {
            return Err("v30 entry A changed".into());
        }
        entry.write_all(b"X").map_err(|e| e.to_string())?;
        let proposal: serde_json::Value =
            serde_json::from_str(&read_v30_frame(&mut entry)?).map_err(|e| e.to_string())?;
        let driver_pid = proposal["driver_pid"]
            .as_i64()
            .ok_or("v30 driver PID absent")?;
        let owner_generation = proposal["owner_generation"]
            .as_str()
            .ok_or("v30 owner generation absent")?;
        let endpoint = proposal["endpoint"].as_str().ok_or("v30 endpoint absent")?;
        let authority = proposal["root_authority"]
            .as_str()
            .ok_or("v30 root authority absent")?;
        // Broker J validates the capability, original entry socket and pinned
        // actors. The proposal is only data until its held child is returned.
        let args = std::env::args_os()
            .skip(1)
            .map(|arg| {
                arg.into_string()
                    .map_err(|_| "non-UTF8 CLI argument".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?;
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
            .filter(|(name, _)| !name.starts_with("OULIPOLY_KERNEL_"))
            .collect();
        let cwd = File::open(".").map_err(|e| e.to_string())?;
        let (mut receipt, completion) = UnixStream::pair().map_err(|e| e.to_string())?;
        let join = JoinSpec {
            root_id: root.clone(),
            domain_id: domain_id.clone(),
            supervisor_id: supervisor.clone(),
            guardian_pid: pid,
            root_authority: authority.into(),
            args,
            environment,
        };
        let held = protocol::join_held_v30_at(
            &broker,
            &join,
            [0, 1, 2, cwd.as_raw_fd(), completion.as_raw_fd()],
        )
        .map_err(|e| format!("v30 held J uncertain: {e}"))?;
        let child_pid: i32 = held
            .strip_prefix(&format!("held-joined {root} "))
            .and_then(|s| s.strip_suffix('\n'))
            .ok_or("v30 held J refused")?
            .parse()
            .map_err(|_| "v30 held child PID invalid")?;
        #[cfg(feature = "age319-private-broker-fixture")]
        if private_normal_mode() {
            let (_replay_receipt, replay_completion) =
                UnixStream::pair().map_err(|e| e.to_string())?;
            if protocol::join_held_v30_at(
                &broker,
                &join,
                [0, 1, 2, cwd.as_raw_fd(), replay_completion.as_raw_fd()],
            )
            .is_ok_and(|reply| reply.starts_with("held-joined "))
            {
                return Err("v30 held J replay created a second child".into());
            }
            private_v30_marker("held", &child_pid)?;
            private_v30_barrier("prepare")?;
        }
        drop(completion);
        entry.write_all(b"J").map_err(|e| e.to_string())?;
        let guardian_prepared: oulipoly_state::mailbox::PreparedBrokerOwner =
            serde_json::from_str(&read_v30_frame(&mut entry)?).map_err(|e| e.to_string())?;
        let spec = protocol::StateReadSpec {
            protocol: "broker-prepared-read-v30".into(),
            source_generation: source_generation.clone(),
            root_id: root.clone(),
            owner_generation: owner_generation.into(),
            attempt_id: None,
        };
        let exact = protocol::read_prepared_owner_at(&broker, &spec).map_err(|e| e.to_string())?;
        if exact != guardian_prepared
            || exact.entry.host_pid != unsafe { libc::getpid() }
            || exact.guardian.host_pid != pid
            || i64::from(exact.driver.host_pid) != driver_pid
            || exact.joined_child.host_pid != child_pid
            || exact.endpoint != endpoint
        {
            return Err("v30 entry and guardian prepared readback conflict".into());
        }
        #[cfg(feature = "age319-private-broker-fixture")]
        if private_normal_mode() {
            for (source, candidate_root, candidate_owner) in [
                (
                    uuid::Uuid::new_v4().to_string(),
                    root.clone(),
                    owner_generation.to_owned(),
                ),
                (
                    source_generation.clone(),
                    uuid::Uuid::new_v4().to_string(),
                    owner_generation.to_owned(),
                ),
                (
                    source_generation.clone(),
                    root.clone(),
                    uuid::Uuid::new_v4().to_string(),
                ),
            ] {
                if protocol::read_prepared_owner_at(
                    &broker,
                    &protocol::StateReadSpec {
                        protocol: "broker-prepared-read-v30".into(),
                        source_generation: source,
                        root_id: candidate_root,
                        owner_generation: candidate_owner,
                        attempt_id: None,
                    },
                )
                .is_ok()
                {
                    return Err("v30 stale, wrong-root or sibling prepared read accepted".into());
                }
            }
            private_v30_marker("prepared", &exact)?;
            private_v30_barrier("release")?;
        }
        entry.write_all(b"A").map_err(|e| e.to_string())?;
        let evidence: oulipoly_state::mailbox::BrokerReleaseEvidence =
            serde_json::from_str(&read_v30_frame(&mut entry)?).map_err(|e| e.to_string())?;
        let running = protocol::read_state_at(
            &broker,
            &protocol::StateReadSpec {
                protocol: "broker-entry-running-readback-v30".into(),
                ..spec
            },
        )
        .map_err(|e| e.to_string())?;
        if evidence.prepared != exact
            || running.owner != evidence.owner
            || running.root_id != root
            || running.source_generation != source_generation
        {
            return Err("v30 running readback changed".into());
        }
        #[cfg(feature = "age319-private-broker-fixture")]
        if private_normal_mode() {
            private_v30_marker("released", &evidence)?;
        }
        let status = read_v30_frame(&mut receipt)?;
        entry.write_all(b"D").map_err(|e| e.to_string())?;
        let code: u8 = status
            .strip_prefix("exit ")
            .ok_or("v30 child completion absent")?
            .parse()
            .map_err(|_| "v30 child completion invalid")?;
        Ok(ExitCode::from(code))
    })();
    if result.is_err() {
        let _ = entry.shutdown(std::net::Shutdown::Both);
    }
    result
}

fn read_v30_frame(socket: &mut UnixStream) -> Result<String, String> {
    let mut bytes = Vec::new();
    loop {
        if bytes.len() >= 16 * 1024 {
            return Err("v30 frame too large".into());
        }
        let mut byte = [0];
        socket.read_exact(&mut byte).map_err(|e| e.to_string())?;
        if byte == [b'\n'] {
            return String::from_utf8(bytes).map_err(|e| e.to_string());
        }
        bytes.push(byte[0]);
    }
}

#[cfg(feature = "age319-private-broker-fixture")]
fn private_v30_marker(name: &str, value: &impl serde::Serialize) -> Result<(), String> {
    let directory = PathBuf::from(
        std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1")
            .ok_or("private v30 gate directory absent")?,
    );
    std::fs::write(
        directory.join(name),
        serde_json::to_vec(value).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

#[cfg(feature = "age319-private-broker-fixture")]
fn private_v30_barrier(name: &str) -> Result<(), String> {
    let directory = PathBuf::from(
        std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1")
            .ok_or("private v30 gate directory absent")?,
    );
    let deadline = std::time::Instant::now() + PRIVATE_V30_BARRIER_WAIT;
    while !directory.join(name).exists() {
        if std::time::Instant::now() >= deadline {
            return Err(format!("private v30 {name} wait expired"));
        }
        std::thread::sleep(PRIVATE_V30_BARRIER_POLL);
    }
    Ok(())
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

    #[cfg(feature = "age319-private-broker-fixture")]
    #[test]
    fn private_caller_bytes_preserve_binary_channels_and_surface_partial_write() {
        struct Partial(std::vec::Vec<u8>);
        impl Write for Partial {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if self.0.is_empty() {
                    self.0.push(bytes[0]);
                    Ok(1)
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "caller gone",
                    ))
                }
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut out = Vec::new();
        let mut err = Vec::new();
        private_write_caller_bytes(&mut out, &mut err, b"\0\xffout", b"err\0\xfe").unwrap();
        assert_eq!(out, b"\0\xffout");
        assert_eq!(err, b"err\0\xfe");
        let mut partial = Partial(Vec::new());
        let mut out = Vec::new();
        let failure = private_write_caller_bytes(&mut out, &mut partial, b"out", b"err");
        assert_eq!(failure.unwrap_err().kind(), std::io::ErrorKind::BrokenPipe);
        assert_eq!(partial.0, b"e");
        assert!(out.is_empty());
    }

    #[cfg(feature = "age319-private-broker-fixture")]
    #[test]
    fn uncertain_provider_attempt_names_exact_readback_artifacts() {
        let error = private_provider_unknown(
            "d-key",
            "handoff-id",
            "session-id",
            Some("grant-id"),
            std::path::Path::new("/tmp/private/v30.sock"),
            "Q readback",
            "broker unavailable",
        );
        let value: serde_json::Value =
            serde_json::from_str(error.strip_prefix("fresh provider unknown: ").unwrap()).unwrap();
        assert_eq!(value["d_key"], "d-key");
        assert_eq!(value["handoff_id"], "handoff-id");
        assert_eq!(value["session_id"], "session-id");
        assert_eq!(value["grant_id"], "grant-id");
        assert_eq!(value["stage"], "Q readback");
        assert_eq!(value["broker_socket"], "/tmp/private/v30.sock");
        assert_eq!(
            value["grant_artifact"],
            "v30/fresh-provider/handoff-id.fresh-grant.json"
        );
        assert_eq!(
            value["effect_artifact_prefix"],
            "v30/fresh-provider/grant-id."
        );
        assert_eq!(value["automatic_replay"], false);
        assert_eq!(value["caller_retry_duplicate_effect_risk"], true);

        let lost_k = private_provider_unknown(
            "d-key",
            "handoff-id",
            "session-id",
            None,
            std::path::Path::new("/tmp/private/v30.sock"),
            "K readback",
            "broker unavailable",
        );
        let lost_k: serde_json::Value =
            serde_json::from_str(lost_k.strip_prefix("fresh provider unknown: ").unwrap()).unwrap();
        assert!(lost_k["grant_id"].is_null());
        assert!(lost_k["effect_artifact_prefix"].is_null());
        assert_eq!(lost_k["grant_artifact"], value["grant_artifact"]);
    }

    #[cfg(feature = "age319-private-broker-fixture")]
    #[test]
    fn uncertain_account_effect_retains_attempt_and_caller_risk() {
        let request = protocol::FreshAccountEffectRequest {
            d_key: "d-key".into(),
            model: "model".into(),
            config_sha256: "a".repeat(64),
            account: "account".into(),
            index: 2,
            kind: protocol::FreshAccountEffectKind::QuotaFirst,
            environment: Vec::new(),
        };
        let error = private_account_effect_unknown(
            "handoff-id",
            &request,
            None,
            "begin/readback",
            "broker unavailable",
        );
        let value: serde_json::Value = serde_json::from_str(
            error
                .strip_prefix("fresh account effect unknown: ")
                .unwrap(),
        )
        .unwrap();
        assert_eq!(value["d_key"], "d-key");
        assert_eq!(value["handoff_id"], "handoff-id");
        assert_eq!(value["account"], "account");
        assert_eq!(
            value["artifact"],
            "v30/fresh-provider/account-effects/handoff-id-2-quota-first"
        );
        assert_eq!(value["automatic_replay"], false);
        assert_eq!(value["caller_retry_duplicate_effect_risk"], true);
    }

    #[test]
    fn installed_image_refuses_draining_and_unrouteable_v30() {
        assert!(needs_installed_entry_gate(
            std::path::Path::new(INSTALLED_RUNNER),
            false
        ));
        assert!(needs_installed_entry_gate(
            std::path::Path::new(&format!("{INSTALLED_RUNNER} (deleted)")),
            false
        ));
        assert!(needs_installed_entry_gate(
            std::path::Path::new("/build/debug/oulipoly-agent-runner"),
            true
        ));
        assert!(!needs_installed_entry_gate(
            std::path::Path::new("/build/debug/oulipoly-agent-runner"),
            false
        ));
        assert!(require_legacy_entry_route(EntryRoute::LegacyOpen).is_ok());
        assert!(require_legacy_entry_route(EntryRoute::Draining).is_err());
        assert!(require_legacy_entry_route(EntryRoute::BrokerV30Closed).is_err());
    }

    #[test]
    fn two_paired_entries_require_same_broker_generation_before_dispatch() {
        let pair = InstalledPair {
            schema: 1,
            version: env!("CARGO_PKG_VERSION").into(),
            generation: uuid::Uuid::new_v4().to_string(),
            runner_sha256: "a".repeat(64),
            broker_sha256: "b".repeat(64),
            launcher_sha256: None,
            bash_sha256: None,
        };
        let observation = protocol::InstalledPairObservation {
            version: pair.version.clone(),
            generation: pair.generation.clone(),
            route: EntryRoute::LegacyOpen,
        };
        assert!(needs_installed_entry_gate(
            std::path::Path::new(INSTALLED_RUNNER),
            false
        ));
        assert!(require_pair_route(&pair, &observation).is_ok());
        // The staged CLI and GUI links run the same image. Both refuse direct
        // launch until the broker can give each an owned entry path.
        assert!(require_paired_launch_mode(false, false).is_err());
        assert!(require_paired_launch_mode(true, false).is_ok());
        assert!(require_paired_launch_mode(false, true).is_ok());
        assert!(require_paired_launch_mode(true, true).is_err());
        // An ordinary Tauri .deb GUI path remains outside opt-in pairing.
        assert!(!needs_installed_entry_gate(
            std::path::Path::new("/usr/bin/oulipoly-agent-runner"),
            false
        ));
        let mut old = observation;
        old.version = "0.0.0".into();
        assert!(require_pair_route(&pair, &old).is_err());
        old.version = pair.version.clone();
        old.generation = uuid::Uuid::new_v4().to_string();
        assert!(require_pair_route(&pair, &old).is_err());
        old.generation = pair.generation.clone();
        old.route = EntryRoute::Draining;
        assert!(require_pair_route(&pair, &old).is_err());
        old.route = EntryRoute::BrokerV30Closed;
        assert!(require_pair_route(&pair, &old).is_err());
    }

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
