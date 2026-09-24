//! Private first provider K: the broker, not a Runner worker, forks the pinned
//! executable. Private socket opcodes carry descriptor-backed plans; the
//! ordinary CLI route remains closed; a private typed runtime backend consumes
//! its Q-gated readbacks without publishing terminal success.
use super::work_launch;
use chrono::{DateTime, Utc};
use oulipoly_kernel_broker::identity::{PinnedProcess, host_proc_file, observed_incarnation_gone};
use oulipoly_kernel_broker::protocol::{
    FreshAccountEffectKind, FreshAccountEffectReadback, FreshAccountEffectRequest,
    FreshQuotaWindow, FreshRouteRequest, FreshRouteSelection,
};
use oulipoly_state::mailbox::{
    FreshNormalWorkPreparation, FreshReleasedHandoff, FreshRootWorkIntent,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static CANCEL: AtomicBool = AtomicBool::new(false);
extern "C" fn request_cancel(_: libc::c_int) {
    CANCEL.store(true, Ordering::Relaxed);
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Binding {
    root_id: String,
    handoff_id: String,
    invocation_uuid: String,
    session_id: String,
    owner_generation: String,
    actor_pid: i32,
    actor_starttime: u64,
    actor_boot_id: String,
    actor_pidns_dev: u64,
    actor_pidns_ino: u64,
    root_pid: i32,
    root_starttime: u64,
    root_pidns_dev: u64,
    root_pidns_ino: u64,
}

/// Caller must have just reattested the old release and fresh U/D through
/// `FreshV30Lane`. This checks the exact held J identity again at K planning.
pub(super) fn binding_from_held(
    release: &FreshReleasedHandoff,
    held: &FreshNormalWorkPreparation,
    actor: &PinnedProcess,
    root: &PinnedProcess,
) -> io::Result<Binding> {
    actor.verify()?;
    root.verify()?;
    let prepared = &release.old_release.prepared;
    if held.state != "held"
        || held.handoff_id != release.handoff_id
        || held.invocation_uuid != release.invocation_uuid
        || held.intent != release.root_work_intent
        || !matches!(held.intent, FreshRootWorkIntent::NormalCli(_))
        || held.actor.host_pid != actor.host_pid
        || held.actor.starttime_ticks != actor.starttime_ticks
        || held.actor.boot_id != actor.boot_id
        || (held.actor.pidns_dev, held.actor.pidns_ino) != (actor.pidns_dev, actor.pidns_ino)
        || prepared.joined_child.host_pid != actor.host_pid
        || prepared.joined_child.starttime_ticks != actor.starttime_ticks
        || prepared.root_init.host_pid != root.host_pid
        || prepared.root_init.starttime_ticks != root.starttime_ticks
        || (prepared.root_init.pidns_dev, prepared.root_init.pidns_ino)
            != (root.pidns_dev, root.pidns_ino)
        || !root.is_namespace_init()?
        || !actor.direct_child_of(root)?
        || !actor.in_namespace(root.namespace())?
    {
        return Err(io::Error::other("fresh provider held root/actor mismatch"));
    }
    Ok(Binding {
        root_id: prepared.root_id.clone(),
        handoff_id: held.handoff_id.clone(),
        invocation_uuid: held.invocation_uuid.clone(),
        session_id: held.session_id.clone(),
        owner_generation: prepared.owner_generation.clone(),
        actor_pid: actor.host_pid,
        actor_starttime: actor.starttime_ticks,
        actor_boot_id: actor.boot_id.clone(),
        actor_pidns_dev: actor.pidns_dev,
        actor_pidns_ino: actor.pidns_ino,
        root_pid: root.host_pid,
        root_starttime: root.starttime_ticks,
        root_pidns_dev: root.pidns_dev,
        root_pidns_ino: root.pidns_ino,
    })
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Recipe {
    argv: Vec<String>,
    env: Vec<(String, String)>,
}

pub(super) struct Plan {
    image: File,
    cwd: File,
    input: File,
    recipe: File,
    digest: String,
    image_hash: String,
    cwd_device: u64,
    cwd_inode: u64,
}

impl Plan {
    fn verify(&self) -> io::Result<()> {
        let cwd = self.cwd.metadata()?;
        if sha_file(&self.image)?.0 != self.image_hash
            || cwd.dev() != self.cwd_device
            || cwd.ino() != self.cwd_inode
            || unsafe { libc::fcntl(self.image.as_raw_fd(), libc::F_GET_SEALS) }
                & libc::F_SEAL_WRITE
                == 0
            || unsafe { libc::fcntl(self.input.as_raw_fd(), libc::F_GET_SEALS) }
                & libc::F_SEAL_WRITE
                == 0
            || unsafe { libc::fcntl(self.recipe.as_raw_fd(), libc::F_GET_SEALS) }
                & libc::F_SEAL_WRITE
                == 0
        {
            return Err(io::Error::other("fresh provider pinned plan changed"));
        }
        Ok(())
    }
}

fn sha_file(file: &File) -> io::Result<(String, u64)> {
    let before = file.metadata()?;
    let mut hash = Sha256::new();
    let mut count = 0u64;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read_at(&mut buf, count)?;
        if n == 0 {
            break;
        }
        count = count
            .checked_add(n as u64)
            .ok_or_else(|| io::Error::other("length overflow"))?;
        hash.update(&buf[..n]);
    }
    let after = file.metadata()?;
    if before.dev() != after.dev()
        || before.ino() != after.ino()
        || before.len() != after.len()
        || count != before.len()
    {
        return Err(io::Error::other("pinned file changed while hashing"));
    }
    Ok((format!("{:x}", hash.finalize()), count))
}

fn sealed_copy(source: &File, name: &'static std::ffi::CStr) -> io::Result<File> {
    let fd =
        unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_ALLOW_SEALING | libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut target = unsafe { File::from_raw_fd(fd) };
    let before = source.metadata()?;
    if !before.is_file() {
        return Err(io::Error::other("provider source is not regular"));
    }
    let mut offset = 0u64;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = source.read_at(&mut buf, offset)?;
        if n == 0 {
            break;
        }
        target.write_all(&buf[..n])?;
        offset = offset
            .checked_add(n as u64)
            .ok_or_else(|| io::Error::other("input length overflow"))?;
    }
    let after = source.metadata()?;
    if before.dev() != after.dev()
        || before.ino() != after.ino()
        || before.len() != after.len()
        || offset != before.len()
    {
        return Err(io::Error::other("provider source changed"));
    }
    let seals = libc::F_SEAL_SEAL | libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_WRITE;
    if unsafe { libc::fcntl(fd, libc::F_ADD_SEALS, seals) } < 0 {
        return Err(io::Error::last_os_error());
    }
    target.seek(SeekFrom::Start(0))?;
    Ok(target)
}

/// One deterministic ELF executable, absolute path, exact argv/env, directory
/// descriptor and sealed stdin. Unsupported shapes are rejected before spend.
/// The setuid/setgid image refusal below scopes only this private proof; it is
/// not an accepted restriction for the eventual unrestricted host-sudo route.
pub(super) fn plan(
    image_path: &Path,
    cwd: &Path,
    input: &File,
    argv: Vec<String>,
    env: Vec<(String, String)>,
) -> io::Result<Plan> {
    let mut keys = HashSet::new();
    if !image_path.is_absolute()
        || !cwd.is_absolute()
        || argv.iter().any(|a| a.contains('\0'))
        || env.iter().any(|(k, v)| {
            k.is_empty()
                || k.contains(['=', '\0'])
                || v.contains('\0')
                || !keys.insert(k)
                || k.starts_with("LD_")
                || k.starts_with("DYLD_")
                || k.starts_with("OULIPOLY_KERNEL_")
                || matches!(k.as_str(), "GLIBC_TUNABLES" | "GCONV_PATH")
        })
    {
        return Err(io::Error::other("unsupported fresh provider plan"));
    }
    let image = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(image_path)?;
    let meta = image.metadata()?;
    let mut magic = [0u8; 4];
    if !meta.is_file()
        || meta.mode() & 0o111 == 0
        || meta.mode() & 0o6000 != 0
        || image.read_at(&mut magic, 0)? != 4
        || magic != *b"\x7fELF"
    {
        return Err(io::Error::other("fresh provider requires pinned local ELF"));
    }
    let cwd = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(cwd)?;
    let input = sealed_copy(input, c"fresh-provider-stdin")?;
    let recipe_fd = unsafe {
        libc::memfd_create(
            c"fresh-provider-recipe".as_ptr(),
            libc::MFD_ALLOW_SEALING | libc::MFD_CLOEXEC,
        )
    };
    if recipe_fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut recipe = unsafe { File::from_raw_fd(recipe_fd) };
    serde_json::to_writer(&mut recipe, &Recipe { argv, env })?;
    let seals = libc::F_SEAL_SEAL | libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_WRITE;
    if unsafe { libc::fcntl(recipe_fd, libc::F_ADD_SEALS, seals) } < 0 {
        return Err(io::Error::last_os_error());
    }
    recipe.seek(SeekFrom::Start(0))?;
    let original_hash = sha_file(&image)?.0;
    let sealed_image = sealed_copy(&image, c"fresh-provider-image")?;
    let (image_hash, _) = sha_file(&sealed_image)?;
    if image_hash != original_hash || sha_file(&image)?.0 != original_hash {
        return Err(io::Error::other("provider image changed while sealing"));
    }
    let (input_hash, input_len) = sha_file(&input)?;
    let (recipe_hash, recipe_len) = sha_file(&recipe)?;
    let cwd_meta = cwd.metadata()?;
    let digest = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(
            &image_hash,
            meta.dev(),
            meta.ino(),
            cwd_meta.dev(),
            cwd_meta.ino(),
            input_hash,
            input_len,
            recipe_hash,
            recipe_len,
        ))?)
    );
    Ok(Plan {
        image: sealed_image,
        cwd,
        input,
        recipe,
        digest,
        image_hash,
        cwd_device: cwd_meta.dev(),
        cwd_inode: cwd_meta.ino(),
    })
}

/// The eventual runtime bridge can pass variable recipe and prompt bytes by
/// descriptor. The private socket uses this form so its control frame remains
/// fixed size. The broker resolves the absolute image path, pins its original
/// inode, then executes a sealed byte-identical copy under K.
/// This private proof refuses a setuid/setgid image before K. Production must
/// resolve that launch shape without narrowing normal host privileges.
pub(super) fn plan_from_descriptors(
    configured_image: &Path,
    image: File,
    cwd: File,
    input: File,
    recipe: File,
) -> io::Result<Plan> {
    if !configured_image.is_absolute() {
        return Err(io::Error::other(
            "fresh provider image path is not absolute",
        ));
    }
    let configured = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(configured_image)?;
    let image_meta = image.metadata()?;
    let configured_meta = configured.metadata()?;
    let cwd_meta = cwd.metadata()?;
    let mut magic = [0u8; 4];
    if !image_meta.is_file()
        || image_meta.mode() & 0o111 == 0
        || image_meta.mode() & 0o6000 != 0
        || !cwd_meta.is_dir()
        || (image_meta.dev(), image_meta.ino()) != (configured_meta.dev(), configured_meta.ino())
        || image.read_at(&mut magic, 0)? != 4
        || magic != *b"\x7fELF"
    {
        return Err(io::Error::other(
            "fresh provider pinned image or cwd mismatch",
        ));
    }
    let mut reader = recipe.try_clone()?;
    reader.seek(SeekFrom::Start(0))?;
    let parsed: Recipe = serde_json::from_reader(reader)?;
    let mut keys = HashSet::new();
    if parsed.argv.iter().any(|arg| arg.contains('\0'))
        || parsed.env.iter().any(|(key, value)| {
            key.is_empty()
                || key.contains(['=', '\0'])
                || value.contains('\0')
                || !keys.insert(key)
                || key.starts_with("LD_")
                || key.starts_with("DYLD_")
                || key.starts_with("OULIPOLY_KERNEL_")
                || matches!(key.as_str(), "GLIBC_TUNABLES" | "GCONV_PATH")
        })
    {
        return Err(io::Error::other("unsupported fresh provider recipe"));
    }
    let original_hash = sha_file(&image)?.0;
    if sha_file(&configured)?.0 != original_hash {
        return Err(io::Error::other(
            "fresh provider image path content changed",
        ));
    }
    let sealed_image = sealed_copy(&image, c"fresh-provider-image")?;
    let (image_hash, _) = sha_file(&sealed_image)?;
    if image_hash != original_hash || sha_file(&image)?.0 != original_hash {
        return Err(io::Error::other(
            "fresh provider image changed while sealing",
        ));
    }
    let input = sealed_copy(&input, c"fresh-provider-stdin")?;
    let recipe = sealed_copy(&recipe, c"fresh-provider-recipe")?;
    let (input_hash, input_len) = sha_file(&input)?;
    let (recipe_hash, recipe_len) = sha_file(&recipe)?;
    let digest = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(
            &image_hash,
            image_meta.dev(),
            image_meta.ino(),
            cwd_meta.dev(),
            cwd_meta.ino(),
            input_hash,
            input_len,
            recipe_hash,
            recipe_len,
        ))?)
    );
    Ok(Plan {
        image: sealed_image,
        cwd,
        input,
        recipe,
        digest,
        image_hash,
        cwd_device: cwd_meta.dev(),
        cwd_inode: cwd_meta.ino(),
    })
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Grant {
    version: u32,
    id: String,
    binding: Binding,
    plan_sha256: String,
}

