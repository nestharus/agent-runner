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
use oulipoly_runtime::executor::cli::fresh_remote::FreshTerminalRecognizer;
use oulipoly_runtime::executor::terminal_signal::TerminalSignalKind;
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
    // v3 also freezes the source-owned physical account identity.
    version: u32,
    binding: Binding,
    model: String,
    config_sha256: String,
    account: String,
    account_identity: String,
    index: usize,
    total: usize,
    pin: Option<String>,
    plan_sha256: String,
    quota_script: Option<String>,
    auth_refresh_command: Option<String>,
    terminal_recognizer: FreshTerminalRecognizer,
}

pub(super) fn terminal_recognizer_from_source(
    config_dir: &File,
    request: &FreshRouteRequest,
) -> io::Result<FreshTerminalRecognizer> {
    let path = PathBuf::from(format!("/proc/self/fd/{}", config_dir.as_raw_fd()));
    let pool = oulipoly_runtime::executor::cli::fresh_remote::load_fresh_headless_pool(
        &path,
        &request.model,
    )
    .map_err(io::Error::other)?;
    let index = request
        .index
        .ok_or_else(|| io::Error::other("route index absent"))?;
    if pool.config_sha256 != request.config_sha256
        || pool.model.providers.len() != request.total
        || pool
            .model
            .providers
            .get(index)
            .map(|provider| provider.name.as_str())
            != request.account.as_deref()
    {
        return Err(io::Error::other("terminal recognizer source changed"));
    }
    Ok(FreshTerminalRecognizer::for_provider(
        &pool.model.providers[index],
    ))
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct RouteDecision {
    version: u32,
    binding: Binding,
    total: usize,
    pin: Option<String>,
    #[serde(default)]
    sequence: u64,
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
    if request.protocol_version != 4
        || request.d_key.is_empty()
        || request.model.is_empty()
        || request.account.as_deref().is_some_and(str::is_empty)
        || request
            .account_identity
            .as_deref()
            .is_some_and(str::is_empty)
        || request.account.is_some() != request.account_identity.is_some()
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
            || request.account_identity.as_deref() != pool.account_identities[index].as_deref()
            || request.account_identity.is_none()
            || request.quota_script != effect.0
            || request.auth_refresh_command != effect.1
        {
            return Err(io::Error::other(
                "fresh config source account or effect forged",
            ));
        }
    } else if request.account.is_some()
        || request.account_identity.is_some()
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
    #[serde(default)]
    auth_source: Option<AuthReuse>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct QuotaReuse {
    source_directory: String,
    source_effect_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct AuthReuse {
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
    if candidate.version != 3
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

/// The identity is source-owned, and the source command must agree too. A
/// matching label, model name, or member index is never quota authority.
fn same_physical_effect_source(
    directory: &Path,
    target: &RouteCandidate,
    source: &AccountEffectIntent,
) -> io::Result<bool> {
    let prior = effect_candidate(directory, &source.binding, &source.request)?;
    Ok(prior.account_identity == target.account_identity
        && prior.quota_script == target.quota_script
        && prior.auth_refresh_command == target.auth_refresh_command)
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

fn latest_terminal_marker_time(
    directory: &Path,
    candidate: &RouteCandidate,
) -> io::Result<Option<u128>> {
    let mut latest = None;
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if !entry
            .file_name()
            .to_string_lossy()
            .ends_with(".terminal.json")
        {
            continue;
        }
        let record: TerminalRecord = serde_json::from_reader(File::open(entry.path())?)?;
        if record.version != 1 {
            return Err(io::Error::other("fresh terminal ledger version changed"));
        }
        let source: RouteCandidate = exact_file(
            directory,
            &candidate_name(&record.binding.handoff_id, record.selection.index),
        )?
        .ok_or_else(|| io::Error::other("fresh terminal source candidate absent"))?;
        if source.version != 3
            || source.binding != record.binding
            || source.account_identity != record.selection.account_identity
            || source.account != record.selection.account
            || source.plan_sha256 != record.selection.plan_sha256
        {
            return Err(io::Error::other("fresh terminal source identity changed"));
        }
        if record.selection.account_identity == candidate.account_identity
            && record.outcome.is_marker()
        {
            latest = Some(latest.map_or(record.physical_q_unix_nanos, |prior: u128| {
                prior.max(record.physical_q_unix_nanos)
            }));
        }
    }
    Ok(latest)
}

fn reusable_quota_source(
    directory: &Path,
    binding: &Binding,
    request: &FreshAccountEffectRequest,
) -> io::Result<Option<(String, AccountEffectIntent)>> {
    let candidate = effect_candidate(directory, binding, request)?;
    let marker_q = latest_terminal_marker_time(directory, &candidate)?;
    let parent = directory.join("account-effects");
    let mut names = std::fs::read_dir(&parent)?
        .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
        .collect::<io::Result<Vec<_>>>()?;
    names.sort();
    let mut latest = None;
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
            || !same_physical_effect_source(directory, &candidate, &intent)?
            || intent.environment_sha256 != environment_digest(request)?
            || source_dir.join("reuse.json").exists()
        {
            continue;
        }
        let readback = effect_readback_from_dir(&source_dir, &intent)?;
        if readback.state != "drained" && unresolved.is_none() {
            unresolved = Some((name, intent));
        } else if readback.state == "drained" {
            let q = effect_physical_q_nanos(&source_dir, &intent)?;
            if latest.as_ref().is_none_or(|(_, _, prior_q)| q > *prior_q) {
                latest = Some((name, intent, q));
            }
        }
    }
    if unresolved.is_some() {
        return Ok(unresolved);
    }
    let Some((name, intent, q)) = latest else {
        return Ok(None);
    };
    let source_dir = parent.join(&name);
    let readback = effect_readback_from_dir(&source_dir, &intent)?;
    if readback.outcome.as_deref() == Some("valid_windows")
        && quota_read_is_fresh(&readback, Utc::now().timestamp())?
        && marker_q.is_none_or(|marker| q > marker)
    {
        Ok(Some((name, intent)))
    } else {
        Ok(None)
    }
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

fn coalescible_auth_source(
    directory: &Path,
    binding: &Binding,
    request: &FreshAccountEffectRequest,
) -> io::Result<Option<(String, AccountEffectIntent)>> {
    let candidate = effect_candidate(directory, binding, request)?;
    let marker_q = latest_terminal_marker_time(directory, &candidate)?;
    let parent = directory.join("account-effects");
    let entries = match std::fs::read_dir(&parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut names = entries
        .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
        .collect::<io::Result<Vec<_>>>()?;
    names.sort();
    for name in names {
        if !name.ends_with("-auth-refresh") {
            continue;
        }
        let source_dir = parent.join(&name);
        let Some(intent) = effect_intent(&source_dir)? else {
            continue;
        };
        // A follower is a reference to the original K, never another source.
        if intent.auth_source.is_some() {
            continue;
        }
        if intent.binding == *binding
            || !same_physical_effect_source(directory, &candidate, &intent)?
        {
            continue;
        }
        let prior = effect_readback_from_dir(&source_dir, &intent)?;
        if prior.state == "drained" && prior.outcome.as_deref() == Some("refreshed") {
            if let Some(marker_q) = marker_q {
                if effect_physical_q_nanos(&source_dir, &intent)? <= marker_q {
                    continue;
                }
            }
        }
        if prior.state != "drained"
            || prior
                .completed_unix_seconds
                .is_some_and(|completed| Utc::now().timestamp() - completed < 30)
        {
            if intent.version != 1
                || intent.request.kind != FreshAccountEffectKind::AuthRefresh
                || intent.environment_sha256 != environment_digest(request)?
            {
                return Err(io::Error::other(format!(
                    "fresh auth peer provenance differs: effect={}, state={}, artifact={}",
                    prior.effect_id, prior.state, prior.artifact
                )));
            }
            return Ok(Some((name, intent)));
        }
    }
    Ok(None)
}

fn effect_readback_from_dir(
    dir: &Path,
    intent: &AccountEffectIntent,
) -> io::Result<FreshAccountEffectReadback> {
    let artifact = dir.display().to_string();
    let broker_directory = dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| io::Error::other("fresh effect broker directory absent"))?;
    let candidate = effect_candidate(broker_directory, &intent.binding, &intent.request)?;
    if let Some(reuse) = &intent.auth_source {
        if intent.request.kind != FreshAccountEffectKind::AuthRefresh
            || reuse.source_directory.contains('/')
            || reuse.source_directory.contains("..")
            || !reuse.source_directory.ends_with("-auth-refresh")
        {
            return Err(io::Error::other("fresh auth reuse source invalid"));
        }
        let source_dir = dir
            .parent()
            .ok_or_else(|| io::Error::other("fresh auth reuse parent absent"))?
            .join(&reuse.source_directory);
        if source_dir == dir {
            return Err(io::Error::other("fresh auth reuse chain refused"));
        }
        let source = effect_intent(&source_dir)?
            .ok_or_else(|| io::Error::other("fresh auth reuse intent absent"))?;
        if source.version != 1
            || source.auth_source.is_some()
            || source.id != reuse.source_effect_id
            || intent.plan_sha256 != format!("coalesced:{}", source.id)
            || source.binding == intent.binding
            || source.request.kind != FreshAccountEffectKind::AuthRefresh
            || !same_physical_effect_source(broker_directory, &candidate, &source)?
            || source.environment_sha256 != intent.environment_sha256
        {
            return Err(io::Error::other("fresh auth reuse provenance changed"));
        }
        let mut peer = effect_readback_from_dir(&source_dir, &source)?;
        peer.peer_effect_id = Some(peer.effect_id.clone());
        peer.peer_artifact = Some(peer.artifact.clone());
        peer.effect_id = intent.id.clone();
        peer.artifact = artifact;
        return Ok(peer);
    }
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
            || !same_physical_effect_source(broker_directory, &candidate, &source)?
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
        peer_effect_id: None,
        peer_artifact: None,
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
                peer_effect_id: None,
                peer_artifact: None,
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
    #[serde(default)]
    remaining: Option<u64>,
}
#[derive(Deserialize)]
struct RawEffectOutput {
    windows: Option<Vec<RawEffectWindow>>,
    used_percent: Option<f64>,
    resets_at: Option<String>,
    #[serde(default)]
    remaining: Option<u64>,
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
            remaining: value.remaining,
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
                remaining: window.remaining,
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
        Some(auth_admission_lock(directory, &candidate.account_identity)?)
    } else {
        None
    };
    if dir.exists() {
        return Err(io::Error::other(
            "fresh account effect already begun; observe exact effect",
        ));
    }
    let first_request = FreshAccountEffectRequest {
        kind: FreshAccountEffectKind::QuotaFirst,
        ..request.clone()
    };
    if request.kind != FreshAccountEffectKind::QuotaFirst {
        let first = observe_account_effect(directory, binding, &first_request)?;
        // The provider can reject expired credentials while quota still
        // reports healthy. Require its exact typed, physical Q before auth.
        let provider_auth_rejection = if first.outcome.as_deref() == Some("valid_windows") {
            route_evidence(directory, &candidate)?
                .3
                .into_iter()
                .any(|marker| {
                    marker.selection.index == request.index
                        && marker.outcome == TerminalOutcome::AuthRejected
                })
        } else {
            false
        };
        if first.state != "drained"
            || (!matches!(
                first.outcome.as_deref(),
                Some("failed" | "empty" | "invalid")
            ) && !provider_auth_rejection)
        {
            return Err(io::Error::other(
                "auth refresh has no failed quota or typed provider auth Q prerequisite",
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
            if auth.state != "drained" || auth.outcome.as_deref() != Some("refreshed") {
                return Err(io::Error::other(format!(
                    "quota retry has no verified successful auth Q: effect={}, state={}, outcome={:?}, artifact={}, peer_effect={:?}, peer_artifact={:?}",
                    auth.effect_id,
                    auth.state,
                    auth.outcome,
                    auth.artifact,
                    auth.peer_effect_id,
                    auth.peer_artifact
                )));
            }
        }
    }
    if request.kind == FreshAccountEffectKind::AuthRefresh {
        route_evidence(directory, &candidate)?;
        if let Some((source_directory, source)) =
            coalescible_auth_source(directory, binding, request)?
        {
            let parent = directory.join("account-effects");
            std::fs::create_dir(&dir)?;
            File::open(&parent)?.sync_all()?;
            let intent = AccountEffectIntent {
                version: 1,
                id: uuid::Uuid::new_v4().to_string(),
                binding: binding.clone(),
                request: redacted_effect_request(request),
                environment_sha256: environment_digest(request)?,
                plan_sha256: format!("coalesced:{}", source.id),
                auth_source: Some(AuthReuse {
                    source_directory,
                    source_effect_id: source.id,
                }),
            };
            durable_new(&dir, "intent.json", &intent)?;
            return effect_readback_from_dir(&dir, &intent);
        }
    }
    if request.kind == FreshAccountEffectKind::QuotaFirst {
        // Materialize typed Q history before considering a cross-root reuse.
        // A pre-rejection healthy Q must never masquerade as verification.
        route_evidence(directory, &candidate)?;
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
                auth_source: None,
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
        auth_source: None,
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
    terminal_recognizer: FreshTerminalRecognizer,
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
        version: 3,
        binding: binding.clone(),
        model: request.model.clone(),
        config_sha256: request.config_sha256.clone(),
        account: account.clone(),
        account_identity: request
            .account_identity
            .clone()
            .ok_or_else(|| io::Error::other("fresh physical account identity absent"))?,
        index,
        total: request.total,
        pin: request.pin.clone(),
        plan_sha256: plan.digest.clone(),
        quota_script: request.quota_script.clone(),
        auth_refresh_command: request.auth_refresh_command.clone(),
        terminal_recognizer,
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
    let mut identities = HashSet::new();
    for index in 0..request.total {
        let candidate: RouteCandidate =
            exact_file(directory, &candidate_name(&binding.handoff_id, index))?
                .ok_or_else(|| io::Error::other("fresh route candidate set incomplete"))?;
        if candidate.version != 3
            || candidate.binding != *binding
            || candidate.model != request.model
            || candidate.config_sha256 != request.config_sha256
            || candidate.index != index
            || candidate.total != request.total
            || candidate.pin != request.pin
            || request.quota_script.is_some()
            || request.auth_refresh_command.is_some()
            || !names.insert(candidate.account.clone())
            || !identities.insert(candidate.account_identity.clone())
        {
            return Err(io::Error::other("fresh route candidate set changed"));
        }
        candidates.push(candidate);
    }
    Ok(candidates)
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum TerminalOutcome {
    Clean,
    GenericFailure,
    Cancelled,
    Unknown,
    QuotaRejected,
    ModelAtCapacity,
    MaybeQuota,
    AuthRejected,
    ProviderUnavailable,
    RateLimited,
    StorageContention,
}

impl TerminalOutcome {
    fn is_marker(self) -> bool {
        !matches!(
            self,
            Self::Clean
                | Self::GenericFailure
                | Self::Cancelled
                | Self::Unknown
                | Self::ModelAtCapacity
        )
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct TerminalRecord {
    version: u32,
    binding: Binding,
    selection: FreshRouteSelection,
    grant_id: String,
    physical_q_sha256: String,
    physical_q_unix_nanos: u128,
    signal_kind: String,
    outcome: TerminalOutcome,
}

fn file_unix_nanos(path: &Path) -> io::Result<u128> {
    Ok(std::fs::metadata(path)?
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos())
}

fn classify_terminal_outcome(
    kind: TerminalSignalKind,
    stdout: &[u8],
    stderr: &[u8],
    status: i32,
    cancelled: bool,
) -> TerminalOutcome {
    if [stdout, stderr]
        .iter()
        .any(|stream| structured_model_capacity(stream))
    {
        return TerminalOutcome::ModelAtCapacity;
    }
    // A cleanup cancellation may drain adopted descendants after the provider
    // itself has already emitted a typed rejection. Keep that provider result.
    match kind {
        TerminalSignalKind::QuotaExhaustedInband => return TerminalOutcome::QuotaRejected,
        TerminalSignalKind::MaybeQuotaExhausted => return TerminalOutcome::MaybeQuota,
        TerminalSignalKind::RateLimited => return TerminalOutcome::RateLimited,
        TerminalSignalKind::ProviderStorageContention => return TerminalOutcome::StorageContention,
        TerminalSignalKind::ProviderUnavailable
        | TerminalSignalKind::ProlongedSilence
        | TerminalSignalKind::SpawnError => return TerminalOutcome::ProviderUnavailable,
        _ => {}
    }
    if matches!(
        kind,
        TerminalSignalKind::CleanExit
            | TerminalSignalKind::NonzeroExit
            | TerminalSignalKind::Unknown
    ) && oulipoly_runtime::diagnostics::non_quota_failure_diagnosis(
        &String::from_utf8_lossy(stderr),
        if libc::WIFEXITED(status) {
            libc::WEXITSTATUS(status)
        } else {
            -1
        },
    )
    .is_some_and(|diagnosis| {
        diagnosis.category == oulipoly_runtime::diagnostics::ErrorCategory::AuthExpired
    }) {
        return TerminalOutcome::AuthRejected;
    }
    if cancelled {
        return TerminalOutcome::Cancelled;
    }
    match kind {
        TerminalSignalKind::CleanExit => TerminalOutcome::Clean,
        TerminalSignalKind::NonzeroExit
        | TerminalSignalKind::SignalExit
        | TerminalSignalKind::Unknown => {
            if kind == TerminalSignalKind::Unknown {
                TerminalOutcome::Unknown
            } else {
                TerminalOutcome::GenericFailure
            }
        }
        _ => unreachable!("typed rejection handled above"),
    }
}

/// A structured provider error code is required. Free text such as "quota"
/// in a model capacity explanation cannot turn this into account exhaustion.
fn structured_model_capacity(stream: &[u8]) -> bool {
    stream.split(|byte| *byte == b'\n').any(|line| {
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(line) else {
            return false;
        };
        if value.get("type").and_then(serde_json::Value::as_str) != Some("error") {
            return false;
        }
        ["/error/code", "/error/data/code"].iter().any(|path| {
            value.pointer(path).and_then(serde_json::Value::as_str) == Some("model_at_capacity")
        })
    })
}

fn terminal_record(
    directory: &Path,
    decision: &RouteDecision,
    candidate: &RouteCandidate,
    grant: &Grant,
    status: i32,
    mut stdout: File,
    mut stderr: File,
    cancelled: bool,
) -> io::Result<TerminalRecord> {
    if candidate.version != 3
        || candidate.binding != decision.binding
        || candidate.account != decision.selection.account
        || candidate.account_identity != decision.selection.account_identity
        || candidate.model != decision.selection.model
        || candidate.config_sha256 != decision.selection.config_sha256
        || candidate.index != decision.selection.index
        || candidate.plan_sha256 != grant.plan_sha256
        || grant.plan_sha256 != decision.selection.plan_sha256
    {
        return Err(io::Error::other(
            "fresh terminal candidate/Q binding changed",
        ));
    }
    let q_path = directory.join(format!("{}.drain.json", grant.id));
    let q_sha = sha_file(&File::open(&q_path)?)?.0;
    let mut stdout_bytes = Vec::new();
    let mut stderr_bytes = Vec::new();
    stdout.read_to_end(&mut stdout_bytes)?;
    stderr.read_to_end(&mut stderr_bytes)?;
    let signal = candidate.terminal_recognizer.classify(
        &candidate.account,
        &stdout_bytes,
        &stderr_bytes,
        status,
    );
    let outcome =
        classify_terminal_outcome(signal.kind, &stdout_bytes, &stderr_bytes, status, cancelled);
    let signal_kind = if outcome == TerminalOutcome::ModelAtCapacity {
        "ModelAtCapacity".to_string()
    } else {
        format!("{:?}", signal.kind)
    };
    let name = format!("{}.terminal.json", grant.id);
    if let Some(existing) = exact_file::<TerminalRecord>(directory, &name)? {
        if existing.version != 1
            || existing.binding != decision.binding
            || existing.selection != decision.selection
            || existing.grant_id != grant.id
            || existing.physical_q_sha256 != q_sha
            || existing.signal_kind != signal_kind
            || existing.outcome != outcome
        {
            return Err(io::Error::other("fresh terminal ledger changed"));
        }
        return Ok(existing);
    }
    let record = TerminalRecord {
        version: 1,
        binding: decision.binding.clone(),
        selection: decision.selection.clone(),
        grant_id: grant.id.clone(),
        physical_q_sha256: q_sha,
        physical_q_unix_nanos: file_unix_nanos(&q_path)?,
        signal_kind,
        outcome,
    };
    match durable_new(directory, &name, &record) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let raced: TerminalRecord = exact_file(directory, &name)?
                .ok_or_else(|| io::Error::other("fresh terminal ledger race unreadable"))?;
            if raced != record {
                return Err(io::Error::other("fresh terminal ledger race changed"));
            }
        }
        Err(error) => return Err(error),
    }
    Ok(record)
}

fn route_evidence(
    directory: &Path,
    candidate: &RouteCandidate,
) -> io::Result<(u64, u64, u64, Vec<TerminalRecord>)> {
    route_evidence_excluding(directory, candidate, None)
}

fn route_evidence_excluding(
    directory: &Path,
    candidate: &RouteCandidate,
    pre_k_handoff: Option<&str>,
) -> io::Result<(u64, u64, u64, Vec<TerminalRecord>)> {
    let mut live = 0;
    let mut failures = 0;
    let mut invocations = 0;
    let mut markers = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.ends_with(".route-selection.json") {
            continue;
        }
        let previous: RouteDecision = serde_json::from_reader(File::open(entry.path())?)?;
        if previous.version != 1 || previous.selection.policy_version != FRESH_ROUTE_POLICY_VERSION
        {
            return Err(io::Error::other(
                "fresh route history policy is incompatible",
            ));
        }
        if previous.selection.account_identity != candidate.account_identity {
            continue;
        }
        let prior_candidate: RouteCandidate = exact_file(
            directory,
            &candidate_name(&previous.binding.handoff_id, previous.selection.index),
        )?
        .ok_or_else(|| io::Error::other("fresh route history candidate absent"))?;
        if prior_candidate.version != 3
            || prior_candidate.binding != previous.binding
            || prior_candidate.account_identity != previous.selection.account_identity
            || prior_candidate.account != previous.selection.account
            || prior_candidate.plan_sha256 != previous.selection.plan_sha256
        {
            return Err(io::Error::other(
                "fresh route history account identity changed",
            ));
        }
        // The selected route is durably recorded before its first provider K.
        // During that one pre-K check, its prepared grant is not history.
        if pre_k_handoff == Some(previous.binding.handoff_id.as_str()) {
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
        let consumed_path = directory.join(format!("{}.consumed.json", grant.id));
        let consumed: Option<Grant> =
            exact_file(directory, &format!("{}.consumed.json", grant.id))?;
        if consumed.as_ref() != Some(&grant) || grant.version != 1 {
            return Err(io::Error::other(format!(
                "fresh route history K absent or changed: {}",
                consumed_path.display()
            )));
        }
        invocations += 1;
        match observe(directory, &grant.id)? {
            Observation::Drained {
                status,
                stdout,
                stderr,
                cancelled,
                ..
            } => {
                let drain_path = directory.join(format!("{}.drain.json", grant.id));
                if status != 0 && file_age_less_than(&drain_path, Duration::from_secs(30 * 60))? {
                    failures += 1;
                }
                let previous_candidate: RouteCandidate = exact_file(
                    directory,
                    &candidate_name(&previous.binding.handoff_id, previous.selection.index),
                )?
                .ok_or_else(|| io::Error::other("fresh terminal candidate absent"))?;
                let record = terminal_record(
                    directory,
                    &previous,
                    &previous_candidate,
                    &grant,
                    status,
                    stdout,
                    stderr,
                    cancelled,
                )?;
                if record.outcome.is_marker() {
                    markers.push(record);
                }
            }
            // A consumed K remains live until physical Q. Provider exit and
            // receipt age alone cannot authorize another selection.
            Observation::Pending | Observation::ProviderExited(_) => live += 1,
            Observation::Unknown => {
                return Err(io::Error::other(format!(
                    "fresh route history K has unknown physical drain: grant={}, K={}, Q={}",
                    grant.id,
                    consumed_path.display(),
                    directory.join(format!("{}.drain.json", grant.id)).display()
                )));
            }
        }
    }
    Ok((live, failures, invocations, markers))
}

fn file_age_less_than(path: &Path, window: Duration) -> io::Result<bool> {
    Ok(std::fs::metadata(path)?
        .modified()?
        .elapsed()
        .is_ok_and(|age| age < window))
}

fn candidate_quota(
    directory: &Path,
    binding: &Binding,
    candidate: &RouteCandidate,
) -> io::Result<(Option<(Option<u32>, Option<u128>)>, Option<String>)> {
    if candidate.quota_script.is_none() {
        return Ok((Some((None, None)), None)); // explicit unmetered account
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
        return Ok((
            None,
            Some(format!("quota effect absent: {}", first_dir.display())),
        ));
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
    let mut verified_dir = first_dir;
    let mut verified_intent = first_intent;
    if result.state != "drained" {
        return Ok((None, Some(result.artifact)));
    }
    if result.outcome.as_deref() != Some("valid_windows")
        && candidate.auth_refresh_command.is_some()
    {
        let auth_dir = effect_directory(
            directory,
            binding,
            &FreshAccountEffectRequest {
                kind: FreshAccountEffectKind::AuthRefresh,
                ..request.clone()
            },
        );
        let auth_intent = effect_intent(&auth_dir)?.ok_or_else(|| {
            io::Error::other(format!(
                "fresh auth evidence absent: {}",
                auth_dir.display()
            ))
        })?;
        if auth_intent.version != 1
            || auth_intent.binding != *binding
            || auth_intent.request.model != candidate.model
            || auth_intent.request.config_sha256 != candidate.config_sha256
            || auth_intent.request.account != candidate.account
            || auth_intent.request.index != candidate.index
            || auth_intent.request.kind != FreshAccountEffectKind::AuthRefresh
        {
            return Err(io::Error::other("fresh auth effect provenance changed"));
        }
        let auth = effect_readback_from_dir(&auth_dir, &auth_intent)?;
        if auth.state != "drained" {
            return Err(io::Error::other(format!(
                "fresh auth effect unknown: effect={}, artifact={}, peer_effect={:?}, peer_artifact={:?}",
                auth.effect_id, auth.artifact, auth.peer_effect_id, auth.peer_artifact
            )));
        }
        if auth.outcome.as_deref() != Some("refreshed") {
            return Ok((None, Some(auth.artifact)));
        }
        let retry_dir = effect_directory(
            directory,
            binding,
            &FreshAccountEffectRequest {
                kind: FreshAccountEffectKind::QuotaRetry,
                ..request
            },
        );
        let Some(retry_intent) = effect_intent(&retry_dir)? else {
            return Ok((None, Some(retry_dir.display().to_string())));
        };
        if retry_intent.version != 1
            || retry_intent.binding != *binding
            || retry_intent.request.model != candidate.model
            || retry_intent.request.account != candidate.account
            || retry_intent.request.config_sha256 != candidate.config_sha256
            || retry_intent.request.index != candidate.index
            || retry_intent.request.kind != FreshAccountEffectKind::QuotaRetry
        {
            return Err(io::Error::other("fresh quota retry provenance changed"));
        }
        result = effect_readback_from_dir(&retry_dir, &retry_intent)?;
        verified_dir = retry_dir;
        verified_intent = retry_intent;
    }
    if result.state != "drained" {
        return Ok((None, Some(result.artifact)));
    }
    if result.outcome.as_deref() != Some("valid_windows") || result.windows.is_empty() {
        return Ok((None, Some(result.artifact)));
    }
    let now = Utc::now().timestamp();
    if !quota_read_is_fresh(&result, now)? {
        return Ok((None, Some(format!("stale quota read: {}", result.artifact))));
    }
    let Some(remaining) = quota_remaining(&result, now)? else {
        return Ok((None, None));
    };
    let physical_q = effect_physical_q_nanos(&verified_dir, &verified_intent)?;
    if let Some(artifact) = newer_quota_effect(directory, candidate, &verified_intent, physical_q)?
    {
        return Ok((None, Some(artifact)));
    }
    Ok((Some((Some(remaining), Some(physical_q))), None))
}

/// The source Q used by this root must still be the newest resolved account
/// observation. A different root may begin a new K after this root created
/// its reuse receipt; that unresolved effect blocks selection and the pre-K
/// recheck rather than allowing the older healthy reading through.
fn newer_quota_effect(
    directory: &Path,
    candidate: &RouteCandidate,
    source: &AccountEffectIntent,
    source_q: u128,
) -> io::Result<Option<String>> {
    for entry in std::fs::read_dir(directory.join("account-effects"))? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.ends_with("-quota-first") && !name.ends_with("-quota-retry") {
            continue;
        }
        let dir = entry.path();
        let Some(intent) = effect_intent(&dir)? else {
            continue;
        };
        if !same_physical_effect_source(directory, candidate, &intent)?
            || intent.environment_sha256 != source.environment_sha256
            || dir.join("reuse.json").exists()
        {
            continue;
        }
        if intent.version != 1 {
            return Err(io::Error::other("newer quota effect provenance changed"));
        }
        let readback = effect_readback_from_dir(&dir, &intent)?;
        if readback.state != "drained" || effect_physical_q_nanos(&dir, &intent)? > source_q {
            return Ok(Some(format!(
                "newer or unresolved quota effect: {}",
                dir.display()
            )));
        }
    }
    Ok(None)
}

/// Return the physical source Q time, never the time of a reused readback.
fn effect_physical_q_nanos(dir: &Path, intent: &AccountEffectIntent) -> io::Result<u128> {
    let broker_directory = dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| io::Error::other("fresh effect broker directory absent"))?;
    let candidate = effect_candidate(broker_directory, &intent.binding, &intent.request)?;
    let (physical_dir, physical_intent) = if let Some(reuse) = &intent.auth_source {
        if intent.request.kind != FreshAccountEffectKind::AuthRefresh
            || reuse.source_directory.contains('/')
            || reuse.source_directory.contains("..")
            || !reuse.source_directory.ends_with("-auth-refresh")
        {
            return Err(io::Error::other("fresh marker auth source invalid"));
        }
        let source_dir = dir
            .parent()
            .ok_or_else(|| io::Error::other("effect parent absent"))?
            .join(&reuse.source_directory);
        let source = effect_intent(&source_dir)?
            .ok_or_else(|| io::Error::other("auth source intent absent"))?;
        if source.version != 1
            || source.id != reuse.source_effect_id
            || source.auth_source.is_some()
            || source.binding == intent.binding
            || source.request.kind != FreshAccountEffectKind::AuthRefresh
            || !same_physical_effect_source(broker_directory, &candidate, &source)?
            || source.environment_sha256 != intent.environment_sha256
            || intent.plan_sha256 != format!("coalesced:{}", source.id)
        {
            return Err(io::Error::other("fresh marker auth source changed"));
        }
        (source_dir, source)
    } else if let Some(reuse) = exact_file::<QuotaReuse>(dir, "reuse.json")? {
        let name = &reuse.source_directory;
        if name.contains('/')
            || name.contains("..")
            || (!name.ends_with("-quota-first") && !name.ends_with("-quota-retry"))
        {
            return Err(io::Error::other("fresh marker quota reuse source invalid"));
        }
        let source_dir = dir
            .parent()
            .ok_or_else(|| io::Error::other("effect parent absent"))?
            .join(name);
        let source = effect_intent(&source_dir)?
            .ok_or_else(|| io::Error::other("quota source intent absent"))?;
        if source.id != reuse.source_effect_id
            || !same_physical_effect_source(broker_directory, &candidate, &source)?
            || source.environment_sha256 != intent.environment_sha256
            || source_dir.join("reuse.json").exists()
        {
            return Err(io::Error::other(
                "fresh marker quota reuse provenance changed",
            ));
        }
        (source_dir, source)
    } else {
        (dir.to_owned(), intent.clone())
    };
    let grant = grant_for_binding(&physical_dir, &physical_intent.binding)?
        .ok_or_else(|| io::Error::other("fresh marker physical Q grant absent"))?;
    if !matches!(observe(&physical_dir, &grant)?, Observation::Drained { .. }) {
        return Err(io::Error::other("fresh marker physical Q not drained"));
    }
    file_unix_nanos(&physical_dir.join(format!("{grant}.drain.json")))
}

fn auth_verified_after_marker(
    directory: &Path,
    binding: &Binding,
    candidate: &RouteCandidate,
    marker: &TerminalRecord,
) -> io::Result<bool> {
    if candidate.auth_refresh_command.is_none() {
        return Ok(false);
    }
    let first_request = FreshAccountEffectRequest {
        d_key: String::new(),
        model: candidate.model.clone(),
        config_sha256: candidate.config_sha256.clone(),
        account: candidate.account.clone(),
        index: candidate.index,
        kind: FreshAccountEffectKind::QuotaFirst,
        environment: Vec::new(),
    };
    let target_environment = if candidate.quota_script.is_some() {
        let first_dir = effect_directory(directory, binding, &first_request);
        let Some(first) = effect_intent(&first_dir)? else {
            return Ok(false);
        };
        if first.version != 1
            || first.binding != *binding
            || first.request.model != candidate.model
            || first.request.config_sha256 != candidate.config_sha256
            || first.request.account != candidate.account
            || first.request.index != candidate.index
            || first.request.kind != FreshAccountEffectKind::QuotaFirst
        {
            return Err(io::Error::other("fresh marker quota provenance changed"));
        }
        Some(first.environment_sha256)
    } else {
        None
    };
    let parent = directory.join("account-effects");
    let entries = match std::fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        if !entry
            .file_name()
            .to_string_lossy()
            .ends_with("-auth-refresh")
        {
            continue;
        }
        let dir = entry.path();
        let Some(intent) = effect_intent(&dir)? else {
            continue;
        };
        if !same_physical_effect_source(directory, candidate, &intent)? {
            continue;
        }
        if intent.version != 1 || intent.request.kind != FreshAccountEffectKind::AuthRefresh {
            return Err(io::Error::other("fresh marker auth provenance changed"));
        }
        if intent.auth_source.is_some()
            || target_environment
                .as_ref()
                .is_some_and(|digest| digest != &intent.environment_sha256)
        {
            continue;
        }
        let result = effect_readback_from_dir(&dir, &intent)?;
        if result.state == "drained"
            && result.outcome.as_deref() == Some("refreshed")
            && effect_physical_q_nanos(&dir, &intent)? > marker.physical_q_unix_nanos
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn marker_allows_candidate(
    directory: &Path,
    binding: &Binding,
    candidate: &RouteCandidate,
    markers: &[TerminalRecord],
    quota_q_nanos: Option<u128>,
) -> io::Result<bool> {
    for marker in markers {
        if !single_marker_allows_candidate(directory, binding, candidate, marker, quota_q_nanos)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn single_marker_allows_candidate(
    directory: &Path,
    binding: &Binding,
    candidate: &RouteCandidate,
    marker: &TerminalRecord,
    quota_q_nanos: Option<u128>,
) -> io::Result<bool> {
    if marker.version != 1 || marker.selection.account_identity != candidate.account_identity {
        return Err(io::Error::other("fresh terminal marker candidate mismatch"));
    }
    let newer_healthy_quota = quota_q_nanos.is_some_and(|q| q > marker.physical_q_unix_nanos);
    match marker.outcome {
        TerminalOutcome::QuotaRejected => Ok(newer_healthy_quota),
        TerminalOutcome::AuthRejected => {
            auth_verified_after_marker(directory, binding, candidate, marker)
        }
        TerminalOutcome::ModelAtCapacity
        | TerminalOutcome::MaybeQuota
        | TerminalOutcome::ProviderUnavailable
        | TerminalOutcome::RateLimited
        | TerminalOutcome::StorageContention => Ok(true),
        TerminalOutcome::Clean
        | TerminalOutcome::GenericFailure
        | TerminalOutcome::Cancelled
        | TerminalOutcome::Unknown => Ok(true),
    }
}

fn quota_remaining(result: &FreshAccountEffectReadback, now: i64) -> io::Result<Option<u32>> {
    if !quota_read_is_fresh(result, now)? {
        return Ok(None);
    }
    let mut binding_remaining = u32::MAX;
    for window in &result.windows {
        let reset = DateTime::parse_from_rfc3339(&window.resets_at)
            .map_err(io::Error::other)?
            .timestamp();
        // A reset timestamp is evidence about the old window, not a fresh
        // account reading. Wait for a new authoritative Q.
        if reset <= now || window.used_percent >= 100.0 || window.remaining == Some(0) {
            return Ok(None);
        }
        binding_remaining =
            binding_remaining.min(((100.0 - window.used_percent) * 100.0).round() as u32);
    }
    Ok((!result.windows.is_empty()).then_some(binding_remaining))
}

const QUOTA_CACHE_TTL_SECONDS: i64 = 5 * 60 * 60;

fn quota_read_is_fresh(result: &FreshAccountEffectReadback, now: i64) -> io::Result<bool> {
    let completed = result
        .completed_unix_seconds
        .ok_or_else(|| io::Error::other("fresh quota completion time absent"))?;
    if now < completed || now - completed >= QUOTA_CACHE_TTL_SECONDS || result.windows.is_empty() {
        return Ok(false);
    }
    for window in &result.windows {
        if DateTime::parse_from_rfc3339(&window.resets_at)
            .map_err(io::Error::other)?
            .timestamp()
            <= now
        {
            return Ok(false);
        }
    }
    Ok(true)
}

const FRESH_ROUTE_POLICY_VERSION: &str = "fresh-quota-account-v4";

/// The fsynced decision files are the cursor. Serialize their scan and the
/// next durable decision across broker threads and restarts, including roots
/// that select before their provider K has begun.
fn route_selection_lock(directory: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .open(directory.join("route-selection.lock"))?;
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

fn last_route_cursor(
    directory: &Path,
    model: &str,
    config: &str,
) -> io::Result<(u64, Option<usize>)> {
    let mut last = (0, None);
    let mut sequences = HashSet::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if !entry
            .file_name()
            .to_string_lossy()
            .ends_with(".route-selection.json")
        {
            continue;
        }
        let decision: RouteDecision = serde_json::from_reader(File::open(entry.path())?)?;
        if decision.selection.model != model || decision.selection.config_sha256 != config {
            continue;
        }
        if decision.version != 1 || decision.selection.policy_version != FRESH_ROUTE_POLICY_VERSION
        {
            return Err(io::Error::other(
                "fresh route cursor contains incompatible policy decision",
            ));
        }
        let source: RouteCandidate = exact_file(
            directory,
            &candidate_name(&decision.binding.handoff_id, decision.selection.index),
        )?
        .ok_or_else(|| io::Error::other("fresh route cursor candidate absent"))?;
        if source.version != 3
            || source.binding != decision.binding
            || source.account_identity != decision.selection.account_identity
            || source.account != decision.selection.account
            || source.plan_sha256 != decision.selection.plan_sha256
        {
            return Err(io::Error::other("fresh route cursor identity changed"));
        }
        if decision.pin.is_none() {
            if decision.sequence == 0 || !sequences.insert(decision.sequence) {
                return Err(io::Error::other(
                    "fresh route cursor sequence missing or repeated",
                ));
            }
            if decision.sequence > last.0 {
                last = (decision.sequence, Some(decision.selection.index));
            }
        } else if decision.sequence != 0 {
            return Err(io::Error::other("pinned route changed round-robin cursor"));
        }
    }
    if u64::try_from(sequences.len()).ok() != Some(last.0) {
        return Err(io::Error::other(
            "fresh route cursor has a missing decision",
        ));
    }
    Ok(last)
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
    let _cursor_lock = route_selection_lock(directory)?;
    let candidates = route_candidates(directory, binding, request)?;
    let name = decision_name(&binding.handoff_id);
    if let Some(existing) = exact_file::<RouteDecision>(directory, &name)? {
        if existing.version != 1
            || existing.binding != *binding
            || existing.total != request.total
            || existing.pin != request.pin
            || existing.selection.model != request.model
            || existing.selection.config_sha256 != request.config_sha256
            || candidates
                .get(existing.selection.index)
                .is_none_or(|c| c.account_identity != existing.selection.account_identity)
            || existing.selection.policy_version != FRESH_ROUTE_POLICY_VERSION
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
        let (live, failures, invocations, markers) = route_evidence(directory, &candidate)?;
        let (quota, unknown) = candidate_quota(directory, binding, &candidate)?;
        if unknown.is_some()
            && request
                .pin
                .as_deref()
                .is_none_or(|pin| pin == candidate.account)
        {
            unknown_artifact = unknown;
        }
        let Some((quota_remaining, quota_q_nanos)) = quota else {
            continue;
        };
        if !marker_allows_candidate(directory, binding, &candidate, &markers, quota_q_nanos)? {
            continue;
        }
        eligible.push((candidate, quota_remaining, (live, failures, invocations)));
    }
    if let Some(artifact) = unknown_artifact {
        return Err(io::Error::other(format!(
            "fresh quota effect unknown: {artifact}"
        )));
    }
    let eligible_accounts: Vec<String> = eligible
        .iter()
        .map(|(candidate, _, _)| candidate.account.clone())
        .collect();
    let (previous_sequence, previous_index) =
        last_route_cursor(directory, &request.model, &request.config_sha256)?;
    let (candidate, quota_remaining, (live, failures, invocations)) = eligible
        .into_iter()
        .filter(|(candidate, _, _)| {
            request
                .pin
                .as_deref()
                .is_none_or(|pin| pin == candidate.account)
        })
        .min_by_key(|(candidate, _, _)| {
            previous_index.map_or(candidate.index, |last| {
                (candidate.index + request.total - (last + 1) % request.total) % request.total
            })
        })
        .ok_or_else(|| io::Error::other("fresh route has no eligible account or pin"))?;
    let sequence = if request.pin.is_some() {
        0
    } else {
        previous_sequence
            .checked_add(1)
            .ok_or_else(|| io::Error::other("fresh route cursor overflow"))?
    };
    let selection = FreshRouteSelection {
        model: candidate.model,
        config_sha256: candidate.config_sha256,
        account: candidate.account,
        account_identity: candidate.account_identity,
        index: candidate.index,
        plan_sha256: candidate.plan_sha256,
        observed_live: live,
        observed_failures: failures,
        observed_invocations: invocations,
        policy_version: FRESH_ROUTE_POLICY_VERSION.into(),
        eligible_accounts: eligible_accounts.clone(),
        quota_remaining_basis_points: quota_remaining,
    };
    durable_new(
        directory,
        &name,
        &RouteDecision {
            version: 1,
            binding: binding.clone(),
            total: request.total,
            pin: request.pin.clone(),
            sequence,
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
    .ok_or_else(|| io::Error::other("fresh provider selected candidate absent"))?;
    if candidate.version != 3
        || candidate.binding != *binding
        || candidate.model != decision.selection.model
        || candidate.config_sha256 != decision.selection.config_sha256
        || candidate.account != decision.selection.account
        || candidate.account_identity != decision.selection.account_identity
        || candidate.plan_sha256 != plan.digest
    {
        return Err(io::Error::other(
            "fresh provider selected candidate changed",
        ));
    }
    let (_, _, _, markers) =
        route_evidence_excluding(directory, &candidate, Some(&binding.handoff_id))?;
    let (quota, unknown) = candidate_quota(directory, binding, &candidate)?;
    if unknown.is_some()
        || quota.is_none()
        || quota.and_then(|(remaining, _)| remaining)
            != decision.selection.quota_remaining_basis_points
        || !marker_allows_candidate(
            directory,
            binding,
            &candidate,
            &markers,
            quota.and_then(|(_, q)| q),
        )?
    {
        return Err(io::Error::other(
            "fresh provider selected account no longer eligible before K",
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
    if prepared
        .directory
        .join(decision_name(&b.handoff_id))
        .exists()
    {
        require_selected_plan(&prepared.directory, b, &prepared.plan)?;
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
    fn typed_terminal_ledger_requires_new_matching_verification() {
        let temp = tempfile::tempdir().unwrap();
        let broker = temp.path().join("fresh-provider");
        std::fs::create_dir(&broker).unwrap();
        let old_wal = temp.path().join("state.db-wal");
        std::fs::write(&old_wal, b"legacy WAL sentinel").unwrap();
        let process = PinnedProcess::open(unsafe { libc::getpid() }).unwrap();
        let binding = fixture_binding(&process, &process);
        let grant = Grant {
            version: 1,
            id: uuid::Uuid::new_v4().to_string(),
            binding: binding.clone(),
            plan_sha256: "p".repeat(64),
        };
        let selection = FreshRouteSelection {
            model: "work".into(),
            config_sha256: "c".repeat(64),
            account: "opencode-one".into(),
            account_identity: "opencode-one".into(),
            index: 0,
            plan_sha256: grant.plan_sha256.clone(),
            observed_live: 0,
            observed_failures: 0,
            observed_invocations: 0,
            policy_version: FRESH_ROUTE_POLICY_VERSION.into(),
            eligible_accounts: vec!["opencode-one".into(), "second".into()],
            quota_remaining_basis_points: Some(8000),
        };
        let candidate = RouteCandidate {
            version: 3,
            binding: binding.clone(),
            model: selection.model.clone(),
            config_sha256: selection.config_sha256.clone(),
            account: selection.account.clone(),
            account_identity: selection.account_identity.clone(),
            index: 0,
            total: 2,
            pin: None,
            plan_sha256: grant.plan_sha256.clone(),
            quota_script: Some("quota source".into()),
            auth_refresh_command: None,
            terminal_recognizer: FreshTerminalRecognizer::OpenCode,
        };
        let mut legacy = serde_json::to_value(&candidate).unwrap();
        legacy["version"] = serde_json::json!(1);
        legacy
            .as_object_mut()
            .unwrap()
            .remove("terminal_recognizer");
        assert!(serde_json::from_value::<RouteCandidate>(legacy).is_err());
        let decision = RouteDecision {
            version: 1,
            binding,
            total: 2,
            pin: None,
            sequence: 1,
            selection,
        };
        let q_path = broker.join(format!("{}.drain.json", grant.id));
        std::fs::write(&q_path, b"exact physical Q").unwrap();
        let stdout_path = broker.join("stdout");
        let stderr_path = broker.join("stderr");
        std::fs::write(&stdout_path, b"").unwrap();
        std::fs::write(
            &stderr_path,
            br#"{"type":"error","error":{"data":{"message":"quota exhausted for account"}}}"#,
        )
        .unwrap();
        let record = terminal_record(
            &broker,
            &decision,
            &candidate,
            &grant,
            0,
            File::open(&stdout_path).unwrap(),
            File::open(&stderr_path).unwrap(),
            false,
        )
        .unwrap();
        assert_eq!(record.signal_kind, "QuotaExhaustedInband");
        assert_eq!(record.outcome, TerminalOutcome::QuotaRejected);
        assert!(
            !marker_allows_candidate(
                &broker,
                &decision.binding,
                &candidate,
                std::slice::from_ref(&record),
                Some(record.physical_q_unix_nanos - 1),
            )
            .unwrap(),
            "older healthy quota Q cannot clear a typed rejection"
        );
        assert!(
            marker_allows_candidate(
                &broker,
                &decision.binding,
                &candidate,
                std::slice::from_ref(&record),
                Some(record.physical_q_unix_nanos + 1),
            )
            .unwrap()
        );
        let mut auth_marker = record.clone();
        auth_marker.outcome = TerminalOutcome::AuthRejected;
        let mut auth_candidate = candidate.clone();
        auth_candidate.auth_refresh_command = Some("refresh".into());
        assert!(
            !marker_allows_candidate(
                &broker,
                &decision.binding,
                &auth_candidate,
                std::slice::from_ref(&auth_marker),
                Some(record.physical_q_unix_nanos + 1),
            )
            .unwrap(),
            "healthy quota Q incorrectly cleared an auth rejection"
        );
        let mut other = candidate.clone();
        other.account = "second".into();
        other.account_identity = "other-physical-account".into();
        other.index = 1;
        assert!(marker_allows_candidate(&broker, &decision.binding, &other, &[], None).unwrap());
        assert!(
            marker_allows_candidate(
                &broker,
                &decision.binding,
                &other,
                std::slice::from_ref(&record),
                None
            )
            .is_err()
        );
        let mut older_quota = record.clone();
        older_quota.physical_q_unix_nanos -= Duration::from_secs(7 * 60).as_nanos();
        let mut later_availability = record.clone();
        later_availability.grant_id = uuid::Uuid::new_v4().to_string();
        later_availability.outcome = TerminalOutcome::ProviderUnavailable;
        later_availability.physical_q_unix_nanos -= Duration::from_secs(6 * 60).as_nanos();
        assert!(
            marker_allows_candidate(
                &broker,
                &decision.binding,
                &candidate,
                std::slice::from_ref(&later_availability),
                None,
            )
            .unwrap(),
            "typed availability recovery was not applied"
        );
        assert!(
            !marker_allows_candidate(
                &broker,
                &decision.binding,
                &candidate,
                &[older_quota, later_availability],
                None,
            )
            .unwrap(),
            "later availability recovery hid unresolved quota"
        );
        let reread: TerminalRecord = exact_file(&broker, &format!("{}.terminal.json", grant.id))
            .unwrap()
            .unwrap();
        assert_eq!(record, reread, "restart changed durable terminal evidence");
        assert_eq!(
            terminal_record(
                &broker,
                &decision,
                &candidate,
                &grant,
                0,
                File::open(&stdout_path).unwrap(),
                File::open(&stderr_path).unwrap(),
                false,
            )
            .unwrap(),
            record,
        );
        std::fs::write(&q_path, b"changed Q").unwrap();
        assert!(
            terminal_record(
                &broker,
                &decision,
                &candidate,
                &grant,
                0,
                File::open(&stdout_path).unwrap(),
                File::open(&stderr_path).unwrap(),
                false,
            )
            .is_err()
        );
        assert_eq!(std::fs::read(old_wal).unwrap(), b"legacy WAL sentinel");
    }

    #[test]
    fn terminal_classes_do_not_infer_quota_from_exit_or_cancellation() {
        let provider = FreshTerminalRecognizer::OpenAiCompat;
        let generic = provider.classify("first", b"", b"plain failure", 1 << 8);
        assert_eq!(generic.kind, TerminalSignalKind::NonzeroExit);
        assert_eq!(
            classify_terminal_outcome(generic.kind, b"", b"plain failure", 1 << 8, false),
            TerminalOutcome::GenericFailure,
        );
        assert_eq!(
            classify_terminal_outcome(
                generic.kind,
                b"",
                b"authentication failed: token expired",
                1 << 8,
                false
            ),
            TerminalOutcome::AuthRejected,
        );
        assert_eq!(
            classify_terminal_outcome(
                TerminalSignalKind::QuotaExhaustedInband,
                b"",
                b"quota exhausted",
                0,
                true
            ),
            TerminalOutcome::QuotaRejected,
        );
        assert_eq!(
            classify_terminal_outcome(generic.kind, b"", b"plain failure", 1 << 8, true),
            TerminalOutcome::Cancelled,
        );
        assert_eq!(
            classify_terminal_outcome(TerminalSignalKind::Unknown, b"", b"", -1, false),
            TerminalOutcome::Unknown,
        );
        let contention = FreshTerminalRecognizer::OpenCode.classify(
            "opencode-first",
            br#"{"type":"error","error":{"data":{"message":"Failed to execute statement"}}}"#,
            b"",
            0,
        );
        assert_eq!(
            contention.kind,
            TerminalSignalKind::ProviderStorageContention
        );
        assert_eq!(
            classify_terminal_outcome(contention.kind, b"", b"", 0, false),
            TerminalOutcome::StorageContention,
        );
    }

    #[test]
    fn recent_failure_observation_is_recorded_without_ranking() {
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
    fn auth_reference_retains_unknown_and_rejects_changed_provenance() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("account-effects");
        std::fs::create_dir(&parent).unwrap();
        let process = PinnedProcess::open(unsafe { libc::getpid() }).unwrap();
        let source_binding = fixture_binding(&process, &process);
        let mut follower_binding = fixture_binding(&process, &process);
        follower_binding.handoff_id = uuid::Uuid::new_v4().to_string();
        let request = FreshAccountEffectRequest {
            d_key: uuid::Uuid::new_v4().to_string(),
            model: "model".into(),
            config_sha256: "a".repeat(64),
            account: "shared-account".into(),
            index: 0,
            kind: FreshAccountEffectKind::AuthRefresh,
            environment: vec![("PATH".into(), "/usr/bin:/bin".into())],
        };
        for binding in [&source_binding, &follower_binding] {
            durable_new(
                temp.path(),
                &candidate_name(&binding.handoff_id, 0),
                &RouteCandidate {
                    version: 3,
                    binding: binding.clone(),
                    model: request.model.clone(),
                    config_sha256: request.config_sha256.clone(),
                    account: request.account.clone(),
                    account_identity: "physical-shared".into(),
                    index: 0,
                    total: 1,
                    pin: None,
                    plan_sha256: "p".repeat(64),
                    quota_script: Some("quota-script".into()),
                    auth_refresh_command: Some("auth-command".into()),
                    terminal_recognizer: FreshTerminalRecognizer::OpenAiCompat,
                },
            )
            .unwrap();
        }
        let source_dir = effect_directory(temp.path(), &source_binding, &request);
        std::fs::create_dir(&source_dir).unwrap();
        let source = AccountEffectIntent {
            version: 1,
            id: uuid::Uuid::new_v4().to_string(),
            binding: source_binding.clone(),
            request: redacted_effect_request(&request),
            environment_sha256: environment_digest(&request).unwrap(),
            plan_sha256: "source-plan".into(),
            auth_source: None,
        };
        durable_new(&source_dir, "intent.json", &source).unwrap();
        let source_candidate: RouteCandidate =
            exact_file(temp.path(), &candidate_name(&source_binding.handoff_id, 0))
                .unwrap()
                .unwrap();
        for (identity, expected_peer) in [("physical-shared", true), ("other-physical", false)] {
            let mut alias_binding = fixture_binding(&process, &process);
            alias_binding.handoff_id = uuid::Uuid::new_v4().to_string();
            let mut alias_candidate = source_candidate.clone();
            alias_candidate.binding = alias_binding.clone();
            alias_candidate.model = "other-model".into();
            alias_candidate.config_sha256 = "b".repeat(64);
            alias_candidate.account = "alias".into();
            alias_candidate.account_identity = identity.into();
            durable_new(
                temp.path(),
                &candidate_name(&alias_binding.handoff_id, 0),
                &alias_candidate,
            )
            .unwrap();
            let mut alias_request = request.clone();
            alias_request.model = alias_candidate.model;
            alias_request.config_sha256 = alias_candidate.config_sha256;
            alias_request.account = alias_candidate.account;
            assert_eq!(
                coalescible_auth_source(temp.path(), &alias_binding, &alias_request)
                    .unwrap()
                    .is_some(),
                expected_peer,
            );
        }
        let (source_name, found) =
            coalescible_auth_source(temp.path(), &follower_binding, &request)
                .unwrap()
                .unwrap();
        assert_eq!(found.id, source.id);
        let mut different = request.clone();
        different.config_sha256 = "b".repeat(64);
        assert!(coalescible_auth_source(temp.path(), &follower_binding, &different).is_err());
        different = request.clone();
        different.environment.push(("CHANGED".into(), "1".into()));
        assert!(coalescible_auth_source(temp.path(), &follower_binding, &different).is_err());
        different = request.clone();
        different.account = "other-account".into();
        assert!(coalescible_auth_source(temp.path(), &follower_binding, &different).is_err());
        assert!(
            coalescible_auth_source(temp.path(), &source_binding, &request)
                .unwrap()
                .is_none()
        );

        let follower_dir = effect_directory(temp.path(), &follower_binding, &request);
        std::fs::create_dir(&follower_dir).unwrap();
        let follower = AccountEffectIntent {
            version: 1,
            id: uuid::Uuid::new_v4().to_string(),
            binding: follower_binding,
            request: redacted_effect_request(&request),
            environment_sha256: environment_digest(&request).unwrap(),
            plan_sha256: format!("coalesced:{}", source.id),
            auth_source: Some(AuthReuse {
                source_directory: source_name,
                source_effect_id: source.id.clone(),
            }),
        };
        durable_new(&follower_dir, "intent.json", &follower).unwrap();
        // Simulates a lost original K reply and a broker process restart: only
        // the durable intents are available, so neither root may assume Q.
        let readback = effect_readback_from_dir(&follower_dir, &follower).unwrap();
        assert_eq!(readback.state, "unknown");
        assert_eq!(readback.effect_id, follower.id);
        assert_eq!(readback.peer_effect_id.as_deref(), Some(source.id.as_str()));
        assert_eq!(readback.artifact, follower_dir.display().to_string());
        assert_eq!(
            readback.peer_artifact.as_deref(),
            Some(source_dir.display().to_string().as_str())
        );
        let mut forged = follower.clone();
        forged.auth_source.as_mut().unwrap().source_effect_id = uuid::Uuid::new_v4().to_string();
        assert!(effect_readback_from_dir(&follower_dir, &forged).is_err());
        assert!(
            coalescible_auth_source(temp.path(), &follower.binding, &request)
                .unwrap()
                .is_some(),
            "a follower must not become a new authoritative auth K"
        );
        assert!(follower.auth_source.is_some());
    }

    #[test]
    fn broker_source_rejects_roster_effect_and_digest_forgery_or_edit() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("models")).unwrap();
        std::fs::write(
            temp.path().join("providers.toml"),
            "[first]\ncommand = \"/bin/true\"\nquota_account_id = \"physical-first\"\nquota_script = \"printf ok\"\n[second]\ncommand = \"/bin/true\"\nquota_account_id = \"physical-second\"\n",
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
            protocol_version: 4,
            d_key: uuid::Uuid::new_v4().to_string(),
            model: "pool".into(),
            config_sha256: pool.config_sha256.clone(),
            account: Some("first".into()),
            account_identity: Some("physical-first".into()),
            index: Some(0),
            total: 2,
            pin: None,
            quota_script: Some("printf ok".into()),
            auth_refresh_command: None,
        };
        validate_route_source(&source, &request).unwrap();
        let mut old_protocol = serde_json::to_value(&request).unwrap();
        old_protocol
            .as_object_mut()
            .unwrap()
            .remove("protocol_version");
        old_protocol
            .as_object_mut()
            .unwrap()
            .remove("account_identity");
        assert!(serde_json::from_value::<FreshRouteRequest>(old_protocol).is_err());
        request.account_identity = None;
        assert!(validate_route_source(&source, &request).is_err());
        request.account_identity = Some("physical-second".into());
        assert!(validate_route_source(&source, &request).is_err());
        request.account_identity = Some("physical-first".into());
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
        request.account_identity = None;
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
        let missing_dir = temp.path().join("missing-identity");
        std::fs::create_dir_all(missing_dir.join("models")).unwrap();
        let without_identity = std::fs::read_to_string(temp.path().join("providers.toml"))
            .unwrap()
            .replace("quota_account_id = \"physical-first\"\n", "");
        std::fs::write(missing_dir.join("providers.toml"), without_identity).unwrap();
        std::fs::copy(&model_path, missing_dir.join("models/pool.toml")).unwrap();
        let missing_pool = oulipoly_runtime::executor::cli::fresh_remote::load_fresh_headless_pool(
            &missing_dir,
            "pool",
        )
        .unwrap();
        let mut missing_request = request.clone();
        missing_request.config_sha256 = missing_pool.config_sha256;
        missing_request.account = Some("first".into());
        missing_request.account_identity = Some("physical-first".into());
        missing_request.index = Some(0);
        missing_request.quota_script = Some("printf ok".into());
        assert!(
            validate_route_source(&File::open(&missing_dir).unwrap(), &missing_request).is_err()
        );
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

    fn register_unmetered_route(
        directory: &Path,
        binding: &Binding,
        pin: Option<&str>,
    ) -> FreshRouteRequest {
        let config_sha256 = "c".repeat(64);
        for (index, account) in ["first", "second", "third"].iter().enumerate() {
            durable_new(
                directory,
                &candidate_name(&binding.handoff_id, index),
                &RouteCandidate {
                    version: 3,
                    binding: binding.clone(),
                    model: "fair".into(),
                    config_sha256: config_sha256.clone(),
                    account: (*account).into(),
                    account_identity: (*account).into(),
                    index,
                    total: 3,
                    pin: pin.map(str::to_owned),
                    plan_sha256: format!("{index:064x}"),
                    quota_script: None,
                    auth_refresh_command: None,
                    terminal_recognizer: FreshTerminalRecognizer::OpenCode,
                },
            )
            .unwrap();
        }
        FreshRouteRequest {
            protocol_version: 4,
            d_key: uuid::Uuid::new_v4().to_string(),
            model: "fair".into(),
            config_sha256,
            account: None,
            account_identity: None,
            index: None,
            total: 3,
            pin: pin.map(str::to_owned),
            quota_script: None,
            auth_refresh_command: None,
        }
    }

    #[test]
    fn one_pool_cannot_weight_one_physical_account_twice() {
        let temp = tempfile::tempdir().unwrap();
        let process = PinnedProcess::open(unsafe { libc::getpid() }).unwrap();
        let binding = fixture_binding(&process, &process);
        let request = register_unmetered_route(temp.path(), &binding, None);
        let second_name = candidate_name(&binding.handoff_id, 1);
        let mut second: RouteCandidate = exact_file(temp.path(), &second_name).unwrap().unwrap();
        second.account_identity = "first".into();
        std::fs::write(
            temp.path().join(second_name),
            serde_json::to_vec(&second).unwrap(),
        )
        .unwrap();
        assert!(
            select_route(temp.path(), &binding, &request)
                .unwrap_err()
                .to_string()
                .contains("candidate set changed")
        );
        assert!(
            !temp
                .path()
                .join(decision_name(&binding.handoff_id))
                .exists()
        );
    }

    #[test]
    fn unmetered_round_robin_is_durable_across_roots_restart_and_concurrent_selection() {
        let temp = tempfile::tempdir().unwrap();
        let process = PinnedProcess::open(unsafe { libc::getpid() }).unwrap();
        let roots: Vec<_> = (0..12)
            .map(|_| fixture_binding(&process, &process))
            .collect();
        let requests: Vec<_> = roots
            .iter()
            .map(|binding| register_unmetered_route(temp.path(), binding, None))
            .collect();
        let directory = temp.path();
        std::thread::scope(|scope| {
            let handles: Vec<_> = roots
                .iter()
                .zip(&requests)
                .map(|(binding, request)| {
                    scope.spawn(move || select_route(directory, binding, request).unwrap())
                })
                .collect();
            for handle in handles {
                let choice = handle.join().unwrap();
                assert_eq!(choice.policy_version, FRESH_ROUTE_POLICY_VERSION);
                assert_eq!(choice.quota_remaining_basis_points, None);
            }
        });
        let mut by_sequence = Vec::new();
        for binding in &roots {
            let decision: RouteDecision =
                exact_file(temp.path(), &decision_name(&binding.handoff_id))
                    .unwrap()
                    .unwrap();
            by_sequence.push((decision.sequence, decision.selection.index));
        }
        by_sequence.sort();
        assert_eq!(
            by_sequence,
            (1..=12)
                .map(|sequence| (sequence, ((sequence - 1) % 3) as usize))
                .collect::<Vec<_>>()
        );

        // A new broker incarnation reads the same fsynced selection files.
        let pinned = fixture_binding(&process, &process);
        let pinned_request = register_unmetered_route(temp.path(), &pinned, Some("third"));
        assert_eq!(
            select_route(temp.path(), &pinned, &pinned_request)
                .unwrap()
                .index,
            2
        );
        let resumed = fixture_binding(&process, &process);
        let request = register_unmetered_route(temp.path(), &resumed, None);
        assert_eq!(
            select_route(temp.path(), &resumed, &request).unwrap().index,
            0
        );
        let decision: RouteDecision = exact_file(temp.path(), &decision_name(&resumed.handoff_id))
            .unwrap()
            .unwrap();
        assert_eq!(decision.sequence, 13);
        std::fs::copy(
            temp.path().join(decision_name(&resumed.handoff_id)),
            temp.path().join("duplicate.route-selection.json"),
        )
        .unwrap();
        let corrupt = fixture_binding(&process, &process);
        let corrupt_request = register_unmetered_route(temp.path(), &corrupt, None);
        assert!(
            select_route(temp.path(), &corrupt, &corrupt_request)
                .unwrap_err()
                .to_string()
                .contains("cursor sequence missing or repeated")
        );
    }

    #[test]
    fn unknown_historical_k_refuses_new_route_with_exact_artifacts() {
        let temp = tempfile::tempdir().unwrap();
        let process = PinnedProcess::open(unsafe { libc::getpid() }).unwrap();
        let previous = fixture_binding(&process, &process);
        let grant = Grant {
            version: 1,
            id: uuid::Uuid::new_v4().to_string(),
            binding: previous.clone(),
            plan_sha256: "b".repeat(64),
        };
        durable_new(
            temp.path(),
            &decision_name(&previous.handoff_id),
            &RouteDecision {
                version: 1,
                binding: previous.clone(),
                total: 1,
                pin: None,
                sequence: 1,
                selection: FreshRouteSelection {
                    model: "model".into(),
                    config_sha256: "a".repeat(64),
                    account: "first".into(),
                    account_identity: "first".into(),
                    index: 0,
                    plan_sha256: grant.plan_sha256.clone(),
                    observed_live: 0,
                    observed_failures: 0,
                    observed_invocations: 0,
                    policy_version: FRESH_ROUTE_POLICY_VERSION.into(),
                    eligible_accounts: vec!["first".into()],
                    quota_remaining_basis_points: None,
                },
            },
        )
        .unwrap();
        durable_new(
            temp.path(),
            &format!("{}.fresh-grant.json", previous.handoff_id),
            &grant,
        )
        .unwrap();
        let k_path = temp.path().join(format!("{}.consumed.json", grant.id));
        let q_path = temp.path().join(format!("{}.drain.json", grant.id));
        durable_new(
            temp.path(),
            k_path.file_name().unwrap().to_str().unwrap(),
            &grant,
        )
        .unwrap();
        let current = fixture_binding(&process, &process);
        let candidate = RouteCandidate {
            version: 3,
            binding: current.clone(),
            model: "model".into(),
            config_sha256: "a".repeat(64),
            account: "first".into(),
            account_identity: "first".into(),
            index: 0,
            total: 1,
            pin: None,
            plan_sha256: "c".repeat(64),
            quota_script: None,
            auth_refresh_command: None,
            terminal_recognizer: FreshTerminalRecognizer::OpenAiCompat,
        };
        durable_new(
            temp.path(),
            &candidate_name(&current.handoff_id, 0),
            &candidate,
        )
        .unwrap();
        let mut previous_candidate = candidate.clone();
        previous_candidate.binding = previous.clone();
        previous_candidate.plan_sha256 = grant.plan_sha256.clone();
        durable_new(
            temp.path(),
            &candidate_name(&previous.handoff_id, 0),
            &previous_candidate,
        )
        .unwrap();
        let request = FreshRouteRequest {
            protocol_version: 4,
            d_key: uuid::Uuid::new_v4().to_string(),
            model: candidate.model.clone(),
            config_sha256: candidate.config_sha256.clone(),
            account: None,
            account_identity: None,
            index: None,
            total: 1,
            pin: None,
            quota_script: None,
            auth_refresh_command: None,
        };
        assert!(matches!(
            observe(temp.path(), &grant.id).unwrap(),
            Observation::Unknown
        ));
        let error = select_route(temp.path(), &current, &request)
            .unwrap_err()
            .to_string();
        assert!(error.contains("unknown physical drain"), "{error}");
        assert!(error.contains(&format!("grant={}", grant.id)), "{error}");
        assert!(
            error.contains(&format!("K={}", k_path.display())),
            "{error}"
        );
        assert!(
            error.contains(&format!("Q={}", q_path.display())),
            "{error}"
        );
        assert!(
            !temp
                .path()
                .join(decision_name(&current.handoff_id))
                .exists()
        );
        assert!(
            !temp
                .path()
                .join(format!("{}.fresh-grant.json", current.handoff_id))
                .exists()
        );

        let mut changed = grant.clone();
        changed.plan_sha256 = "d".repeat(64);
        std::fs::write(&k_path, serde_json::to_vec(&changed).unwrap()).unwrap();
        let error = select_route(temp.path(), &current, &request)
            .unwrap_err()
            .to_string();
        assert!(error.contains("K absent or changed"), "{error}");
        assert!(error.contains(&k_path.display().to_string()), "{error}");
        assert!(
            !temp
                .path()
                .join(decision_name(&current.handoff_id))
                .exists()
        );

        std::fs::remove_file(&k_path).unwrap();
        let error = select_route(temp.path(), &current, &request)
            .unwrap_err()
            .to_string();
        assert!(error.contains("K absent or changed"), "{error}");
        assert!(error.contains(&k_path.display().to_string()), "{error}");
        assert!(
            !temp
                .path()
                .join(decision_name(&current.handoff_id))
                .exists()
        );
        assert!(
            !temp
                .path()
                .join(format!("{}.terminal.json", grant.id))
                .exists(),
            "lost Q invented a typed terminal marker"
        );
    }

    #[test]
    fn unknown_quota_effect_prevents_fallback_provider_k() {
        let temp = tempfile::tempdir().unwrap();
        let process = PinnedProcess::open(unsafe { libc::getpid() }).unwrap();
        let binding = fixture_binding(&process, &process);
        for (index, account, quota_script) in [(0, "healthy", None), (1, "unknown", Some("quota"))]
        {
            durable_new(
                temp.path(),
                &candidate_name(&binding.handoff_id, index),
                &RouteCandidate {
                    version: 3,
                    binding: binding.clone(),
                    model: "model".into(),
                    config_sha256: "a".repeat(64),
                    account: account.into(),
                    account_identity: account.into(),
                    index,
                    total: 2,
                    pin: None,
                    plan_sha256: "b".repeat(64),
                    quota_script: quota_script.map(str::to_owned),
                    auth_refresh_command: None,
                    terminal_recognizer: FreshTerminalRecognizer::OpenAiCompat,
                },
            )
            .unwrap();
        }
        let effect = FreshAccountEffectRequest {
            d_key: uuid::Uuid::new_v4().to_string(),
            model: "model".into(),
            config_sha256: "a".repeat(64),
            account: "unknown".into(),
            index: 1,
            kind: FreshAccountEffectKind::QuotaFirst,
            environment: Vec::new(),
        };
        let effect_dir = effect_directory(temp.path(), &binding, &effect);
        std::fs::create_dir_all(&effect_dir).unwrap();
        durable_new(
            &effect_dir,
            "intent.json",
            &AccountEffectIntent {
                version: 1,
                id: uuid::Uuid::new_v4().to_string(),
                binding: binding.clone(),
                request: redacted_effect_request(&effect),
                environment_sha256: environment_digest(&effect).unwrap(),
                plan_sha256: "c".repeat(64),
                auth_source: None,
            },
        )
        .unwrap();
        let request = FreshRouteRequest {
            protocol_version: 4,
            d_key: effect.d_key,
            model: effect.model,
            config_sha256: effect.config_sha256,
            account: None,
            account_identity: None,
            index: None,
            total: 2,
            pin: None,
            quota_script: None,
            auth_refresh_command: None,
        };
        let error = select_route(temp.path(), &binding, &request)
            .unwrap_err()
            .to_string();
        assert!(error.contains("fresh quota effect unknown"), "{error}");
        assert!(error.contains(&effect_dir.display().to_string()), "{error}");
        assert!(
            !temp
                .path()
                .join(decision_name(&binding.handoff_id))
                .exists()
        );
        assert!(
            !temp
                .path()
                .join(format!("{}.fresh-grant.json", binding.handoff_id))
                .exists()
        );
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
                remaining: None,
            }],
            completed_unix_seconds: Some(now),
            artifact: "/fresh/effect".into(),
            peer_effect_id: None,
            peer_artifact: None,
        };
        assert_eq!(quota_remaining(&result, now).unwrap(), Some(7500));
        assert_eq!(quota_remaining(&result, now + 30).unwrap(), Some(7500));
        assert_eq!(
            quota_remaining(&result, now + QUOTA_CACHE_TTL_SECONDS - 1).unwrap(),
            Some(7500)
        );
        assert_eq!(
            quota_remaining(&result, now + QUOTA_CACHE_TTL_SECONDS).unwrap(),
            None
        );
        result.windows.push(FreshQuotaWindow {
            used_percent: 100.0,
            resets_at: "2099-01-01T00:00:00Z".into(),
            remaining: None,
        });
        assert_eq!(quota_remaining(&result, now).unwrap(), None);
        result.windows.pop();
        result.windows[0].remaining = Some(0);
        assert_eq!(quota_remaining(&result, now).unwrap(), None);
        result.windows[0].remaining = None;
        result.windows[0].used_percent = 100.0;
        assert_eq!(quota_remaining(&result, now).unwrap(), None);
        result.windows[0].used_percent = 0.0;
        result.windows[0].resets_at = "2020-01-01T00:00:00Z".into();
        assert_eq!(quota_remaining(&result, now).unwrap(), None);
        let parsed = parse_effect_windows(
            r#"{"windows":[{"used_percent":12,"remaining":0,"resets_at":"2099-01-01T00:00:00Z"},{"used_percent":7,"resets_at":"2099-01-01T00:00:00Z"}]}"#,
        )
        .unwrap();
        assert_eq!(parsed[0].remaining, Some(0));
        result.windows = parsed;
        assert_eq!(quota_remaining(&result, now).unwrap(), None);
    }

    #[test]
    fn structured_model_capacity_never_becomes_account_quota_marker() {
        let event = br#"{"type":"error","error":{"data":{"code":"model_at_capacity","message":"quota exhausted for this model"}}}"#;
        assert_eq!(
            classify_terminal_outcome(
                TerminalSignalKind::QuotaExhaustedInband,
                event,
                b"",
                1 << 8,
                false
            ),
            TerminalOutcome::ModelAtCapacity,
        );
        assert!(!TerminalOutcome::ModelAtCapacity.is_marker());
        assert_eq!(
            classify_terminal_outcome(
                TerminalSignalKind::QuotaExhaustedInband,
                b"quota exhausted",
                b"",
                1 << 8,
                false
            ),
            TerminalOutcome::QuotaRejected,
        );
    }

    #[test]
    fn typed_quota_q_excludes_account_even_with_pin_and_falls_back() {
        if std::env::var_os("AGE319_FRESH_TERMINAL_INNER").is_none() {
            let Some(image) = std::env::var_os("OULIPOLY_AGE319_PROVIDER_IMAGE") else {
                return;
            };
            let output = Command::new("unshare")
                .args(["-Urpfm", "--mount-proc"])
                .arg(std::env::current_exe().unwrap())
                .args(["--exact", "linux_main::fresh_provider::tests::typed_quota_q_excludes_account_even_with_pin_and_falls_back", "--nocapture"])
                .env("AGE319_FRESH_TERMINAL_INNER", "1")
                .env("OULIPOLY_AGE319_PROVIDER_IMAGE", image)
                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", "/tmp/fresh-terminal-fixture-socket")
                .output().unwrap();
            assert!(
                output.status.success(),
                "stdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        assert_eq!(unsafe { libc::getpid() }, 1);
        let temporary = tempfile::tempdir().unwrap();
        let old_wal = temporary.path().join("state.db-wal");
        std::fs::write(&old_wal, b"old WAL unchanged").unwrap();
        let image = PathBuf::from(std::env::var("OULIPOLY_AGE319_PROVIDER_IMAGE").unwrap());
        let input = temporary.path().join("input");
        std::fs::write(&input, b"prompt").unwrap();
        let mut actor_child = Command::new("sleep").arg("60").spawn().unwrap();
        let root = PinnedProcess::open(unsafe { libc::getpid() }).unwrap();
        let actor = PinnedProcess::open(actor_child.id() as i32).unwrap();
        let mut binding = fixture_binding(&root, &actor);
        let request =
            |_binding: &Binding, index: Option<usize>, pin: Option<&str>| FreshRouteRequest {
                protocol_version: 4,
                d_key: uuid::Uuid::new_v4().to_string(),
                model: "work".into(),
                config_sha256: "c".repeat(64),
                account: index
                    .map(|i| if i == 0 { "opencode-first" } else { "second" }.to_string()),
                account_identity: index
                    .map(|i| if i == 0 { "opencode-first" } else { "second" }.to_string()),
                index,
                total: 2,
                pin: pin.map(str::to_owned),
                quota_script: None,
                auth_refresh_command: None,
            };
        let make_plan = |index: usize| {
            plan(
                &image,
                temporary.path(),
                &File::open(&input).unwrap(),
                vec![
                    temporary
                        .path()
                        .join(format!("effect-{index}"))
                        .display()
                        .to_string(),
                    if index == 0 { "--quota" } else { "--success" }.into(),
                ],
                vec![("PATH".into(), "/usr/bin:/bin".into())],
            )
            .unwrap()
        };
        let register = |binding: &Binding, pin: Option<&str>| {
            for index in 0..2 {
                register_route_candidate(
                    temporary.path(),
                    binding,
                    &request(binding, Some(index), pin),
                    make_plan(index),
                    if index == 0 {
                        FreshTerminalRecognizer::OpenCode
                    } else {
                        FreshTerminalRecognizer::OpenAiCompat
                    },
                )
                .unwrap();
            }
        };
        register(&binding, None);
        let first =
            select_route(temporary.path(), &binding, &request(&binding, None, None)).unwrap();
        assert_eq!(first.account, "opencode-first");
        let provider_grant = launch(
            prepare(temporary.path(), binding.clone(), make_plan(0)).unwrap(),
            &root,
            &actor,
            0,
            0,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match observe(temporary.path(), &provider_grant).unwrap() {
                Observation::Drained {
                    status, cancelled, ..
                } => {
                    assert_eq!(status, 1 << 8);
                    assert!(!cancelled);
                    break;
                }
                Observation::Unknown => panic!("provider Q became unknown"),
                _ if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
                _ => panic!("provider Q did not drain"),
            }
        }
        binding.handoff_id = uuid::Uuid::new_v4().to_string();
        register(&binding, None);
        let second =
            select_route(temporary.path(), &binding, &request(&binding, None, None)).unwrap();
        assert_eq!(second.account, "second");
        assert_eq!(second.eligible_accounts, ["second"]);
        let record: TerminalRecord =
            exact_file(temporary.path(), &format!("{provider_grant}.terminal.json"))
                .unwrap()
                .unwrap();
        assert_eq!(record.outcome, TerminalOutcome::QuotaRejected);
        let mut pinned = binding.clone();
        pinned.handoff_id = uuid::Uuid::new_v4().to_string();
        register(&pinned, Some("opencode-first"));
        assert!(
            select_route(
                temporary.path(),
                &pinned,
                &request(&pinned, None, Some("opencode-first"))
            )
            .unwrap_err()
            .to_string()
            .contains("no eligible account or pin")
        );
        let mut held_choice = pinned.clone();
        held_choice.handoff_id = uuid::Uuid::new_v4().to_string();
        register(&held_choice, Some("opencode-first"));
        durable_new(
            temporary.path(),
            &decision_name(&held_choice.handoff_id),
            &RouteDecision {
                version: 1,
                binding: held_choice.clone(),
                total: 2,
                pin: Some("opencode-first".into()),
                sequence: 0,
                selection: first,
            },
        )
        .unwrap();
        assert!(
            require_selected_plan(temporary.path(), &held_choice, &make_plan(0))
                .unwrap_err()
                .to_string()
                .contains("no longer eligible before K")
        );
        assert!(
            launch(
                prepare(temporary.path(), held_choice.clone(), make_plan(0)).unwrap(),
                &root,
                &actor,
                0,
                0,
            )
            .is_err(),
            "stale held choice consumed provider K"
        );
        let held_grant = grant_for_binding(temporary.path(), &held_choice)
            .unwrap()
            .unwrap();
        assert!(
            !temporary
                .path()
                .join(format!("{held_grant}.consumed.json"))
                .exists()
        );
        assert_eq!(std::fs::read(old_wal).unwrap(), b"old WAL unchanged");
        actor_child.kill().unwrap();
        actor_child.wait().unwrap();
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
                protocol_version: 4,
                d_key: uuid::Uuid::new_v4().to_string(),
                model: "configured-model".into(),
                config_sha256: "a".repeat(64),
                account: account.map(str::to_owned),
                account_identity: account.map(str::to_owned),
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
                FreshTerminalRecognizer::OpenAiCompat,
            )
            .unwrap();
            register_route_candidate(
                temporary.path(),
                binding,
                &request(Some(1), Some("second")),
                prepare_plan(vec![
                    temporary.path().join("other-effect").display().to_string(),
                ]),
                FreshTerminalRecognizer::OpenAiCompat,
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
            (1, 0, 1, Vec::new())
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
            protocol_version: 4,
            d_key: uuid::Uuid::new_v4().to_string(),
            model: "configured-model".into(),
            config_sha256: "b".repeat(64),
            account: None,
            account_identity: None,
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
            (0, 1, 1, Vec::new()),
            "only physical Q may turn the nonzero exit into failure evidence"
        );
        File::open(temporary.path().join(format!("{id}.drain.json")))
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(std::time::SystemTime::now() - Duration::from_secs(31 * 60)),
            )
            .unwrap();
        assert_eq!(
            route_evidence(temporary.path(), &first_candidate).unwrap(),
            (0, 0, 1, Vec::new()),
            "old completed Q is neither live nor a recent failure"
        );
        assert!(matches!(
            observe(temporary.path(), &id).unwrap(),
            Observation::Drained { .. }
        ));
        let mut third_binding = binding.clone();
        third_binding.handoff_id = uuid::Uuid::new_v4().to_string();
        assert_eq!(
            route(&third_binding, None).account,
            "first",
            "recent failure history must not override the durable round-robin cursor"
        );
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
        let auth_ready = temporary.path().join("auth-ready");
        let auth_release = temporary.path().join("auth-release");
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
        let auth_script = format!(
            "printf ready > '{}'; while test ! -e '{}'; do sleep 0.02; done; printf x >> '{}'",
            auth_ready.display(),
            auth_release.display(),
            marker.display()
        );
        let request =
            |index: usize, account: &str, quota: &str, auth: Option<&str>| FreshRouteRequest {
                protocol_version: 4,
                d_key: uuid::Uuid::new_v4().to_string(),
                model: "model".into(),
                config_sha256: "a".repeat(64),
                account: Some(account.into()),
                account_identity: Some(account.into()),
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
                FreshTerminalRecognizer::OpenAiCompat,
            )
            .unwrap();
        }
        assert!(
            select_route(
                temporary.path(),
                &binding,
                &FreshRouteRequest {
                    protocol_version: 4,
                    d_key: uuid::Uuid::new_v4().to_string(),
                    model: "model".into(),
                    config_sha256: "a".repeat(64),
                    account: None,
                    account_identity: None,
                    index: None,
                    total: 2,
                    pin: None,
                    quota_script: None,
                    auth_refresh_command: None,
                }
            )
            .unwrap_err()
            .to_string()
            .contains("fresh quota effect unknown"),
            "missing quota evidence was not reported as unknown"
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
        let mut follower_binding = fixture_binding(&root, &actor);
        follower_binding.handoff_id = uuid::Uuid::new_v4().to_string();
        register_route_candidate(
            temporary.path(),
            &follower_binding,
            &request(0, "recovering", &shell_script, Some(&auth_script)),
            plan(
                &image,
                temporary.path(),
                &File::open(&input).unwrap(),
                vec!["--recovering".into()],
                vec![],
            )
            .unwrap(),
            FreshTerminalRecognizer::OpenAiCompat,
        )
        .unwrap();
        let follower_first = effect(0, "recovering", FreshAccountEffectKind::QuotaFirst);
        begin_account_effect(
            temporary.path(),
            &follower_binding,
            &follower_first,
            &root,
            &actor,
            0,
            0,
        )
        .unwrap();
        let wait_follower = |request: &FreshAccountEffectRequest| {
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                let readback =
                    observe_account_effect(temporary.path(), &follower_binding, request).unwrap();
                if readback.state == "drained" {
                    return readback;
                }
                assert!(
                    Instant::now() < deadline,
                    "follower effect did not drain: {readback:?}"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        assert_eq!(
            wait_follower(&follower_first).outcome.as_deref(),
            Some("failed")
        );
        let auth = effect(0, "recovering", FreshAccountEffectKind::AuthRefresh);
        let original_auth =
            begin_account_effect(temporary.path(), &binding, &auth, &root, &actor, 0, 0).unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while !auth_ready.exists() {
            assert!(Instant::now() < deadline, "auth effect never entered");
            std::thread::sleep(Duration::from_millis(20));
        }
        let follower_auth = begin_account_effect(
            temporary.path(),
            &follower_binding,
            &auth,
            &root,
            &actor,
            0,
            0,
        )
        .unwrap();
        assert_eq!(
            follower_auth.peer_effect_id.as_deref(),
            Some(original_auth.effect_id.as_str())
        );
        assert_eq!(
            follower_auth.peer_artifact.as_deref(),
            Some(original_auth.artifact.as_str())
        );
        assert_eq!(follower_auth.state, "pending");
        assert!(
            begin_account_effect(
                temporary.path(),
                &follower_binding,
                &auth,
                &root,
                &actor,
                0,
                0,
            )
            .is_err(),
            "follower auth intent was spent twice"
        );
        let retry = effect(0, "recovering", FreshAccountEffectKind::QuotaRetry);
        assert!(
            begin_account_effect(
                temporary.path(),
                &follower_binding,
                &retry,
                &root,
                &actor,
                0,
                0,
            )
            .unwrap_err()
            .to_string()
            .contains("no verified successful auth Q")
        );
        assert!(
            grant_for_binding(
                &effect_directory(temporary.path(), &follower_binding, &auth),
                &follower_binding
            )
            .unwrap()
            .is_none()
        );
        std::fs::write(&auth_release, b"go").unwrap();
        assert_eq!(wait(&auth).outcome.as_deref(), Some("refreshed"));
        let follower_auth = wait_follower(&auth);
        assert_eq!(follower_auth.outcome.as_deref(), Some("refreshed"));
        assert_eq!(
            follower_auth.peer_effect_id.as_deref(),
            Some(original_auth.effect_id.as_str())
        );
        let follower_auth_dir = effect_directory(temporary.path(), &follower_binding, &auth);
        let follower_auth_intent = effect_intent(&follower_auth_dir).unwrap().unwrap();
        let peer_q = effect_physical_q_nanos(&follower_auth_dir, &follower_auth_intent).unwrap();
        let follower_candidate: RouteCandidate = exact_file(
            temporary.path(),
            &candidate_name(&follower_binding.handoff_id, 0),
        )
        .unwrap()
        .unwrap();
        let mut auth_marker = TerminalRecord {
            version: 1,
            binding: binding.clone(),
            selection: FreshRouteSelection {
                model: follower_candidate.model.clone(),
                config_sha256: follower_candidate.config_sha256.clone(),
                account: follower_candidate.account.clone(),
                account_identity: follower_candidate.account_identity.clone(),
                index: 0,
                plan_sha256: follower_candidate.plan_sha256.clone(),
                observed_live: 0,
                observed_failures: 0,
                observed_invocations: 0,
                policy_version: FRESH_ROUTE_POLICY_VERSION.into(),
                eligible_accounts: vec![follower_candidate.account.clone()],
                quota_remaining_basis_points: Some(8000),
            },
            grant_id: uuid::Uuid::new_v4().to_string(),
            physical_q_sha256: "a".repeat(64),
            physical_q_unix_nanos: peer_q - 1,
            signal_kind: "Unknown".into(),
            outcome: TerminalOutcome::AuthRejected,
        };
        assert!(
            marker_allows_candidate(
                temporary.path(),
                &follower_binding,
                &follower_candidate,
                std::slice::from_ref(&auth_marker),
                None,
            )
            .unwrap()
        );
        auth_marker.physical_q_unix_nanos = peer_q;
        assert!(
            !marker_allows_candidate(
                temporary.path(),
                &follower_binding,
                &follower_candidate,
                std::slice::from_ref(&auth_marker),
                None,
            )
            .unwrap()
        );
        assert_eq!(std::fs::read(&marker).unwrap(), b"x");
        assert!(
            begin_account_effect(temporary.path(), &binding, &auth, &root, &actor, 0, 0).is_err()
        );
        begin_account_effect(
            temporary.path(),
            &follower_binding,
            &retry,
            &root,
            &actor,
            0,
            0,
        )
        .unwrap();
        assert_eq!(
            wait_follower(&retry).outcome.as_deref(),
            Some("valid_windows")
        );
        begin_account_effect(temporary.path(), &binding, &retry, &root, &actor, 0, 0).unwrap();
        assert_eq!(wait(&retry).outcome.as_deref(), Some("valid_windows"));
        // A peer's completed nonzero Q is observed as failure, never as
        // permission for a follower quota retry or another auth shellout.
        let mut failing_binding = fixture_binding(&root, &actor);
        failing_binding.handoff_id = uuid::Uuid::new_v4().to_string();
        let mut failing_follower = fixture_binding(&root, &actor);
        failing_follower.handoff_id = uuid::Uuid::new_v4().to_string();
        for bound in [&failing_binding, &failing_follower] {
            register_route_candidate(
                temporary.path(),
                bound,
                &request(0, "failed-auth", "exit 7", Some("exit 19")),
                plan(
                    &image,
                    temporary.path(),
                    &File::open(&input).unwrap(),
                    vec!["--failed-auth".into()],
                    vec![],
                )
                .unwrap(),
                FreshTerminalRecognizer::OpenAiCompat,
            )
            .unwrap();
        }
        let mut failure_effect = effect(0, "failed-auth", FreshAccountEffectKind::QuotaFirst);
        for bound in [&failing_binding, &failing_follower] {
            begin_account_effect(
                temporary.path(),
                bound,
                &failure_effect,
                &root,
                &actor,
                0,
                0,
            )
            .unwrap();
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                let result =
                    observe_account_effect(temporary.path(), bound, &failure_effect).unwrap();
                if result.state == "drained" {
                    assert_eq!(result.outcome.as_deref(), Some("failed"));
                    break;
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        failure_effect.kind = FreshAccountEffectKind::AuthRefresh;
        let failed_source = begin_account_effect(
            temporary.path(),
            &failing_binding,
            &failure_effect,
            &root,
            &actor,
            0,
            0,
        )
        .unwrap();
        let failed_peer = begin_account_effect(
            temporary.path(),
            &failing_follower,
            &failure_effect,
            &root,
            &actor,
            0,
            0,
        )
        .unwrap();
        assert_eq!(
            failed_peer.peer_effect_id.as_deref(),
            Some(failed_source.effect_id.as_str())
        );
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let result =
                observe_account_effect(temporary.path(), &failing_follower, &failure_effect)
                    .unwrap();
            if result.state == "drained" {
                assert_eq!(result.outcome.as_deref(), Some("failed"));
                assert_eq!(
                    result.peer_effect_id.as_deref(),
                    Some(failed_source.effect_id.as_str())
                );
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(20));
        }
        failure_effect.kind = FreshAccountEffectKind::QuotaRetry;
        assert!(
            begin_account_effect(
                temporary.path(),
                &failing_follower,
                &failure_effect,
                &root,
                &actor,
                0,
                0,
            )
            .unwrap_err()
            .to_string()
            .contains("no verified successful auth Q")
        );
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
            FreshTerminalRecognizer::OpenAiCompat,
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
        // A second model may reuse the exact physical Q only when its
        // source-owned account identity and effect commands agree.
        let mut cross_model_binding = fixture_binding(&root, &actor);
        cross_model_binding.handoff_id = uuid::Uuid::new_v4().to_string();
        let mut cross_model_route = request(0, "alias", &shell_script, Some(&auth_script));
        cross_model_route.model = "other-model".into();
        cross_model_route.config_sha256 = "b".repeat(64);
        cross_model_route.account_identity = Some("recovering".into());
        register_route_candidate(
            temporary.path(),
            &cross_model_binding,
            &cross_model_route,
            plan(
                &image,
                temporary.path(),
                &File::open(&input).unwrap(),
                vec!["--alias".into()],
                vec![],
            )
            .unwrap(),
            FreshTerminalRecognizer::OpenAiCompat,
        )
        .unwrap();
        let mut cross_model_effect = sibling_first.clone();
        cross_model_effect.model = cross_model_route.model.clone();
        cross_model_effect.config_sha256 = cross_model_route.config_sha256.clone();
        cross_model_effect.account = "alias".into();
        let cross_model_q = begin_account_effect(
            temporary.path(),
            &cross_model_binding,
            &cross_model_effect,
            &root,
            &actor,
            0,
            0,
        )
        .unwrap();
        assert_eq!(cross_model_q.outcome.as_deref(), Some("valid_windows"));
        assert!(
            grant_for_binding(
                &effect_directory(temporary.path(), &cross_model_binding, &cross_model_effect),
                &cross_model_binding,
            )
            .unwrap()
            .is_none(),
            "cross-model reuse launched a second quota K"
        );

        // The same display label with a distinct explicit identity cannot
        // borrow that Q, even if the script text happens to match.
        let mut collision_binding = fixture_binding(&root, &actor);
        collision_binding.handoff_id = uuid::Uuid::new_v4().to_string();
        let mut collision_route = request(0, "recovering", &shell_script, Some(&auth_script));
        collision_route.account_identity = Some("another-physical-account".into());
        register_route_candidate(
            temporary.path(),
            &collision_binding,
            &collision_route,
            plan(
                &image,
                temporary.path(),
                &File::open(&input).unwrap(),
                vec!["--collision".into()],
                vec![],
            )
            .unwrap(),
            FreshTerminalRecognizer::OpenAiCompat,
        )
        .unwrap();
        let collision_effect = FreshAccountEffectRequest {
            d_key: uuid::Uuid::new_v4().to_string(),
            ..first.clone()
        };
        begin_account_effect(
            temporary.path(),
            &collision_binding,
            &collision_effect,
            &root,
            &actor,
            0,
            0,
        )
        .unwrap();
        assert!(
            grant_for_binding(
                &effect_directory(temporary.path(), &collision_binding, &collision_effect),
                &collision_binding,
            )
            .unwrap()
            .is_some(),
            "different physical identity borrowed another account Q"
        );
        let deadline = Instant::now() + Duration::from_secs(15);
        while observe_account_effect(temporary.path(), &collision_binding, &collision_effect)
            .unwrap()
            .state
            != "drained"
        {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(20));
        }
        let sibling_candidate: RouteCandidate = exact_file(
            temporary.path(),
            &candidate_name(&sibling_binding.handoff_id, 0),
        )
        .unwrap()
        .unwrap();
        assert!(
            effect_intent(&effect_directory(temporary.path(), &sibling_binding, &auth))
                .unwrap()
                .is_none()
        );
        auth_marker.physical_q_unix_nanos = peer_q - 1;
        assert!(
            marker_allows_candidate(
                temporary.path(),
                &sibling_binding,
                &sibling_candidate,
                std::slice::from_ref(&auth_marker),
                None,
            )
            .unwrap(),
            "a verified successful auth Q did not release a later root"
        );
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
        let mut pending_binding = binding.clone();
        pending_binding.handoff_id = uuid::Uuid::new_v4().to_string();
        let mut pending_candidate: RouteCandidate =
            exact_file(temporary.path(), &candidate_name(&binding.handoff_id, 0))
                .unwrap()
                .unwrap();
        pending_candidate.binding = pending_binding.clone();
        durable_new(
            temporary.path(),
            &candidate_name(&pending_binding.handoff_id, 0),
            &pending_candidate,
        )
        .unwrap();
        let pending_dir = effect_directory(temporary.path(), &pending_binding, &first);
        std::fs::create_dir(&pending_dir).unwrap();
        let pending_intent = AccountEffectIntent {
            version: 1,
            id: uuid::Uuid::new_v4().to_string(),
            binding: pending_binding.clone(),
            request: redacted_effect_request(&first),
            environment_sha256: environment_digest(&first).unwrap(),
            plan_sha256: "pending-plan".into(),
            auth_source: None,
        };
        durable_new(&pending_dir, "intent.json", &pending_intent).unwrap();
        assert_eq!(
            reusable_quota_source(temporary.path(), &sibling_binding, &first)
                .unwrap()
                .unwrap()
                .1
                .id,
            pending_intent.id,
            "a cached Q hid a matching unresolved quota effect"
        );
        let mut another_binding = binding.clone();
        another_binding.handoff_id = uuid::Uuid::new_v4().to_string();
        register_route_candidate(
            temporary.path(),
            &another_binding,
            &request(0, "recovering", &shell_script, Some(&auth_script)),
            plan(
                &image,
                temporary.path(),
                &File::open(&input).unwrap(),
                vec!["--recovering".into()],
                vec![],
            )
            .unwrap(),
            FreshTerminalRecognizer::OpenAiCompat,
        )
        .unwrap();
        let blocked = begin_account_effect(
            temporary.path(),
            &another_binding,
            &first,
            &root,
            &actor,
            0,
            0,
        )
        .unwrap();
        assert_eq!(blocked.state, "unknown");
        assert!(
            blocked
                .artifact
                .contains(&pending_dir.display().to_string())
        );
        assert!(
            grant_for_binding(
                &effect_directory(temporary.path(), &another_binding, &first),
                &another_binding
            )
            .unwrap()
            .is_none()
        );
        let negative = effect(1, "exhausted", FreshAccountEffectKind::QuotaFirst);
        begin_account_effect(temporary.path(), &binding, &negative, &root, &actor, 0, 0).unwrap();
        assert_eq!(wait(&negative).outcome.as_deref(), Some("valid_windows"));
        let mut exhausted_sibling = binding.clone();
        exhausted_sibling.handoff_id = uuid::Uuid::new_v4().to_string();
        let mut exhausted_candidate: RouteCandidate =
            exact_file(temporary.path(), &candidate_name(&binding.handoff_id, 1))
                .unwrap()
                .unwrap();
        exhausted_candidate.binding = exhausted_sibling.clone();
        durable_new(
            temporary.path(),
            &candidate_name(&exhausted_sibling.handoff_id, 1),
            &exhausted_candidate,
        )
        .unwrap();
        assert!(
            reusable_quota_source(temporary.path(), &exhausted_sibling, &negative)
                .unwrap()
                .is_some(),
            "an authoritative exhausted Q should remain cached until a fresh probe is due"
        );
        assert!(
            select_route(
                temporary.path(),
                &binding,
                &FreshRouteRequest {
                    protocol_version: 4,
                    d_key: uuid::Uuid::new_v4().to_string(),
                    model: "model".into(),
                    config_sha256: "a".repeat(64),
                    account: None,
                    account_identity: None,
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
        let unresolved = select_route(
            temporary.path(),
            &binding,
            &FreshRouteRequest {
                protocol_version: 4,
                d_key: uuid::Uuid::new_v4().to_string(),
                model: "model".into(),
                config_sha256: "a".repeat(64),
                account: None,
                account_identity: None,
                index: None,
                total: 2,
                pin: None,
                quota_script: None,
                auth_refresh_command: None,
            },
        )
        .unwrap_err()
        .to_string();
        assert!(
            unresolved.contains("newer or unresolved quota effect"),
            "{unresolved}"
        );
        // This pending intent was synthesized above without a real K; remove
        // only that test artifact to exercise the settled branch below.
        std::fs::remove_dir_all(&pending_dir).unwrap();
        let selection = select_route(
            temporary.path(),
            &binding,
            &FreshRouteRequest {
                protocol_version: 4,
                d_key: uuid::Uuid::new_v4().to_string(),
                model: "model".into(),
                config_sha256: "a".repeat(64),
                account: None,
                account_identity: None,
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
            protocol_version: 4,
            d_key: uuid::Uuid::new_v4().to_string(), model: "model".into(),
            config_sha256: "a".repeat(64), account: Some("slow".into()),
            account_identity: Some("slow".into()),
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
            FreshTerminalRecognizer::OpenAiCompat,
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
            FreshTerminalRecognizer::OpenAiCompat,
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
                    protocol_version: 4,
                    account: None,
                    account_identity: None,
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
        // The synthetic pending intent above was removed after its refusal;
        // its dependent alias has no independent physical K.
        std::fs::remove_dir_all(effect_directory(temporary.path(), &another_binding, &first))
            .unwrap();
        let retry_dir = effect_directory(temporary.path(), &binding, &retry);
        let retry_intent = effect_intent(&retry_dir).unwrap().unwrap();
        let old_healthy_q = effect_physical_q_nanos(&retry_dir, &retry_intent).unwrap();
        let selected_candidate: RouteCandidate =
            exact_file(temporary.path(), &candidate_name(&binding.handoff_id, 0))
                .unwrap()
                .unwrap();
        let marker_grant = uuid::Uuid::new_v4().to_string();
        let terminal_marker = TerminalRecord {
            version: 1,
            binding: binding.clone(),
            selection: FreshRouteSelection {
                model: "model".into(),
                config_sha256: "a".repeat(64),
                account: "recovering".into(),
                account_identity: "recovering".into(),
                index: 0,
                plan_sha256: selected_candidate.plan_sha256.clone(),
                observed_live: 0,
                observed_failures: 0,
                observed_invocations: 1,
                policy_version: FRESH_ROUTE_POLICY_VERSION.into(),
                eligible_accounts: vec!["recovering".into()],
                quota_remaining_basis_points: Some(8000),
            },
            grant_id: marker_grant.clone(),
            physical_q_sha256: "q".repeat(64),
            physical_q_unix_nanos: old_healthy_q,
            signal_kind: "QuotaExhaustedInband".into(),
            outcome: TerminalOutcome::QuotaRejected,
        };
        durable_new(
            temporary.path(),
            &format!("{marker_grant}.terminal.json"),
            &terminal_marker,
        )
        .unwrap();
        let cross_model_candidate: RouteCandidate = exact_file(
            temporary.path(),
            &candidate_name(&cross_model_binding.handoff_id, 0),
        )
        .unwrap()
        .unwrap();
        assert!(
            !marker_allows_candidate(
                temporary.path(),
                &cross_model_binding,
                &cross_model_candidate,
                std::slice::from_ref(&terminal_marker),
                Some(old_healthy_q),
            )
            .unwrap(),
            "another model accepted a pre-rejection Q for the same physical account"
        );
        let mut verification_binding = binding.clone();
        verification_binding.handoff_id = uuid::Uuid::new_v4().to_string();
        register_route_candidate(
            temporary.path(),
            &verification_binding,
            &request(0, "recovering", &shell_script, Some(&auth_script)),
            plan(
                &image,
                temporary.path(),
                &File::open(&input).unwrap(),
                vec!["--recovering".into()],
                vec![],
            )
            .unwrap(),
            FreshTerminalRecognizer::OpenAiCompat,
        )
        .unwrap();
        let verify = effect(0, "recovering", FreshAccountEffectKind::QuotaFirst);
        assert!(
            reusable_quota_source(temporary.path(), &verification_binding, &verify)
                .unwrap()
                .is_none(),
            "pre-rejection healthy Q was offered as marker verification"
        );
        begin_account_effect(
            temporary.path(),
            &verification_binding,
            &verify,
            &root,
            &actor,
            0,
            0,
        )
        .unwrap();
        let verify_dir = effect_directory(temporary.path(), &verification_binding, &verify);
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let result =
                observe_account_effect(temporary.path(), &verification_binding, &verify).unwrap();
            if result.state == "drained" {
                assert_eq!(result.outcome.as_deref(), Some("valid_windows"));
                break;
            }
            assert!(Instant::now() < deadline, "new quota Q did not drain");
            std::thread::sleep(Duration::from_millis(20));
        }
        let verify_intent = effect_intent(&verify_dir).unwrap().unwrap();
        let new_q = effect_physical_q_nanos(&verify_dir, &verify_intent).unwrap();
        assert!(new_q > old_healthy_q);
        let verify_candidate: RouteCandidate = exact_file(
            temporary.path(),
            &candidate_name(&verification_binding.handoff_id, 0),
        )
        .unwrap()
        .unwrap();
        assert!(
            marker_allows_candidate(
                temporary.path(),
                &verification_binding,
                &verify_candidate,
                std::slice::from_ref(&terminal_marker),
                Some(new_q),
            )
            .unwrap()
        );
        actor_child.kill().unwrap();
        actor_child.wait().unwrap();
    }
}
