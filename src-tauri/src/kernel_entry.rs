//! Host-side pinned completion authority and broker-attested root child join.
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
fn private_prepared_mode() -> bool {
    std::env::args().nth(1).as_deref() == Some(PRIVATE_PREPARED_ENTRY)
        && unsafe { libc::geteuid() } == 0
        && std::fs::read_to_string("/proc/self/uid_map")
            .ok()
            .is_some_and(|map| map.split_ascii_whitespace().nth(2) == Some("1"))
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
    if route == EntryRoute::BrokerV30Closed && private_prepared_mode() {
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
    let result = stage_host_entry(
        || {
            // A selected v30 route must never read the retired user-owned
            // sidecar, even if an intact v29 copy is still present there.
            // Owner publication currently precedes J, while Y/W require its
            // consumed child. Keep admission closed until that ordering and
            // the remaining writer/read topology are routed together.
            match protocol::state_route_at(&broker_socket())
                .map_err(|e| format!("broker State route unavailable: {e}"))?
            {
                StateRoute::Legacy => {}
                StateRoute::BrokerOwned {
                    source_generation, ..
                } => {
                    return Err(format!(
                        "broker-owned State generation {source_generation} requires production client routing"
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
        let mut release = [0u8];
        let _ = driver_child.read_exact(&mut release);
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
    let published = protocol::prepare_owner_at(broker, &write).map_err(|e| e.to_string())?;
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
    let _ = channel.read_exact(&mut byte);
    let _ = driver_parent.write_all(b"X");
    unsafe { libc::waitpid(driver_pid, std::ptr::null_mut(), 0) };
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
    let result = (|| {
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
        let expect_death = std::env::var_os("AGE319_PRIVATE_EXPECT_GUARDIAN_DEATH_V1").is_some();
        let mut death_refused = false;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
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
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if expect_death && !death_refused {
            return Err("guardian death was not read back".into());
        }
        Ok(())
    })();
    let _ = parent.write_all(b"X");
    drop(parent);
    unsafe { libc::waitpid(guardian_pid, std::ptr::null_mut(), 0) };
    result.and(Err("prepared-only child gate remains closed".into()))
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
