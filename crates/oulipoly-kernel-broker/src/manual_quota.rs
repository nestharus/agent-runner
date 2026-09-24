//! Broker-custodied local manual quota K/Q. This private operation has no D
//! key and cannot launch a model. Its source is the pinned config directory;
//! the caller supplies only an account selector and effect environment.
use chrono::Utc;
use oulipoly_kernel_broker::protocol::{ManualQuotaReadback, ManualQuotaRequest};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const MAX_OUTPUT: u64 = 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    version: u32,
    request: ManualQuotaRequest,
    environment_sha256: String,
    peer_uid: u32,
    peer_gid: u32,
    physical_account_id: String,
    quota_script: Option<String>,
    auth_refresh_command: Option<String>,
    source_device: u64,
    source_inode: u64,
    effect_id: Option<String>,
    source_operation_id: Option<String>,
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
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(dir.join(name))?;
    serde_json::to_writer(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    File::open(dir)?.sync_all()
}

fn read_exact<T: for<'a> Deserialize<'a>>(dir: &Path, name: &str) -> io::Result<Option<T>> {
    match File::open(dir.join(name)) {
        Ok(file) => serde_json::from_reader(file)
            .map(Some)
            .map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn operation_dir(directory: &Path, id: &str) -> PathBuf {
    directory.join("manual-quota").join(id)
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
                || key.starts_with("LD_")
                || key.starts_with("DYLD_")
                || key.starts_with("OULIPOLY_KERNEL_")
                || matches!(key.as_str(), "GLIBC_TUNABLES" | "GCONV_PATH")
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

pub(super) fn begin(
    directory: &Path,
    source: &File,
    request: &ManualQuotaRequest,
    uid: u32,
    gid: u32,
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

pub(super) fn readback_id(
    directory: &Path,
    operation_id: &str,
    uid: u32,
    gid: u32,
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
    let stdout = fs::read(dir.join("stdout.bin"))?;
    let stderr = fs::read(dir.join("stderr.bin"))?;
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
}
