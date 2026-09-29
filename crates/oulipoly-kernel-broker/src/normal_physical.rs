//! Featureless, one-use physical provider execution for a held normal plan.
//! State K precedes the detached supervisor. Only that supervisor can produce
//! a Q; missing or damaged evidence is unknown and never permits a retry.

use crate::identity::{
    PinnedProcess, host_proc_file, install_detached_host_proc, observed_incarnation_gone,
};
use crate::normal_model_selection;
use crate::normal_plan_custody;
use oulipoly_runtime::executor::cli::fresh_remote::FreshProviderPlan;
use oulipoly_state::mailbox::{
    FreshNormalExecutablePlan, FreshNormalProviderAdmission, FreshNormalProviderK,
    FreshRecipientIdentity, FreshReleasedHandoff, FreshV30Lane, FreshV30Session,
};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const MAX_RECIPE: u64 = 64 * 1024 * 1024;
const HASH_BUFFER: usize = 64 * 1024;
const CHILD_DRAIN_POLL: std::time::Duration = std::time::Duration::from_millis(20);

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SupervisorRecipe {
    k: FreshNormalProviderK,
    root_id: String,
    plan: FreshNormalExecutablePlan,
    root: FreshRecipientIdentity,
    actor: FreshRecipientIdentity,
    uid: u32,
    gid: u32,
    groups: Vec<u32>,
    private_fixture: bool,
    config_directory: PathBuf,
    executable: PathBuf,
    cwd: PathBuf,
    argv: Vec<String>,
    environment: Vec<(String, String)>,
    stdin: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OutputEvidence {
    pub device: u64,
    pub inode: u64,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PhysicalQ {
    pub admission_id: String,
    pub plan_sha256: String,
    pub provider_wait_status: i32,
    pub tree_drained: bool,
    pub stdout: OutputEvidence,
    pub stderr: OutputEvidence,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParentWait {
    admission_id: String,
    pid1_parent_namespace_pid: i32,
    pid1_wait_status: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BashParent {
    handoff_id: String,
    admission_id: String,
    plan_sha256: String,
    root_id: String,
    pid1_host_pid: i32,
    pid1_starttime_ticks: u64,
    pidns_dev: u64,
    pidns_ino: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BashChildReservation {
    admission_id: String,
    grant_id: String,
}

#[derive(Deserialize)]
struct ChildAttach {
    grant_id: String,
    work_id: String,
    pid1: i32,
    pid1_starttime: u64,
    pidns_dev: u64,
    pidns_ino: u64,
}

#[derive(Deserialize)]
struct ChildDrain {
    grant_id: String,
    work_id: String,
    zero_remaining: bool,
}

#[derive(Deserialize)]
struct ChildWait {
    grant_id: String,
    work_id: String,
    reaped: bool,
    wait_status: i32,
}

fn lock_directory(directory: &File) -> io::Result<()> {
    if unsafe { libc::flock(directory.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Serialize a child's durable pre-K reservation with the normal PID1's
/// final drain decision. The returned directory lock is held through K.
pub fn reserve_bash_child(
    state_root: &Path,
    admission_id: &str,
    init: &PinnedProcess,
    grant_id: &str,
) -> io::Result<File> {
    let directory = id_path(&store_root(state_root)?, admission_id)?;
    let lock = File::open(&directory)?;
    lock_directory(&lock)?;
    let parent: BashParent = read_optional(&directory, "bash-parent.json")?
        .ok_or_else(|| io::Error::other("normal Bash parent attach absent"))?;
    if parent.admission_id != admission_id
        || parent.pid1_host_pid != init.host_pid
        || parent.pid1_starttime_ticks != init.starttime_ticks
        || (parent.pidns_dev, parent.pidns_ino) != (init.pidns_dev, init.pidns_ino)
        || init.exited()?
        || directory.join("q.json").exists()
    {
        return Err(io::Error::other(
            "normal Bash parent already drained or changed",
        ));
    }
    init.verify()?;
    let grant_id = uuid::Uuid::parse_str(grant_id).map_err(io::Error::other)?;
    if grant_id.is_nil() {
        return Err(io::Error::other("normal Bash child grant ID invalid"));
    }
    let grant_id = grant_id.to_string();
    write_new(
        &directory,
        &format!("bash-child-{grant_id}.json"),
        &BashChildReservation {
            admission_id: admission_id.to_owned(),
            grant_id,
        },
    )?;
    Ok(lock)
}

fn bash_children_drained(directory: &Path, admission_id: &str, boot_id: &str) -> io::Result<bool> {
    let physical = directory
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| io::Error::other("normal child physical store absent"))?
        .join("fresh-provider");
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(grant_id) = name
            .to_str()
            .and_then(|name| name.strip_prefix("bash-child-"))
            .and_then(|name| name.strip_suffix(".json"))
        else {
            continue;
        };
        id_path(&physical, grant_id)?;
        let reservation: BashChildReservation = read_optional(directory, &name.to_string_lossy())?
            .ok_or_else(|| io::Error::other("normal child reservation disappeared"))?;
        if reservation.admission_id != admission_id || reservation.grant_id != grant_id {
            return Err(io::Error::other("normal child reservation changed"));
        }
        // The Broker holds the same lock from reservation through durable K.
        // A crash before K leaves an inert reservation, never a launched child.
        if !physical.join(format!("{grant_id}.consumed.json")).exists() {
            continue;
        }
        let attach: Option<ChildAttach> =
            read_optional(&physical, &format!("{grant_id}.attach.json"))?;
        let drain: Option<ChildDrain> =
            read_optional(&physical, &format!("{grant_id}.drain.json"))?;
        let wait: Option<ChildWait> =
            read_optional(&physical, &format!("{grant_id}.pid1-wait.json"))?;
        let (Some(attach), Some(drain), Some(wait)) = (attach, drain, wait) else {
            return Ok(false);
        };
        if attach.grant_id != grant_id
            || drain.grant_id != grant_id
            || wait.grant_id != grant_id
            || drain.work_id != attach.work_id
            || wait.work_id != attach.work_id
            || !drain.zero_remaining
            || !wait.reaped
            || !libc::WIFEXITED(wait.wait_status)
            || libc::WEXITSTATUS(wait.wait_status) != 0
            || !observed_incarnation_gone(
                attach.pid1,
                boot_id,
                attach.pid1_starttime,
                (attach.pidns_dev, attach.pidns_ino),
            )?
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn own_host_pid_and_starttime() -> io::Result<(i32, u64)> {
    let mut stat = String::new();
    host_proc_file("self/stat")?.read_to_string(&mut stat)?;
    let (pid, remaining) = stat
        .split_once(" (")
        .ok_or_else(|| io::Error::other("normal provider PID1 host stat invalid"))?;
    let fields = remaining
        .rsplit_once(") ")
        .ok_or_else(|| io::Error::other("normal provider PID1 host stat invalid"))?
        .1;
    let host_pid = pid.parse().map_err(io::Error::other)?;
    let starttime = fields
        .split_ascii_whitespace()
        .nth(19)
        .ok_or_else(|| io::Error::other("normal provider PID1 host starttime absent"))?
        .parse()
        .map_err(io::Error::other)?;
    Ok((host_pid, starttime))
}

pub fn parent_for_bash(
    lane: &FreshV30Lane,
    release: &FreshReleasedHandoff,
    actor: &FreshRecipientIdentity,
    session: &FreshV30Session,
    state_root: &Path,
) -> io::Result<(String, PinnedProcess)> {
    let k = lane
        .read_normal_provider_k(release, actor, session)
        .map_err(io::Error::other)?
        .ok_or_else(|| io::Error::other("normal Bash parent K absent"))?;
    let directory = id_path(&store_root(state_root)?, &k.admission_id)?;
    let parent: BashParent = read_optional(&directory, "bash-parent.json")?
        .ok_or_else(|| io::Error::other("normal Bash parent attach absent"))?;
    if parent.handoff_id != release.handoff_id
        || parent.admission_id != k.admission_id
        || parent.plan_sha256 != k.plan_sha256
        || parent.root_id != release.old_release.prepared.root_id
    {
        return Err(io::Error::other("normal Bash parent K binding changed"));
    }
    let init = PinnedProcess::open(parent.pid1_host_pid)?;
    if init.starttime_ticks != parent.pid1_starttime_ticks
        || (init.pidns_dev, init.pidns_ino) != (parent.pidns_dev, parent.pidns_ino)
    {
        return Err(io::Error::other("normal Bash parent PID1 changed"));
    }
    Ok((k.admission_id, init))
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PhysicalReadback {
    pub k: FreshNormalProviderK,
    pub state: String,
    pub q: Option<PhysicalQ>,
    pub unknown_reason: Option<String>,
}

/// An immutable caller write reservation. The Q and root identity are copied
/// into this root-only receipt before either output descriptor is touched.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct PublicationIntent {
    root_id: String,
    owner_generation: String,
    handoff_id: String,
    invocation_uuid: String,
    session_id: String,
    actor: FreshRecipientIdentity,
    k: FreshNormalProviderK,
    q: PhysicalQ,
    exit_code: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PublicationReadback {
    pub state: String,
    pub admission_id: String,
    pub plan_sha256: String,
    pub exit_code: Option<u8>,
    pub publication_sha256: Option<String>,
}

fn stamp(process: &PinnedProcess) -> FreshRecipientIdentity {
    FreshRecipientIdentity {
        host_pid: process.host_pid,
        boot_id: process.boot_id.clone(),
        starttime_ticks: process.starttime_ticks,
        pidns_dev: process.pidns_dev,
        pidns_ino: process.pidns_ino,
    }
}

fn exact_process(expected: &FreshRecipientIdentity) -> io::Result<PinnedProcess> {
    let live = PinnedProcess::open(expected.host_pid)?;
    if stamp(&live) != *expected {
        return Err(io::Error::other(
            "normal physical process incarnation changed",
        ));
    }
    live.verify()?;
    Ok(live)
}

fn store_root(state_root: &Path) -> io::Result<PathBuf> {
    let lane = state_root.join("v30");
    let meta = fs::symlink_metadata(&lane)?;
    if !meta.is_dir() || meta.uid() != 0 || meta.mode() & 0o077 != 0 {
        return Err(io::Error::other("normal physical lane is not root-only"));
    }
    let path = lane.join("normal-provider");
    if !path.exists() {
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        File::open(&lane)?.sync_all()?;
    }
    let meta = fs::symlink_metadata(&path)?;
    if !meta.is_dir() || meta.uid() != 0 || meta.mode() & 0o077 != 0 {
        return Err(io::Error::other("normal physical store is not root-only"));
    }
    Ok(path)
}

fn id_path(store: &Path, id: &str) -> io::Result<PathBuf> {
    let uuid = uuid::Uuid::parse_str(id).map_err(io::Error::other)?;
    if uuid.is_nil() || uuid.to_string() != id {
        return Err(io::Error::other("normal physical admission ID invalid"));
    }
    Ok(store.join(id))
}

fn write_new<T: Serialize>(directory: &Path, name: &str, value: &T) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(directory.join(name))?;
    serde_json::to_writer(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    File::open(directory)?.sync_all()
}

fn read_optional<T: for<'de> Deserialize<'de>>(
    directory: &Path,
    name: &str,
) -> io::Result<Option<T>> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(directory.join(name))
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !file.metadata()?.is_file() || file.metadata()?.len() > 16 * 1024 {
        return Err(io::Error::other("normal physical receipt invalid"));
    }
    serde_json::from_reader(file)
        .map(Some)
        .map_err(io::Error::other)
}

fn output_evidence(file: &mut File) -> io::Result<OutputEvidence> {
    file.sync_all()?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.uid() != 0 || meta.mode() & 0o077 != 0 {
        return Err(io::Error::other("normal physical output ownership changed"));
    }
    let mut hash = Sha256::new();
    let mut bytes = 0u64;
    let mut buffer = [0u8; HASH_BUFFER];
    let mut input = file.try_clone()?;
    use std::io::Seek;
    input.rewind()?;
    loop {
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        bytes = bytes
            .checked_add(n as u64)
            .ok_or_else(|| io::Error::other("normal output length overflow"))?;
        hash.update(&buffer[..n]);
    }
    if bytes != meta.len() || file.metadata()?.len() != bytes {
        return Err(io::Error::other("normal physical output changed"));
    }
    Ok(OutputEvidence {
        device: meta.dev(),
        inode: meta.ino(),
        bytes,
        sha256: format!("{:x}", hash.finalize()),
    })
}

fn verify_output(directory: &Path, name: &str, expected: &OutputEvidence) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(directory.join(name))?;
    if &output_evidence(&mut file)? != expected {
        return Err(io::Error::other("normal physical output changed after Q"));
    }
    Ok(())
}

fn sealed_recipe(value: &SupervisorRecipe) -> io::Result<File> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() as u64 > MAX_RECIPE {
        return Err(io::Error::other("normal physical recipe oversized"));
    }
    let fd = unsafe {
        libc::memfd_create(
            c"normal-provider-recipe".as_ptr(),
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut file = unsafe { File::from_raw_fd(fd) };
    file.write_all(&bytes)?;
    let seals = libc::F_SEAL_SEAL | libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_WRITE;
    if unsafe { libc::fcntl(fd, libc::F_ADD_SEALS, seals) } != 0 {
        return Err(io::Error::last_os_error());
    }
    use std::io::Seek;
    file.rewind()?;
    Ok(file)
}

pub fn launch(
    lane: &FreshV30Lane,
    receipt: &FreshReleasedHandoff,
    actor_identity: &FreshRecipientIdentity,
    session: &FreshV30Session,
    admission: &FreshNormalProviderAdmission,
    plan: &FreshNormalExecutablePlan,
    materialized: FreshProviderPlan,
    config_dir: &File,
    state_root: &Path,
    uid: u32,
    gid: u32,
    private_fixture: bool,
) -> io::Result<PhysicalReadback> {
    let root = PinnedProcess::open(receipt.old_release.prepared.root_init.host_pid)?;
    let actor = exact_process(actor_identity)?;
    if stamp(&root).host_pid != receipt.old_release.prepared.root_init.host_pid
        || stamp(&root).boot_id != receipt.old_release.prepared.root_init.boot_id
        || stamp(&root).starttime_ticks != receipt.old_release.prepared.root_init.starttime_ticks
        || stamp(&root).pidns_dev != receipt.old_release.prepared.root_init.pidns_dev
        || stamp(&root).pidns_ino != receipt.old_release.prepared.root_init.pidns_ino
        || !root.is_namespace_init()?
        || !actor.direct_child_of(&root)?
        || !actor.in_namespace(root.namespace())?
    {
        return Err(io::Error::other("normal physical root or actor changed"));
    }
    if unsafe { libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) } != 0
        || unsafe { libc::prctl(libc::PR_GET_SECCOMP, 0, 0, 0, 0) } != 0
    {
        return Err(io::Error::other("normal physical inherited NNP/seccomp"));
    }
    if lane
        .read_normal_provider_admission(receipt, actor_identity, session)
        .map_err(io::Error::other)?
        .as_ref()
        != Some(admission)
        || admission.plan_sha256 != plan.plan_sha256
        || materialized.executable.to_str() != Some(plan.executable.as_str())
        || materialized.argv != plan.argv
        || format!("{:x}", Sha256::digest(&materialized.stdin)) != plan.stdin_sha256
        || format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&materialized.environment)?)
        ) != plan.environment_sha256
    {
        return Err(io::Error::other(
            "normal physical recipe differs from admission",
        ));
    }
    store_root(state_root)?;
    let proposed_k = FreshNormalProviderK {
        handoff_id: receipt.handoff_id.clone(),
        admission_id: admission.admission_id.clone(),
        plan_sha256: plan.plan_sha256.clone(),
        state: "consumed".into(),
    };
    let recipe = SupervisorRecipe {
        k: proposed_k,
        root_id: receipt.old_release.prepared.root_id.clone(),
        plan: plan.clone(),
        root: stamp(&root),
        actor: actor_identity.clone(),
        uid,
        gid,
        groups: actor.supplementary_groups()?,
        private_fixture,
        config_directory: fs::read_link(format!("/proc/self/fd/{}", config_dir.as_raw_fd()))?,
        executable: materialized.executable,
        cwd: materialized.cwd,
        argv: materialized.argv,
        environment: materialized.environment,
        stdin: materialized.stdin,
    };
    let recipe_fd = sealed_recipe(&recipe)?;
    root.verify()?;
    actor.verify()?;
    let (k, inserted) = lane
        .consume_normal_provider_k(receipt, actor_identity, session, admission)
        .map_err(io::Error::other)?;
    if !inserted {
        return observe(lane, receipt, actor_identity, session, state_root)?
            .ok_or_else(|| io::Error::other("normal physical K disappeared"));
    }
    // K is now durable. Every following error is spent unknown debt.
    // /proc/self/exe names the serving broker's already opened inode even if
    // its installation pathname changes between K and this handoff.
    let mut child = Command::new("/proc/self/exe");
    child
        .arg("--normal-provider-supervisor")
        .arg(state_root)
        .stdin(Stdio::from(recipe_fd))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env_clear();
    match child.spawn() {
        Ok(mut supervisor) => {
            let id = k.admission_id.clone();
            if let Err(error) = std::thread::Builder::new()
                .name("normal-provider-reaper".into())
                .spawn(move || {
                    if let Err(error) = supervisor.wait() {
                        eprintln!("normal physical supervisor wait failed {id}: {error}");
                    }
                })
            {
                eprintln!(
                    "normal physical reaper unavailable after K {}: {error}",
                    k.admission_id
                );
            }
        }
        Err(error) => eprintln!(
            "normal physical supervisor failed after K {}: {error}",
            k.admission_id
        ),
    }
    observe(lane, receipt, actor_identity, session, state_root)?
        .ok_or_else(|| io::Error::other("normal physical K disappeared"))
}

pub fn observe(
    lane: &FreshV30Lane,
    receipt: &FreshReleasedHandoff,
    actor: &FreshRecipientIdentity,
    session: &FreshV30Session,
    state_root: &Path,
) -> io::Result<Option<PhysicalReadback>> {
    let Some(k) = lane
        .read_normal_provider_k(receipt, actor, session)
        .map_err(io::Error::other)?
    else {
        return Ok(None);
    };
    let physical = (|| {
        let directory = id_path(&store_root(state_root)?, &k.admission_id)?;
        let q: Option<PhysicalQ> = read_optional(&directory, "q.json")?;
        let wait: Option<ParentWait> = read_optional(&directory, "parent-wait.json")?;
        let (Some(q), Some(wait)) = (q, wait) else {
            return Err(io::Error::other("physical Q or PID1 wait absent"));
        };
        if q.admission_id != k.admission_id
            || q.plan_sha256 != k.plan_sha256
            || !q.tree_drained
            || wait.admission_id != k.admission_id
            || wait.pid1_parent_namespace_pid <= 0
            || !libc::WIFEXITED(wait.pid1_wait_status)
            || libc::WEXITSTATUS(wait.pid1_wait_status) != 0
        {
            return Err(io::Error::other("physical Q or PID1 wait changed"));
        }
        verify_output(&directory, "stdout", &q.stdout)?;
        verify_output(&directory, "stderr", &q.stderr)?;
        Ok(q)
    })();
    let (q, unknown_reason) = match physical {
        Ok(q) => (Some(q), None),
        Err(error) => (None, Some(error.to_string())),
    };
    Ok(Some(PhysicalReadback {
        k,
        state: if q.is_some() { "drained" } else { "unknown" }.into(),
        q,
        unknown_reason,
    }))
}

fn publication_intent(
    receipt: &FreshReleasedHandoff,
    actor: &FreshRecipientIdentity,
    session: &FreshV30Session,
    read: &PhysicalReadback,
) -> io::Result<PublicationIntent> {
    if read.state != "drained" {
        return Err(io::Error::other("normal caller physical Q unknown"));
    }
    let q = read
        .q
        .clone()
        .ok_or_else(|| io::Error::other("normal caller Q absent"))?;
    if !libc::WIFEXITED(q.provider_wait_status) {
        return Err(io::Error::other(
            "normal caller non-exit wait status cannot be represented",
        ));
    }
    let exit_code =
        u8::try_from(libc::WEXITSTATUS(q.provider_wait_status)).map_err(io::Error::other)?;
    Ok(PublicationIntent {
        root_id: receipt.old_release.prepared.root_id.clone(),
        owner_generation: receipt.old_release.prepared.owner_generation.clone(),
        handoff_id: receipt.handoff_id.clone(),
        invocation_uuid: receipt.invocation_uuid.clone(),
        session_id: session.session_id.clone(),
        actor: actor.clone(),
        k: read.k.clone(),
        q,
        exit_code,
    })
}

fn publication_readback(
    directory: &Path,
    expected: &PublicationIntent,
) -> io::Result<PublicationReadback> {
    let intent: Option<PublicationIntent> = read_optional(directory, "caller-intent.json")?;
    let settled: Option<PublicationIntent> = read_optional(directory, "caller-settled.json")?;
    if intent.as_ref().is_some_and(|value| value != expected)
        || settled.as_ref().is_some_and(|value| value != expected)
        || (settled.is_some() && intent.is_none())
    {
        return Err(io::Error::other(
            "normal caller publication binding changed",
        ));
    }
    Ok(PublicationReadback {
        state: if settled.is_some() {
            "settled"
        } else if intent.is_some() {
            "unknown"
        } else {
            "not_started"
        }
        .into(),
        admission_id: expected.k.admission_id.clone(),
        plan_sha256: expected.k.plan_sha256.clone(),
        exit_code: settled.as_ref().map(|value| value.exit_code),
        publication_sha256: settled
            .as_ref()
            .map(|value| serde_json::to_vec(value))
            .transpose()?
            .map(|bytes| format!("{:x}", Sha256::digest(bytes))),
    })
}

pub fn observe_publication(
    lane: &FreshV30Lane,
    receipt: &FreshReleasedHandoff,
    actor: &FreshRecipientIdentity,
    session: &FreshV30Session,
    state_root: &Path,
) -> io::Result<PublicationReadback> {
    let read = observe(lane, receipt, actor, session, state_root)?
        .ok_or_else(|| io::Error::other("normal caller K absent"))?;
    let expected = publication_intent(receipt, actor, session, &read)?;
    let directory = id_path(&store_root(state_root)?, &read.k.admission_id)?;
    publication_readback(&directory, &expected)
}

pub fn publish(
    lane: &FreshV30Lane,
    receipt: &FreshReleasedHandoff,
    actor: &FreshRecipientIdentity,
    session: &FreshV30Session,
    state_root: &Path,
    stdout: &mut File,
    stderr: &mut File,
    private_fixture: bool,
) -> io::Result<PublicationReadback> {
    #[cfg(not(feature = "age319-private-broker-fixture"))]
    let _ = private_fixture;
    let read = observe(lane, receipt, actor, session, state_root)?
        .ok_or_else(|| io::Error::other("normal caller K absent"))?;
    let expected = publication_intent(receipt, actor, session, &read)?;
    let directory = id_path(&store_root(state_root)?, &read.k.admission_id)?;
    let before = publication_readback(&directory, &expected)?;
    if before.state != "not_started" {
        return Ok(before);
    }
    // O_EXCL and the directory fsync make an interrupted or concurrent write
    // permanently unknown. No request can write the same Q a second time.
    if let Err(error) = write_new(&directory, "caller-intent.json", &expected) {
        if error.kind() == io::ErrorKind::AlreadyExists {
            return publication_readback(&directory, &expected);
        }
        return Err(error);
    }
    #[cfg(feature = "age319-private-broker-fixture")]
    if private_fixture && std::env::var_os("AGE319_TEST_NORMAL_CALLER_LOST_V1").is_some() {
        return Err(io::Error::other(
            "private caller write lost after reservation",
        ));
    }
    #[cfg(feature = "age319-private-broker-fixture")]
    if private_fixture && std::env::var_os("AGE319_TEST_NORMAL_CALLER_PARTIAL_V1").is_some() {
        stdout.write_all(b"n")?;
        return Err(io::Error::other(
            "private caller write partial after reservation",
        ));
    }
    let copy = |name: &str, evidence: &OutputEvidence, destination: &mut File| -> io::Result<()> {
        let mut source = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(directory.join(name))?;
        if output_evidence(&mut source)? != *evidence {
            return Err(io::Error::other("normal caller output changed after Q"));
        }
        use std::io::Seek;
        source.rewind()?;
        if io::copy(&mut source, destination)? != evidence.bytes {
            return Err(io::Error::other("normal caller copied byte count changed"));
        }
        destination.flush()?;
        verify_output(&directory, name, evidence)
    };
    copy("stdout", &expected.q.stdout, stdout)?;
    copy("stderr", &expected.q.stderr, stderr)?;
    write_new(&directory, "caller-settled.json", &expected)?;
    publication_readback(&directory, &expected)
}

fn retained_k_matches(state_root: &Path, recipe: &SupervisorRecipe) -> io::Result<()> {
    let state = state_root.join("v30/state.db");
    let db = Connection::open_with_flags(state, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(io::Error::other)?;
    let (k, admission, plan): (String, String, String) = db
        .query_row(
            "SELECT k.k_json,a.admission_json,p.plan_json FROM fresh_normal_provider_k k
         JOIN fresh_normal_provider_admission a USING(handoff_id)
         JOIN fresh_normal_executable_plan p USING(handoff_id) WHERE k.handoff_id=?1",
            [&recipe.k.handoff_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(io::Error::other)?;
    let k: FreshNormalProviderK = serde_json::from_str(&k)?;
    let admission: FreshNormalProviderAdmission = serde_json::from_str(&admission)?;
    let plan: FreshNormalExecutablePlan = serde_json::from_str(&plan)?;
    if k != recipe.k
        || plan != recipe.plan
        || admission.admission_id != k.admission_id
        || admission.plan_sha256 != k.plan_sha256
        || plan.plan_sha256 != k.plan_sha256
        || format!("{:x}", Sha256::digest(&recipe.stdin)) != plan.stdin_sha256
        || format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&recipe.environment)?)
        ) != plan.environment_sha256
        || recipe.argv != plan.argv
        || recipe.executable.to_str() != Some(plan.executable.as_str())
        || recipe.cwd.to_str() != Some(plan.cwd.as_str())
    {
        return Err(io::Error::other(
            "normal physical supervisor recipe changed",
        ));
    }
    Ok(())
}

struct Pid1Context {
    recipe: SupervisorRecipe,
    directory: PathBuf,
    root: PinnedProcess,
    actor: PinnedProcess,
}

extern "C" fn pid1_entry(pointer: *mut libc::c_void) -> libc::c_int {
    let context = unsafe { Box::from_raw(pointer.cast::<Pid1Context>()) };
    match pid1_run(
        &context.recipe,
        &context.directory,
        &context.root,
        &context.actor,
    ) {
        Ok(()) => 0,
        Err(error) => {
            let _ = write_new(
                &context.directory,
                "incomplete.json",
                &serde_json::json!({ "admission_id": context.recipe.k.admission_id, "reason": error.to_string() }),
            );
            eprintln!("normal provider PID1 failed after K: {error}");
            70
        }
    }
}

fn pid1_run(
    recipe: &SupervisorRecipe,
    directory: &Path,
    root: &PinnedProcess,
    actor: &PinnedProcess,
) -> io::Result<()> {
    if unsafe { libc::getpid() } != 1 {
        return Err(io::Error::other("normal provider worker is not PID1"));
    }
    let (pid1_host_pid, pid1_starttime_ticks) = own_host_pid_and_starttime()?;
    let namespace = host_proc_file("self/ns/pid")?;
    let namespace_meta = namespace.metadata()?;
    // The serving Broker verifies this PID1's namespace parent against the
    // exact released root on every challenged Bash request. PID1 itself has
    // no host-namespace ioctl authority after entering the child namespace.
    write_new(
        directory,
        "bash-parent.json",
        &BashParent {
            handoff_id: recipe.k.handoff_id.clone(),
            admission_id: recipe.k.admission_id.clone(),
            plan_sha256: recipe.k.plan_sha256.clone(),
            root_id: recipe.root_id.clone(),
            pid1_host_pid,
            pid1_starttime_ticks,
            pidns_dev: namespace_meta.dev(),
            pidns_ino: namespace_meta.ino(),
        },
    )?;
    let mut stdout = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(directory.join("stdout"))?;
    let mut stderr = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(directory.join("stderr"))?;
    File::open(directory)?.sync_all()?;
    let cwd = File::open(&recipe.cwd)?;
    let meta = cwd.metadata()?;
    if !meta.is_dir() || meta.dev() != recipe.plan.cwd_device || meta.ino() != recipe.plan.cwd_inode
    {
        return Err(io::Error::other("normal physical cwd changed before exec"));
    }
    // Scripts are launched through their ordinary pathname. This source
    // observation is explicitly pre-exec evidence, not an inode-exec claim.
    let source = normal_plan_custody::image_evidence(&recipe.executable)?;
    if source
        != (
            recipe.plan.executable_device,
            recipe.plan.executable_inode,
            recipe.plan.executable_mount_id,
            recipe.plan.executable_sha256.clone(),
            recipe.plan.executable_metadata_sha256.clone(),
            recipe.plan.path_execution,
        )
    {
        return Err(io::Error::other(
            "normal physical executable path changed before exec",
        ));
    }
    let input_fd = unsafe {
        libc::memfd_create(
            c"normal-provider-stdin".as_ptr(),
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
        )
    };
    if input_fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut input = unsafe { File::from_raw_fd(input_fd) };
    input.write_all(&recipe.stdin)?;
    use std::io::Seek;
    input.rewind()?;
    let mut command = Command::new(&recipe.executable);
    command
        .arg0(&recipe.plan.configured_program)
        .args(&recipe.argv)
        .env_clear()
        .envs(recipe.environment.iter().cloned())
        .stdin(Stdio::from(input))
        .stdout(Stdio::from(stdout.try_clone()?))
        .stderr(Stdio::from(stderr.try_clone()?));
    let cwd_fd = std::os::fd::AsRawFd::as_raw_fd(&cwd);
    let uid = recipe.uid;
    let gid = recipe.gid;
    let groups = recipe.groups.clone();
    let fixture = recipe.private_fixture;
    unsafe {
        command.pre_exec(move || {
            if libc::fchdir(cwd_fd) != 0
                || (!fixture && libc::setgroups(groups.len(), groups.as_ptr()) != 0)
                || libc::setresgid(gid, gid, gid) != 0
                || libc::setresuid(uid, uid, uid) != 0
            {
                return Err(io::Error::last_os_error());
            }
            if libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) != 0
                || libc::prctl(libc::PR_GET_SECCOMP, 0, 0, 0, 0) != 0
            {
                return Err(io::Error::other("normal provider inherited NNP/seccomp"));
            }
            Ok(())
        });
    }
    if stamp(root) != recipe.root
        || stamp(actor) != recipe.actor
        || !root.is_namespace_init()?
        || !actor.direct_child_of(root)?
        || !actor.in_namespace(root.namespace())?
    {
        return Err(io::Error::other(
            "normal provider root or actor closed before exec",
        ));
    }
    root.verify()?;
    actor.verify()?;
    let provider = command.spawn()?;
    let provider_pid = provider.id() as i32;
    let mut provider_wait = None;
    loop {
        let mut status = 0;
        let waited = unsafe { libc::waitpid(-1, &mut status, 0) };
        if waited == provider_pid {
            provider_wait = Some(status);
        }
        if waited > 0 {
            continue;
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        if error.raw_os_error() == Some(libc::ECHILD) {
            break;
        }
        return Err(error);
    }
    let provider_wait_status =
        provider_wait.ok_or_else(|| io::Error::other("normal provider wait absent"))?;
    // This interim observation is useful to fixture and diagnostics; failure
    // to publish it must not terminate PID1 with an attached child still live.
    let _ = write_new(
        directory,
        "provider-exit.json",
        &serde_json::json!({
            "admission_id": recipe.k.admission_id,
            "provider_wait_status": provider_wait_status,
        }),
    );
    // A Broker-spawned Bash child shares this PID namespace but is not this
    // PID1's process child. ECHILD alone cannot certify its lifetime. The
    // Broker records an exact child grant while holding this directory lock
    // through K; PID1 takes the same lock before deciding that Q may close.
    let lock = loop {
        if let Ok(lock) = File::open(directory) {
            if lock_directory(&lock).is_ok() {
                match bash_children_drained(directory, &recipe.k.admission_id, &root.boot_id) {
                    Ok(true) => break lock,
                    Ok(false) | Err(_) => {}
                }
            }
            drop(lock);
        }
        std::thread::sleep(CHILD_DRAIN_POLL);
    };
    let q = PhysicalQ {
        admission_id: recipe.k.admission_id.clone(),
        plan_sha256: recipe.k.plan_sha256.clone(),
        provider_wait_status,
        tree_drained: true,
        stdout: output_evidence(&mut stdout)?,
        stderr: output_evidence(&mut stderr)?,
    };
    let result = write_new(directory, "q.json", &q);
    drop(lock);
    result
}

/// Runs only as a fresh process spawned by the broker after State K commits.
pub fn supervisor(state_root: &Path) -> io::Result<()> {
    // Keep a host-PID observer across setns/clone so PID1 can recheck the
    // original root and actor immediately before it forks the provider.
    install_detached_host_proc()?;
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(MAX_RECIPE + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_RECIPE {
        return Err(io::Error::other("normal supervisor recipe oversized"));
    }
    let recipe: SupervisorRecipe = serde_json::from_slice(&bytes)?;
    retained_k_matches(state_root, &recipe)?;
    let root = exact_process(&recipe.root)?;
    let actor = exact_process(&recipe.actor)?;
    if !root.is_namespace_init()?
        || !actor.direct_child_of(&root)?
        || !actor.in_namespace(root.namespace())?
    {
        return Err(io::Error::other(
            "normal supervisor root or actor closed before launch",
        ));
    }
    if unsafe { libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) } != 0
        || unsafe { libc::prctl(libc::PR_GET_SECCOMP, 0, 0, 0, 0) } != 0
    {
        return Err(io::Error::other("normal supervisor inherited NNP/seccomp"));
    }
    let config = File::open(&recipe.config_directory)?;
    let meta = config.metadata()?;
    if !meta.is_dir()
        || meta.dev() != recipe.plan.selection.source.directory_device
        || meta.ino() != recipe.plan.selection.source.directory_inode
        || normal_model_selection::candidate(recipe.plan.selection.invocation.clone(), &config)?
            != recipe.plan.selection
    {
        return Err(io::Error::other(
            "normal supervisor selected config or account changed",
        ));
    }
    let store = store_root(state_root)?;
    let directory = id_path(&store, &recipe.k.admission_id)?;
    fs::DirBuilder::new().mode(0o700).create(&directory)?;
    File::open(&store)?.sync_all()?;
    if unsafe { libc::setns(root.namespace().as_raw_fd(), libc::CLONE_NEWPID) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // setns changes the PID namespace of future children. Fork once into
    // that namespace before making the nested PID1 that owns provider Q.
    let intermediate = unsafe { libc::fork() };
    if intermediate < 0 {
        return Err(io::Error::last_os_error());
    }
    if intermediate == 0 {
        let result = (|| {
            let context = Box::into_raw(Box::new(Pid1Context {
                recipe: recipe.clone(),
                directory: directory.clone(),
                root,
                actor,
            }));
            let mut stack = vec![0u8; 1024 * 1024];
            let top = unsafe { stack.as_mut_ptr().add(stack.len()) };
            let child = unsafe {
                libc::clone(
                    pid1_entry,
                    top.cast(),
                    libc::CLONE_NEWPID | libc::SIGCHLD,
                    context.cast(),
                )
            };
            unsafe {
                drop(Box::from_raw(context));
            }
            if child < 0 {
                return Err(io::Error::last_os_error());
            }
            let mut status = 0;
            if unsafe { libc::waitpid(child, &mut status, 0) } != child {
                return Err(io::Error::last_os_error());
            }
            write_new(
                &directory,
                "parent-wait.json",
                &ParentWait {
                    admission_id: recipe.k.admission_id,
                    pid1_parent_namespace_pid: child,
                    pid1_wait_status: status,
                },
            )
        })();
        unsafe { libc::_exit(if result.is_ok() { 0 } else { 70 }) };
    }
    let mut status = 0;
    if unsafe { libc::waitpid(intermediate, &mut status, 0) } != intermediate {
        return Err(io::Error::last_os_error());
    }
    if !libc::WIFEXITED(status) || libc::WEXITSTATUS(status) != 0 {
        return Err(io::Error::other("normal provider namespace helper failed"));
    }
    Ok(())
}
