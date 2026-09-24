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
        let pinned = private_pin_plan(&plan)?;
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
        while !std::path::Path::new(&gate).join("provider-cancel").exists() {
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
        protocol::private_fresh_provider_at(&socket, &self.authority.receipt.d_key, b'7', None)
            .map_err(|e| self.unknown(Some(&grant), "cancel reply", &e.to_string()))?;
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
        if output.grant_id != grant || !output.cancelled {
            return Err(self.unknown(Some(&grant), "output readback", "grant/Q mismatch"));
        }
        let stdout =
            private_verified_output(output.stdout, output.stdout_len, &output.stdout_sha256)
                .map_err(|e| self.unknown(Some(&grant), "stdout verification", &e))?;
        let stderr =
            private_verified_output(output.stderr, output.stderr_len, &output.stderr_sha256)
                .map_err(|e| self.unknown(Some(&grant), "stderr verification", &e))?;
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
        self, FreshAccountEffectKind, FreshAccountEffectRequest, FreshRouteRequest,
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
            ]);
            if std::env::var_os("AGE319_PRIVATE_BASH_SOURCE_SUCCESS_V1").is_some() {
                plan.plan.argv.push("no-cancel".into());
            } else if std::env::var_os("AGE319_PRIVATE_BASH_ORIGINAL_NOTIFY_V1").is_some() {
                plan.plan.argv.push("notify".into());
            }
        }
        prepared.push(plan);
    }
    for (index, candidate) in prepared.iter().enumerate() {
        let request = FreshRouteRequest {
            d_key: authority.receipt.d_key.clone(),
            model: pool.model.name.clone(),
            config_sha256: pool.config_sha256.clone(),
            account: Some(pool.model.providers[index].name.clone()),
            index: Some(index),
            total,
            pin: provider_pin.map(str::to_owned),
            quota_script: pool.account_effects[index].0.clone(),
            auth_refresh_command: pool.account_effects[index].1.clone(),
        };
        let pinned = private_pin_plan(&candidate.plan)?;
        let [image, cwd, input, recipe] = pinned.descriptors();
        protocol::private_fresh_route_at(
            &socket,
            &request,
            b'h',
            &[image, cwd, input, recipe, config_source.as_raw_fd()],
        )
        .map_err(|e| format!("fresh route candidate refused before K: {e}"))?;
    }
    let mut quota_receipts = Vec::new();
    let mut auth_receipts = Vec::new();
    for (index, (quota_script, auth_command)) in pool.account_effects.iter().enumerate() {
        if quota_script.is_none() {
            continue;
        }
        let mut environment = Vec::new();
        for (key, value) in std::env::vars_os() {
            let key = key
                .into_string()
                .map_err(|_| "fresh effect environment key is not UTF-8")?;
            if key.starts_with("LD_")
                || key.starts_with("DYLD_")
                || key.starts_with("OULIPOLY_KERNEL_")
                || matches!(key.as_str(), "GLIBC_TUNABLES" | "GCONV_PATH")
            {
                continue;
            }
            let value = value
                .into_string()
                .map_err(|_| "fresh effect environment value is not UTF-8")?;
            environment.push((key, value));
        }
        environment.sort();
        let mut effect = FreshAccountEffectRequest {
            d_key: authority.receipt.d_key.clone(),
            model: pool.model.name.clone(),
            config_sha256: pool.config_sha256.clone(),
            account: pool.model.providers[index].name.clone(),
            index,
            kind: FreshAccountEffectKind::QuotaFirst,
            environment,
        };
        let first = private_run_account_effect(&socket, &authority.receipt.handoff_id, &effect)?;
        quota_receipts.push((
            effect.clone(),
            first.effect_id.clone(),
            first.outcome.clone(),
        ));
        if first.outcome.as_deref() != Some("valid_windows") && auth_command.is_some() {
            effect.kind = FreshAccountEffectKind::AuthRefresh;
            let auth = private_run_account_effect(&socket, &authority.receipt.handoff_id, &effect)?;
            auth_receipts.push((effect.clone(), auth.effect_id.clone(), auth.outcome.clone()));
            if auth.outcome.as_deref() == Some("refreshed") {
                effect.kind = FreshAccountEffectKind::QuotaRetry;
                let retry =
                    private_run_account_effect(&socket, &authority.receipt.handoff_id, &effect)?;
                auth_receipts.push((effect.clone(), retry.effect_id, retry.outcome));
            }
        }
    }
    let request = FreshRouteRequest {
        d_key: authority.receipt.d_key.clone(),
        model: pool.model.name.clone(),
        config_sha256: pool.config_sha256.clone(),
        account: None,
        index: None,
        total,
        pin: provider_pin.map(str::to_owned),
        quota_script: None,
        auth_refresh_command: None,
    };
    let selected =
        protocol::private_fresh_route_at(&socket, &request, b'f', &[config_source.as_raw_fd()])
            .map_err(|e| format!("fresh route selection refused before K: {e}"))?
            .ok_or("fresh route selection absent before K")?;
    if selected.model != pool.model.name
        || selected.config_sha256 != pool.config_sha256
        || selected.policy_version != "fresh-account-effects-v2"
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
    let selected_plan = prepared.swap_remove(selected.index);
    let mut backend = PrivateFreshBroker {
        authority,
        grant_id: None,
    };
    let result = run_prepared_fresh_headless(selected_plan, &mut backend)?;
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
        "auth_after_provider_q": auth_after_provider_q,
        "terminal_reason": result.terminal_reason,
    });
    std::fs::write(
        std::path::Path::new(&gate).join("provider-runtime-result"),
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
    }
    // The private mapped result and terminal readback do not activate the
    // ordinary caller publication path.
    Err("private provider runtime result mapped after Q; root terminal publication closed".into())
}

#[cfg(feature = "age319-private-broker-fixture")]
fn private_run_account_effect(
    socket: &std::path::Path,
    handoff_id: &str,
    request: &oulipoly_kernel_broker::protocol::FreshAccountEffectRequest,
) -> Result<oulipoly_kernel_broker::protocol::FreshAccountEffectReadback, String> {
    use oulipoly_kernel_broker::protocol;
    let started = protocol::private_fresh_account_effect_at(socket, request, true)
        .or_else(|_| protocol::private_fresh_account_effect_at(socket, request, false))
        .map_err(|e| {
            private_account_effect_unknown(
                handoff_id,
                request,
                None,
                "begin/readback",
                &e.to_string(),
            )
        })?;
    let mut effect = started;
    while effect.state == "pending" {
        std::thread::sleep(PRIVATE_ACCOUNT_EFFECT_POLL);
        effect =
            protocol::private_fresh_account_effect_at(socket, request, false).map_err(|e| {
                private_account_effect_unknown(
                    handoff_id,
                    request,
                    Some(&effect),
                    "Q readback",
                    &e.to_string(),
                )
            })?;
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
    if !matches!(mode.as_str(), "ack" | "lost_pending") {
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
    let _pending_endpoint =
        std::os::unix::net::UnixListener::bind(&endpoint).map_err(|e| e.to_string())?;
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
    let _ = channel.read_exact(&mut byte);
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