pub(super) struct Prepared {
    grant: Grant,
    plan: Plan,
    directory: PathBuf,
}

fn durable_new<T: Serialize>(directory: &Path, name: &str, value: &T) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(directory.join(name))?;
    serde_json::to_writer(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    File::open(directory)?.sync_all()
}

fn durable_result<T: Serialize>(directory: &Path, value: &T) -> io::Result<()> {
    let temporary = format!("{}.result-pending.json", uuid::Uuid::new_v4());
    durable_new(directory, &temporary, value)?;
    std::fs::rename(directory.join(&temporary), directory.join("result.json"))?;
    File::open(directory)?.sync_all()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct RouteCandidate {
    version: u32,
    binding: Binding,
    model: String,
    config_sha256: String,
    account: String,
    index: usize,
    total: usize,
    pin: Option<String>,
    plan_sha256: String,
    quota_script: Option<String>,
    auth_refresh_command: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct RouteDecision {
    version: u32,
    binding: Binding,
    total: usize,
    pin: Option<String>,
    selection: FreshRouteSelection,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct RouteSource {
    version: u32,
    binding: Binding,
    config_sha256: String,
    directory_device: u64,
    directory_inode: u64,
}

fn route_request_valid(request: &FreshRouteRequest, binding: &Binding) -> io::Result<()> {
    if request.d_key.is_empty()
        || request.model.is_empty()
        || request.account.as_deref().is_some_and(str::is_empty)
        || request.config_sha256.len() != 64
        || !request.config_sha256.bytes().all(|c| c.is_ascii_hexdigit())
        || request.total == 0
        || request.index.is_some_and(|index| index >= request.total)
        || request.pin.as_deref().is_some_and(str::is_empty)
        || uuid::Uuid::parse_str(&binding.handoff_id).is_err()
    {
        return Err(io::Error::other("fresh route request invalid"));
    }
    Ok(())
}

/// Read the configured pool in the broker through the caller's pinned directory
/// descriptor. The request digest and roster are assertions checked against
/// source bytes, not authority supplied by the Runner. Registration and the
/// final choice both recheck the source, so a configuration edit between them
/// refuses the choice before any provider K.
pub(super) fn validate_route_source(
    config_dir: &File,
    request: &FreshRouteRequest,
) -> io::Result<()> {
    if !config_dir.metadata()?.is_dir() {
        return Err(io::Error::other("fresh config source is not a directory"));
    }
    let path = PathBuf::from(format!("/proc/self/fd/{}", config_dir.as_raw_fd()));
    let pool = oulipoly_runtime::executor::cli::fresh_remote::load_fresh_headless_pool(
        &path,
        &request.model,
    )
    .map_err(|error| io::Error::other(format!("fresh config source invalid: {error}")))?;
    if pool.config_sha256 != request.config_sha256 || pool.model.providers.len() != request.total {
        return Err(io::Error::other(
            "fresh config source digest or roster changed",
        ));
    }
    if let Some(index) = request.index {
        let member = pool
            .model
            .providers
            .get(index)
            .ok_or_else(|| io::Error::other("fresh config source account index invalid"))?;
        let effect = pool
            .account_effects
            .get(index)
            .ok_or_else(|| io::Error::other("fresh config source account effect absent"))?;
        if request.account.as_deref() != Some(member.name.as_str())
            || request.quota_script != effect.0
            || request.auth_refresh_command != effect.1
        {
            return Err(io::Error::other(
                "fresh config source account or effect forged",
            ));
        }
    } else if request.account.is_some()
        || request.quota_script.is_some()
        || request.auth_refresh_command.is_some()
    {
        return Err(io::Error::other(
            "fresh config source selection carries candidate",
        ));
    }
    Ok(())
}

/// Bind every candidate and the final choice to the same directory inode.
/// A source with identical bytes at another path is a different snapshot
/// origin and cannot replace this held root's already registered source.
pub(super) fn bind_route_source(
    directory: &Path,
    binding: &Binding,
    request: &FreshRouteRequest,
    config_dir: &File,
    registration: bool,
) -> io::Result<()> {
    let meta = config_dir.metadata()?;
    let source = RouteSource {
        version: 1,
        binding: binding.clone(),
        config_sha256: request.config_sha256.clone(),
        directory_device: meta.dev(),
        directory_inode: meta.ino(),
    };
    let name = format!("{}.route-source.json", binding.handoff_id);
    match exact_file::<RouteSource>(directory, &name)? {
        Some(existing) if existing == source => Ok(()),
        Some(_) => Err(io::Error::other("fresh route source directory changed")),
        None if registration => durable_new(directory, &name, &source),
        None => Err(io::Error::other("fresh route source registration absent")),
    }
}

fn candidate_name(handoff: &str, index: usize) -> String {
    format!("{handoff}.route-{index}.json")
}

fn decision_name(handoff: &str) -> String {
    format!("{handoff}.route-selection.json")
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct AccountEffectIntent {
    version: u32,
    id: String,
    binding: Binding,
    request: FreshAccountEffectRequest,
    environment_sha256: String,
    plan_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct QuotaReuse {
    source_directory: String,
    source_effect_id: String,
}

fn redacted_effect_request(request: &FreshAccountEffectRequest) -> FreshAccountEffectRequest {
    FreshAccountEffectRequest {
        environment: Vec::new(),
        ..request.clone()
    }
}

fn environment_digest(request: &FreshAccountEffectRequest) -> io::Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&request.environment)?)
    ))
}

fn effect_directory(
    directory: &Path,
    binding: &Binding,
    request: &FreshAccountEffectRequest,
) -> PathBuf {
    let kind = match request.kind {
        FreshAccountEffectKind::QuotaFirst => "quota-first",
        FreshAccountEffectKind::AuthRefresh => "auth-refresh",
        FreshAccountEffectKind::QuotaRetry => "quota-retry",
    };
    directory
        .join("account-effects")
        .join(format!("{}-{}-{kind}", binding.handoff_id, request.index))
}

fn effect_candidate(
    directory: &Path,
    binding: &Binding,
    request: &FreshAccountEffectRequest,
) -> io::Result<RouteCandidate> {
    let candidate: RouteCandidate = exact_file(
        directory,
        &candidate_name(&binding.handoff_id, request.index),
    )?
    .ok_or_else(|| io::Error::other("fresh account effect candidate absent"))?;
    if candidate.version != 1
        || candidate.binding != *binding
        || candidate.model != request.model
        || candidate.config_sha256 != request.config_sha256
        || candidate.account != request.account
        || candidate.index != request.index
    {
        return Err(io::Error::other("fresh account effect candidate changed"));
    }
    Ok(candidate)
}

fn effect_command<'a>(
    candidate: &'a RouteCandidate,
    kind: FreshAccountEffectKind,
) -> io::Result<&'a str> {
    let command = match kind {
        FreshAccountEffectKind::QuotaFirst | FreshAccountEffectKind::QuotaRetry => {
            candidate.quota_script.as_deref()
        }
        FreshAccountEffectKind::AuthRefresh => candidate.auth_refresh_command.as_deref(),
    };
    command
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| io::Error::other("fresh account effect command absent"))
}

fn effect_intent(dir: &Path) -> io::Result<Option<AccountEffectIntent>> {
    exact_file(dir, "intent.json")
}

fn reusable_quota_source(
    directory: &Path,
    binding: &Binding,
    request: &FreshAccountEffectRequest,
) -> io::Result<Option<(String, AccountEffectIntent)>> {
    let parent = directory.join("account-effects");
    let mut names = std::fs::read_dir(&parent)?
        .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
        .collect::<io::Result<Vec<_>>>()?;
    names.sort();
    let mut fresh = None;
    let mut unresolved = None;
    for name in names {
        if !name.ends_with("-quota-first") && !name.ends_with("-quota-retry") {
            continue;
        }
        let source_dir = parent.join(&name);
        let Some(intent) = effect_intent(&source_dir)? else {
            continue;
        };
        if intent.version != 1
            || intent.binding == *binding
            || intent.request.model != request.model
            || intent.request.config_sha256 != request.config_sha256
            || intent.request.account != request.account
            || intent.request.index != request.index
            || intent.environment_sha256 != environment_digest(request)?
            || source_dir.join("reuse.json").exists()
        {
            continue;
        }
        let readback = effect_readback_from_dir(&source_dir, &intent)?;
        if readback.state == "drained" && readback.outcome.as_deref() == Some("valid_windows") {
            if matches!(
                quota_remaining(&readback, Utc::now().timestamp()),
                Ok(Some(_))
            ) {
                if fresh
                    .as_ref()
                    .is_none_or(|(_, _, prior_time)| readback.completed_unix_seconds > *prior_time)
                {
                    fresh = Some((name, intent, readback.completed_unix_seconds));
                }
            }
        } else if readback.state != "drained" && unresolved.is_none() {
            unresolved = Some((name, intent));
        }
    }
    Ok(fresh.map(|(name, intent, _)| (name, intent)).or(unresolved))
}

/// Serialize the scan and durable auth intent across broker threads and
/// incarnations. The lock protects the decision only; an already started K is
/// represented by the fsynced intent and must be observed, never launched again.
fn auth_admission_lock(directory: &Path, account: &str) -> io::Result<File> {
    let parent = directory.join("account-effects");
    std::fs::create_dir_all(&parent)?;
    let name = format!("auth-{:x}.lock", Sha256::digest(account.as_bytes()));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .open(parent.join(name))?;
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
            return Ok(file);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

fn refuse_concurrent_auth(directory: &Path, binding: &Binding, account: &str) -> io::Result<()> {
    let parent = directory.join("account-effects");
    let entries = match std::fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        if !entry
            .file_name()
            .to_string_lossy()
            .ends_with("auth-refresh")
        {
            continue;
        }
        let Some(intent) = effect_intent(&entry.path())? else {
            continue;
        };
        if intent.binding == *binding || intent.request.account != account {
            continue;
        }
        let prior = effect_readback_from_dir(&entry.path(), &intent)?;
        if prior.state != "drained"
            || prior
                .completed_unix_seconds
                .is_some_and(|completed| Utc::now().timestamp() - completed < 30)
        {
            return Err(io::Error::other(format!(
                "fresh auth refresh already active or recently completed: effect={}, state={}, artifact={}",
                prior.effect_id, prior.state, prior.artifact
            )));
        }
    }
    Ok(())
}

