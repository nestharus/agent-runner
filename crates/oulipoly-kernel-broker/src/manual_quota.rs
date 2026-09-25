//! Broker-custodied local manual quota K/Q. This private operation has no D
//! key and cannot launch a model. Its source is the pinned config directory;
//! the caller supplies only an account selector and effect environment.
use chrono::Utc;
use oulipoly_kernel_broker::protocol::{ManualQuotaReadback, ManualQuotaRequest};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const MAX_OUTPUT: u64 = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Intent {
    version: u32,
    pub(super) request: ManualQuotaRequest,
    pub(super) environment_sha256: String,
    peer_uid: u32,
    peer_gid: u32,
    pub(super) physical_account_id: String,
    pub(super) quota_script: Option<String>,
    pub(super) auth_refresh_command: Option<String>,
    source_device: u64,
    source_inode: u64,
    pub(super) effect_id: Option<String>,
    pub(super) source_operation_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PhysicalQ {
    version: u32,
    effect_id: String,
    wait_success: bool,
    stdout_len: u64,
    stdout_sha256: String,
    stderr_len: u64,
    stderr_sha256: String,
    completed_unix_seconds: i64,
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn environment_digest(environment: &[(String, String)]) -> io::Result<String> {
    Ok(sha(&serde_json::to_vec(environment)?))
}

fn redacted_request(request: &ManualQuotaRequest) -> ManualQuotaRequest {
    ManualQuotaRequest {
        environment: Vec::new(),
        ..request.clone()
    }
}

fn durable_new<T: Serialize>(dir: &Path, name: &str, value: &T) -> io::Result<()> {
    let opened = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(dir.join(name));
    super::fresh_index::reader_open_attempt();
    if opened.is_ok() {
        super::fresh_index::reader_opened();
    }
    let mut file = opened?;
    let bytes = serde_json::to_vec(value)?;
    file.write_all(&bytes)?;
    super::fresh_index::reader_bytes_written(bytes.len() as u64);
    file.write_all(b"\n")?;
    super::fresh_index::reader_bytes_written(1);
    file.sync_all()?;
    let opened = File::open(dir);
    super::fresh_index::reader_open_attempt();
    if opened.is_ok() {
        super::fresh_index::reader_opened();
    }
    opened?.sync_all()
}

fn read_exact<T: for<'a> Deserialize<'a>>(dir: &Path, name: &str) -> io::Result<Option<T>> {
    let opened = File::open(dir.join(name));
    super::fresh_index::reader_open_attempt();
    if opened.is_ok() {
        super::fresh_index::reader_opened();
    }
    match opened {
        Ok(mut file) => {
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            super::fresh_index::reader_bytes_parsed(bytes.len() as u64);
            serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(io::Error::other)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn read_bytes(path: &Path) -> io::Result<Vec<u8>> {
    let opened = File::open(path);
    super::fresh_index::reader_open_attempt();
    if opened.is_ok() {
        super::fresh_index::reader_opened();
    }
    let mut file = opened?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    super::fresh_index::reader_bytes_parsed(bytes.len() as u64);
    Ok(bytes)
}

fn operation_dir(directory: &Path, id: &str) -> PathBuf {
    directory.join("manual-quota").join(id)
}

pub(super) fn v3_intent(directory: &Path, id: &str) -> io::Result<Intent> {
    if !valid_id(id) {
        return Err(io::Error::other("v3 manual operation ID invalid"));
    }
    let intent: Intent = read_exact(&operation_dir(directory, id), "intent.json")?
        .ok_or_else(|| io::Error::other("manual quota operation absent"))?;
    if intent.version != 1 || intent.request.operation_id != id {
        return Err(io::Error::other("v3 manual intent identity changed"));
    }
    Ok(intent)
}

pub(super) fn v3_readback(directory: &Path, intent: &Intent) -> io::Result<ManualQuotaReadback> {
    readback_intent(directory, intent)
}

pub(super) fn v3_source_key(intent: &Intent) -> io::Result<super::fresh_index::SourceKey> {
    Ok(super::fresh_index::SourceKey {
        commands_sha256: sha(&serde_json::to_vec(&(
            &intent.quota_script,
            &intent.auth_refresh_command,
        ))?),
        environment_sha256: intent.environment_sha256.clone(),
    })
}

/// Recheck the original manual Q's pinned config when another root reuses it.
/// A matching command digest alone cannot certify a replaced config source.
pub(super) fn v3_validate_origin_source(source: &Path, intent: &Intent) -> io::Result<()> {
    let meta = source.metadata()?;
    let pool = oulipoly_runtime::executor::cli::fresh_remote::load_fresh_headless_pool(
        source,
        &intent.request.model,
    )
    .map_err(io::Error::other)?;
    let index = pool
        .model
        .providers
        .iter()
        .position(|member| member.name == intent.request.account)
        .ok_or_else(|| io::Error::other("shared manual origin account absent"))?;
    if !meta.is_dir()
        || meta.dev() != intent.source_device
        || meta.ino() != intent.source_inode
        || pool.config_sha256 != intent.request.config_sha256
        || pool.account_identities[index].as_deref() != Some(intent.physical_account_id.as_str())
        || pool.account_effects[index]
            != (
                intent.quota_script.clone(),
                intent.auth_refresh_command.clone(),
            )
    {
        return Err(io::Error::other(
            "shared manual origin config/source changed",
        ));
    }
    Ok(())
}

fn valid_id(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|parsed| !parsed.is_nil() && parsed.to_string() == id)
}

fn validate_environment(env: &[(String, String)]) -> io::Result<()> {
    let mut keys = std::collections::HashSet::new();
    if env.len() > 256
        || env.windows(2).any(|pair| pair[0].0 >= pair[1].0)
        || env.iter().any(|(key, value)| {
            key.is_empty()
                || key.contains(['=', '\0'])
                || value.contains('\0')
                || !keys.insert(key)
                || oulipoly_runtime::executor::cli::fresh_remote::forbidden_fresh_environment(key)
        })
    {
        return Err(io::Error::other("manual quota environment invalid"));
    }
    Ok(())
}

fn source_intent(
    source: &File,
    request: &ManualQuotaRequest,
    uid: u32,
    gid: u32,
) -> io::Result<Intent> {
    if !source.metadata()?.is_dir() || !valid_id(&request.operation_id) {
        return Err(io::Error::other("manual quota source or operation invalid"));
    }
    validate_environment(&request.environment)?;
    let path = PathBuf::from(format!("/proc/self/fd/{}", source.as_raw_fd()));
    let pool = oulipoly_runtime::executor::cli::fresh_remote::load_fresh_headless_pool(
        &path,
        &request.model,
    )
    .map_err(io::Error::other)?;
    if pool.config_sha256 != request.config_sha256 {
        return Err(io::Error::other("manual quota config source changed"));
    }
    let index = pool
        .model
        .providers
        .iter()
        .position(|member| member.name == request.account)
        .ok_or_else(|| io::Error::other("manual quota account absent from source model"))?;
    let provider = &pool.model.providers[index];
    for (key, value) in &provider.environment {
        if !request
            .environment
            .iter()
            .any(|pair| pair == &(key.clone(), value.clone()))
        {
            return Err(io::Error::other(
                "manual quota pinned provider environment mismatch",
            ));
        }
    }
    for key in &provider.unset_environment {
        if !provider.environment.contains_key(key)
            && request.environment.iter().any(|pair| &pair.0 == key)
        {
            return Err(io::Error::other(
                "manual quota pinned provider environment removal missing",
            ));
        }
    }
    let physical_account_id = pool.account_identities[index]
        .clone()
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| io::Error::other("manual quota physical account ID absent"))?;
    let meta = source.metadata()?;
    Ok(Intent {
        version: 1,
        request: redacted_request(request),
        environment_sha256: environment_digest(&request.environment)?,
        peer_uid: uid,
        peer_gid: gid,
        physical_account_id,
        quota_script: pool.account_effects[index].0.clone(),
        auth_refresh_command: pool.account_effects[index].1.clone(),
        source_device: meta.dev(),
        source_inode: meta.ino(),
        effect_id: pool.account_effects[index]
            .0
            .as_ref()
            .map(|_| uuid::Uuid::new_v4().to_string()),
        source_operation_id: None,
    })
}

fn same_source(a: &Intent, b: &Intent) -> bool {
    a.physical_account_id == b.physical_account_id
        && a.quota_script == b.quota_script
        && a.auth_refresh_command == b.auth_refresh_command
        && a.environment_sha256 == b.environment_sha256
        && a.source_device == b.source_device
        && a.source_inode == b.source_inode
}

fn indexed_artifact(
    index: &super::fresh_index::Index,
    path: &Path,
) -> io::Result<super::fresh_index::Artifact> {
    let root = index.evidence_root();
    let relative = path
        .strip_prefix(root)
        .map_err(|_| io::Error::other("manual quota artifact outside broker root"))?;
    super::fresh_index::Artifact::from_existing(root, relative).map_err(io::Error::other)
}

/// Project one retained manual operation. The retained intent is the exact
/// source/physical-account grant; only its source operation owns a physical K.
/// A CAS reply may be lost, so every transition is followed by exact readback.
fn reconcile_indexed_manual(index: &super::fresh_index::Index, intent: &Intent) -> io::Result<()> {
    use super::fresh_index::{
        AccountUpdate, EffectIntent, EffectKind, PhysicalQ as IndexedQ, SourceKey,
    };
    let root = index.evidence_root();
    let id = &intent.request.operation_id;
    let dir = operation_dir(root, id);
    if !valid_id(id)
        || intent.version != 1
        || intent.physical_account_id.is_empty()
        || intent.request.environment.len() != 0
        || intent.environment_sha256.len() != 64
        || !intent
            .environment_sha256
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
        || read_exact::<Intent>(&dir, "intent.json")?.as_ref() != Some(intent)
    {
        return Err(io::Error::other("indexed manual intent/source changed"));
    }
    let source = SourceKey {
        commands_sha256: sha(&serde_json::to_vec(&(
            &intent.quota_script,
            &intent.auth_refresh_command,
        ))?),
        environment_sha256: intent.environment_sha256.clone(),
    };
    let announced = EffectIntent {
        kind: EffectKind::ManualQuota,
        source,
        decision_handoff: String::new(),
        route_source: None,
        candidate: None,
        intent: indexed_artifact(index, &dir.join("intent.json"))?,
        reuse: None,
        consumed_k: None,
        certified_q: None,
        result: None,
    };
    if let Some(source_id) = &intent.source_operation_id {
        if !valid_id(source_id) || source_id == id {
            return Err(io::Error::other("indexed manual reuse source invalid"));
        }
    }
    // This also validates a follower's exact source identity and any
    // independently observed physical Q before it can enter the index.
    let readback = readback_intent(root, intent)?;
    let k: Option<serde_json::Value> = read_exact(&dir, "k.json")?;
    if (intent.source_operation_id.is_some() || intent.quota_script.is_none()) && k.is_some() {
        return Err(io::Error::other(
            "indexed manual nonphysical operation has K",
        ));
    }
    if let Some(k) = &k {
        if k.get("version").and_then(|v| v.as_u64()) != Some(1)
            || k.get("operation_id").and_then(|v| v.as_str()) != Some(id)
            || k.get("effect_id").and_then(|v| v.as_str()) != intent.effect_id.as_deref()
        {
            return Err(io::Error::other("indexed manual K identity changed"));
        }
    }
    let key = &intent.physical_account_id;
    let mut account = index.account(key).map_err(io::Error::other)?;
    if let Some(existing) = account.effects.get(id) {
        if existing.kind != announced.kind
            || existing.source != announced.source
            || existing.intent != announced.intent
            || existing.decision_handoff != announced.decision_handoff
            || existing.route_source.is_some()
            || existing.candidate.is_some()
            || existing.reuse.is_some()
        {
            return Err(io::Error::other("indexed manual announcement changed"));
        }
    } else {
        if k.is_some() || dir.join("q.json").exists() {
            return Err(io::Error::other(
                "physical manual K/Q lacks indexed announcement",
            ));
        }
        let update = index.update_account(
            key,
            account.revision,
            AccountUpdate::AnnounceEffect {
                id: id.clone(),
                effect: announced.clone(),
            },
        );
        account = index.account(key).map_err(io::Error::other)?;
        if account.effects.get(id) != Some(&announced) {
            return Err(io::Error::other(format!(
                "indexed manual announcement failed: {update:?}"
            )));
        }
    }
    if k.is_some() {
        let k_ref = indexed_artifact(index, &dir.join("k.json"))?;
        if account.effects[id].consumed_k.as_ref() != Some(&k_ref) {
            if account.effects[id].consumed_k.is_some() {
                return Err(io::Error::other("indexed manual K changed"));
            }
            let update = index.update_account(
                key,
                account.revision,
                AccountUpdate::ConsumeEffect {
                    id: id.clone(),
                    k: k_ref.clone(),
                },
            );
            account = index.account(key).map_err(io::Error::other)?;
            if account.effects[id].consumed_k.as_ref() != Some(&k_ref) {
                return Err(io::Error::other(format!(
                    "indexed manual K publication failed: {update:?}"
                )));
            }
        }
        if readback.state == "drained" {
            let q_path = dir.join("q.json");
            let q_ref = indexed_artifact(index, &q_path)?;
            let q = IndexedQ {
                physical_k: k_ref,
                q: q_ref.clone(),
                terminal: None,
                completed_unix_nanos: i64::try_from(physical_q_nanos(root, id)?)
                    .map_err(io::Error::other)?,
            };
            let indexed = &account.effects[id];
            if indexed.certified_q.as_ref() != Some(&q) || indexed.result.as_ref() != Some(&q_ref) {
                if indexed.certified_q.is_some() || indexed.result.is_some() {
                    return Err(io::Error::other("indexed manual Q changed"));
                }
                // The typed readback certifies complete stdout/stderr and Q.
                // Keep Q as the result identity for admission and restart.
                let update = index.update_account(
                    key,
                    account.revision,
                    AccountUpdate::SettleEffect {
                        id: id.clone(),
                        q: q.clone(),
                        result: Some(q_ref.clone()),
                        marker: (readback.outcome.as_deref() == Some("valid_windows"))
                            .then_some(false),
                    },
                );
                account = index.account(key).map_err(io::Error::other)?;
                if account.effects[id].certified_q.as_ref() != Some(&q)
                    || account.effects[id].result.as_ref() != Some(&q_ref)
                {
                    return Err(io::Error::other(format!(
                        "indexed manual Q publication failed: {update:?}"
                    )));
                }
            }
        } else if account.effects[id].certified_q.is_some() {
            return Err(io::Error::other("indexed manual Q lost physical readback"));
        }
    } else if account.effects[id].consumed_k.is_some() || dir.join("q.json").exists() {
        return Err(io::Error::other(
            "indexed manual K/Q lost physical reference",
        ));
    }
    Ok(())
}

pub(super) fn reconcile_live_manual_accounts(index: &super::fresh_index::Index) -> io::Result<()> {
    let parent = index.evidence_root().join("manual-quota");
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            return Err(io::Error::other(
                "indexed manual operation is not directory",
            ));
        }
        let dir = entry.path();
        let intent: Intent = read_exact(&dir, "intent.json")?
            .ok_or_else(|| io::Error::other("indexed manual intent absent"))?;
        if dir != operation_dir(index.evidence_root(), &intent.request.operation_id) {
            return Err(io::Error::other("indexed manual operation path changed"));
        }
        reconcile_indexed_manual(index, &intent)?;
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn begin(
    directory: &Path,
    source: &File,
    request: &ManualQuotaRequest,
    uid: u32,
    gid: u32,
) -> io::Result<ManualQuotaReadback> {
    begin_indexed(directory, source, request, uid, gid, None)
}

pub(super) fn begin_indexed(
    directory: &Path,
    source: &File,
    request: &ManualQuotaRequest,
    uid: u32,
    gid: u32,
    index: Option<&super::fresh_index::Index>,
) -> io::Result<ManualQuotaReadback> {
    let mut intent = source_intent(source, request, uid, gid)?;
    let parent = directory.join("manual-quota");
    fs::create_dir_all(&parent)?;
    let _lock = super::fresh_provider::auth_admission_lock(directory, &intent.physical_account_id)?;
    let own = operation_dir(directory, &request.operation_id);
    if own.exists() {
        let old: Intent = read_exact(&own, "intent.json")?
            .ok_or_else(|| io::Error::other("manual quota prior intent unknown"))?;
        if old.version != 1
            || old.request != redacted_request(request)
            || old.environment_sha256 != environment_digest(&request.environment)?
            || old.peer_uid != uid
            || old.peer_gid != gid
            || old.physical_account_id != intent.physical_account_id
            || old.quota_script != intent.quota_script
            || old.auth_refresh_command != intent.auth_refresh_command
            || old.source_device != intent.source_device
            || old.source_inode != intent.source_inode
        {
            return Err(io::Error::other("manual quota operation/source mismatch"));
        }
        if let Some(index) = index {
            reconcile_indexed_manual(index, &old)?;
        }
        return readback(directory, request, uid, gid);
    }
    if let Some(artifact) = super::fresh_provider::unresolved_account_effect_for_physical(
        directory,
        &intent.physical_account_id,
    )? {
        return Err(io::Error::other(format!(
            "manual quota prior route K/Q unknown: {artifact}"
        )));
    }
    // An unresolved prior K is never replaced, even when its source differs.
    let mut previous = Vec::new();
    for entry in fs::read_dir(&parent)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        if let Some(prior) = read_exact::<Intent>(&entry.path(), "intent.json")? {
            if prior.physical_account_id == intent.physical_account_id
                && prior.source_operation_id.is_none()
            {
                previous.push(prior);
            }
        }
    }
    previous.sort_by(|a, b| a.request.operation_id.cmp(&b.request.operation_id));
    for prior in &previous {
        let state = readback_intent(directory, prior)?;
        if state.state != "drained" && state.state != "unmetered" {
            if !same_source(&intent, prior) {
                return Err(io::Error::other(format!(
                    "manual quota prior K unknown: {}",
                    state.artifact
                )));
            }
            intent.source_operation_id = Some(prior.request.operation_id.clone());
            intent.effect_id = prior.effect_id.clone();
            break;
        }
    }
    fs::create_dir(&own)?;
    File::open(&parent)?.sync_all()?;
    durable_new(&own, "intent.json", &intent)?;
    if let Some(index) = index {
        // No physical K may be published until the exact source, account,
        // intent, and index generation have been announced and read back.
        reconcile_indexed_manual(index, &intent)?;
    }
    if intent.source_operation_id.is_some() || intent.quota_script.is_none() {
        return readback_intent(directory, &intent);
    }
    // K is durable before the worker is started. A failed spawn remains
    // unknown; neither a broker restart nor another request may replay it.
    durable_new(
        &own,
        "k.json",
        &serde_json::json!({
            "version": 1, "effect_id": intent.effect_id,
            "operation_id": request.operation_id,
        }),
    )?;
    if let Some(index) = index {
        // A failed post-K CAS leaves one-use debt; never start the worker.
        reconcile_indexed_manual(index, &intent)?;
    }
    #[cfg(not(test))]
    {
        let executable = std::env::current_exe()?;
        Command::new(executable)
            .arg("--manual-quota-worker")
            .arg(&own)
            .env_clear()
            .envs(request.environment.iter().cloned())
            .env(
                "OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1",
                std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1")
                    .ok_or_else(|| io::Error::other("manual quota fixture socket absent"))?,
            )
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
    }
    readback_intent(directory, &intent)
}

/// Private v3 manual entry. The account flight is read by exact key while the
/// physical account admission lock is held. An unresolved K is never replayed.
pub(super) fn begin_v3(
    directory: &Path,
    source: &File,
    request: &ManualQuotaRequest,
    uid: u32,
    gid: u32,
    generation: &super::fresh_index::KeyedGeneration,
) -> io::Result<ManualQuotaReadback> {
    let mut intent = source_intent(source, request, uid, gid)?;
    let source_path = PathBuf::from(format!("/proc/self/fd/{}", source.as_raw_fd()));
    let pool = oulipoly_runtime::executor::cli::fresh_remote::load_fresh_headless_pool(
        &source_path,
        &request.model,
    )
    .map_err(io::Error::other)?;
    let index = pool
        .model
        .providers
        .iter()
        .position(|p| p.name == request.account)
        .ok_or_else(|| io::Error::other("v3 manual source account absent"))?;
    oulipoly_runtime::executor::cli::fresh_remote::validate_fresh_headless_shape(
        &pool.model,
        index,
        Path::new("/"),
    )
    .map_err(|error| io::Error::other(format!("v3 manual source unsupported: {error}")))?;
    let admitted = generation.admitted_source().map_err(io::Error::other)?;
    let source_meta = source.metadata()?;
    let admitted_meta = admitted.metadata()?;
    if (source_meta.dev(), source_meta.ino()) != (admitted_meta.dev(), admitted_meta.ino()) {
        return Err(io::Error::other(
            "v3 manual config directory differs from admission",
        ));
    }
    generation
        .check_manual_model(&request.model, &request.config_sha256)
        .map_err(io::Error::other)?;
    let parent = directory.join("manual-quota");
    fs::create_dir_all(&parent)?;
    let _lock = super::fresh_provider::auth_admission_lock(directory, &intent.physical_account_id)?;
    let own = operation_dir(directory, &request.operation_id);
    if own.exists() {
        let old = v3_intent(directory, &request.operation_id)?;
        if old.request != redacted_request(request)
            || old.environment_sha256 != environment_digest(&request.environment)?
            || old.peer_uid != uid
            || old.peer_gid != gid
            || !same_source(&old, &intent)
        {
            return Err(io::Error::other("v3 manual operation/source mismatch"));
        }
        return generation.observe_manual(&old).map_err(io::Error::other);
    }
    if intent.quota_script.is_some() {
        if let Some(peer) = generation
            .manual_flight(&intent.physical_account_id)
            .map_err(io::Error::other)?
        {
            let prior = v3_intent(directory, &peer)?;
            if prior.source_operation_id.is_some() || !same_source(&intent, &prior) {
                return Err(io::Error::other("v3 manual prior physical K unknown"));
            }
            intent.source_operation_id = Some(peer);
            intent.effect_id = prior.effect_id;
        }
    }
    fs::create_dir(&own)?;
    File::open(&parent)?.sync_all()?;
    durable_new(&own, "intent.json", &intent)?;
    let revision = generation
        .announce_manual(&intent)
        .map_err(io::Error::other)?;
    if intent.source_operation_id.is_some() || intent.quota_script.is_none() {
        return generation.observe_manual(&intent).map_err(io::Error::other);
    }
    let before_k = source_intent(source, request, uid, gid)?;
    if !same_source(&intent, &before_k)
        || before_k.request != intent.request
        || before_k.peer_uid != intent.peer_uid
        || before_k.peer_gid != intent.peer_gid
    {
        return Err(io::Error::other("v3 manual source changed before K"));
    }
    durable_new(
        &own,
        "k.json",
        &serde_json::json!({
            "version": 1, "effect_id": intent.effect_id,
            "operation_id": request.operation_id,
        }),
    )?;
    generation
        .record_manual_k(&intent, revision)
        .map_err(io::Error::other)?;
    #[cfg(not(test))]
    {
        let executable = std::env::current_exe()?;
        Command::new(executable)
            .arg("--manual-quota-worker")
            .arg(&own)
            .env_clear()
            .envs(request.environment.iter().cloned())
            .env(
                "OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1",
                std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1")
                    .ok_or_else(|| io::Error::other("manual quota fixture socket absent"))?,
            )
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
    }
    generation.observe_manual(&intent).map_err(io::Error::other)
}

pub(super) fn readback_id_v3(
    directory: &Path,
    operation_id: &str,
    uid: u32,
    gid: u32,
    generation: &super::fresh_index::KeyedGeneration,
) -> io::Result<ManualQuotaReadback> {
    let intent = v3_intent(directory, operation_id)?;
    if intent.peer_uid != uid || intent.peer_gid != gid {
        return Err(io::Error::other("v3 manual readback peer changed"));
    }
    generation.observe_manual(&intent).map_err(io::Error::other)
}

pub(super) fn readback(
    directory: &Path,
    request: &ManualQuotaRequest,
    uid: u32,
    gid: u32,
) -> io::Result<ManualQuotaReadback> {
    if !valid_id(&request.operation_id) {
        return Err(io::Error::other("manual quota operation ID invalid"));
    }
    let dir = operation_dir(directory, &request.operation_id);
    let intent: Intent = read_exact(&dir, "intent.json")?
        .ok_or_else(|| io::Error::other("manual quota operation absent"))?;
    if intent.version != 1
        || intent.request != redacted_request(request)
        || intent.environment_sha256 != environment_digest(&request.environment)?
        || intent.peer_uid != uid
        || intent.peer_gid != gid
    {
        return Err(io::Error::other("manual quota readback identity mismatch"));
    }
    readback_intent(directory, &intent)
}

pub(super) fn readback_id_indexed(
    directory: &Path,
    operation_id: &str,
    uid: u32,
    gid: u32,
    index: Option<&super::fresh_index::Index>,
) -> io::Result<ManualQuotaReadback> {
    if !valid_id(operation_id) {
        return Err(io::Error::other("manual quota operation ID invalid"));
    }
    let dir = operation_dir(directory, operation_id);
    let intent: Intent = read_exact(&dir, "intent.json")?
        .ok_or_else(|| io::Error::other("manual quota operation absent"))?;
    if intent.version != 1
        || intent.request.operation_id != operation_id
        || intent.peer_uid != uid
        || intent.peer_gid != gid
    {
        return Err(io::Error::other("manual quota readback identity mismatch"));
    }
    if let Some(index) = index {
        reconcile_indexed_manual(index, &intent)?;
    }
    readback_intent(directory, &intent)
}

fn readback_intent(directory: &Path, intent: &Intent) -> io::Result<ManualQuotaReadback> {
    let dir = operation_dir(directory, &intent.request.operation_id);
    let artifact = dir.display().to_string();
    let mut result = ManualQuotaReadback {
        operation_id: intent.request.operation_id.clone(),
        physical_account_id: intent.physical_account_id.clone(),
        effect_id: intent.effect_id.clone(),
        state: "unknown".into(),
        outcome: None,
        windows: Vec::new(),
        completed_unix_seconds: None,
        artifact,
    };
    if let Some(source) = &intent.source_operation_id {
        if !valid_id(source) || source == &intent.request.operation_id {
            return Err(io::Error::other("manual quota coalesced source invalid"));
        }
        let source_intent: Intent =
            read_exact(&operation_dir(directory, source), "intent.json")?
                .ok_or_else(|| io::Error::other("manual quota coalesced source absent"))?;
        if source_intent.source_operation_id.is_some()
            || !same_source(intent, &source_intent)
            || source_intent.effect_id != intent.effect_id
        {
            return Err(io::Error::other("manual quota coalesced source changed"));
        }
        let source_result = readback_intent(directory, &source_intent)?;
        result.state = source_result.state;
        result.outcome = source_result.outcome;
        result.windows = source_result.windows;
        result.completed_unix_seconds = source_result.completed_unix_seconds;
        return Ok(result);
    }
    if intent.quota_script.is_none() {
        result.state = "unmetered".into();
        result.outcome = Some("unmetered".into());
        return Ok(result);
    }
    if read_exact::<serde_json::Value>(&dir, "k.json")?.is_none() {
        return Ok(result);
    }
    let Some(q) = read_exact::<PhysicalQ>(&dir, "q.json")? else {
        return Ok(result);
    };
    if q.version != 1 || Some(&q.effect_id) != intent.effect_id.as_ref() {
        return Err(io::Error::other("manual quota physical Q identity changed"));
    }
    let stdout = read_bytes(&dir.join("stdout.bin"))?;
    let stderr = read_bytes(&dir.join("stderr.bin"))?;
    if stdout.len() as u64 != q.stdout_len
        || sha(&stdout) != q.stdout_sha256
        || stderr.len() as u64 != q.stderr_len
        || sha(&stderr) != q.stderr_sha256
    {
        return Err(io::Error::other("manual quota physical output changed"));
    }
    result.state = "drained".into();
    result.completed_unix_seconds = Some(q.completed_unix_seconds);
    if !q.wait_success {
        result.outcome = Some("failed".into());
        return Ok(result);
    }
    if stdout.len() as u64 > MAX_OUTPUT {
        result.outcome = Some("invalid".into());
        return Ok(result);
    }
    let Ok(raw) = std::str::from_utf8(&stdout) else {
        result.outcome = Some("invalid".into());
        return Ok(result);
    };
    match super::fresh_provider::parse_effect_windows(raw) {
        Ok(windows) if !windows.is_empty() => {
            result.outcome = Some("valid_windows".into());
            result.windows = windows;
        }
        Ok(_) => result.outcome = Some("empty".into()),
        Err(_) => result.outcome = Some("invalid".into()),
    }
    Ok(result)
}

/// Exact physical manual Q interpretation for the compact index projection.
/// Rebuild calls this only after the retained intent has passed its census.
pub(super) fn indexed_physical_readback(
    directory: &Path,
    operation_id: &str,
) -> io::Result<ManualQuotaReadback> {
    if !valid_id(operation_id) {
        return Err(io::Error::other("manual projection operation ID invalid"));
    }
    let dir = operation_dir(directory, operation_id);
    let intent: Intent = read_exact(&dir, "intent.json")?
        .ok_or_else(|| io::Error::other("manual projection intent absent"))?;
    if intent.request.operation_id != operation_id || intent.source_operation_id.is_some() {
        return Err(io::Error::other("manual projection source mismatch"));
    }
    readback_intent(directory, &intent)
}

pub(super) fn physical_q_nanos(directory: &Path, operation_id: &str) -> io::Result<u128> {
    let dir = operation_dir(directory, operation_id);
    super::fresh_provider::file_unix_nanos(&dir.join("q.json"))
}

pub(super) fn source_readback(
    directory: &Path,
    operation_id: &str,
    physical_id: &str,
    quota_script: &str,
    auth_command: Option<&str>,
    environment_sha256: &str,
    effect_id: &str,
) -> io::Result<ManualQuotaReadback> {
    if !valid_id(operation_id) {
        return Err(io::Error::other("manual quota source ID invalid"));
    }
    let intent: Intent = read_exact(&operation_dir(directory, operation_id), "intent.json")?
        .ok_or_else(|| io::Error::other("manual quota source absent"))?;
    if intent.version != 1
        || intent.source_operation_id.is_some()
        || intent.physical_account_id != physical_id
        || intent.quota_script.as_deref() != Some(quota_script)
        || intent.auth_refresh_command.as_deref() != auth_command
        || intent.environment_sha256 != environment_sha256
        || intent.effect_id.as_deref() != Some(effect_id)
    {
        return Err(io::Error::other("manual quota source provenance changed"));
    }
    readback_intent(directory, &intent)
}

pub(super) fn latest(
    directory: &Path,
    physical_id: &str,
    quota_script: &str,
    auth_command: Option<&str>,
    environment_sha256: &str,
) -> io::Result<Option<(IntentSummary, ManualQuotaReadback)>> {
    let parent = directory.join("manual-quota");
    let entries = match fs::read_dir(&parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut latest: Option<(IntentSummary, ManualQuotaReadback, u128)> = None;
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let Some(intent) = read_exact::<Intent>(&entry.path(), "intent.json")? else {
            continue;
        };
        if intent.source_operation_id.is_some()
            || intent.physical_account_id != physical_id
            || intent.quota_script.as_deref() != Some(quota_script)
            || intent.auth_refresh_command.as_deref() != auth_command
            || intent.environment_sha256 != environment_sha256
        {
            continue;
        }
        let readback = readback_intent(directory, &intent)?;
        if readback.state != "drained" {
            return Ok(Some((
                IntentSummary {
                    operation_id: intent.request.operation_id,
                    effect_id: intent.effect_id,
                },
                readback,
            )));
        }
        let time = if readback.state == "drained" {
            physical_q_nanos(directory, &intent.request.operation_id)?
        } else {
            super::fresh_provider::file_unix_nanos(&entry.path().join("intent.json"))?
        };
        if latest.as_ref().is_none_or(|(_, _, prior)| time > *prior) {
            latest = Some((
                IntentSummary {
                    operation_id: intent.request.operation_id,
                    effect_id: intent.effect_id,
                },
                readback,
                time,
            ));
        }
    }
    Ok(latest.map(|(summary, readback, _)| (summary, readback)))
}

pub(super) struct IntentSummary {
    pub operation_id: String,
    pub effect_id: Option<String>,
}

/// Read-only import of retained manual intents. Followers remain explicit
/// unresolved references; only their source owns a physical K/Q.
pub(super) fn offline_collect(
    directory: &Path,
    source: &Path,
    snapshot: &mut super::fresh_index::OfflineSnapshot,
) -> io::Result<()> {
    offline_collect_mode(directory, source, snapshot, false)
}

pub(super) fn offline_collect_mode(
    directory: &Path,
    source: &Path,
    snapshot: &mut super::fresh_index::OfflineSnapshot,
    allow_uncertain_q: bool,
) -> io::Result<()> {
    use super::fresh_index::{
        Account, Artifact, EffectIntent, EffectKind, MarkerTimes, PhysicalQ as IndexPhysicalQ,
        SourceKey,
    };
    use std::collections::BTreeMap;
    let parent = directory.join("manual-quota");
    let entries = match fs::read_dir(&parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let meta = source.metadata()?;
    if !meta.is_dir() {
        return Err(io::Error::other("offline manual source not directory"));
    }
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let dir = entry.path();
        let id = entry.file_name().to_string_lossy().into_owned();
        if !valid_id(&id) {
            return Err(io::Error::other("offline manual operation ID invalid"));
        }
        let intent: Intent = read_exact(&dir, "intent.json")?
            .ok_or_else(|| io::Error::other("offline manual intent absent"))?;
        let pool = oulipoly_runtime::executor::cli::fresh_remote::load_fresh_headless_pool(
            source,
            &intent.request.model,
        )
        .map_err(io::Error::other)?;
        let index = pool
            .model
            .providers
            .iter()
            .position(|p| p.name == intent.request.account)
            .ok_or_else(|| io::Error::other("offline manual source account absent"))?;
        if intent.version != 1
            || intent.request.operation_id != id
            || intent.request.config_sha256 != pool.config_sha256
            || intent.physical_account_id != pool.account_identities[index].as_deref().unwrap_or("")
            || (
                intent.quota_script.clone(),
                intent.auth_refresh_command.clone(),
            ) != pool.account_effects[index]
            || intent.source_device != meta.dev()
            || intent.source_inode != meta.ino()
            || intent.environment_sha256.len() != 64
            || !intent
                .environment_sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
        {
            return Err(io::Error::other(
                "offline manual config/source/environment changed",
            ));
        }
        if snapshot
            .source_models
            .insert(
                intent.request.model.clone(),
                intent.request.config_sha256.clone(),
            )
            .is_some_and(|prior| prior != intent.request.config_sha256)
        {
            return Err(io::Error::other(
                "offline manual model has multiple config digests",
            ));
        }
        let k: Option<serde_json::Value> = read_exact(&dir, "k.json")?;
        if intent.source_operation_id.is_some() && k.is_some() {
            return Err(io::Error::other("offline manual reuse has physical K"));
        }
        if intent.quota_script.is_none() && k.is_some() {
            return Err(io::Error::other("offline unmetered manual operation has K"));
        }
        if let Some(k) = &k {
            if k.get("version").and_then(|v| v.as_u64()) != Some(1)
                || k.get("effect_id").and_then(|v| v.as_str()) != intent.effect_id.as_deref()
                || k.get("operation_id").and_then(|v| v.as_str()) != Some(id.as_str())
            {
                return Err(io::Error::other("offline manual K identity changed"));
            }
        }
        let result = readback_intent(directory, &intent)?;
        if dir.join("q.json").exists()
            && (k.is_none() || (!allow_uncertain_q && result.state != "drained"))
        {
            return Err(io::Error::other(
                "offline manual Q lacks certified K/readback",
            ));
        }
        let source_key = SourceKey {
            commands_sha256: sha(&serde_json::to_vec(&(
                &intent.quota_script,
                &intent.auth_refresh_command,
            ))?),
            environment_sha256: intent.environment_sha256.clone(),
        };
        let relative = Path::new("manual-quota").join(&id);
        let effect = EffectIntent {
            kind: EffectKind::ManualQuota,
            source: source_key.clone(),
            decision_handoff: String::new(),
            route_source: None,
            candidate: None,
            intent: Artifact::from_existing(directory, &relative.join("intent.json"))
                .map_err(io::Error::other)?,
            reuse: None,
            consumed_k: if k.is_some() {
                Some(
                    Artifact::from_existing(directory, &relative.join("k.json"))
                        .map_err(io::Error::other)?,
                )
            } else {
                None
            },
            certified_q: if result.state == "drained" && k.is_some() {
                let k = Artifact::from_existing(directory, &relative.join("k.json"))
                    .map_err(io::Error::other)?;
                let q = Artifact::from_existing(directory, &relative.join("q.json"))
                    .map_err(io::Error::other)?;
                Some(IndexPhysicalQ {
                    physical_k: k,
                    q,
                    terminal: None,
                    completed_unix_nanos: i64::try_from(physical_q_nanos(directory, &id)?)
                        .map_err(io::Error::other)?,
                })
            } else {
                None
            },
            result: if result.state == "drained" && k.is_some() {
                Some(
                    Artifact::from_existing(directory, &relative.join("q.json"))
                        .map_err(io::Error::other)?,
                )
            } else {
                None
            },
        };
        let account = snapshot
            .accounts
            .entry(intent.physical_account_id.clone())
            .or_insert_with(|| Account {
                generation: String::new(),
                physical_key: intent.physical_account_id.clone(),
                revision: 0,
                grants: BTreeMap::new(),
                effects: BTreeMap::new(),
                observed_invocations: 0,
                markers: MarkerTimes::default(),
                source_q: BTreeMap::new(),
                recent_failure_nanos: Vec::new(),
            });
        if result.outcome.as_deref() == Some("valid_windows") {
            if let Some(q) = &effect.certified_q {
                let digest = sha(&serde_json::to_vec(&source_key)?);
                let source =
                    account
                        .source_q
                        .entry(digest)
                        .or_insert_with(|| super::fresh_index::SourceQ {
                            source: source_key.clone(),
                            latest_quota_q: None,
                            latest_auth_q: None,
                        });
                if source
                    .latest_quota_q
                    .as_ref()
                    .is_none_or(|old| old.completed_unix_nanos <= q.completed_unix_nanos)
                {
                    source.latest_quota_q = Some(q.clone());
                }
            }
        }
        if account.effects.insert(id, effect).is_some() {
            return Err(io::Error::other("offline manual operation duplicated"));
        }
    }
    Ok(())
}

pub(super) fn offline_certified_q(
    directory: &Path,
    operation_id: &str,
    source: &Path,
) -> io::Result<Option<(u128, bool)>> {
    let dir = operation_dir(directory, operation_id);
    let intent: Intent = read_exact(&dir, "intent.json")?
        .ok_or_else(|| io::Error::other("manual reconcile intent absent"))?;
    let meta = source.metadata()?;
    let pool = oulipoly_runtime::executor::cli::fresh_remote::load_fresh_headless_pool(
        source,
        &intent.request.model,
    )
    .map_err(io::Error::other)?;
    let index = pool
        .model
        .providers
        .iter()
        .position(|p| p.name == intent.request.account)
        .ok_or_else(|| io::Error::other("manual reconcile account absent"))?;
    if intent.version != 1
        || intent.request.operation_id != operation_id
        || intent.source_operation_id.is_some()
        || intent.source_device != meta.dev()
        || intent.source_inode != meta.ino()
        || intent.request.config_sha256 != pool.config_sha256
        || intent.physical_account_id != pool.account_identities[index].as_deref().unwrap_or("")
        || (
            intent.quota_script.clone(),
            intent.auth_refresh_command.clone(),
        ) != pool.account_effects[index]
    {
        return Err(io::Error::other("manual reconcile source changed"));
    }
    if !dir.join("q.json").exists() {
        return Ok(None);
    }
    let k: serde_json::Value =
        read_exact(&dir, "k.json")?.ok_or_else(|| io::Error::other("manual Q precedes K"))?;
    if k.get("effect_id").and_then(|v| v.as_str()) != intent.effect_id.as_deref()
        || k.get("operation_id").and_then(|v| v.as_str()) != Some(operation_id)
    {
        return Err(io::Error::other("manual reconcile K changed"));
    }
    let result = readback_intent(directory, &intent)?;
    if result.state != "drained" {
        return Err(io::Error::other("manual Q not certified"));
    }
    Ok(Some((
        physical_q_nanos(directory, operation_id)?,
        result.outcome.as_deref() == Some("valid_windows"),
    )))
}

pub(super) fn unresolved_for_physical(
    directory: &Path,
    physical_id: &str,
) -> io::Result<Option<String>> {
    let parent = directory.join("manual-quota");
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let Some(intent) = read_exact::<Intent>(&entry.path(), "intent.json")? else {
            continue;
        };
        if intent.source_operation_id.is_some() || intent.physical_account_id != physical_id {
            continue;
        }
        let result = readback_intent(directory, &intent)?;
        if result.state != "drained" && result.state != "unmetered" {
            return Ok(Some(result.artifact));
        }
    }
    Ok(None)
}

pub(super) fn worker(dir: &Path) -> io::Result<()> {
    let mut environment = std::env::vars_os()
        .filter(|(key, _)| key != "OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1")
        .map(|(key, value)| {
            Ok((
                key.into_string()
                    .map_err(|_| io::Error::other("manual worker environment key invalid"))?,
                value
                    .into_string()
                    .map_err(|_| io::Error::other("manual worker environment value invalid"))?,
            ))
        })
        .collect::<io::Result<Vec<_>>>()?;
    environment.sort();
    worker_with_environment(dir, &environment)
}

pub(super) fn worker_with_environment(
    dir: &Path,
    environment: &[(String, String)],
) -> io::Result<()> {
    let intent: Intent = read_exact(dir, "intent.json")?
        .ok_or_else(|| io::Error::other("manual quota worker intent absent"))?;
    if intent.version != 1
        || intent.source_operation_id.is_some()
        || intent.quota_script.is_none()
        || !valid_id(&intent.request.operation_id)
        || dir.file_name().and_then(|s| s.to_str()) != Some(&intent.request.operation_id)
    {
        return Err(io::Error::other("manual quota worker intent invalid"));
    }
    validate_environment(environment)?;
    if environment_digest(environment)? != intent.environment_sha256 {
        return Err(io::Error::other(
            "manual quota worker effect environment changed",
        ));
    }
    let effect_id = intent
        .effect_id
        .as_ref()
        .ok_or_else(|| io::Error::other("manual quota K absent"))?;
    let k: serde_json::Value =
        read_exact(dir, "k.json")?.ok_or_else(|| io::Error::other("manual quota K absent"))?;
    if k["effect_id"] != *effect_id || k["operation_id"] != intent.request.operation_id {
        return Err(io::Error::other("manual quota K changed"));
    }
    let stdout = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(dir.join("stdout.bin"))?;
    let stderr = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(dir.join("stderr.bin"))?;
    let status = Command::new("/bin/sh")
        .arg("-c")
        .arg(intent.quota_script.as_deref().unwrap())
        .env_clear()
        .envs(environment.iter().cloned())
        .uid(intent.peer_uid)
        .gid(intent.peer_gid)
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .status()?;
    let stdout = fs::read(dir.join("stdout.bin"))?;
    let stderr = fs::read(dir.join("stderr.bin"))?;
    File::open(dir.join("stdout.bin"))?.sync_all()?;
    File::open(dir.join("stderr.bin"))?.sync_all()?;
    durable_new(
        dir,
        "q.json",
        &PhysicalQ {
            version: 1,
            effect_id: effect_id.clone(),
            wait_success: status.success(),
            stdout_len: stdout.len() as u64,
            stdout_sha256: sha(&stdout),
            stderr_len: stderr.len() as u64,
            stderr_sha256: sha(&stderr),
            completed_unix_seconds: Utc::now().timestamp(),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::super::fresh_index::{Index, broker_admission_lease};
    use super::*;

    struct Fixture {
        _temp: tempfile::TempDir,
        source: PathBuf,
        ledger: PathBuf,
    }

    impl Fixture {
        fn new(script: &str) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let source = temp.path().join("config");
            let ledger = temp.path().join("ledger");
            fs::create_dir_all(source.join("models")).unwrap();
            fs::create_dir_all(&ledger).unwrap();
            fs::write(source.join("providers.toml"), format!(
                "[first]\ncommand = '/bin/true'\nquota_account_id = 'physical-first'\nquota_script = {}\n[second]\ncommand = '/bin/true'\nquota_account_id = 'physical-second'\n",
                serde_json::to_string(script).unwrap(),
            )).unwrap();
            fs::write(
                source.join("models/work.toml"),
                "[[providers]]\nname = 'first'\n[[providers]]\nname = 'second'\n",
            )
            .unwrap();
            Self {
                _temp: temp,
                source,
                ledger,
            }
        }
        fn request(&self, account: &str) -> ManualQuotaRequest {
            let pool = oulipoly_runtime::executor::cli::fresh_remote::load_fresh_headless_pool(
                &self.source,
                "work",
            )
            .unwrap();
            ManualQuotaRequest {
                operation_id: uuid::Uuid::new_v4().to_string(),
                model: "work".into(),
                account: account.into(),
                config_sha256: pool.config_sha256,
                environment: vec![("PATH".into(), "/usr/bin:/bin".into())],
            }
        }
        fn begin(&self, request: &ManualQuotaRequest) -> io::Result<ManualQuotaReadback> {
            begin(
                &self.ledger,
                &File::open(&self.source)?,
                request,
                unsafe { libc::getuid() },
                unsafe { libc::getgid() },
            )
        }
        fn read(&self, request: &ManualQuotaRequest) -> io::Result<ManualQuotaReadback> {
            readback(&self.ledger, request, unsafe { libc::getuid() }, unsafe {
                libc::getgid()
            })
        }
        fn run_worker(&self, request: &ManualQuotaRequest) {
            worker_with_environment(
                &operation_dir(&self.ledger, &request.operation_id),
                &request.environment,
            )
            .unwrap();
        }
    }

    #[test]
    fn v3_shared_manual_origin_rejects_changed_config_source() {
        let f = Fixture::new(r#"printf '{"used_percent":24,"resets_at":"2099-01-01T00:00:00Z"}'"#);
        let request = f.request("first");
        let source = File::open(&f.source).unwrap();
        let intent = source_intent(&source, &request, unsafe { libc::getuid() }, unsafe {
            libc::getgid()
        })
        .unwrap();
        v3_validate_origin_source(&f.source, &intent).unwrap();
        fs::write(
            f.source.join("models/work.toml"),
            "[[providers]]\nname = 'second'\n[[providers]]\nname = 'first'\n",
        )
        .unwrap();
        assert!(
            v3_validate_origin_source(&f.source, &intent)
                .unwrap_err()
                .to_string()
                .contains("config/source changed")
        );
    }

    #[test]
    fn indexed_manual_announces_before_k_and_reuses_exact_k_q_after_restart() {
        let f = Fixture::new(r#"printf '{"used_percent":24,"resets_at":"2099-01-01T00:00:00Z"}'"#);
        let lease = broker_admission_lease(&f.ledger).unwrap();
        let index = Index::admit_live_routes(&f.ledger, &lease).unwrap();
        let request = f.request("first");
        let uid = unsafe { libc::getuid() };
        let gid = unsafe { libc::getgid() };
        let source = File::open(&f.source).unwrap();
        let pending = begin_indexed(&f.ledger, &source, &request, uid, gid, Some(&index)).unwrap();
        assert_eq!(pending.state, "unknown");
        let dir = operation_dir(&f.ledger, &request.operation_id);
        let before = index.account("physical-first").unwrap();
        assert_eq!(
            before.effects[&request.operation_id].kind,
            super::super::fresh_index::EffectKind::ManualQuota
        );
        assert!(before.effects[&request.operation_id].consumed_k.is_some());
        assert!(
            index
                .route_reader_preflight("physical-first")
                .unwrap_err()
                .to_string()
                .contains("announced effect or manual debt")
        );
        let k_bytes = fs::read(dir.join("k.json")).unwrap();
        let first_revision = before.revision;
        f.run_worker(&request);
        let drained =
            readback_id_indexed(&f.ledger, &request.operation_id, uid, gid, Some(&index)).unwrap();
        assert_eq!(drained.outcome.as_deref(), Some("valid_windows"));
        let settled = index.account("physical-first").unwrap();
        assert!(settled.effects[&request.operation_id].certified_q.is_some());
        assert_eq!(
            settled.effects[&request.operation_id].result,
            settled.effects[&request.operation_id]
                .certified_q
                .as_ref()
                .map(|q| q.q.clone())
        );
        assert_eq!(settled.source_q.len(), 1);
        assert!(
            index
                .route_reader_preflight("physical-first")
                .unwrap_err()
                .to_string()
                .contains("atomic account revision join")
        );
        assert_eq!(settled.observed_invocations, 0);
        assert!(settled.revision > first_revision);
        let restarted = Index::admit_live_routes(&f.ledger, &lease).unwrap();
        let again =
            begin_indexed(&f.ledger, &source, &request, uid, gid, Some(&restarted)).unwrap();
        assert_eq!(again.effect_id, drained.effect_id);
        assert_eq!(fs::read(dir.join("k.json")).unwrap(), k_bytes);
        assert_eq!(
            restarted.account("physical-first").unwrap().revision,
            settled.revision
        );
    }

    #[test]
    fn indexed_manual_damaged_generation_blocks_physical_k() {
        let f = Fixture::new("exit 0");
        let lease = broker_admission_lease(&f.ledger).unwrap();
        let index = Index::admit_live_routes(&f.ledger, &lease).unwrap();
        fs::remove_file(f.ledger.join("index-v1/manifest.json")).unwrap();
        let request = f.request("first");
        assert!(
            begin_indexed(
                &f.ledger,
                &File::open(&f.source).unwrap(),
                &request,
                unsafe { libc::getuid() },
                unsafe { libc::getgid() },
                Some(&index)
            )
            .is_err()
        );
        assert!(
            !operation_dir(&f.ledger, &request.operation_id)
                .join("k.json")
                .exists()
        );
    }

    #[test]
    fn indexed_manual_incomplete_q_is_debt_and_invalid_q_has_no_healthy_source() {
        let f = Fixture::new("printf invalid");
        let lease = broker_admission_lease(&f.ledger).unwrap();
        let index = Index::admit_live_routes(&f.ledger, &lease).unwrap();
        let request = f.request("first");
        let uid = unsafe { libc::getuid() };
        let gid = unsafe { libc::getgid() };
        let pending = begin_indexed(
            &f.ledger,
            &File::open(&f.source).unwrap(),
            &request,
            uid,
            gid,
            Some(&index),
        )
        .unwrap();
        let debt = index.account("physical-first").unwrap();
        assert!(debt.effects[&request.operation_id].consumed_k.is_some());
        assert!(debt.effects[&request.operation_id].certified_q.is_none());
        assert!(debt.source_q.is_empty());
        let dir = operation_dir(&f.ledger, &request.operation_id);
        durable_new(
            &dir,
            "q.json",
            &PhysicalQ {
                version: 1,
                effect_id: pending.effect_id.unwrap(),
                wait_success: true,
                stdout_len: 0,
                stdout_sha256: sha(b""),
                stderr_len: 0,
                stderr_sha256: sha(b""),
                completed_unix_seconds: Utc::now().timestamp(),
            },
        )
        .unwrap();
        assert!(
            readback_id_indexed(&f.ledger, &request.operation_id, uid, gid, Some(&index)).is_err()
        );
        assert!(
            index.account("physical-first").unwrap().effects[&request.operation_id]
                .certified_q
                .is_none()
        );
        assert!(Index::admit_live_routes(&f.ledger, &lease).is_err());
        fs::remove_file(dir.join("q.json")).unwrap();
        Index::admit_live_routes(&f.ledger, &lease).unwrap();
        f.run_worker(&request);
        let readback =
            readback_id_indexed(&f.ledger, &request.operation_id, uid, gid, Some(&index)).unwrap();
        assert_eq!(readback.outcome.as_deref(), Some("invalid"));
        let settled = index.account("physical-first").unwrap();
        assert!(settled.effects[&request.operation_id].certified_q.is_some());
        assert!(settled.source_q.is_empty());
    }

    #[test]
    fn indexed_manual_shared_physical_account_keeps_newest_q_across_models() {
        let f = Fixture::new(r#"printf '{"used_percent":36,"resets_at":"2099-01-01T00:00:00Z"}'"#);
        fs::write(
            f.source.join("models/other.toml"),
            "[[providers]]\nname = 'first'\n",
        )
        .unwrap();
        let lease = broker_admission_lease(&f.ledger).unwrap();
        let index = Index::admit_live_routes(&f.ledger, &lease).unwrap();
        let first = f.request("first");
        let pool = oulipoly_runtime::executor::cli::fresh_remote::load_fresh_headless_pool(
            &f.source, "other",
        )
        .unwrap();
        let second = ManualQuotaRequest {
            operation_id: uuid::Uuid::new_v4().to_string(),
            model: "other".into(),
            account: "first".into(),
            config_sha256: pool.config_sha256,
            environment: first.environment.clone(),
        };
        let uid = unsafe { libc::getuid() };
        let gid = unsafe { libc::getgid() };
        let source = File::open(&f.source).unwrap();
        for request in [&first, &second] {
            begin_indexed(&f.ledger, &source, request, uid, gid, Some(&index)).unwrap();
            f.run_worker(request);
            assert_eq!(
                readback_id_indexed(&f.ledger, &request.operation_id, uid, gid, Some(&index))
                    .unwrap()
                    .state,
                "drained"
            );
        }
        let account = index.account("physical-first").unwrap();
        assert!(account.effects.contains_key(&first.operation_id));
        assert!(account.effects.contains_key(&second.operation_id));
        assert!(account.markers.model_capacity_nanos.is_none());
        assert!(account.markers.quota_rejection_nanos.is_none());
        let latest = account
            .source_q
            .values()
            .next()
            .unwrap()
            .latest_quota_q
            .as_ref()
            .unwrap()
            .clone();
        readback_id_indexed(&f.ledger, &first.operation_id, uid, gid, Some(&index)).unwrap();
        assert_eq!(
            index
                .account("physical-first")
                .unwrap()
                .source_q
                .values()
                .next()
                .unwrap()
                .latest_quota_q
                .as_ref(),
            Some(&latest)
        );
    }

    #[test]
    fn positive_physical_q_force_and_exact_restart_readback() {
        let f = Fixture::new(r#"printf '{"used_percent":24,"resets_at":"2099-01-01T00:00:00Z"}'"#);
        let mut first = f.request("first");
        first
            .environment
            .push(("SECRET_TOKEN".into(), "sentinel-secret".into()));
        let pending = f.begin(&first).unwrap();
        assert!(
            !fs::read_to_string(operation_dir(&f.ledger, &first.operation_id).join("intent.json"))
                .unwrap()
                .contains("sentinel-secret")
        );
        assert_eq!(pending.state, "unknown");
        assert_eq!(f.read(&first).unwrap().effect_id, pending.effect_id);
        f.run_worker(&first);
        let q = f.read(&first).unwrap();
        assert_eq!(q.state, "drained");
        assert_eq!(q.outcome.as_deref(), Some("valid_windows"));
        assert_eq!(q.physical_account_id, "physical-first");
        assert_eq!(q.windows[0].used_percent, 24.0);
        assert_eq!(f.begin(&first).unwrap().effect_id, q.effect_id);
        let forced = f.request("first");
        assert_ne!(f.begin(&forced).unwrap().effect_id, q.effect_id);
        assert!(
            operation_dir(&f.ledger, &forced.operation_id)
                .join("k.json")
                .exists()
        );
        assert!(
            operation_dir(&f.ledger, &first.operation_id)
                .join("q.json")
                .exists()
        );
    }

    #[test]
    fn unresolved_k_coalesces_then_lost_reply_is_exact() {
        let f = Fixture::new(r#"printf '{"used_percent":40,"resets_at":"2099-01-01T00:00:00Z"}'"#);
        let first = f.request("first");
        let second = f.request("first");
        let a = f.begin(&first).unwrap();
        let b = f.begin(&second).unwrap();
        assert_eq!(a.effect_id, b.effect_id);
        assert!(
            !operation_dir(&f.ledger, &second.operation_id)
                .join("k.json")
                .exists()
        );
        f.run_worker(&first);
        assert_eq!(
            f.read(&second).unwrap().outcome.as_deref(),
            Some("valid_windows")
        );
    }

    #[test]
    fn failed_and_exhausted_q_do_not_fabricate_healthy_quota() {
        let failed = Fixture::new("exit 7");
        let request = failed.request("first");
        failed.begin(&request).unwrap();
        failed.run_worker(&request);
        let q = failed.read(&request).unwrap();
        assert_eq!(q.outcome.as_deref(), Some("failed"));
        assert!(q.windows.is_empty());
        let exhausted = Fixture::new(
            r#"printf '{"windows":[{"used_percent":25,"resets_at":"2099-01-01T00:00:00Z"},{"used_percent":100,"resets_at":"2099-01-01T00:00:00Z"}]}'"#,
        );
        let request = exhausted.request("first");
        exhausted.begin(&request).unwrap();
        exhausted.run_worker(&request);
        let q = exhausted.read(&request).unwrap();
        assert_eq!(q.outcome.as_deref(), Some("valid_windows"));
        assert!(q.windows.iter().any(|w| w.used_percent == 100.0));
    }

    #[test]
    fn wrong_account_config_and_no_source_refuse_before_k() {
        let f = Fixture::new("exit 7");
        let mut request = f.request("first");
        request.account = "missing".into();
        assert!(f.begin(&request).is_err());
        request.account = "first".into();
        request.config_sha256 = "0".repeat(64);
        assert!(f.begin(&request).is_err());
        request = f.request("second");
        let unmetered = f.begin(&request).unwrap();
        assert_eq!(unmetered.state, "unmetered");
        assert!(unmetered.windows.is_empty());
        assert!(
            !operation_dir(&f.ledger, &request.operation_id)
                .join("k.json")
                .exists()
        );
        let source = fs::read_to_string(f.source.join("providers.toml")).unwrap();
        fs::write(
            f.source.join("providers.toml"),
            source.replace(
                "[first]\n",
                "[first]\nenvironment = { QUOTA_PROBE_TOKEN = 'configured' }\n",
            ),
        )
        .unwrap();
        let wrong_environment = f.request("first");
        assert!(f.begin(&wrong_environment).is_err());
        assert!(
            !operation_dir(&f.ledger, &wrong_environment.operation_id)
                .join("k.json")
                .exists()
        );
    }

    #[test]
    fn changed_source_cannot_take_over_unknown_physical_k() {
        let f = Fixture::new("exit 7");
        let first = f.request("first");
        f.begin(&first).unwrap();
        fs::write(f.source.join("providers.toml"),
            "[first]\ncommand = '/bin/true'\nquota_account_id = 'physical-first'\nquota_script = 'exit 8'\n[second]\ncommand = '/bin/true'\nquota_account_id = 'physical-second'\n").unwrap();
        let second = f.request("first");
        assert!(f.begin(&second).is_err());
        assert_eq!(f.read(&first).unwrap().state, "unknown");
    }

    #[test]
    fn private_socket_accepts_local_manual_request_and_exact_q_readback() {
        if std::env::var_os("AGE319_MANUAL_SOCKET_INNER").is_none() {
            let output = Command::new("unshare")
                .args(["-Urpfm", "--mount-proc"])
                .arg(std::env::current_exe().unwrap())
                .args(["--exact", "linux_main::manual_quota::tests::private_socket_accepts_local_manual_request_and_exact_q_readback", "--nocapture"])
                .env("AGE319_MANUAL_SOCKET_INNER", "1")
                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", "/tmp/age319-manual-socket-fixture")
                .output().unwrap();
            assert!(
                output.status.success(),
                "stdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let f = Fixture::new(r#"printf '{"used_percent":31,"resets_at":"2099-01-01T00:00:00Z"}'"#);
        let state = f._temp.path().join("state");
        fs::create_dir_all(&state).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
        oulipoly_state::mailbox::FreshV30Lane::initialize_at(&state).unwrap();
        let socket = f._temp.path().join("v30.sock");
        let image = File::open(std::env::current_exe().unwrap()).unwrap();
        let mut broker = if let Some(executable) = std::env::var_os("OULIPOLY_AGE319_BROKER_IMAGE")
        {
            Some(
                Command::new(executable)
                    .arg("--serve-fresh-v30")
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &state)
                    .env(
                        "OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1",
                        std::env::current_exe().unwrap(),
                    )
                    .spawn()
                    .unwrap(),
            )
        } else {
            let state_for_server = state.clone();
            let socket_for_server = socket.clone();
            std::thread::spawn(move || {
                super::super::serve_fresh_v30_at(
                    &state_for_server,
                    &socket_for_server,
                    image,
                    None,
                )
                .unwrap();
            });
            None
        };
        for _ in 0..100 {
            if socket.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(socket.exists());
        let request = f.request("first");
        let source = File::open(&f.source).unwrap();
        let started = oulipoly_kernel_broker::protocol::private_manual_quota_at(
            &socket,
            &request,
            true,
            Some(source.as_raw_fd()),
        )
        .unwrap();
        assert_eq!(started.state, "unknown");
        let broker_ledger = state.join("v30/fresh-provider");
        if broker.is_none() {
            worker_with_environment(
                &operation_dir(&broker_ledger, &request.operation_id),
                &request.environment,
            )
            .unwrap();
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let drained = loop {
            let result = oulipoly_kernel_broker::protocol::private_manual_quota_at(
                &socket, &request, false, None,
            )
            .unwrap();
            if result.state == "drained" {
                break result;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "manual K/Q stayed unknown: {}",
                result.artifact
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        assert_eq!(drained.outcome.as_deref(), Some("valid_windows"));
        assert_eq!(drained.windows[0].used_percent, 31.0);
        if std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_ROUTE_INDEX_V1")
            .is_some_and(|value| value == "1")
        {
            let index = Index::open(&broker_ledger).unwrap();
            let account = index.account("physical-first").unwrap();
            let effect = &account.effects[&request.operation_id];
            assert!(effect.consumed_k.is_some());
            assert!(effect.certified_q.is_some());
            assert_eq!(
                effect.result.as_ref(),
                effect.certified_q.as_ref().map(|q| &q.q)
            );
            assert!(
                account
                    .source_q
                    .values()
                    .any(|source| source.latest_quota_q.is_some())
            );
        }
        if let Some(child) = broker.as_mut() {
            child.kill().unwrap();
            child.wait().unwrap();
            let mut restarted =
                Command::new(std::env::var_os("OULIPOLY_AGE319_BROKER_IMAGE").unwrap())
                    .arg("--serve-fresh-v30")
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &state)
                    .env(
                        "OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1",
                        std::env::current_exe().unwrap(),
                    )
                    .spawn()
                    .unwrap();
            for _ in 0..100 {
                if std::os::unix::net::UnixStream::connect(&socket).is_ok() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            let recovered = oulipoly_kernel_broker::protocol::private_manual_quota_at(
                &socket, &request, false, None,
            )
            .unwrap();
            assert_eq!(recovered.effect_id, drained.effect_id);
            assert_eq!(recovered.artifact, drained.artifact);
            assert_eq!(recovered.windows[0].used_percent, 31.0);
            restarted.kill().unwrap();
            restarted.wait().unwrap();
        }
    }

    #[test]
    fn private_v3_socket_announces_manual_before_k() {
        if std::env::var_os("AGE319_MANUAL_V3_SOCKET_INNER").is_none() {
            let output = Command::new("unshare")
                .args(["-Urpfm", "--mount-proc"])
                .arg(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "linux_main::manual_quota::tests::private_v3_socket_announces_manual_before_k",
                    "--nocapture",
                ])
                .env("AGE319_MANUAL_V3_SOCKET_INNER", "1")
                .env(
                    "OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1",
                    "/tmp/age319-manual-v3-fixture",
                )
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "stdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let f = Fixture::new("printf '{\"used_percent\":31}'");
        let providers_path = f.source.join("providers.toml");
        let providers = fs::read_to_string(&providers_path).unwrap();
        fs::write(
            &providers_path,
            providers
                .replace("[first]\n", "[first]\nprompt_mode = 'stdin'\n")
                .replace("[second]\n", "[second]\nprompt_mode = 'stdin'\n"),
        )
        .unwrap();
        let state = f._temp.path().join("state");
        fs::create_dir_all(&state).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
        oulipoly_state::mailbox::FreshV30Lane::initialize_at(&state).unwrap();
        let root = state.join("v30/fresh-provider");
        drop(super::super::fresh_index::broker_admission_lease(&root).unwrap());
        let socket = f._temp.path().join("v30.sock");
        super::super::fresh_index::rebuild_keyed_offline(&root, &socket, &f.source).unwrap();
        // This is the sole test in its isolated unshare subprocess; set the
        // server's fixture mode before creating its thread.
        unsafe {
            std::env::set_var(
                "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_V1",
                "1",
            );
            std::env::set_var(
                "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_SOURCE_V1",
                &f.source,
            );
        }
        let image = File::open(std::env::current_exe().unwrap()).unwrap();
        let state_for_server = state.clone();
        let socket_for_server = socket.clone();
        std::thread::spawn(move || {
            super::super::serve_fresh_v30_at(&state_for_server, &socket_for_server, image, None)
                .unwrap();
        });
        for _ in 0..100 {
            if std::os::unix::net::UnixStream::connect(&socket).is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(std::os::unix::net::UnixStream::connect(&socket).is_ok());
        let request = f.request("first");
        let source = File::open(&f.source).unwrap();
        let started = oulipoly_kernel_broker::protocol::private_manual_quota_at(
            &socket,
            &request,
            true,
            Some(source.as_raw_fd()),
        )
        .unwrap();
        assert_eq!(started.state, "unknown");
        assert!(
            root.join("manual-quota")
                .join(&request.operation_id)
                .join("k.json")
                .exists()
        );
        let generation = super::super::fresh_index::KeyedGeneration::open(&root).unwrap();
        assert_eq!(
            generation.manual_flight("physical-first").unwrap(),
            Some(request.operation_id)
        );
        assert!(super::super::fresh_index::Index::open(&root).is_err());
    }

    #[test]
    fn v3_manual_post_k_unknown_survives_restart_without_second_k() {
        let f = Fixture::new(r#"printf '{"used_percent":24,"resets_at":"2099-01-01T00:00:00Z"}'"#);
        let path = f.source.join("providers.toml");
        let providers = fs::read_to_string(&path).unwrap();
        fs::write(
            &path,
            providers
                .replace("[first]\n", "[first]\nprompt_mode = 'stdin'\n")
                .replace("[second]\n", "[second]\nprompt_mode = 'stdin'\n"),
        )
        .unwrap();
        let socket = f._temp.path().join("absent.sock");
        drop(broker_admission_lease(&f.ledger).unwrap());
        super::super::fresh_index::rebuild_keyed_offline(&f.ledger, &socket, &f.source).unwrap();
        let lease = broker_admission_lease(&f.ledger).unwrap();
        let generation = super::super::fresh_index::KeyedGeneration::admit_provider_readback(
            &f.ledger, &lease, &f.source,
        )
        .unwrap();
        let source = File::open(&f.source).unwrap();
        let uid = unsafe { libc::getuid() };
        let gid = unsafe { libc::getgid() };
        let request = f.request("first");
        // This test uses an isolated source tree and the suite's serial test
        // mode; the injected failure occurs after the physical K fsync.
        unsafe {
            std::env::set_var(
                "OULIPOLY_KERNEL_BROKER_FIXTURE_FAIL_MANUAL_POST_K_CAS_V3_V1",
                "1",
            );
        }
        let failed = begin_v3(&f.ledger, &source, &request, uid, gid, &generation);
        unsafe {
            std::env::remove_var("OULIPOLY_KERNEL_BROKER_FIXTURE_FAIL_MANUAL_POST_K_CAS_V3_V1");
        }
        assert!(failed.unwrap_err().to_string().contains("post-K CAS"));
        let dir = operation_dir(&f.ledger, &request.operation_id);
        let k = fs::read(dir.join("k.json")).unwrap();
        assert!(!dir.join("q.json").exists());
        assert_eq!(
            readback_id_v3(&f.ledger, &request.operation_id, uid, gid, &generation)
                .unwrap()
                .state,
            "unknown"
        );
        drop(generation);
        drop(lease);
        let lease = broker_admission_lease(&f.ledger).unwrap();
        let restarted = super::super::fresh_index::KeyedGeneration::admit_provider_readback(
            &f.ledger, &lease, &f.source,
        )
        .unwrap();
        assert_eq!(
            begin_v3(&f.ledger, &source, &request, uid, gid, &restarted)
                .unwrap()
                .state,
            "unknown"
        );
        let follower = f.request("first");
        let reused = begin_v3(&f.ledger, &source, &follower, uid, gid, &restarted).unwrap();
        assert_eq!(reused.state, "unknown");
        assert_eq!(
            reused.effect_id,
            readback_id_v3(&f.ledger, &request.operation_id, uid, gid, &restarted)
                .unwrap()
                .effect_id
        );
        assert_eq!(fs::read(dir.join("k.json")).unwrap(), k);
        assert!(
            !operation_dir(&f.ledger, &follower.operation_id)
                .join("k.json")
                .exists()
        );
    }

    #[test]
    fn v3_manual_refuses_changed_source_environment_and_account_before_k() {
        let f = Fixture::new("printf '{}'");
        let path = f.source.join("providers.toml");
        let providers = fs::read_to_string(&path).unwrap();
        fs::write(
            &path,
            providers
                .replace("[first]\n", "[first]\nprompt_mode = 'stdin'\n")
                .replace("[second]\n", "[second]\nprompt_mode = 'stdin'\n"),
        )
        .unwrap();
        let socket = f._temp.path().join("absent.sock");
        drop(broker_admission_lease(&f.ledger).unwrap());
        super::super::fresh_index::rebuild_keyed_offline(&f.ledger, &socket, &f.source).unwrap();
        let lease = broker_admission_lease(&f.ledger).unwrap();
        let generation = super::super::fresh_index::KeyedGeneration::admit_provider_readback(
            &f.ledger, &lease, &f.source,
        )
        .unwrap();
        let source = File::open(&f.source).unwrap();
        let uid = unsafe { libc::getuid() };
        let gid = unsafe { libc::getgid() };
        let request = f.request("first");
        assert_eq!(
            begin_v3(&f.ledger, &source, &request, uid, gid, &generation)
                .unwrap()
                .state,
            "unknown"
        );
        let mut changed_env = request.clone();
        changed_env.environment[0].1 = "/different/path".into();
        assert!(begin_v3(&f.ledger, &source, &changed_env, uid, gid, &generation).is_err());
        let mut changed_account = f.request("first");
        changed_account.account = "missing".into();
        assert!(begin_v3(&f.ledger, &source, &changed_account, uid, gid, &generation).is_err());
        let unmetered = f.request("second");
        assert_eq!(
            begin_v3(&f.ledger, &source, &unmetered, uid, gid, &generation)
                .unwrap()
                .state,
            "unmetered"
        );
        assert!(
            !operation_dir(&f.ledger, &unmetered.operation_id)
                .join("k.json")
                .exists()
        );
        fs::write(
            &path,
            providers.replace("quota_script", "quota_script_changed"),
        )
        .unwrap();
        let mut changed_source = request.clone();
        changed_source.operation_id = uuid::Uuid::new_v4().to_string();
        assert!(begin_v3(&f.ledger, &source, &changed_source, uid, gid, &generation).is_err());
        assert!(
            !operation_dir(&f.ledger, &changed_source.operation_id)
                .join("k.json")
                .exists()
        );
        let unsupported_config = providers
            .replace("[first]\n", "[first]\nprompt_mode = 'stdin'\n")
            .replace("[second]\n", "[second]\nprompt_mode = 'stdin'\n")
            .replacen("command = '/bin/true'", "command = 'env /bin/true'", 1);
        fs::write(&path, unsupported_config).unwrap();
        let unsupported = f.request("first");
        assert!(
            begin_v3(&f.ledger, &source, &unsupported, uid, gid, &generation)
                .unwrap_err()
                .to_string()
                .contains("unsupported")
        );
        assert!(
            !operation_dir(&f.ledger, &unsupported.operation_id)
                .join("k.json")
                .exists()
        );
    }

    #[test]
    fn v3_manual_physical_q_owns_newest_route_fact() {
        use super::super::fresh_index::{KeyedGeneration, RouteEligibility, rebuild_keyed_offline};
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("quota.sh");
        let f = Fixture::new(&format!("sh {}", script.display()));
        let path = f.source.join("providers.toml");
        let providers = fs::read_to_string(&path).unwrap();
        fs::write(
            &path,
            providers
                .replace("[first]\n", "[first]\nprompt_mode = 'stdin'\n")
                .replace("[second]\n", "[second]\nprompt_mode = 'stdin'\n"),
        )
        .unwrap();
        let socket = f._temp.path().join("absent.sock");
        drop(broker_admission_lease(&f.ledger).unwrap());
        rebuild_keyed_offline(&f.ledger, &socket, &f.source).unwrap();
        let lease = broker_admission_lease(&f.ledger).unwrap();
        let generation =
            KeyedGeneration::admit_provider_readback(&f.ledger, &lease, &f.source).unwrap();
        let source = File::open(&f.source).unwrap();
        let uid = unsafe { libc::getuid() };
        let gid = unsafe { libc::getgid() };
        let mut first_source = None;
        for (step, (body, expected, eligibility)) in [
            (
                "printf '{\"used_percent\":20,\"resets_at\":\"2099-01-01T00:00:00Z\"}'",
                "valid_windows",
                "eligible",
            ),
            ("printf 'invalid quota'", "invalid", "probe"),
            (
                "printf '{\"windows\":[{\"used_percent\":20,\"resets_at\":\"2099-01-01T00:00:00Z\"},{\"used_percent\":100,\"resets_at\":\"2099-01-01T00:00:00Z\"}]}'",
                "valid_windows",
                "excluded",
            ),
            ("exit 7", "failed", "probe"),
        ]
        .into_iter()
        .enumerate()
        {
            fs::write(&script, body).unwrap();
            let mut request = f.request("first");
            if step != 0 {
                request.environment.push(("SOURCE".into(), step.to_string()));
            }
            assert_eq!(
                begin_v3(&f.ledger, &source, &request, uid, gid, &generation)
                    .unwrap()
                    .state,
                "unknown"
            );
            let dir = operation_dir(&f.ledger, &request.operation_id);
            worker_with_environment(&dir, &request.environment).unwrap();
            let result =
                readback_id_v3(&f.ledger, &request.operation_id, uid, gid, &generation).unwrap();
            assert_eq!(result.outcome.as_deref(), Some(expected));
            let intent = v3_intent(&f.ledger, &request.operation_id).unwrap();
            let key = v3_source_key(&intent).unwrap();
            if step == 0 {
                first_source = Some(key.clone());
            }
            let fact = generation
                .route_facts(
                    "physical-first",
                    Some(&key),
                    "work",
                    &request.config_sha256,
                    Utc::now().timestamp(),
                )
                .unwrap();
            assert!(
                match eligibility {
                    "eligible" => matches!(fact, RouteEligibility::Eligible { .. }),
                    "probe" => fact == RouteEligibility::ProbeRequired,
                    "excluded" => fact == RouteEligibility::Excluded,
                    _ => false,
                },
                "{eligibility}: {fact:?}"
            );
            if step != 0 {
                let old_source_fact = generation
                    .route_facts(
                        "physical-first",
                        first_source.as_ref(),
                        "work",
                        &request.config_sha256,
                        Utc::now().timestamp(),
                    )
                    .unwrap();
                assert_eq!(
                    old_source_fact,
                    if eligibility == "excluded" {
                        RouteEligibility::Excluded
                    } else {
                        RouteEligibility::ProbeRequired
                    },
                    "newer manual {expected} revived older healthy source"
                );
            }
        }
    }

    #[test]
    fn v3_manual_cross_model_follower_shares_one_physical_k_then_refreshes() {
        use super::super::fresh_index::{KeyedGeneration, rebuild_keyed_offline};
        let f = Fixture::new(r#"printf '{"used_percent":24,"resets_at":"2099-01-01T00:00:00Z"}'"#);
        let path = f.source.join("providers.toml");
        let providers = fs::read_to_string(&path).unwrap();
        fs::write(
            &path,
            providers
                .replace("[first]\n", "[first]\nprompt_mode = 'stdin'\n")
                .replace("[second]\n", "[second]\nprompt_mode = 'stdin'\n"),
        )
        .unwrap();
        fs::write(
            f.source.join("models/alias.toml"),
            "[[providers]]\nname = 'first'\n",
        )
        .unwrap();
        let socket = f._temp.path().join("absent.sock");
        drop(broker_admission_lease(&f.ledger).unwrap());
        rebuild_keyed_offline(&f.ledger, &socket, &f.source).unwrap();
        let lease = broker_admission_lease(&f.ledger).unwrap();
        let generation =
            KeyedGeneration::admit_provider_readback(&f.ledger, &lease, &f.source).unwrap();
        let source = File::open(&f.source).unwrap();
        let uid = unsafe { libc::getuid() };
        let gid = unsafe { libc::getgid() };
        let first = f.request("first");
        assert_eq!(
            begin_v3(&f.ledger, &source, &first, uid, gid, &generation)
                .unwrap()
                .state,
            "unknown"
        );
        let alias_pool = oulipoly_runtime::executor::cli::fresh_remote::load_fresh_headless_pool(
            &f.source, "alias",
        )
        .unwrap();
        let alias = ManualQuotaRequest {
            operation_id: uuid::Uuid::new_v4().to_string(),
            model: "alias".into(),
            account: "first".into(),
            config_sha256: alias_pool.config_sha256,
            environment: first.environment.clone(),
        };
        let follower = begin_v3(&f.ledger, &source, &alias, uid, gid, &generation).unwrap();
        assert_eq!(follower.state, "unknown");
        assert_eq!(
            follower.effect_id,
            readback_id_v3(&f.ledger, &first.operation_id, uid, gid, &generation)
                .unwrap()
                .effect_id
        );
        assert!(
            !operation_dir(&f.ledger, &alias.operation_id)
                .join("k.json")
                .exists()
        );
        worker_with_environment(
            &operation_dir(&f.ledger, &first.operation_id),
            &first.environment,
        )
        .unwrap();
        assert_eq!(
            readback_id_v3(&f.ledger, &alias.operation_id, uid, gid, &generation)
                .unwrap()
                .outcome
                .as_deref(),
            Some("valid_windows")
        );
        drop(generation);
        drop(lease);
        let lease = broker_admission_lease(&f.ledger).unwrap();
        let restarted =
            KeyedGeneration::admit_provider_readback(&f.ledger, &lease, &f.source).unwrap();
        assert_eq!(
            readback_id_v3(&f.ledger, &alias.operation_id, uid, gid, &restarted)
                .unwrap()
                .outcome
                .as_deref(),
            Some("valid_windows")
        );
        let later = f.request("first");
        assert_eq!(
            begin_v3(&f.ledger, &source, &later, uid, gid, &restarted)
                .unwrap()
                .state,
            "unknown"
        );
        assert!(
            operation_dir(&f.ledger, &later.operation_id)
                .join("k.json")
                .exists()
        );
    }

    #[test]
    fn v3_manual_exact_io_is_constant_with_hundreds_of_settled_sources() {
        use super::super::fresh_index::{
            KeyedGeneration, ReaderIo, ReaderIoGuard, last_reader_io, measure_keyed_io,
            rebuild_keyed_offline,
        };
        fn measured(count: usize) -> Vec<(super::super::fresh_index::KeyedIoCount, ReaderIo)> {
            let f =
                Fixture::new(r#"printf '{"used_percent":24,"resets_at":"2099-01-01T00:00:00Z"}'"#);
            let path = f.source.join("providers.toml");
            let providers = fs::read_to_string(&path).unwrap();
            fs::write(
                &path,
                providers
                    .replace("[first]\n", "[first]\nprompt_mode = 'stdin'\n")
                    .replace("[second]\n", "[second]\nprompt_mode = 'stdin'\n"),
            )
            .unwrap();
            let socket = f._temp.path().join("absent.sock");
            drop(broker_admission_lease(&f.ledger).unwrap());
            rebuild_keyed_offline(&f.ledger, &socket, &f.source).unwrap();
            let lease = broker_admission_lease(&f.ledger).unwrap();
            let generation =
                KeyedGeneration::admit_provider_readback(&f.ledger, &lease, &f.source).unwrap();
            let source = File::open(&f.source).unwrap();
            let uid = unsafe { libc::getuid() };
            let gid = unsafe { libc::getgid() };
            for n in 0..count {
                let mut request = f.request("first");
                request.environment.push(("SOURCE".into(), n.to_string()));
                assert_eq!(
                    begin_v3(&f.ledger, &source, &request, uid, gid, &generation)
                        .unwrap()
                        .state,
                    "unknown"
                );
                worker_with_environment(
                    &operation_dir(&f.ledger, &request.operation_id),
                    &request.environment,
                )
                .unwrap();
                assert_eq!(
                    readback_id_v3(&f.ledger, &request.operation_id, uid, gid, &generation)
                        .unwrap()
                        .outcome
                        .as_deref(),
                    Some("valid_windows")
                );
            }
            let mut request = f.request("first");
            request.environment.push(("SOURCE".into(), "writer".into()));
            let step = |label: &'static str, run: &mut dyn FnMut()| {
                let (_, keyed) = measure_keyed_io(|| {
                    let _guard = ReaderIoGuard::start(label);
                    run();
                });
                (keyed, last_reader_io().unwrap())
            };
            let begin = step("v3-manual-begin", &mut || {
                assert_eq!(
                    begin_v3(&f.ledger, &source, &request, uid, gid, &generation)
                        .unwrap()
                        .state,
                    "unknown"
                );
            });
            worker_with_environment(
                &operation_dir(&f.ledger, &request.operation_id),
                &request.environment,
            )
            .unwrap();
            let settle = step("v3-manual-settle", &mut || {
                assert_eq!(
                    readback_id_v3(&f.ledger, &request.operation_id, uid, gid, &generation)
                        .unwrap()
                        .outcome
                        .as_deref(),
                    Some("valid_windows")
                );
            });
            let read = step("v3-manual-read", &mut || {
                assert_eq!(
                    readback_id_v3(&f.ledger, &request.operation_id, uid, gid, &generation)
                        .unwrap()
                        .outcome
                        .as_deref(),
                    Some("valid_windows")
                );
            });
            vec![begin, settle, read]
        }
        let small = measured(1);
        let large = measured(205);
        eprintln!("v3 manual begin/settle/read keyed+physical 1={small:?} 205={large:?}");
        for (one, many) in small.iter().zip(&large) {
            assert_eq!(one.0.open_attempts, many.0.open_attempts);
            assert_eq!(one.0.opened, many.0.opened);
            assert_eq!(one.1.open_attempts, many.1.open_attempts);
            assert_eq!(one.1.opened, many.1.opened);
            assert_eq!(one.0.directory_entries + one.1.directory_entries, 0);
            assert_eq!(many.0.directory_entries + many.1.directory_entries, 0);
        }
        assert!(small[0].0.bytes_written + small[0].1.bytes_written > 0);
        assert!(small[1].0.bytes_written + small[1].1.bytes_written > 0);
        assert_eq!(small[2].0.bytes_written + small[2].1.bytes_written, 0);
    }
}