fn effect_readback_from_dir(
    dir: &Path,
    intent: &AccountEffectIntent,
) -> io::Result<FreshAccountEffectReadback> {
    let artifact = dir.display().to_string();
    if let Some(reuse) = exact_file::<QuotaReuse>(dir, "reuse.json")? {
        if reuse.source_directory.contains('/')
            || reuse.source_directory.contains("..")
            || (!reuse.source_directory.ends_with("-quota-first")
                && !reuse.source_directory.ends_with("-quota-retry"))
        {
            return Err(io::Error::other("fresh quota reuse source invalid"));
        }
        let source_dir = dir
            .parent()
            .ok_or_else(|| io::Error::other("fresh quota reuse parent absent"))?
            .join(&reuse.source_directory);
        if source_dir.join("reuse.json").exists() {
            return Err(io::Error::other("fresh quota reuse chain refused"));
        }
        let source = effect_intent(&source_dir)?
            .ok_or_else(|| io::Error::other("fresh quota reuse intent absent"))?;
        if source.id != reuse.source_effect_id
            || source.version != 1
            || source.request.model != intent.request.model
            || source.request.config_sha256 != intent.request.config_sha256
            || source.request.account != intent.request.account
            || source.request.index != intent.request.index
            || source.environment_sha256 != intent.environment_sha256
        {
            return Err(io::Error::other("fresh quota reuse provenance changed"));
        }
        let mut readback = effect_readback_from_dir(&source_dir, &source)?;
        readback.effect_id = intent.id.clone();
        readback.artifact = format!("{artifact} -> {}", readback.artifact);
        return Ok(readback);
    }
    let unknown = |state: &str| FreshAccountEffectReadback {
        effect_id: intent.id.clone(),
        state: state.into(),
        outcome: None,
        windows: Vec::new(),
        completed_unix_seconds: None,
        artifact: artifact.clone(),
    };
    let grant = grant_for_binding(dir, &intent.binding)?;
    let Some(grant) = grant else {
        return Ok(unknown("unknown"));
    };
    match observe(dir, &grant)? {
        Observation::Unknown => Ok(unknown("unknown")),
        Observation::Pending | Observation::ProviderExited(_) => Ok(unknown("pending")),
        Observation::Drained {
            status, mut stdout, ..
        } => {
            let completed = std::fs::metadata(dir.join(format!("{grant}.drain.json")))?
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(io::Error::other)?
                .as_secs() as i64;
            let (outcome, windows) = if status != 0 {
                ("failed".to_string(), Vec::new())
            } else if intent.request.kind == FreshAccountEffectKind::AuthRefresh {
                ("refreshed".to_string(), Vec::new())
            } else {
                let mut raw = String::new();
                stdout.read_to_string(&mut raw)?;
                match parse_effect_windows(&raw) {
                    Ok(windows) if !windows.is_empty() => ("valid_windows".into(), windows),
                    Ok(_) => ("empty".into(), Vec::new()),
                    Err(_) => ("invalid".into(), Vec::new()),
                }
            };
            let receipt = FreshAccountEffectReadback {
                effect_id: intent.id.clone(),
                state: "drained".into(),
                outcome: Some(outcome),
                windows,
                completed_unix_seconds: Some(completed),
                artifact,
            };
            if let Some(existing) = exact_file::<FreshAccountEffectReadback>(dir, "result.json")? {
                if serde_json::to_value(&existing)? != serde_json::to_value(&receipt)? {
                    return Err(io::Error::other("fresh account effect result changed"));
                }
            } else {
                durable_result(dir, &receipt)?;
            }
            Ok(receipt)
        }
    }
}

#[derive(Deserialize)]
struct RawEffectWindow {
    used_percent: f64,
    resets_at: String,
}
#[derive(Deserialize)]
struct RawEffectOutput {
    windows: Option<Vec<RawEffectWindow>>,
    used_percent: Option<f64>,
    resets_at: Option<String>,
}

fn parse_effect_windows(raw: &str) -> io::Result<Vec<FreshQuotaWindow>> {
    let value: RawEffectOutput = serde_json::from_str(raw)?;
    let windows = if let Some(windows) = value.windows {
        windows
    } else {
        vec![RawEffectWindow {
            used_percent: value
                .used_percent
                .ok_or_else(|| io::Error::other("quota used_percent absent"))?,
            resets_at: value
                .resets_at
                .ok_or_else(|| io::Error::other("quota resets_at absent"))?,
        }]
    };
    windows
        .into_iter()
        .map(|window| {
            if !window.used_percent.is_finite()
                || !(0.0..=100.0).contains(&window.used_percent)
                || DateTime::parse_from_rfc3339(&window.resets_at).is_err()
            {
                return Err(io::Error::other("invalid quota window"));
            }
            Ok(FreshQuotaWindow {
                used_percent: window.used_percent,
                resets_at: window.resets_at,
            })
        })
        .collect()
}

pub(super) fn observe_account_effect(
    directory: &Path,
    binding: &Binding,
    request: &FreshAccountEffectRequest,
) -> io::Result<FreshAccountEffectReadback> {
    effect_candidate(directory, binding, request)?;
    let dir = effect_directory(directory, binding, request);
    let intent = effect_intent(&dir)?
        .ok_or_else(|| io::Error::other("fresh account effect intent absent"))?;
    if intent.version != 1
        || intent.binding != *binding
        || intent.request != redacted_effect_request(request)
        || intent.environment_sha256 != environment_digest(request)?
    {
        return Err(io::Error::other("fresh account effect readback mismatch"));
    }
    effect_readback_from_dir(&dir, &intent)
}

/// The intent is fsynced before a separate one-use K. Any failure after that
/// point is unknown, and a second begin can only be observed, never launched.
pub(super) fn begin_account_effect(
    directory: &Path,
    binding: &Binding,
    request: &FreshAccountEffectRequest,
    root: &PinnedProcess,
    actor: &PinnedProcess,
    uid: u32,
    gid: u32,
) -> io::Result<FreshAccountEffectReadback> {
    let candidate = effect_candidate(directory, binding, request)?;
    let command = effect_command(&candidate, request.kind)?;
    let dir = effect_directory(directory, binding, request);
    if dir.exists() {
        return Err(io::Error::other(
            "fresh account effect already begun; observe exact effect",
        ));
    }
    let _auth_lock = if matches!(
        request.kind,
        FreshAccountEffectKind::AuthRefresh | FreshAccountEffectKind::QuotaFirst
    ) {
        Some(auth_admission_lock(directory, &request.account)?)
    } else {
        None
    };
    if dir.exists() {
        return Err(io::Error::other(
            "fresh account effect already begun; observe exact effect",
        ));
    }
    if request.kind == FreshAccountEffectKind::AuthRefresh {
        refuse_concurrent_auth(directory, binding, &request.account)?;
    }
    let first_request = FreshAccountEffectRequest {
        kind: FreshAccountEffectKind::QuotaFirst,
        ..request.clone()
    };
    if request.kind != FreshAccountEffectKind::QuotaFirst {
        let first = observe_account_effect(directory, binding, &first_request)?;
        if first.state != "drained"
            || !matches!(
                first.outcome.as_deref(),
                Some("failed" | "empty" | "invalid")
            )
        {
            return Err(io::Error::other(
                "auth refresh has no failed or empty quota prerequisite",
            ));
        }
        if request.kind == FreshAccountEffectKind::QuotaRetry {
            let auth = observe_account_effect(
                directory,
                binding,
                &FreshAccountEffectRequest {
                    kind: FreshAccountEffectKind::AuthRefresh,
                    ..request.clone()
                },
            )?;
            if auth.state != "drained" {
                return Err(io::Error::other(
                    "quota retry has no drained auth prerequisite",
                ));
            }
        }
    }
    if request.kind == FreshAccountEffectKind::QuotaFirst {
        let parent = directory.join("account-effects");
        if let Some((source_directory, source)) =
            reusable_quota_source(directory, binding, request)?
        {
            std::fs::create_dir(&dir)?;
            File::open(&parent)?.sync_all()?;
            let intent = AccountEffectIntent {
                version: 1,
                id: uuid::Uuid::new_v4().to_string(),
                binding: binding.clone(),
                request: redacted_effect_request(request),
                environment_sha256: environment_digest(request)?,
                plan_sha256: format!("reused:{}", source.id),
            };
            durable_new(&dir, "intent.json", &intent)?;
            durable_new(
                &dir,
                "reuse.json",
                &QuotaReuse {
                    source_directory,
                    source_effect_id: source.id,
                },
            )?;
            return effect_readback_from_dir(&dir, &intent);
        }
    }
    let cwd = std::fs::read_link(format!("/proc/{}/cwd", actor.host_pid))?;
    let shell = std::fs::canonicalize("/bin/sh")?;
    let input_fd =
        unsafe { libc::memfd_create(c"fresh-account-empty-stdin".as_ptr(), libc::MFD_CLOEXEC) };
    if input_fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let input = unsafe { File::from_raw_fd(input_fd) };
    let plan = plan(
        &shell,
        &cwd,
        &input,
        vec!["-c".into(), command.into()],
        request.environment.clone(),
    )?;
    let parent = directory.join("account-effects");
    std::fs::create_dir_all(&parent)?;
    File::open(directory)?.sync_all()?;
    std::fs::create_dir(&dir)?;
    File::open(&parent)?.sync_all()?;
    let intent = AccountEffectIntent {
        version: 1,
        id: uuid::Uuid::new_v4().to_string(),
        binding: binding.clone(),
        request: redacted_effect_request(request),
        environment_sha256: environment_digest(request)?,
        plan_sha256: plan.digest.clone(),
    };
    durable_new(&dir, "intent.json", &intent)?;
    let prepared = prepare(&dir, binding.clone(), plan)?;
    launch(prepared, root, actor, uid, gid)?;
    effect_readback_from_dir(&dir, &intent)
}

/// A candidate is a broker-pinned exact provider plan. It is durable before
/// selection and has no fork/effect. Repeated registrations must be identical.
pub(super) fn register_route_candidate(
    directory: &Path,
    binding: &Binding,
    request: &FreshRouteRequest,
    plan: Plan,
) -> io::Result<()> {
    route_request_valid(request, binding)?;
    let index = request
        .index
        .ok_or_else(|| io::Error::other("fresh route index absent"))?;
    let account = request
        .account
        .as_ref()
        .ok_or_else(|| io::Error::other("fresh route account absent"))?;
    let candidate = RouteCandidate {
        version: 1,
        binding: binding.clone(),
        model: request.model.clone(),
        config_sha256: request.config_sha256.clone(),
        account: account.clone(),
        index,
        total: request.total,
        pin: request.pin.clone(),
        plan_sha256: plan.digest.clone(),
        quota_script: request.quota_script.clone(),
        auth_refresh_command: request.auth_refresh_command.clone(),
    };
    let name = candidate_name(&binding.handoff_id, index);
    if let Some(existing) = exact_file::<RouteCandidate>(directory, &name)? {
        if existing != candidate {
            return Err(io::Error::other("fresh route candidate changed"));
        }
    } else {
        durable_new(directory, &name, &candidate)?;
    }
    Ok(())
}

fn route_candidates(
    directory: &Path,
    binding: &Binding,
    request: &FreshRouteRequest,
) -> io::Result<Vec<RouteCandidate>> {
    let mut candidates = Vec::new();
    let mut names = HashSet::new();
    for index in 0..request.total {
        let candidate: RouteCandidate =
            exact_file(directory, &candidate_name(&binding.handoff_id, index))?
                .ok_or_else(|| io::Error::other("fresh route candidate set incomplete"))?;
        if candidate.version != 1
            || candidate.binding != *binding
            || candidate.model != request.model
            || candidate.config_sha256 != request.config_sha256
            || candidate.index != index
            || candidate.total != request.total
            || candidate.pin != request.pin
            || request.quota_script.is_some()
            || request.auth_refresh_command.is_some()
            || !names.insert(candidate.account.clone())
        {
            return Err(io::Error::other("fresh route candidate set changed"));
        }
        candidates.push(candidate);
    }
    Ok(candidates)
}

fn route_evidence(directory: &Path, candidate: &RouteCandidate) -> io::Result<(u64, u64, u64)> {
    let mut live = 0;
    let mut failures = 0;
    let mut invocations = 0;
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.ends_with(".route-selection.json") {
            continue;
        }
        let previous: RouteDecision = serde_json::from_reader(File::open(entry.path())?)?;
        if previous.version != 1
            || previous.selection.model != candidate.model
            || previous.selection.config_sha256 != candidate.config_sha256
            || previous.selection.account != candidate.account
        {
            continue;
        }
        let Some(grant): Option<Grant> = exact_file(
            directory,
            &format!("{}.fresh-grant.json", previous.binding.handoff_id),
        )?
        else {
            continue;
        };
        if grant.binding != previous.binding || grant.plan_sha256 != previous.selection.plan_sha256
        {
            return Err(io::Error::other("fresh route history grant mismatch"));
        }
        if exact_file::<Grant>(directory, &format!("{}.consumed.json", grant.id))?.is_none() {
            continue;
        }
        invocations += 1;
        match observe(directory, &grant.id)? {
            Observation::Drained { status, .. } => {
                let drain_path = directory.join(format!("{}.drain.json", grant.id));
                if status != 0 && file_age_less_than(&drain_path, Duration::from_secs(30 * 60))? {
                    failures += 1;
                }
            }
            // A consumed K remains live until a physical Q or a known
            // outcome. Its age alone cannot authorize another selection.
            _ => live += 1,
        }
    }
    Ok((live, failures, invocations))
}

fn file_age_less_than(path: &Path, window: Duration) -> io::Result<bool> {
    Ok(std::fs::metadata(path)?
        .modified()?
        .elapsed()
        .is_ok_and(|age| age < window))
}

fn recent_failure_admitted(failures: u64, has_unsuppressed: bool, pinned: bool) -> bool {
    pinned || !has_unsuppressed || failures < 3
}

fn candidate_quota(
    directory: &Path,
    binding: &Binding,
    candidate: &RouteCandidate,
) -> io::Result<(Option<u32>, Option<String>)> {
    if candidate.quota_script.is_none() {
        return Ok((Some(0), None)); // no configured quota source, invocation fallback
    }
    let request = FreshAccountEffectRequest {
        d_key: String::new(),
        model: candidate.model.clone(),
        config_sha256: candidate.config_sha256.clone(),
        account: candidate.account.clone(),
        index: candidate.index,
        kind: FreshAccountEffectKind::QuotaFirst,
        environment: Vec::new(),
    };
    let first_dir = effect_directory(directory, binding, &request);
    let Some(first_intent) = effect_intent(&first_dir)? else {
        return Ok((None, None));
    };
    if first_intent.version != 1
        || first_intent.binding != *binding
        || first_intent.request.model != candidate.model
        || first_intent.request.config_sha256 != candidate.config_sha256
        || first_intent.request.account != candidate.account
        || first_intent.request.index != candidate.index
        || first_intent.request.kind != FreshAccountEffectKind::QuotaFirst
    {
        return Err(io::Error::other("fresh quota effect provenance changed"));
    }
    let mut result = effect_readback_from_dir(&first_dir, &first_intent)?;
    if result.state != "drained" {
        return Ok((None, Some(result.artifact)));
    }
    if result.outcome.as_deref() != Some("valid_windows")
        && candidate.auth_refresh_command.is_some()
    {
        let retry_dir = effect_directory(
            directory,
            binding,
            &FreshAccountEffectRequest {
                kind: FreshAccountEffectKind::QuotaRetry,
                ..request
            },
        );
        let Some(retry_intent) = effect_intent(&retry_dir)? else {
            return Ok((None, None));
        };
        if retry_intent.version != 1
            || retry_intent.binding != *binding
            || retry_intent.request.account != candidate.account
            || retry_intent.request.config_sha256 != candidate.config_sha256
            || retry_intent.request.kind != FreshAccountEffectKind::QuotaRetry
        {
            return Err(io::Error::other("fresh quota retry provenance changed"));
        }
        result = effect_readback_from_dir(&retry_dir, &retry_intent)?;
    }
    if result.state != "drained" {
        return Ok((None, Some(result.artifact)));
    }
    if result.outcome.as_deref() != Some("valid_windows") || result.windows.is_empty() {
        return Ok((None, None));
    }
    Ok((quota_remaining(&result, Utc::now().timestamp())?, None))
}

fn quota_remaining(result: &FreshAccountEffectReadback, now: i64) -> io::Result<Option<u32>> {
    let completed = result
        .completed_unix_seconds
        .ok_or_else(|| io::Error::other("fresh quota completion time absent"))?;
    if now < completed || now - completed >= 30 {
        return Ok(None);
    }
    let mut binding_remaining = u32::MAX;
    for window in &result.windows {
        let reset = DateTime::parse_from_rfc3339(&window.resets_at)
            .map_err(io::Error::other)?
            .timestamp();
        if reset <= now {
            return Ok(None);
        }
        if window.used_percent >= 100.0 {
            return Ok(None);
        }
        binding_remaining =
            binding_remaining.min(((100.0 - window.used_percent) * 100.0).round() as u32);
    }
    Ok(Some(binding_remaining))
}

/// One fsynced choice for this held J. Selection has no provider effect.
/// Cached Q and live K are measured only from this fresh broker directory.
pub(super) fn select_route(
    directory: &Path,
    binding: &Binding,
    request: &FreshRouteRequest,
) -> io::Result<FreshRouteSelection> {
    route_request_valid(request, binding)?;
    if request.account.is_some() || request.index.is_some() {
        return Err(io::Error::other("fresh route selection includes candidate"));
    }
    let candidates = route_candidates(directory, binding, request)?;
    let name = decision_name(&binding.handoff_id);
    if let Some(existing) = exact_file::<RouteDecision>(directory, &name)? {
        if existing.version != 1
            || existing.binding != *binding
            || existing.total != request.total
            || existing.pin != request.pin
            || existing.selection.model != request.model
            || existing.selection.config_sha256 != request.config_sha256
            || existing.selection.policy_version != "fresh-account-effects-v2"
            || !existing
                .selection
                .eligible_accounts
                .contains(&existing.selection.account)
            || candidates.get(existing.selection.index).is_none_or(|c| {
                c.account != existing.selection.account
                    || c.plan_sha256 != existing.selection.plan_sha256
            })
        {
            return Err(io::Error::other("fresh route selection changed"));
        }
        return Ok(existing.selection);
    }
    let mut eligible = Vec::new();
    let mut unknown_artifact = None;
    for candidate in candidates {
        let (quota_remaining, unknown) = candidate_quota(directory, binding, &candidate)?;
        if unknown.is_some()
            && request
                .pin
                .as_deref()
                .is_none_or(|pin| pin == candidate.account)
        {
            unknown_artifact = unknown;
        }
        let Some(quota_remaining) = quota_remaining else {
            continue;
        };
        eligible.push((candidate, quota_remaining));
    }
    let all_metered = eligible
        .iter()
        .all(|(candidate, _)| candidate.quota_script.is_some());
    let eligible_accounts: Vec<String> = eligible
        .iter()
        .map(|(candidate, _)| candidate.account.clone())
        .collect();
    let observations: Vec<_> = eligible
        .into_iter()
        .map(|(candidate, quota_remaining)| {
            let evidence = route_evidence(directory, &candidate)?;
            Ok((candidate, quota_remaining, evidence))
        })
        .collect::<io::Result<_>>()?;
    let has_unsuppressed = observations.iter().any(|(candidate, _, (_, failures, _))| {
        request
            .pin
            .as_deref()
            .is_none_or(|pin| pin == candidate.account)
            && *failures < 3
    });
    let mut best: Option<((u64, u32, u64, usize), FreshRouteSelection)> = None;
    for (candidate, quota_remaining, (live, failures, invocations)) in observations {
        if request
            .pin
            .as_deref()
            .is_some_and(|pin| pin != candidate.account)
            || !recent_failure_admitted(failures, has_unsuppressed, request.pin.is_some())
        {
            continue;
        }
        let score = (
            live,
            if all_metered && has_unsuppressed {
                u32::MAX - quota_remaining
            } else {
                0
            },
            invocations.saturating_add(if !all_metered && has_unsuppressed {
                failures.saturating_mul(10)
            } else {
                0
            }),
            candidate.index,
        );
        let selection = FreshRouteSelection {
            model: candidate.model,
            config_sha256: candidate.config_sha256,
            account: candidate.account,
            index: candidate.index,
            plan_sha256: candidate.plan_sha256,
            observed_live: live,
            observed_failures: failures,
            observed_invocations: invocations,
            policy_version: "fresh-account-effects-v2".into(),
            eligible_accounts: eligible_accounts.clone(),
            quota_remaining_basis_points: candidate.quota_script.as_ref().map(|_| quota_remaining),
        };
        if best.as_ref().is_none_or(|(old, _)| score < *old) {
            best = Some((score, selection));
        }
    }
    let selection = best
        .ok_or_else(|| {
            io::Error::other(match unknown_artifact {
                Some(artifact) => format!("fresh quota effect unknown: {artifact}"),
                None => "fresh route has no eligible account or pin".into(),
            })
        })?
        .1;
    durable_new(
        directory,
        &name,
        &RouteDecision {
            version: 1,
            binding: binding.clone(),
            total: request.total,
            pin: request.pin.clone(),
            selection: selection.clone(),
        },
    )?;
    Ok(selection)
}

pub(super) fn require_selected_plan(
    directory: &Path,
    binding: &Binding,
    plan: &Plan,
) -> io::Result<()> {
    let decision: RouteDecision = exact_file(directory, &decision_name(&binding.handoff_id))?
        .ok_or_else(|| io::Error::other("fresh route selection absent before K"))?;
    if decision.version != 1
        || decision.binding != *binding
        || decision.selection.plan_sha256 != plan.digest
    {
        return Err(io::Error::other(
            "fresh provider K differs from selected route",
        ));
    }
    // Selection and K are separate requests. A delayed K must not spend a
    // choice whose Q-verified quota evidence has expired in the meantime.
    let candidate: RouteCandidate = exact_file(
        directory,
        &candidate_name(&binding.handoff_id, decision.selection.index),
    )?
    .ok_or_else(|| io::Error::other("fresh provider K candidate absent"))?;
    if candidate.binding != *binding
        || candidate.account != decision.selection.account
        || candidate.plan_sha256 != plan.digest
    {
        return Err(io::Error::other("fresh provider K candidate changed"));
    }
    let (remaining, unknown) = candidate_quota(directory, binding, &candidate)?;
    if unknown.is_some()
        || remaining.is_none()
        || candidate.quota_script.as_ref().map(|_| remaining.unwrap())
            != decision.selection.quota_remaining_basis_points
    {
        return Err(io::Error::other(
            "fresh provider K quota evidence no longer eligible",
        ));
    }
    Ok(())
}

pub(super) fn prepare(directory: &Path, binding: Binding, plan: Plan) -> io::Result<Prepared> {
    for id in [
        &binding.root_id,
        &binding.handoff_id,
        &binding.invocation_uuid,
        &binding.owner_generation,
    ] {
        uuid::Uuid::parse_str(id).map_err(|_| io::Error::other("invalid fresh grant binding"))?;
    }
    if !binding.session_id.starts_with("v30:")
        || uuid::Uuid::parse_str(&binding.actor_boot_id).is_err()
        || binding.actor_pid <= 0
        || binding.root_pid <= 0
        || binding.actor_starttime == 0
        || binding.root_starttime == 0
    {
        return Err(io::Error::other("invalid fresh grant session or actor"));
    }
    let name = format!("{}.fresh-grant.json", binding.handoff_id);
    let path = directory.join(&name);
    let grant = if path.exists() {
        let old: Grant = serde_json::from_reader(File::open(&path)?)?;
        if old.version != 1 || old.binding != binding || old.plan_sha256 != plan.digest {
            return Err(io::Error::other(
                "fresh provider grant binding or plan changed",
            ));
        }
        old
    } else {
        let grant = Grant {
            version: 1,
            id: uuid::Uuid::new_v4().to_string(),
            binding,
            plan_sha256: plan.digest.clone(),
        };
        durable_new(directory, &name, &grant)?;
        grant
    };
    if directory
        .join(format!("{}.consumed.json", grant.id))
        .exists()
    {
        return Err(io::Error::other(
            "fresh provider K already consumed; observe exact grant",
        ));
    }
    Ok(Prepared {
        grant,
        plan,
        directory: directory.to_owned(),
    })
}

pub(super) fn grant_for_binding(directory: &Path, binding: &Binding) -> io::Result<Option<String>> {
    let name = format!("{}.fresh-grant.json", binding.handoff_id);
    let Some(grant): Option<Grant> = exact_file(directory, &name)? else {
        return Ok(None);
    };
    if grant.version != 1 || grant.binding != *binding {
        return Err(io::Error::other(
            "fresh provider grant readback binding changed",
        ));
    }
    Ok(Some(grant.id))
}

/// Resolve an uncertain K only against the exact pinned plan submitted by
/// this caller. A D-bound grant for a different recipe is not a retry result.
pub(super) fn grant_for_matching_plan(
    directory: &Path,
    binding: &Binding,
    plan: &Plan,
) -> io::Result<String> {
    let name = format!("{}.fresh-grant.json", binding.handoff_id);
    let grant: Grant = exact_file(directory, &name)?
        .ok_or_else(|| io::Error::other("fresh provider grant absent"))?;
    if grant.version != 1 || grant.binding != *binding || grant.plan_sha256 != plan.digest {
        return Err(io::Error::other("fresh provider K readback plan mismatch"));
    }
    Ok(grant.id)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Attach {
    version: u32,
    grant_id: String,
    work_id: String,
    pid1: i32,
    pid1_starttime: u64,
    pidns_dev: u64,
    pidns_ino: u64,
    pid1_parent_namespace_pid: i32,
    provider_pid: i32,
    provider_starttime: u64,
    provider_local_pid: i32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderExit {
    version: u32,
    grant_id: String,
    work_id: String,
    provider_local_pid: i32,
    wait_status: i32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Output {
    bytes: u64,
    sha256: String,
    device: u64,
    inode: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Drain {
    version: u32,
    grant_id: String,
    work_id: String,
    stdout: Output,
    stderr: Output,
    cancelled: bool,
    zero_remaining: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pid1Wait {
    version: u32,
    grant_id: String,
    work_id: String,
    pid1_parent_namespace_pid: i32,
    wait_status: i32,
    reaped: bool,
}

fn output(file: &File) -> io::Result<Output> {
    file.sync_all()?;
    let (sha256, bytes) = sha_file(file)?;
    let meta = file.metadata()?;
    Ok(Output {
        bytes,
        sha256,
        device: meta.dev(),
        inode: meta.ino(),
    })
}

struct Init {
    plan: Plan,
    dir: File,
    grant_id: String,
    work_id: String,
    stdout: File,
    stderr: File,
    control: UnixStream,
    gate: UnixStream,
    uid: u32,
    gid: u32,
    groups: Vec<libc::gid_t>,
}
extern "C" fn init_start(ptr: *mut libc::c_void) -> libc::c_int {
    let init = unsafe { Box::from_raw(ptr.cast::<Init>()) };
    if run_init(*init).is_ok() { 0 } else { 70 }
}

fn run_init(mut init: Init) -> io::Result<()> {
    work_launch::close_other_descriptors(&[
        init.plan.image.as_raw_fd(),
        init.plan.cwd.as_raw_fd(),
        init.plan.input.as_raw_fd(),
        init.plan.recipe.as_raw_fd(),
        init.dir.as_raw_fd(),
        init.stdout.as_raw_fd(),
        init.stderr.as_raw_fd(),
        init.control.as_raw_fd(),
        init.gate.as_raw_fd(),
    ])?;
    if unsafe { libc::getpid() } != 1
        || unsafe { libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) } != 0
        || unsafe { libc::prctl(libc::PR_GET_SECCOMP, 0, 0, 0, 0) } != 0
    {
        return Err(io::Error::other(
            "fresh provider PID1 lost host sudo semantics",
        ));
    }
    CANCEL.store(false, Ordering::Relaxed);
    if unsafe { libc::signal(libc::SIGUSR1, request_cancel as libc::sighandler_t) } == libc::SIG_ERR
    {
        return Err(io::Error::last_os_error());
    }
    init.control.write_all(b"I")?;
    let mut release = [0u8; 1];
    init.control.read_exact(&mut release)?;
    if release != [b'P'] {
        return Err(io::Error::other(
            "fresh provider PID1 persistence gate refused",
        ));
    }
    let recipe: Recipe = serde_json::from_reader(&init.plan.recipe)?;
    let image = format!("/proc/self/fd/{}", init.plan.image.as_raw_fd());
    let mut command = Command::new(image);
    command.args(&recipe.argv).env_clear();
    for (k, v) in recipe.env {
        command.env(k, v);
    }
    init.plan.input.seek(SeekFrom::Start(0))?;
    command
        .stdin(Stdio::from(init.plan.input.try_clone()?))
        .stdout(Stdio::from(init.stdout.try_clone()?))
        .stderr(Stdio::from(init.stderr.try_clone()?));
    let control_fd = init.control.as_raw_fd();
    let gate_fd = init.gate.as_raw_fd();
    let cwd_fd = init.plan.cwd.as_raw_fd();
    let uid = init.uid;
    let gid = init.gid;
    let groups = init.groups;
    let fixture = super::private_fixture();
    unsafe {
        command.pre_exec(move || {
            if libc::fchdir(cwd_fd) != 0
                || libc::setsid() < 0
                || (!fixture && libc::setgroups(groups.len(), groups.as_ptr()) != 0)
                || libc::setresgid(gid, gid, gid) != 0
                || libc::setresuid(uid, uid, uid) != 0
            {
                return Err(io::Error::last_os_error());
            }
            if libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) != 0
                || libc::prctl(libc::PR_GET_SECCOMP, 0, 0, 0, 0) != 0
            {
                return Err(io::Error::other("fresh provider inherited NNP/seccomp"));
            }
            if libc::send(control_fd, b"C".as_ptr().cast(), 1, libc::MSG_NOSIGNAL) != 1 {
                return Err(io::Error::last_os_error());
            }
            let mut byte = 0u8;
            if libc::read(gate_fd, (&mut byte as *mut u8).cast(), 1) != 1 || byte != b'R' {
                return Err(io::Error::other("fresh provider pre-exec gate refused"));
            }
            Ok(())
        });
    }
    let provider = command.spawn()?;
    drop(init.gate);
    let provider_local_pid = provider.id() as i32;
    let mut provider_wait = None;
    let mut cancellation_started = None;
    loop {
        if CANCEL.load(Ordering::Relaxed) {
            let started = *cancellation_started.get_or_insert_with(Instant::now);
            work_launch::signal_work_members(if started.elapsed() >= Duration::from_secs(2) {
                libc::SIGKILL
            } else {
                libc::SIGTERM
            })?;
        }
        let mut status = 0;
        let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
        if pid == provider_local_pid {
            provider_wait = Some(status);
            durable_new(
                &PathBuf::from(format!("/proc/self/fd/{}", init.dir.as_raw_fd())),
                &format!("{}.exit.json", init.grant_id),
                &ProviderExit {
                    version: 1,
                    grant_id: init.grant_id.clone(),
                    work_id: init.work_id.clone(),
                    provider_local_pid,
                    wait_status: status,
                },
            )?;
        }
        if pid > 0 {
            continue;
        }
        if pid == 0 {
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
    if provider_wait.is_none() {
        return Err(io::Error::other("provider wait missing"));
    }
    let drain = Drain {
        version: 1,
        grant_id: init.grant_id.clone(),
        work_id: init.work_id.clone(),
        stdout: output(&init.stdout)?,
        stderr: output(&init.stderr)?,
        cancelled: cancellation_started.is_some(),
        zero_remaining: true,
    };
    durable_new(
        &PathBuf::from(format!("/proc/self/fd/{}", init.dir.as_raw_fd())),
        &format!("{}.drain.json", init.grant_id),
        &drain,
    )
}

fn create_init(parent_ns: &File, init: Init) -> io::Result<(i32, UnixStream, UnixStream)> {
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
    let init = Init {
        control: init_control,
        gate: init_gate,
        ..init
    };
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(io::Error::last_os_error());
    }
    if pid == 0 {
        drop(broker_control);
        drop(broker_gate);
        if unsafe { libc::setns(parent_ns.as_raw_fd(), libc::CLONE_NEWPID) } != 0 {
            unsafe { libc::_exit(70) };
        }
        let entered = unsafe { libc::fork() };
        if entered < 0 {
            unsafe { libc::_exit(70) };
        }
        if entered > 0 {
            unsafe { libc::_exit(0) };
        }
        let ptr = Box::into_raw(Box::new(init));
        let mut stack = vec![0u8; 1024 * 1024];
        let top = unsafe { stack.as_mut_ptr().add(stack.len()) };
        let child = unsafe {
            libc::clone(
                init_start,
                top.cast(),
                libc::CLONE_NEWPID | libc::SIGCHLD,
                ptr.cast(),
            )
        };
        let context = unsafe { Box::from_raw(ptr) };
        if child < 0 {
            unsafe { libc::_exit(70) };
        }
        if work_launch::close_other_descriptors(&[context.dir.as_raw_fd()]).is_err() {
            unsafe { libc::_exit(70) };
        }
        let mut status = 0;
        let waited = unsafe { libc::waitpid(child, &mut status, 0) };
        let path = PathBuf::from(format!("/proc/self/fd/{}", context.dir.as_raw_fd()));
        let okay = waited == child
            && durable_new(
                &path,
                &format!("{}.pid1-wait.json", context.grant_id),
                &Pid1Wait {
                    version: 1,
                    grant_id: context.grant_id,
                    work_id: context.work_id,
                    pid1_parent_namespace_pid: child,
                    wait_status: status,
                    reaped: true,
                },
            )
            .is_ok();
        unsafe { libc::_exit(if okay { 0 } else { 70 }) };
    }
    drop(init);
    let mut status = 0;
    if unsafe { libc::waitpid(pid, &mut status, 0) } != pid
        || !libc::WIFEXITED(status)
        || libc::WEXITSTATUS(status) != 0
    {
        return Err(io::Error::other("fresh provider namespace helper failed"));
    }
    let cred = work_launch::child_credential(&broker_control, b'I')?;
    if cred.uid != 0 || cred.pid <= 0 {
        return Err(io::Error::other("fresh provider PID1 identity refused"));
    }
    Ok((cred.pid, broker_control, broker_gate))
}

/// The fsynced `.consumed.json` is K. No error after this point authorizes a
/// second launch; observe the same grant or retain unknown debt.
pub(super) fn launch(
    prepared: Prepared,
    root: &PinnedProcess,
    actor: &PinnedProcess,
    uid: u32,
    gid: u32,
) -> io::Result<String> {
    root.verify()?;
    actor.verify()?;
    let b = &prepared.grant.binding;
    if root.host_pid != b.root_pid
        || root.starttime_ticks != b.root_starttime
        || (root.pidns_dev, root.pidns_ino) != (b.root_pidns_dev, b.root_pidns_ino)
        || actor.host_pid != b.actor_pid
        || actor.starttime_ticks != b.actor_starttime
        || actor.boot_id != b.actor_boot_id
        || (actor.pidns_dev, actor.pidns_ino) != (b.actor_pidns_dev, b.actor_pidns_ino)
        || !root.is_namespace_init()?
        || !actor.direct_child_of(root)?
    {
        return Err(io::Error::other("fresh provider K root or actor changed"));
    }
    if unsafe { libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) } != 0
        || unsafe { libc::prctl(libc::PR_GET_SECCOMP, 0, 0, 0, 0) } != 0
    {
        return Err(io::Error::other("fresh provider K inherited NNP/seccomp"));
    }
    prepared.plan.verify()?;
    if prepared.plan.digest != prepared.grant.plan_sha256 {
        return Err(io::Error::other("fresh provider plan changed before K"));
    }
    durable_new(
        &prepared.directory,
        &format!("{}.consumed.json", prepared.grant.id),
        &prepared.grant,
    )?;
    let stdout = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(
            prepared
                .directory
                .join(format!("{}.stdout", prepared.grant.id)),
        )?;
    let stderr = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(
            prepared
                .directory
                .join(format!("{}.stderr", prepared.grant.id)),
        )?;
    let placeholder = UnixStream::pair()?;
    let work_id = uuid::Uuid::new_v4().to_string();
    let init = Init {
        plan: prepared.plan,
        dir: File::open(&prepared.directory)?,
        grant_id: prepared.grant.id.clone(),
        work_id: work_id.clone(),
        stdout,
        stderr,
        control: placeholder.0,
        gate: placeholder.1,
        uid,
        gid,
        groups: actor.supplementary_groups()?,
    };
    let (pid, mut control, mut gate) = create_init(root.namespace(), init)?;
    let init_pin = PinnedProcess::open(pid)?;
    if !init_pin.is_namespace_init()? {
        return Err(io::Error::other("fresh provider PID1 not namespace init"));
    }
    control.write_all(b"P")?;
    let cred = work_launch::child_credential(&control, b'C')?;
    let provider = PinnedProcess::open(cred.pid)?;
    if cred.uid != uid
        || cred.gid != gid
        || !provider.direct_child_of(&init_pin)?
        || !provider.in_namespace(init_pin.namespace())?
    {
        return Err(io::Error::other(
            "fresh provider held exec identity changed",
        ));
    }
    let attach = Attach {
        version: 1,
        grant_id: prepared.grant.id.clone(),
        work_id,
        pid1: pid,
        pid1_starttime: init_pin.starttime_ticks,
        pidns_dev: init_pin.pidns_dev,
        pidns_ino: init_pin.pidns_ino,
        pid1_parent_namespace_pid: parent_namespace_pid(pid)?,
        provider_pid: cred.pid,
        provider_starttime: provider.starttime_ticks,
        provider_local_pid: namespace_pids(cred.pid)?
            .last()
            .copied()
            .ok_or_else(|| io::Error::other("provider namespace PID absent"))?,
    };
    durable_new(
        &prepared.directory,
        &format!("{}.attach.json", prepared.grant.id),
        &attach,
    )?;
    init_pin.verify()?;
    provider.verify()?;
    gate.write_all(b"R")?;
    Ok(prepared.grant.id)
}

#[derive(Debug)]
pub(super) enum Observation {
    Unknown,
    Pending,
    ProviderExited(i32),
    Drained {
        status: i32,
        stdout: File,
        stderr: File,
        stdout_len: u64,
        stderr_len: u64,
        stdout_sha256: String,
        stderr_sha256: String,
        cancelled: bool,
    },
}

fn namespace_pids(host_pid: i32) -> io::Result<Vec<i32>> {
    let mut status = String::new();
    host_proc_file(&format!("{host_pid}/status"))?.read_to_string(&mut status)?;
    let value = status
        .lines()
        .find_map(|line| line.strip_prefix("NSpid:"))
        .ok_or_else(|| io::Error::other("process NSpid mapping absent"))?;
    let pids = value
        .split_ascii_whitespace()
        .map(|v| v.parse::<i32>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(io::Error::other)?;
    if pids.is_empty() || pids.iter().any(|pid| *pid <= 0) {
        return Err(io::Error::other("process NSpid mapping invalid"));
    }
    Ok(pids)
}

fn parent_namespace_pid(host_pid: i32) -> io::Result<i32> {
    let pids = namespace_pids(host_pid)?;
    pids.get(
        pids.len()
            .checked_sub(2)
            .ok_or_else(|| io::Error::other("PID1 has no parent namespace"))?,
    )
    .copied()
    .ok_or_else(|| io::Error::other("PID1 parent namespace PID absent"))
}

fn exact_file<T: for<'de> Deserialize<'de>>(dir: &Path, name: &str) -> io::Result<Option<T>> {
    match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(dir.join(name))
    {
        Ok(file) if file.metadata()?.is_file() => Ok(Some(serde_json::from_reader(file)?)),
        Ok(_) => Err(io::Error::other("fresh provider receipt is not regular")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

fn verified_output(dir: &Path, name: &str, expected: &Output) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(dir.join(name))?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.dev() != expected.device || meta.ino() != expected.inode {
        return Err(io::Error::other("provider output inode changed"));
    }
    let mut hash = Sha256::new();
    let mut offset = 0u64;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read_at(&mut buf, offset)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
        offset = offset
            .checked_add(n as u64)
            .ok_or_else(|| io::Error::other("output overflow"))?;
    }
    if offset != expected.bytes
        || format!("{:x}", hash.finalize()) != expected.sha256
        || file.metadata()?.len() != offset
    {
        return Err(io::Error::other("provider output incomplete or changed"));
    }
    Ok(file)
}

pub(super) fn observe(dir: &Path, grant_id: &str) -> io::Result<Observation> {
    let consumed: Option<Grant> = exact_file(dir, &format!("{grant_id}.consumed.json"))?;
    let Some(grant) = consumed else {
        return Ok(Observation::Unknown);
    };
    if grant.id != grant_id {
        return Ok(Observation::Unknown);
    }
    let attach: Option<Attach> = exact_file(dir, &format!("{grant_id}.attach.json"))?;
    let Some(attach) = attach else {
        return Ok(Observation::Unknown);
    };
    if attach.grant_id != grant_id {
        return Ok(Observation::Unknown);
    }
    let exit: Option<ProviderExit> = exact_file(dir, &format!("{grant_id}.exit.json"))?;
    let drain: Option<Drain> = exact_file(dir, &format!("{grant_id}.drain.json"))?;
    if let Some(drain) = drain {
        let wait: Option<Pid1Wait> = exact_file(dir, &format!("{grant_id}.pid1-wait.json"))?;
        let Some(wait) = wait else {
            return Ok(Observation::Pending);
        };
        if drain.grant_id != grant_id
            || drain.work_id != attach.work_id
            || !drain.zero_remaining
            || wait.grant_id != grant_id
            || wait.work_id != attach.work_id
            || wait.pid1_parent_namespace_pid != attach.pid1_parent_namespace_pid
            || !wait.reaped
            || !libc::WIFEXITED(wait.wait_status)
            || libc::WEXITSTATUS(wait.wait_status) != 0
            || !observed_incarnation_gone(
                attach.pid1,
                &grant.binding.actor_boot_id,
                attach.pid1_starttime,
                (attach.pidns_dev, attach.pidns_ino),
            )?
        {
            return Ok(Observation::Unknown);
        }
        let Some(exit) = exit else {
            return Ok(Observation::Unknown);
        };
        if exit.grant_id != grant_id || exit.work_id != attach.work_id {
            return Ok(Observation::Unknown);
        }
        if exit.provider_local_pid != attach.provider_local_pid {
            return Ok(Observation::Unknown);
        }
        let stdout = verified_output(dir, &format!("{grant_id}.stdout"), &drain.stdout)?;
        let stderr = verified_output(dir, &format!("{grant_id}.stderr"), &drain.stderr)?;
        return Ok(Observation::Drained {
            status: exit.wait_status,
            stdout,
            stderr,
            stdout_len: drain.stdout.bytes,
            stderr_len: drain.stderr.bytes,
            stdout_sha256: drain.stdout.sha256,
            stderr_sha256: drain.stderr.sha256,
            cancelled: drain.cancelled,
        });
    }
    if let Some(exit) = exit {
        if exit.grant_id == grant_id
            && exit.work_id == attach.work_id
            && !observed_incarnation_gone(
                attach.pid1,
                &grant.binding.actor_boot_id,
                attach.pid1_starttime,
                (attach.pidns_dev, attach.pidns_ino),
            )?
        {
            return Ok(Observation::ProviderExited(exit.wait_status));
        }
        return Ok(Observation::Unknown);
    }
    if observed_incarnation_gone(
        attach.pid1,
        &grant.binding.actor_boot_id,
        attach.pid1_starttime,
        (attach.pidns_dev, attach.pidns_ino),
    )? {
        Ok(Observation::Unknown)
    } else {
        Ok(Observation::Pending)
    }
}

pub(super) fn cancel(dir: &Path, grant_id: &str) -> io::Result<()> {
    let Some(attach): Option<Attach> = exact_file(dir, &format!("{grant_id}.attach.json"))? else {
        return Err(io::Error::other("fresh provider attach absent"));
    };
    let Some(grant): Option<Grant> = exact_file(dir, &format!("{grant_id}.consumed.json"))? else {
        return Err(io::Error::other("fresh provider K absent"));
    };
    let pid1 = PinnedProcess::open(attach.pid1)?;
    if attach.grant_id != grant_id
        || grant.id != grant_id
        || pid1.starttime_ticks != attach.pid1_starttime
        || (pid1.pidns_dev, pid1.pidns_ino) != (attach.pidns_dev, attach.pidns_ino)
    {
        return Err(io::Error::other(
            "fresh provider cancellation identity changed",
        ));
    }
    let intent = serde_json::json!({ "grant_id": grant_id, "work_id": attach.work_id });
    let name = format!("{grant_id}.cancel.json");
    if dir.join(&name).exists() {
        let old: serde_json::Value = serde_json::from_reader(File::open(dir.join(&name))?)?;
        if old != intent {
            return Err(io::Error::other(
                "fresh provider cancellation intent changed",
            ));
        }
    } else {
        durable_new(dir, &name, &intent)?;
    }
    pid1.signal(libc::SIGUSR1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn recent_failure_window_threshold_fallback_and_pin() {
        let temp = tempfile::tempdir().unwrap();
        let receipt = temp.path().join("drain.json");
        let file = File::create(&receipt).unwrap();
        assert!(file_age_less_than(&receipt, Duration::from_secs(30 * 60)).unwrap());
        file.set_times(
            std::fs::FileTimes::new()
                .set_modified(std::time::SystemTime::now() - Duration::from_secs(31 * 60)),
        )
        .unwrap();
        assert!(!file_age_less_than(&receipt, Duration::from_secs(30 * 60)).unwrap());
        assert!(recent_failure_admitted(2, true, false));
        assert!(!recent_failure_admitted(3, true, false));
        assert!(recent_failure_admitted(3, false, false));
        assert!(recent_failure_admitted(3, true, true));
    }

    #[test]
    fn auth_admission_serializes_same_account_across_threads() {
        let temp = tempfile::tempdir().unwrap();
        let first = auth_admission_lock(temp.path(), "same-account").unwrap();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (acquired_tx, acquired_rx) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                ready_tx.send(()).unwrap();
                let _second = auth_admission_lock(temp.path(), "same-account").unwrap();
                acquired_tx.send(()).unwrap();
            });
            ready_rx.recv().unwrap();
            assert!(acquired_rx.try_recv().is_err());
            drop(first);
            acquired_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        });
    }

    #[test]
    fn broker_source_rejects_roster_effect_and_digest_forgery_or_edit() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("models")).unwrap();
        std::fs::write(
            temp.path().join("providers.toml"),
            "[first]\ncommand = \"/bin/true\"\nquota_script = \"printf ok\"\n[second]\ncommand = \"/bin/true\"\n",
        )
        .unwrap();
        let model_path = temp.path().join("models/pool.toml");
        std::fs::write(
            &model_path,
            "[[providers]]\nname = \"first\"\n[[providers]]\nname = \"second\"\n",
        )
        .unwrap();
        let source = File::open(temp.path()).unwrap();
        let pool = oulipoly_runtime::executor::cli::fresh_remote::load_fresh_headless_pool(
            temp.path(),
            "pool",
        )
        .unwrap();
        let mut request = FreshRouteRequest {
            d_key: uuid::Uuid::new_v4().to_string(),
            model: "pool".into(),
            config_sha256: pool.config_sha256.clone(),
            account: Some("first".into()),
            index: Some(0),
            total: 2,
            pin: None,
            quota_script: Some("printf ok".into()),
            auth_refresh_command: None,
        };
        validate_route_source(&source, &request).unwrap();
        request.account = Some("second".into());
        assert!(validate_route_source(&source, &request).is_err());
        request.account = Some("first".into());
        request.index = Some(2);
        assert!(validate_route_source(&source, &request).is_err());
        request.index = Some(0);
        request.quota_script = Some("printf forged".into());
        assert!(validate_route_source(&source, &request).is_err());
        request.quota_script = Some("printf ok".into());
        request.config_sha256 = "0".repeat(64);
        assert!(validate_route_source(&source, &request).is_err());
        request.config_sha256 = pool.config_sha256;
        request.account = None;
        request.index = None;
        request.quota_script = None;
        validate_route_source(&source, &request).unwrap();
        let observer = PinnedProcess::open(unsafe { libc::getpid() }).unwrap();
        let binding = fixture_binding(&observer, &observer);
        let broker_dir = temp.path().join("broker");
        std::fs::create_dir(&broker_dir).unwrap();
        bind_route_source(&broker_dir, &binding, &request, &source, true).unwrap();
        bind_route_source(&broker_dir, &binding, &request, &source, false).unwrap();
        let duplicate_dir = temp.path().join("duplicate");
        std::fs::create_dir_all(duplicate_dir.join("models")).unwrap();
        std::fs::copy(
            temp.path().join("providers.toml"),
            duplicate_dir.join("providers.toml"),
        )
        .unwrap();
        std::fs::copy(&model_path, duplicate_dir.join("models/pool.toml")).unwrap();
        let duplicate = File::open(&duplicate_dir).unwrap();
        validate_route_source(&duplicate, &request).unwrap();
        assert!(bind_route_source(&broker_dir, &binding, &request, &duplicate, false).is_err());
        std::fs::write(
            &model_path,
            "[[providers]]\nname = \"second\"\n[[providers]]\nname = \"first\"\n",
        )
        .unwrap();
        assert!(validate_route_source(&source, &request).is_err());
    }

    fn fixture_binding(root: &PinnedProcess, actor: &PinnedProcess) -> Binding {
        Binding {
            root_id: uuid::Uuid::new_v4().to_string(),
            handoff_id: uuid::Uuid::new_v4().to_string(),
            invocation_uuid: uuid::Uuid::new_v4().to_string(),
            session_id: format!("v30:{}:{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4()),
            owner_generation: uuid::Uuid::new_v4().to_string(),
            actor_pid: actor.host_pid,
            actor_starttime: actor.starttime_ticks,
            actor_boot_id: actor.boot_id.clone(),
            actor_pidns_dev: actor.pidns_dev,
            actor_pidns_ino: actor.pidns_ino,
            root_pid: root.host_pid,
            root_starttime: root.starttime_ticks,
            root_pidns_dev: root.pidns_dev,
            root_pidns_ino: root.pidns_ino,
        }
    }

    #[test]
    fn quota_window_freshness_and_exhaustion_are_distinct() {
        let now = Utc::now().timestamp();
        let mut result = FreshAccountEffectReadback {
            effect_id: uuid::Uuid::new_v4().to_string(),
            state: "drained".into(),
            outcome: Some("valid_windows".into()),
            windows: vec![FreshQuotaWindow {
                used_percent: 25.0,
                resets_at: "2099-01-01T00:00:00Z".into(),
            }],
            completed_unix_seconds: Some(now),
            artifact: "/fresh/effect".into(),
        };
        assert_eq!(quota_remaining(&result, now).unwrap(), Some(7500));
        assert_eq!(quota_remaining(&result, now + 30).unwrap(), None);
        result.windows[0].used_percent = 100.0;
        assert_eq!(quota_remaining(&result, now).unwrap(), None);
        result.windows[0].used_percent = 0.0;
        result.windows[0].resets_at = "2020-01-01T00:00:00Z".into();
        assert_eq!(quota_remaining(&result, now).unwrap(), None);
    }

    #[test]
    fn direct_pinned_provider_has_distinct_exit_output_and_physical_q() {
        if std::env::var_os("AGE319_FRESH_PROVIDER_INNER").is_none() {
            let Some(image) = std::env::var_os("OULIPOLY_AGE319_PROVIDER_IMAGE") else {
                return;
            };
            let output = Command::new("unshare")
                .args(["-Urpfm", "--mount-proc"])
                .arg(std::env::current_exe().unwrap())
                .args(["--exact", "linux_main::fresh_provider::tests::direct_pinned_provider_has_distinct_exit_output_and_physical_q", "--nocapture"])
                .env("AGE319_FRESH_PROVIDER_INNER", "1")
                .env("OULIPOLY_AGE319_PROVIDER_IMAGE", image)
                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", "/tmp/fresh-provider-fixture-socket")
                .output().unwrap();
            assert!(
                output.status.success(),
                "stdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
            return;
        }
        assert_eq!(unsafe { libc::getpid() }, 1);
        let temporary = tempfile::tempdir().unwrap();
        let marker = temporary.path().join("one-effect");
        let stdin_path = temporary.path().join("stdin");
        let input = vec![b'Q'; 1024 * 1024];
        std::fs::write(&stdin_path, &input).unwrap();
        let mut actor_child = Command::new("sleep").arg("60").spawn().unwrap();
        let root = PinnedProcess::open(unsafe { libc::getpid() }).unwrap();
        let actor = PinnedProcess::open(actor_child.id() as i32).unwrap();
        let binding = fixture_binding(&root, &actor);
        let image = Path::new(&std::env::var("OULIPOLY_AGE319_PROVIDER_IMAGE").unwrap()).to_owned();
        let prepare_plan = |mut args: Vec<String>| {
            if args
                .first()
                .is_some_and(|arg| arg == &marker.display().to_string())
            {
                args.push("--fail".into());
            }
            plan(
                &image,
                temporary.path(),
                &File::open(&stdin_path).unwrap(),
                args,
                vec![("PATH".into(), "/usr/bin:/bin".into())],
            )
            .unwrap()
        };
        let route = |binding: &Binding, pin: Option<&str>| {
            let request = |index: Option<usize>, account: Option<&str>| FreshRouteRequest {
                d_key: uuid::Uuid::new_v4().to_string(),
                model: "configured-model".into(),
                config_sha256: "a".repeat(64),
                account: account.map(str::to_owned),
                index,
                total: 2,
                pin: pin.map(str::to_owned),
                quota_script: None,
                auth_refresh_command: None,
            };
            register_route_candidate(
                temporary.path(),
                binding,
                &request(Some(0), Some("first")),
                prepare_plan(vec![marker.display().to_string()]),
            )
            .unwrap();
            register_route_candidate(
                temporary.path(),
                binding,
                &request(Some(1), Some("second")),
                prepare_plan(vec![
                    temporary.path().join("other-effect").display().to_string(),
                ]),
            )
            .unwrap();
            select_route(temporary.path(), binding, &request(None, None)).unwrap()
        };
        let first_route = route(&binding, None);
        assert_eq!(first_route.account, "first");
        require_selected_plan(
            temporary.path(),
            &binding,
            &prepare_plan(vec![marker.display().to_string()]),
        )
        .unwrap();
        assert!(
            require_selected_plan(
                temporary.path(),
                &binding,
                &prepare_plan(vec!["changed".into()])
            )
            .is_err()
        );
        let prepared = prepare(
            temporary.path(),
            binding.clone(),
            prepare_plan(vec![marker.display().to_string()]),
        )
        .unwrap();
        assert!(!marker.exists(), "pre-consume provider effect");
        let mut wrong_root = binding.clone();
        wrong_root.root_id = uuid::Uuid::new_v4().to_string();
        assert!(
            prepare(
                temporary.path(),
                wrong_root,
                prepare_plan(vec![marker.display().to_string()])
            )
            .is_err()
        );
        assert!(
            prepare(
                temporary.path(),
                binding.clone(),
                prepare_plan(vec!["changed-argv".into()])
            )
            .is_err()
        );
        assert!(
            prepare(
                temporary.path(),
                binding.clone(),
                plan(
                    &std::env::current_exe().unwrap(),
                    temporary.path(),
                    &File::open(&stdin_path).unwrap(),
                    vec![marker.display().to_string()],
                    vec![("PATH".into(), "/usr/bin:/bin".into())]
                )
                .unwrap()
            )
            .is_err(),
            "changed provider image prepared under the same held root"
        );
        let wrong_prepared = prepare(
            temporary.path(),
            binding.clone(),
            prepare_plan(vec![marker.display().to_string()]),
        )
        .unwrap();
        let mut wrong_child = Command::new("sleep").arg("60").spawn().unwrap();
        let wrong_actor = PinnedProcess::open(wrong_child.id() as i32).unwrap();
        assert!(
            launch(wrong_prepared, &root, &wrong_actor, 0, 0).is_err(),
            "wrong actor consumed fresh provider K"
        );
        wrong_child.kill().unwrap();
        wrong_child.wait().unwrap();
        assert!(!marker.exists(), "pre-consume refusal had provider effects");
        let same = prepare(
            temporary.path(),
            binding.clone(),
            prepare_plan(vec![marker.display().to_string()]),
        )
        .unwrap();
        assert_eq!(same.grant.id, prepared.grant.id);
        let id = launch(prepared, &root, &actor, 0, 0).unwrap();
        assert_eq!(id, same.grant.id);
        assert_eq!(
            grant_for_matching_plan(
                temporary.path(),
                &binding,
                &prepare_plan(vec![marker.display().to_string()])
            )
            .unwrap(),
            id
        );
        assert!(
            grant_for_matching_plan(
                temporary.path(),
                &binding,
                &prepare_plan(vec!["changed-after-k".into()])
            )
            .is_err(),
            "changed plan recovered a consumed K"
        );
        assert!(
            launch(same, &root, &actor, 0, 0).is_err(),
            "second K succeeded"
        );
        let deadline = Instant::now() + Duration::from_secs(15);
        while !temporary.path().join(format!("{id}.exit.json")).exists()
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(marker.exists(), "provider effect absent");
        assert!(
            matches!(
                observe(temporary.path(), &id).unwrap(),
                Observation::ProviderExited(_)
            ),
            "provider exit with adopted child was treated as Q"
        );
        let consumed = File::options()
            .write(true)
            .open(temporary.path().join(format!("{id}.consumed.json")))
            .unwrap();
        consumed
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(std::time::SystemTime::now() - Duration::from_secs(61 * 60)),
            )
            .unwrap();
        let first_candidate: RouteCandidate =
            exact_file(temporary.path(), &candidate_name(&binding.handoff_id, 0))
                .unwrap()
                .unwrap();
        assert_eq!(
            route_evidence(temporary.path(), &first_candidate).unwrap(),
            (1, 0, 1),
            "aged consumed K with a live descendant was dropped before Q"
        );
        let mut second_binding = binding.clone();
        second_binding.handoff_id = uuid::Uuid::new_v4().to_string();
        let loaded_route = route(&second_binding, None);
        assert_eq!(
            loaded_route.account, "second",
            "genuine consumed K without Q must count as live"
        );
        assert_eq!(
            route(&binding, None),
            first_route,
            "uncertain K changed the held root's durable choice"
        );
        let mut changed_config = FreshRouteRequest {
            d_key: uuid::Uuid::new_v4().to_string(),
            model: "configured-model".into(),
            config_sha256: "b".repeat(64),
            account: None,
            index: None,
            total: 2,
            pin: None,
            quota_script: None,
            auth_refresh_command: None,
        };
        assert!(
            select_route(temporary.path(), &second_binding, &changed_config).is_err(),
            "config change reselected a held root"
        );
        changed_config.config_sha256 = "a".repeat(64);
        let mut wrong_actor = second_binding.clone();
        wrong_actor.actor_starttime += 1;
        assert!(
            select_route(temporary.path(), &wrong_actor, &changed_config).is_err(),
            "wrong actor read back a route"
        );
        std::thread::sleep(Duration::from_millis(250));
        assert!(!temporary.path().join(format!("{id}.drain.json")).exists());
        cancel(temporary.path(), &id).unwrap();
        cancel(temporary.path(), &id).unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut drained = None;
        while Instant::now() < deadline {
            if let Observation::Drained {
                status,
                stdout,
                stderr,
                stdout_len,
                stderr_len,
                cancelled,
                ..
            } = observe(temporary.path(), &id).unwrap()
            {
                drained = Some((status, stdout, stderr, stdout_len, stderr_len, cancelled));
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let (status, mut stdout, mut stderr, stdout_len, stderr_len, cancelled) =
            drained.expect("physical Q absent");
        assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 9 && cancelled);
        let mut stdout_bytes = Vec::new();
        stdout.read_to_end(&mut stdout_bytes).unwrap();
        let mut stderr_bytes = Vec::new();
        stderr.read_to_end(&mut stderr_bytes).unwrap();
        assert_eq!(stdout_len as usize, stdout_bytes.len());
        assert_eq!(stderr_len as usize, stderr_bytes.len());
        assert_eq!(&stdout_bytes[..16], b"provider-stdout:");
        assert_eq!(&stdout_bytes[16..], input);
        assert_eq!(stderr_bytes, b"provider-stderr\n");
        assert_eq!(std::fs::read(&marker).unwrap(), b"one-provider-effect\n");
        let stdout_path = temporary.path().join(format!("{id}.stdout"));
        let hidden = temporary.path().join("missing-output");
        std::fs::rename(&stdout_path, &hidden).unwrap();
        assert!(
            observe(temporary.path(), &id).is_err(),
            "Q plus provider exit certified missing stdout"
        );
        std::fs::rename(hidden, stdout_path).unwrap();
        let first_candidate: RouteCandidate =
            exact_file(temporary.path(), &candidate_name(&binding.handoff_id, 0))
                .unwrap()
                .unwrap();
        assert_eq!(
            route_evidence(temporary.path(), &first_candidate).unwrap(),
            (0, 1, 1),
            "only physical Q may turn the nonzero exit into failure evidence"
        );
        let mut third_binding = binding.clone();
        third_binding.handoff_id = uuid::Uuid::new_v4().to_string();
        assert_eq!(route(&third_binding, None).account, "second");
        let mut pinned_binding = binding.clone();
        pinned_binding.handoff_id = uuid::Uuid::new_v4().to_string();
        assert_eq!(
            route(&pinned_binding, Some("first")).account,
            "first",
            "explicit pin must retain account identity"
        );
        actor_child.kill().unwrap();
        actor_child.wait().unwrap();
    }

    #[test]
    fn direct_account_effects_are_one_use_and_gate_eligible_set() {
        if std::env::var_os("AGE319_FRESH_ACCOUNT_INNER").is_none() {
            let output = Command::new("unshare")
                .args(["-Urpfm", "--mount-proc"])
                .arg(std::env::current_exe().unwrap())
                .args(["--exact", "linux_main::fresh_provider::tests::direct_account_effects_are_one_use_and_gate_eligible_set", "--nocapture"])
                .env("AGE319_FRESH_ACCOUNT_INNER", "1")
                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", "/tmp/fresh-account-effect-fixture-socket")
                .output().unwrap();
            assert!(
                output.status.success(),
                "stdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
            return;
        }
        let temporary = tempfile::tempdir().unwrap();
        let marker = temporary.path().join("auth-marker");
        let input = temporary.path().join("empty-input");
        std::fs::write(&input, b"").unwrap();
        let mut actor_child = Command::new("sleep").arg("60").spawn().unwrap();
        let root = PinnedProcess::open(unsafe { libc::getpid() }).unwrap();
        let actor = PinnedProcess::open(actor_child.id() as i32).unwrap();
        let binding = fixture_binding(&root, &actor);
        let shell_script = format!(
            "if test -e '{}'; then printf '{{\"used_percent\":20,\"resets_at\":\"2099-01-01T00:00:00Z\"}}'; else exit 7; fi",
            marker.display()
        );
        let auth_script = format!("printf x >> '{}'", marker.display());
        let request =
            |index: usize, account: &str, quota: &str, auth: Option<&str>| FreshRouteRequest {
                d_key: uuid::Uuid::new_v4().to_string(),
                model: "model".into(),
                config_sha256: "a".repeat(64),
                account: Some(account.into()),
                index: Some(index),
                total: 2,
                pin: None,
                quota_script: Some(quota.into()),
                auth_refresh_command: auth.map(str::to_owned),
            };
        let image = std::fs::canonicalize("/bin/true").unwrap();
        for (index, account, quota, auth) in [
            (
                0,
                "recovering",
                shell_script.as_str(),
                Some(auth_script.as_str()),
            ),
            (
                1,
                "exhausted",
                "printf '{\"used_percent\":100,\"resets_at\":\"2099-01-01T00:00:00Z\"}'",
                None,
            ),
        ] {
            register_route_candidate(
                temporary.path(),
                &binding,
                &request(index, account, quota, auth),
                plan(
                    &image,
                    temporary.path(),
                    &File::open(&input).unwrap(),
                    vec![format!("--{account}")],
                    vec![],
                )
                .unwrap(),
            )
            .unwrap();
        }
        assert!(
            select_route(
                temporary.path(),
                &binding,
                &FreshRouteRequest {
                    d_key: uuid::Uuid::new_v4().to_string(),
                    model: "model".into(),
                    config_sha256: "a".repeat(64),
                    account: None,
                    index: None,
                    total: 2,
                    pin: None,
                    quota_script: None,
                    auth_refresh_command: None,
                }
            )
            .unwrap_err()
            .to_string()
            .contains("fresh route has no eligible account or pin"),
            "missing quota evidence was treated as available"
        );
        let effect_d_key = uuid::Uuid::new_v4().to_string();
        let effect = |index: usize, account: &str, kind| FreshAccountEffectRequest {
            d_key: effect_d_key.clone(),
            model: "model".into(),
            config_sha256: "a".repeat(64),
            account: account.into(),
            index,
            kind,
            environment: vec![("PATH".into(), "/usr/bin:/bin".into())],
        };
        let wait = |request: &FreshAccountEffectRequest| {
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                let readback = observe_account_effect(temporary.path(), &binding, request).unwrap();
                if readback.state == "drained" {
                    return readback;
                }
                assert!(
                    Instant::now() < deadline,
                    "effect did not physically drain: {readback:?}"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        let first = effect(0, "recovering", FreshAccountEffectKind::QuotaFirst);
        let begun =
            begin_account_effect(temporary.path(), &binding, &first, &root, &actor, 0, 0).unwrap();
        assert!(
            begin_account_effect(temporary.path(), &binding, &first, &root, &actor, 0, 0).is_err()
        );
        let failed = wait(&first);
        assert_eq!(failed.effect_id, begun.effect_id);
        assert_eq!(failed.outcome.as_deref(), Some("failed"));
        assert!(!marker.exists());
        let auth = effect(0, "recovering", FreshAccountEffectKind::AuthRefresh);
        begin_account_effect(temporary.path(), &binding, &auth, &root, &actor, 0, 0).unwrap();
        assert_eq!(wait(&auth).outcome.as_deref(), Some("refreshed"));
        assert_eq!(std::fs::read(&marker).unwrap(), b"x");
        assert!(
            begin_account_effect(temporary.path(), &binding, &auth, &root, &actor, 0, 0).is_err()
        );
        let retry = effect(0, "recovering", FreshAccountEffectKind::QuotaRetry);
        begin_account_effect(temporary.path(), &binding, &retry, &root, &actor, 0, 0).unwrap();
        assert_eq!(wait(&retry).outcome.as_deref(), Some("valid_windows"));
        let mut sibling_binding = binding.clone();
        sibling_binding.root_id = uuid::Uuid::new_v4().to_string();
        sibling_binding.handoff_id = uuid::Uuid::new_v4().to_string();
        let sibling_first = FreshAccountEffectRequest {
            d_key: uuid::Uuid::new_v4().to_string(),
            ..first.clone()
        };
        register_route_candidate(
            temporary.path(),
            &sibling_binding,
            &request(0, "recovering", &shell_script, Some(&auth_script)),
            plan(
                &image,
                temporary.path(),
                &File::open(&input).unwrap(),
                vec!["--recovering".into()],
                vec![],
            )
            .unwrap(),
        )
        .unwrap();
        let reused = begin_account_effect(
            temporary.path(),
            &sibling_binding,
            &sibling_first,
            &root,
            &actor,
            0,
            0,
        )
        .unwrap();
        assert_eq!(reused.outcome.as_deref(), Some("valid_windows"));
        assert!(
            grant_for_binding(
                &effect_directory(temporary.path(), &sibling_binding, &sibling_first),
                &sibling_binding,
            )
            .unwrap()
            .is_none(),
            "a matching fresh quota Q was rerun for a second root"
        );
        assert_eq!(
            observe_account_effect(temporary.path(), &sibling_binding, &sibling_first)
                .unwrap()
                .effect_id,
            reused.effect_id,
        );
        let negative = effect(1, "exhausted", FreshAccountEffectKind::QuotaFirst);
        begin_account_effect(temporary.path(), &binding, &negative, &root, &actor, 0, 0).unwrap();
        assert_eq!(wait(&negative).outcome.as_deref(), Some("valid_windows"));
        let mut exhausted_sibling = binding.clone();
        exhausted_sibling.handoff_id = uuid::Uuid::new_v4().to_string();
        assert!(
            reusable_quota_source(temporary.path(), &exhausted_sibling, &negative)
                .unwrap()
                .is_none(),
            "an exhausted physical Q was reused instead of allowing a new quota probe"
        );
        assert!(
            select_route(
                temporary.path(),
                &binding,
                &FreshRouteRequest {
                    d_key: uuid::Uuid::new_v4().to_string(),
                    model: "model".into(),
                    config_sha256: "a".repeat(64),
                    account: None,
                    index: None,
                    total: 2,
                    pin: Some("exhausted".into()),
                    quota_script: None,
                    auth_refresh_command: None,
                }
            )
            .is_err(),
            "an exhausted explicit pin was launched"
        );
        let selection = select_route(
            temporary.path(),
            &binding,
            &FreshRouteRequest {
                d_key: uuid::Uuid::new_v4().to_string(),
                model: "model".into(),
                config_sha256: "a".repeat(64),
                account: None,
                index: None,
                total: 2,
                pin: None,
                quota_script: None,
                auth_refresh_command: None,
            },
        )
        .unwrap();
        assert_eq!(selection.account, "recovering");
        assert_eq!(selection.eligible_accounts, ["recovering"]);
        assert_eq!(selection.quota_remaining_basis_points, Some(8000));
        let reloaded = observe_account_effect(temporary.path(), &binding, &auth).unwrap();
        assert_eq!(reloaded.outcome.as_deref(), Some("refreshed"));
        let auth_dir = effect_directory(temporary.path(), &binding, &auth);
        let persisted: AccountEffectIntent = exact_file(&auth_dir, "intent.json").unwrap().unwrap();
        assert_eq!(
            effect_readback_from_dir(&auth_dir, &persisted)
                .unwrap()
                .effect_id,
            reloaded.effect_id,
            "persisted intent could not reconstruct Q readback"
        );
        assert_eq!(std::fs::read(&marker).unwrap(), b"x", "readback reran auth");
        let mut uncertain_binding = binding.clone();
        uncertain_binding.handoff_id = uuid::Uuid::new_v4().to_string();
        let unknown_route = FreshRouteRequest {
            d_key: uuid::Uuid::new_v4().to_string(), model: "model".into(),
            config_sha256: "a".repeat(64), account: Some("slow".into()),
            index: Some(0), total: 1, pin: Some("slow".into()),
            quota_script: Some("printf '{\"used_percent\":10,\"resets_at\":\"2099-01-01T00:00:00Z\"}'; sleep 60 & wait".into()),
            auth_refresh_command: None,
        };
        register_route_candidate(
            temporary.path(),
            &uncertain_binding,
            &unknown_route,
            plan(
                &image,
                temporary.path(),
                &File::open(&input).unwrap(),
                vec!["--slow".into()],
                vec![],
            )
            .unwrap(),
        )
        .unwrap();
        let slow = FreshAccountEffectRequest {
            d_key: effect_d_key.clone(),
            model: "model".into(),
            config_sha256: "a".repeat(64),
            account: "slow".into(),
            index: 0,
            kind: FreshAccountEffectKind::QuotaFirst,
            environment: vec![("PATH".into(), "/usr/bin:/bin".into())],
        };
        begin_account_effect(
            temporary.path(),
            &uncertain_binding,
            &slow,
            &root,
            &actor,
            0,
            0,
        )
        .unwrap();
        let slow_dir = effect_directory(temporary.path(), &uncertain_binding, &slow);
        let slow_grant = grant_for_binding(&slow_dir, &uncertain_binding)
            .unwrap()
            .unwrap();
        let mut concurrent_binding = uncertain_binding.clone();
        concurrent_binding.root_id = uuid::Uuid::new_v4().to_string();
        concurrent_binding.handoff_id = uuid::Uuid::new_v4().to_string();
        let concurrent_slow = FreshAccountEffectRequest {
            d_key: uuid::Uuid::new_v4().to_string(),
            ..slow.clone()
        };
        register_route_candidate(
            temporary.path(),
            &concurrent_binding,
            &unknown_route,
            plan(
                &image,
                temporary.path(),
                &File::open(&input).unwrap(),
                vec!["--slow".into()],
                vec![],
            )
            .unwrap(),
        )
        .unwrap();
        let concurrent = begin_account_effect(
            temporary.path(),
            &concurrent_binding,
            &concurrent_slow,
            &root,
            &actor,
            0,
            0,
        )
        .unwrap();
        assert_ne!(concurrent.state, "drained");
        assert!(
            concurrent
                .artifact
                .contains(&slow_dir.display().to_string())
        );
        assert!(
            grant_for_binding(
                &effect_directory(temporary.path(), &concurrent_binding, &concurrent_slow),
                &concurrent_binding,
            )
            .unwrap()
            .is_none(),
            "concurrent fresh root started a duplicate quota K"
        );
        assert!(
            begin_account_effect(
                temporary.path(),
                &uncertain_binding,
                &slow,
                &root,
                &actor,
                0,
                0
            )
            .is_err(),
            "an uncertain quota K was replayed"
        );
        assert!(
            select_route(
                temporary.path(),
                &uncertain_binding,
                &FreshRouteRequest {
                    account: None,
                    index: None,
                    quota_script: None,
                    auth_refresh_command: None,
                    ..unknown_route
                }
            )
            .unwrap_err()
            .to_string()
            .contains("fresh quota effect unknown"),
            "K without Q was treated as quota availability"
        );
        cancel(&slow_dir, &slow_grant).unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while observe_account_effect(temporary.path(), &uncertain_binding, &slow)
            .unwrap()
            .state
            != "drained"
        {
            assert!(
                Instant::now() < deadline,
                "cancelled quota effect did not drain"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            observe_account_effect(temporary.path(), &concurrent_binding, &concurrent_slow)
                .unwrap()
                .outcome
                .as_deref(),
            Some("failed")
        );
        actor_child.kill().unwrap();
        actor_child.wait().unwrap();
    }
}
