//! Broker-owned evidence index and detached frozen rebuild. The private broker
//! holds the admission lease. The fixture route and original root provider
//! writers mirror exact provider, account-effect, and manual quota evidence
//! here; routing and effect/manual readers still use retained files. Index Q
//! pointers follow physical certification and a
//! typed terminal marker; their hashes alone do not certify completion.
//!
//! Lock order for a same-broker cutover: route lock, then account locks
//! in sorted physical-key order. Grant/effect admission takes only its account
//! lock. PID1 Q takes no index lock. Never acquire route lock from an account
//! lock. Each operation here takes its own required lock; a caller holding an
//! account lock must not call a route operation.
//! `index-v1/route.lock` is distinct from the current live
//! `route-selection.lock`; a future cutover must join their authority before
//! either can admit a decision.
use chrono::DateTime;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};

// v1 account records have no atomic compact head. They must be rebuilt offline.
const VERSION: u32 = 2;
const ADMISSION_PROTOCOL_VERSION: u32 = 1;
const MAX_RECORD: u64 = 4 * 1024 * 1024;
const MAX_RECENT_FAILURES: usize = 256;
const MAX_HEAD_PENDING: usize = 128;
const MAX_HEAD_SOURCES: usize = 128;

/// Counts actual open attempts/successes and directory entries while an opt-in
/// route decision or pre-K read is running on this thread. The guard is placed
/// at the production read boundary, so a future scan is visible in the audit.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(super) struct ReaderIo {
    pub open_attempts: u64,
    pub opened: u64,
    pub directory_entries: u64,
}
thread_local! {
    static READER_IO: Cell<Option<ReaderIo>> = const { Cell::new(None) };
    #[cfg(test)]
    static LAST_READER_IO: Cell<Option<ReaderIo>> = const { Cell::new(None) };
}
#[cfg(test)]
pub(super) fn last_reader_io() -> Option<ReaderIo> {
    LAST_READER_IO.with(Cell::get)
}
pub(super) fn reader_open_attempt() {
    READER_IO.with(|cell| {
        if let Some(mut count) = cell.get() {
            count.open_attempts += 1;
            cell.set(Some(count));
        }
    });
}
pub(super) fn reader_opened() {
    READER_IO.with(|cell| {
        if let Some(mut count) = cell.get() {
            count.opened += 1;
            cell.set(Some(count));
        }
    });
}
pub(super) fn reader_directory_entry() {
    READER_IO.with(|cell| {
        if let Some(mut count) = cell.get() {
            count.directory_entries += 1;
            cell.set(Some(count));
        }
    });
}
pub(super) struct ReaderIoGuard(&'static str);
impl ReaderIoGuard {
    pub(super) fn start(boundary: &'static str) -> Self {
        READER_IO.with(|cell| {
            assert!(cell.get().is_none(), "nested route reader audit");
            cell.set(Some(ReaderIo::default()));
        });
        Self(boundary)
    }
}
impl Drop for ReaderIoGuard {
    fn drop(&mut self) {
        let count = READER_IO.with(|cell| cell.replace(None).unwrap_or_default());
        #[cfg(test)]
        LAST_READER_IO.with(|cell| cell.set(Some(count)));
        eprintln!(
            "age319 indexed {} read: open_attempts={} opened={} directory_entries={}",
            self.0, count.open_attempts, count.opened, count.directory_entries
        );
    }
}

#[derive(Debug)]
pub(super) enum IndexError {
    RebuildRequired(&'static str),
    Corrupt(String),
    Conflict(&'static str),
    Io(io::Error),
}
impl From<io::Error> for IndexError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}
impl std::fmt::Display for IndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RebuildRequired(s) => write!(f, "offline rebuild required: {s}"),
            Self::Corrupt(s) => write!(f, "index corruption: {s}"),
            Self::Conflict(s) => write!(f, "index conflict: {s}"),
            Self::Io(e) => write!(f, "index I/O: {e}"),
        }
    }
}
impl std::error::Error for IndexError {}
pub(super) type Result<T> = std::result::Result<T, IndexError>;
fn corrupt(s: impl Into<String>) -> IndexError {
    IndexError::Corrupt(s.into())
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn keyed<T: Serialize>(key: &T) -> Result<String> {
    Ok(hash(
        &serde_json::to_vec(key).map_err(|e| corrupt(e.to_string()))?,
    ))
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Sealed<T> {
    data: T,
    sha256: String,
}
fn read<T: DeserializeOwned + Serialize>(path: &Path) -> Result<Option<T>> {
    reader_open_attempt();
    let file = match File::open(path) {
        Ok(f) => {
            reader_opened();
            f
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if file.metadata()?.len() > MAX_RECORD {
        return Err(corrupt(format!("oversize {}", path.display())));
    }
    let mut bytes = Vec::new();
    file.take(MAX_RECORD + 1).read_to_end(&mut bytes)?;
    let sealed: Sealed<T> =
        serde_json::from_slice(&bytes).map_err(|e| corrupt(format!("{}: {e}", path.display())))?;
    let actual = keyed(&sealed.data)?;
    if actual != sealed.sha256 {
        return Err(corrupt(format!("checksum {}", path.display())));
    }
    Ok(Some(sealed.data))
}
fn sync_dir(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}
fn write_atomic<T: Serialize>(path: &Path, data: &T) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| corrupt("record parent absent"))?;
    let bytes = serde_json::to_vec(&Sealed {
        data,
        sha256: keyed(data)?,
    })
    .map_err(|e| corrupt(e.to_string()))?;
    if bytes.len() as u64 > MAX_RECORD {
        return Err(IndexError::Conflict("record exceeds bound"));
    }
    let temp = parent.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)?;
    let result = (|| -> Result<()> {
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        sync_dir(parent)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
fn write_new<T: Serialize>(path: &Path, data: &T) -> Result<()> {
    let bytes = serde_json::to_vec(&Sealed {
        data,
        sha256: keyed(data)?,
    })
    .map_err(|e| corrupt(e.to_string()))?;
    if bytes.len() as u64 > MAX_RECORD {
        return Err(IndexError::Conflict("record exceeds bound"));
    }
    let parent = path
        .parent()
        .ok_or_else(|| corrupt("record parent absent"))?;
    let temp = parent.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)?;
    let result = (|| -> Result<()> {
        f.write_all(&bytes)?;
        f.sync_all()?;
        // hard_link publishes an immutable name without replacing an existing
        // decision. The temporary inode is already durable before visibility.
        fs::hard_link(&temp, path)?;
        sync_dir(parent)?;
        fs::remove_file(&temp)?;
        sync_dir(parent)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
fn locked(path: &Path) -> Result<File> {
    reader_open_attempt();
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    reader_opened();
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
            return Ok(file);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error.into());
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AdmissionMode {
    Broker,
    Frozen,
}
static ADMISSION_IN_PROCESS: OnceLock<Mutex<HashMap<PathBuf, AdmissionMode>>> = OnceLock::new();
fn admission_table() -> &'static Mutex<HashMap<PathBuf, AdmissionMode>> {
    ADMISSION_IN_PROCESS.get_or_init(|| Mutex::new(HashMap::new()))
}
pub(super) struct AdmissionLease {
    file: Option<File>,
    path: PathBuf,
}
impl Drop for AdmissionLease {
    fn drop(&mut self) {
        if let Ok(mut table) = admission_table().lock() {
            // POSIX record locks are process-owned. Close before making this
            // path available to another thread in the same process.
            drop(self.file.take());
            table.remove(&self.path);
        }
    }
}
fn admission_lock(file: &File, mode: AdmissionMode) -> Result<()> {
    let lock = libc::flock {
        l_type: if mode == AdmissionMode::Broker {
            libc::F_RDLCK
        } else {
            libc::F_WRLCK
        } as _,
        l_whence: libc::SEEK_SET as _,
        l_start: 0,
        l_len: 0,
        l_pid: 0,
    };
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETLK, &lock) } != 0 {
        return Err(IndexError::Conflict("broker admission lock held"));
    }
    Ok(())
}
fn take_admission(path: &Path, create: bool, mode: AdmissionMode) -> Result<AdmissionLease> {
    let canonical_parent = path
        .parent()
        .ok_or_else(|| corrupt("admission path has no parent"))?
        .canonicalize()?;
    let canonical_path = canonical_parent.join("admission.lock");
    let mut table = admission_table()
        .lock()
        .map_err(|_| corrupt("admission lock poisoned"))?;
    // Check before opening: closing any descriptor for a POSIX-locked inode
    // releases this process's lock, even if another descriptor still exists.
    if table.contains_key(&canonical_path) {
        return Err(IndexError::Conflict("broker admission active in process"));
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(create)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&canonical_path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(corrupt("admission lock inode is not private regular file"));
    }
    admission_lock(&file, mode)?;
    table.insert(canonical_path.clone(), mode);
    Ok(AdmissionLease {
        file: Some(file),
        path: canonical_path,
    })
}

/// Held for the whole lifetime of a fresh broker, including every accepted
/// request. A rebuild needs the exclusive side and fails immediately if any
/// compatible broker is still admitting requests. POSIX locks are not
/// inherited across fork, so PID1 may finish Q while the broker is stopped.
pub(super) fn broker_admission_lease(root: &Path) -> Result<AdmissionLease> {
    let base = root.join("index-v1");
    fs::create_dir_all(&base)?;
    let lease = take_admission(&base.join("admission.lock"), true, AdmissionMode::Broker)?;
    // This marker means a version with the lifetime lease has actually run.
    // Old pre-index binaries never wrote it and cannot be frozen by this lock.
    let marker = base.join("admission-protocol.json");
    if marker.exists() {
        let version: u32 = read(&marker)?.ok_or_else(|| corrupt("admission marker absent"))?;
        if version != ADMISSION_PROTOCOL_VERSION {
            return Err(IndexError::RebuildRequired("admission protocol changed"));
        }
    } else {
        write_new(&marker, &ADMISSION_PROTOCOL_VERSION)?;
    }
    sync_dir(root)?;
    Ok(lease)
}

fn frozen_admission(root: &Path, socket: &Path) -> Result<AdmissionLease> {
    let base = root.join("index-v1");
    if read::<u32>(&base.join("admission-protocol.json"))? != Some(ADMISSION_PROTOCOL_VERSION) {
        return Err(IndexError::RebuildRequired(
            "no compatible broker admission freeze proof",
        ));
    }
    let lease = take_admission(&base.join("admission.lock"), false, AdmissionMode::Frozen)?;
    match UnixStream::connect(socket) {
        Ok(_) => return Err(IndexError::Conflict("broker socket still accepts requests")),
        Err(e) if matches!(e.raw_os_error(), Some(libc::ENOENT | libc::ECONNREFUSED)) => {}
        Err(e) => return Err(IndexError::Io(e)),
    }
    Ok(lease)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    generation: String,
    #[serde(default)]
    generation_dir: bool,
}
#[derive(Clone, Debug)]
pub(super) struct Index {
    root: PathBuf,
    generation: String,
    storage: PathBuf,
    route_reader_probe: bool,
}
/// The caller must hold an admission freeze for this previously unused broker
/// directory. The empty-directory check below is a second, local guard.
pub(super) enum GenesisAuthority {
    ConfirmedFreshEmptyDirectory,
}
impl Index {
    /// The private reader probe remains non-activating until revision-joined
    /// choice and pre-K admission, plus the old-writer deployment guard.
    pub(super) fn enable_route_reader_probe(mut self) -> Self {
        self.route_reader_probe = true;
        self
    }
    pub(super) fn route_reader_probe(&self) -> bool {
        self.route_reader_probe
    }
    pub(super) fn route_reader_preflight(&self, physical_key: &str) -> Result<()> {
        let head = self.compact_account(physical_key)?;
        if head.pending.keys().any(|key| key.starts_with("grant:")) {
            return Err(IndexError::Conflict(
                "indexed route reader has announced provider debt",
            ));
        }
        if !head.pending.is_empty() {
            return Err(IndexError::Conflict(
                "indexed route reader has announced effect or manual debt",
            ));
        }
        // A typed read is still not an atomic route/pre-K revision join.
        Err(IndexError::RebuildRequired(
            "route reader lacks atomic account revision join",
        ))
    }
    pub(super) fn compact_account(&self, key: &str) -> Result<AccountHead> {
        let _lock = locked(&self.key_path("accounts", &format!("lock:{key}"))?)?;
        self.check_generation()?;
        if !self.known("accounts", &key.to_owned())? {
            return Err(IndexError::RebuildRequired(
                "compact account has no known key",
            ));
        }
        let head: AccountHead = read(&self.key_path("heads", &key)?)?
            .ok_or(IndexError::RebuildRequired("compact account head absent"))?;
        head.validate_current(&self.root, &self.generation, key)?;
        Ok(head)
    }
    pub(super) fn evidence_root(&self) -> &Path {
        &self.root
    }
    pub(super) fn generation(&self) -> &str {
        &self.generation
    }
    /// Explicit authority for a genuinely empty broker evidence directory.
    /// A missing manifest on an old nonempty directory always needs an offline
    /// rebuild, which this substrate deliberately does not implement.
    pub(super) fn create_fresh_genesis(root: &Path, _authority: GenesisAuthority) -> Result<Self> {
        fs::create_dir_all(root)?;
        if root.read_dir()?.next().is_some() {
            return Err(IndexError::RebuildRequired(
                "pre-index broker evidence exists",
            ));
        }
        let base = root.join("index-v1");
        fs::create_dir_all(&base)?;
        let _guard = locked(&base.join("genesis.lock"))?;
        for entry in root.read_dir()? {
            if entry?.file_name() != "index-v1" {
                return Err(IndexError::RebuildRequired(
                    "pre-index broker evidence exists",
                ));
            }
        }
        for entry in base.read_dir()? {
            if entry?.file_name() != "genesis.lock" {
                return Err(IndexError::RebuildRequired(
                    "index directory lacks a clean genesis",
                ));
            }
        }
        for name in ["cursors", "accounts", "heads", "decisions"] {
            fs::create_dir(base.join(name))?;
        }
        for name in ["cursors", "accounts", "heads", "decisions"] {
            sync_dir(&base.join(name))?;
        }
        sync_dir(&base)?;
        let manifest = Manifest {
            version: VERSION,
            generation: uuid::Uuid::new_v4().to_string(),
            generation_dir: false,
        };
        write_new(&base.join("manifest.json"), &manifest)?;
        sync_dir(root)?;
        Ok(Self {
            root: root.to_owned(),
            generation: manifest.generation,
            storage: base,
            route_reader_probe: false,
        })
    }
    pub(super) fn open(root: &Path) -> Result<Self> {
        let path = root.join("index-v1/manifest.json");
        let manifest: Manifest = read(&path)?.ok_or(IndexError::RebuildRequired(
            "manifest absent; pre-index history cannot be treated as empty",
        ))?;
        if manifest.version != VERSION
            || uuid::Uuid::parse_str(&manifest.generation)
                .map_or(true, |u| u.is_nil() || u.to_string() != manifest.generation)
        {
            return Err(IndexError::RebuildRequired(
                "unsupported or invalid index generation",
            ));
        }
        let storage = if manifest.generation_dir {
            root.join("index-v1/generations").join(&manifest.generation)
        } else {
            root.join("index-v1")
        };
        for name in ["cursors", "accounts", "heads", "decisions"] {
            if !storage.join(name).is_dir() {
                return Err(IndexError::RebuildRequired("index storage missing"));
            }
        }
        Ok(Self {
            root: root.to_owned(),
            generation: manifest.generation,
            storage,
            route_reader_probe: false,
        })
    }
    #[cfg(test)]
    pub(super) fn downgrade_manifest_for_migration_test(root: &Path) -> Result<()> {
        let path = root.join("index-v1/manifest.json");
        let mut manifest: Manifest = read(&path)?.ok_or_else(|| corrupt("test manifest absent"))?;
        manifest.version = 1;
        write_atomic(&path, &manifest)
    }
    fn base(&self) -> PathBuf {
        self.storage.clone()
    }
    fn check_generation(&self) -> Result<()> {
        let current = Self::open(&self.root)?;
        if current.generation != self.generation {
            return Err(IndexError::RebuildRequired("index generation changed"));
        }
        Ok(())
    }
    fn key_path<T: Serialize>(&self, class: &str, key: &T) -> Result<PathBuf> {
        Ok(self
            .base()
            .join(class)
            .join(format!("{}.json", keyed(key)?)))
    }
    fn known_path<T: Serialize>(&self, class: &str, key: &T) -> Result<PathBuf> {
        Ok(self
            .base()
            .join(class)
            .join(format!("{}.known.json", keyed(key)?)))
    }
    fn known<T: Serialize + DeserializeOwned + PartialEq>(
        &self,
        class: &str,
        key: &T,
    ) -> Result<bool> {
        let marker: Option<KnownKey<T>> = read(&self.known_path(class, key)?)?;
        match marker {
            Some(m) if m.generation == self.generation && m.key == *key => Ok(true),
            Some(_) => Err(corrupt("known-key collision or generation mismatch")),
            None => Ok(false),
        }
    }
    fn mark_known<T: Serialize + DeserializeOwned + PartialEq>(
        &self,
        class: &str,
        key: &T,
    ) -> Result<()> {
        if !self.known(class, key)? {
            write_new(
                &self.known_path(class, key)?,
                &KnownKey {
                    generation: self.generation.clone(),
                    key,
                },
            )?;
        }
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct KnownKey<T> {
    generation: String,
    key: T,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct CursorKey {
    pub model: String,
    pub config_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Cursor {
    generation: String,
    key: CursorKey,
    pub sequence: u64,
    pub index: Option<usize>,
    last_handoff: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Decision {
    pub generation: String,
    pub handoff: String,
    pub key: CursorKey,
    pub candidate_identity: String,
    pub candidate_index: usize,
    pub pin: bool,
    pub sequence: u64,
    #[serde(default)]
    receipt: Option<Artifact>,
}

#[derive(Clone, Debug)]
pub(super) struct OfflineDecision {
    pub handoff: String,
    pub key: CursorKey,
    pub candidate_identity: String,
    pub candidate_index: usize,
    pub pin: bool,
    pub sequence: u64,
    pub receipt: Artifact,
}

#[derive(Default)]
pub(super) struct OfflineSnapshot {
    pub decisions: Vec<OfflineDecision>,
    pub accounts: BTreeMap<String, Account>,
    pub source_models: BTreeMap<String, String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Pending {
    generation: String,
    expected: Cursor,
    decision: Decision,
    decision_sha256: String,
}

impl Index {
    /// Service admission for the optional live route writer. The caller holds
    /// this root's broker lease, before binding the request socket. A missing
    /// manifest is a genesis only when the lease's two files are the entire
    /// retained root; every other missing-manifest state needs offline repair.
    pub(super) fn admit_live_routes(root: &Path, lease: &AdmissionLease) -> Result<Self> {
        let expected = root.canonicalize()?.join("index-v1/admission.lock");
        if lease.path != expected {
            return Err(IndexError::Conflict("route index admission lease differs"));
        }
        let manifest = root.join("index-v1/manifest.json");
        let index = if manifest.exists() {
            Self::open(root)?
        } else {
            let base = root.join("index-v1");
            let root_entries = fs::read_dir(root)?
                .map(|e| e.map(|e| e.file_name()))
                .collect::<io::Result<Vec<_>>>()?;
            let base_entries = fs::read_dir(&base)?
                .map(|e| e.map(|e| e.file_name()))
                .collect::<io::Result<std::collections::HashSet<_>>>()?;
            if root_entries != ["index-v1"]
                || base_entries
                    != ["admission.lock".into(), "admission-protocol.json".into()].into()
            {
                return Err(IndexError::RebuildRequired(
                    "pre-index broker evidence exists",
                ));
            }
            if read::<u32>(&base.join("admission-protocol.json"))?
                != Some(ADMISSION_PROTOCOL_VERSION)
            {
                return Err(IndexError::RebuildRequired("admission protocol changed"));
            }
            let _guard = locked(&base.join("genesis.lock"))?;
            for name in ["cursors", "accounts", "heads", "decisions"] {
                fs::create_dir(base.join(name))?;
                sync_dir(&base.join(name))?;
            }
            sync_dir(&base)?;
            let generation = uuid::Uuid::new_v4().to_string();
            write_new(
                &base.join("manifest.json"),
                &Manifest {
                    version: VERSION,
                    generation,
                    generation_dir: false,
                },
            )?;
            sync_dir(root)?;
            Self::open(root)?
        };
        index.validate_live_routes()?;
        for key in index.live_account_keys()? {
            index.compact_account(&key)?;
        }
        super::fresh_provider::reconcile_live_provider_accounts(&index)
            .map_err(|error| corrupt(format!("live provider account admission: {error}")))?;
        super::fresh_provider::reconcile_live_account_effects(&index)
            .map_err(|error| corrupt(format!("live effect account admission: {error}")))?;
        super::manual_quota::reconcile_live_manual_accounts(&index)
            .map_err(|error| corrupt(format!("live manual account admission: {error}")))?;
        Ok(index)
    }

    /// At service admission, compare the entire retained route receipt set to
    /// the generation. This is deliberately a full scan; choice/effect readers
    /// have not been converted to bounded index reads.
    fn validate_live_routes(&self) -> Result<()> {
        let _lock = locked(&self.base().join("route.lock"))?;
        self.check_generation()?;
        self.reconcile_pending()?;
        let mut indexed = std::collections::HashSet::new();
        let mut sequences = HashMap::<String, (CursorKey, std::collections::HashSet<u64>)>::new();
        for entry in fs::read_dir(self.base().join("decisions"))? {
            let path = entry?.path();
            if !path.extension().is_some_and(|x| x == "json") {
                continue;
            }
            if path.to_string_lossy().contains(".known.") {
                let marker: KnownKey<String> =
                    read(&path)?.ok_or_else(|| corrupt("decision known marker absent"))?;
                if marker.generation != self.generation
                    || path != self.known_path("decisions", &marker.key)?
                    || self.read_decision(&marker.key)?.is_none()
                {
                    return Err(corrupt("decision known marker lacks receipt"));
                }
                continue;
            }
            let decision: Decision =
                read(&path)?.ok_or_else(|| corrupt("indexed route decision absent"))?;
            if !self.known("decisions", &decision.handoff)?
                || path != self.decision_path(&decision.handoff)?
                || self.read_decision(&decision.handoff)? != Some(decision.clone())
            {
                return Err(corrupt("indexed route decision changed"));
            }
            let expected_path = format!("{}.route-selection.json", decision.handoff);
            if decision
                .receipt
                .as_ref()
                .is_none_or(|r| r.path != expected_path)
            {
                return Err(IndexError::RebuildRequired(
                    "indexed route receipt binding absent",
                ));
            }
            super::fresh_provider::validate_indexed_receipt(&self.root, &decision)
                .map_err(|e| corrupt(format!("indexed broker route receipt: {e}")))?;
            if !indexed.insert(decision.handoff.clone()) {
                return Err(corrupt("duplicate indexed route decision"));
            }
            if !decision.pin {
                let entry = sequences
                    .entry(keyed(&decision.key)?)
                    .or_insert((decision.key.clone(), std::collections::HashSet::new()));
                if entry.0 != decision.key {
                    return Err(corrupt("route cursor key collision"));
                }
                if decision.sequence == 0 || !entry.1.insert(decision.sequence) {
                    return Err(corrupt("route decision sequence repeated or zero"));
                }
            }
        }
        let mut receipts = std::collections::HashSet::new();
        for entry in fs::read_dir(&self.root)? {
            let name = entry?.file_name();
            let name = name.to_string_lossy();
            if let Some(handoff) = name.strip_suffix(".route-selection.json") {
                if !indexed.contains(handoff) || !receipts.insert(handoff.to_owned()) {
                    return Err(IndexError::RebuildRequired("orphan broker route receipt"));
                }
            }
        }
        if indexed != receipts {
            return Err(IndexError::RebuildRequired(
                "indexed broker route receipt missing",
            ));
        }
        let mut cursor_keys = std::collections::HashSet::new();
        for entry in fs::read_dir(self.base().join("cursors"))? {
            let path = entry?.path();
            if !path.extension().is_some_and(|x| x == "json") {
                continue;
            }
            let key = if path.to_string_lossy().contains(".known.") {
                let marker: KnownKey<CursorKey> =
                    read(&path)?.ok_or_else(|| corrupt("cursor known marker absent"))?;
                if marker.generation != self.generation
                    || path != self.known_path("cursors", &marker.key)?
                {
                    return Err(corrupt("cursor known marker changed"));
                }
                marker.key
            } else {
                let cursor: Cursor = read(&path)?.ok_or_else(|| corrupt("route cursor absent"))?;
                if path != self.key_path("cursors", &cursor.key)? {
                    return Err(corrupt("route cursor path changed"));
                }
                cursor.key
            };
            cursor_keys.insert(keyed(&key)?);
        }
        for digest in cursor_keys {
            let (key, set) = sequences
                .remove(&digest)
                .ok_or_else(|| corrupt("route cursor has no indexed receipts"))?;
            let count = set.len() as u64;
            if count == 0
                || set.iter().copied().min() != Some(1)
                || set.iter().copied().max() != Some(count)
                || self.cursor_unlocked(&key)?.sequence != count
            {
                return Err(corrupt("route cursor count differs from receipts"));
            }
        }
        if !sequences.is_empty() {
            return Err(corrupt("route cursor absent for indexed receipts"));
        }
        Ok(())
    }

    pub(super) fn require_live_route(&self, handoff: &str) -> Result<()> {
        let decision = self.decision(handoff)?.ok_or(IndexError::RebuildRequired(
            "broker route receipt lacks indexed decision",
        ))?;
        if decision
            .receipt
            .as_ref()
            .is_none_or(|r| r.path != format!("{handoff}.route-selection.json"))
        {
            return Err(IndexError::RebuildRequired(
                "indexed route receipt binding absent",
            ));
        }
        if !decision.pin && self.cursor(&decision.key)?.sequence < decision.sequence {
            return Err(corrupt("indexed route cursor has not published decision"));
        }
        super::fresh_provider::validate_indexed_receipt(&self.root, &decision)
            .map_err(|e| corrupt(format!("indexed broker route receipt: {e}")))
    }
    /// Detached maintenance only. The caller gives the actual broker socket
    /// and the retained source directory; a changed source is unsupported.
    /// The generation is built off to the side and the manifest is the only
    /// publication point. Every consumed K remains unresolved at publication.
    pub(super) fn rebuild_offline(root: &Path, socket: &Path, source: &Path) -> Result<Self> {
        Self::rebuild_offline_inner(root, socket, source, || {})
    }

    fn rebuild_offline_inner(
        root: &Path,
        socket: &Path,
        source: &Path,
        after_scan: impl FnOnce(),
    ) -> Result<Self> {
        let _freeze = frozen_admission(root, socket)?;
        let source_before = source.metadata()?;
        if !source_before.is_dir() {
            return Err(corrupt("offline config source is not a directory"));
        }
        let snapshot = super::fresh_provider::offline_snapshot(root, source)
            .map_err(|e| corrupt(format!("retained evidence: {e}")))?;
        after_scan();
        let source_after = source.metadata()?;
        if (source_before.dev(), source_before.ino()) != (source_after.dev(), source_after.ino()) {
            return Err(corrupt(
                "offline config source directory changed during scan",
            ));
        }
        for (model, digest) in &snapshot.source_models {
            let pool = oulipoly_runtime::executor::cli::fresh_remote::load_fresh_headless_pool(
                source, model,
            )
            .map_err(|e| corrupt(format!("offline source readback: {e}")))?;
            if &pool.config_sha256 != digest {
                return Err(corrupt("offline config source changed before publication"));
            }
        }
        Self::publish_offline(root, snapshot)
    }

    #[cfg(test)]
    pub(crate) fn rebuild_offline_test_hook(
        root: &Path,
        socket: &Path,
        source: &Path,
        after_scan: impl FnOnce(),
    ) -> Result<Self> {
        Self::rebuild_offline_inner(root, socket, source, after_scan)
    }

    /// Detached exact Q reconciliation after the generation is visible.
    /// It visits only this physical account's unresolved references.
    pub(super) fn reconcile_offline_account(&self, key: &str, source: &Path) -> Result<Account> {
        super::fresh_provider::reconcile_offline_account(self, key, source)
            .map_err(|e| corrupt(format!("exact Q reconciliation: {e}")))
    }

    pub(super) fn reconcile_offline_account_frozen(
        &self,
        socket: &Path,
        key: &str,
        source: &Path,
    ) -> Result<Account> {
        let _freeze = frozen_admission(&self.root, socket)?;
        self.reconcile_offline_account(key, source)
    }

    fn publish_offline(root: &Path, mut snapshot: OfflineSnapshot) -> Result<Self> {
        let base = root.join("index-v1");
        let generations = base.join("generations");
        fs::create_dir_all(&generations)?;
        let generation = uuid::Uuid::new_v4().to_string();
        let storage = generations.join(&generation);
        fs::create_dir(&storage)?;
        for class in ["cursors", "accounts", "heads", "decisions"] {
            fs::create_dir(storage.join(class))?;
        }
        sync_dir(&generations)?;
        let staged = Self {
            root: root.to_owned(),
            generation: generation.clone(),
            storage,
            route_reader_probe: false,
        };
        let mut cursors: BTreeMap<String, Cursor> = BTreeMap::new();
        let mut seen_handoffs = std::collections::HashSet::new();
        snapshot.decisions.sort_by_key(|d| d.sequence);
        for seed in snapshot.decisions {
            if !seen_handoffs.insert(seed.handoff.clone())
                || seed.handoff.is_empty()
                || seed.key.model.is_empty()
                || seed.key.config_sha256.is_empty()
                || seed.candidate_identity.is_empty()
            {
                return Err(corrupt("duplicate or empty offline decision identity"));
            }
            let digest = keyed(&seed.key)?;
            let cursor = cursors.entry(digest).or_insert_with(|| Cursor {
                generation: generation.clone(),
                key: seed.key.clone(),
                sequence: 0,
                index: None,
                last_handoff: None,
            });
            if cursor.key != seed.key {
                return Err(corrupt("offline cursor key collision"));
            }
            let decision = Decision {
                generation: generation.clone(),
                handoff: seed.handoff,
                key: seed.key,
                candidate_identity: seed.candidate_identity,
                candidate_index: seed.candidate_index,
                pin: seed.pin,
                sequence: seed.sequence,
                receipt: Some(seed.receipt),
            };
            let next = staged.advanced(cursor, &decision)?;
            write_new(&staged.decision_path(&decision.handoff)?, &decision)?;
            staged.mark_known("decisions", &decision.handoff)?;
            *cursor = next;
        }
        for cursor in cursors.into_values() {
            if cursor.sequence != 0 {
                staged.mark_known("cursors", &cursor.key)?;
                write_new(&staged.key_path("cursors", &cursor.key)?, &cursor)?;
            }
        }
        for (key, mut account) in snapshot.accounts {
            if key.is_empty() || account.physical_key != key {
                return Err(corrupt("offline account identity mismatch"));
            }
            account.generation = generation.clone();
            account.revision = account.revision.max(1);
            let head = AccountHead::from_account(root, &account)?;
            head.validate_current(root, &generation, &key)?;
            staged.mark_known("accounts", &key)?;
            write_new(&staged.key_path("accounts", &key)?, &account)?;
            write_new(&staged.key_path("heads", &key)?, &head)?;
        }
        for class in ["cursors", "accounts", "heads", "decisions"] {
            sync_dir(&staged.base().join(class))?;
        }
        sync_dir(&staged.base())?;
        // Validate every staged record before the only publication rename.
        for entry in fs::read_dir(staged.base().join("decisions"))? {
            let path = entry?.path();
            if path.extension().is_some_and(|x| x == "json")
                && !path.to_string_lossy().contains(".known.")
            {
                let record: Decision =
                    read(&path)?.ok_or_else(|| corrupt("staged decision absent"))?;
                if record.generation != generation
                    || !staged.known("decisions", &record.handoff)?
                    || staged.read_decision(&record.handoff)? != Some(record)
                {
                    return Err(corrupt("staged decision invalid"));
                }
            }
        }
        for entry in fs::read_dir(staged.base().join("cursors"))? {
            let path = entry?.path();
            if path.extension().is_some_and(|x| x == "json")
                && !path.to_string_lossy().contains(".known.")
            {
                let record: Cursor = read(&path)?.ok_or_else(|| corrupt("staged cursor absent"))?;
                if record.generation != generation || record.sequence == 0 {
                    return Err(corrupt("staged cursor invalid"));
                }
            }
        }
        for entry in fs::read_dir(staged.base().join("accounts"))? {
            let path = entry?.path();
            if path.extension().is_some_and(|x| x == "json")
                && !path.to_string_lossy().contains(".known.")
            {
                let record: Account =
                    read(&path)?.ok_or_else(|| corrupt("staged account absent"))?;
                if record.generation != generation {
                    return Err(corrupt("staged account invalid"));
                }
            }
        }
        let manifest = Manifest {
            version: VERSION,
            generation,
            generation_dir: true,
        };
        write_atomic(&base.join("manifest.json"), &manifest)?;
        Self::open(root)
    }
    fn cursor_unlocked(&self, key: &CursorKey) -> Result<Cursor> {
        let path = self.key_path("cursors", key)?;
        let record: Option<Cursor> = read(&path)?;
        if record.is_some() != self.known("cursors", key)? {
            return Err(corrupt("cursor record or known-key marker absent"));
        }
        let cursor = record.unwrap_or(Cursor {
            generation: self.generation.clone(),
            key: key.clone(),
            sequence: 0,
            index: None,
            last_handoff: None,
        });
        if cursor.key != *key
            || cursor.generation != self.generation
            || (cursor.sequence == 0) != cursor.index.is_none()
            || (cursor.sequence == 0) != cursor.last_handoff.is_none()
        {
            return Err(corrupt("cursor key, generation or sequence invariant"));
        }
        if let Some(handoff) = &cursor.last_handoff {
            let decision = self
                .read_decision(handoff)?
                .ok_or_else(|| corrupt("cursor's last decision absent"))?;
            if decision.key != *key
                || decision.pin
                || decision.sequence != cursor.sequence
                || Some(decision.candidate_index) != cursor.index
            {
                return Err(corrupt("cursor's last decision differs"));
            }
        }
        Ok(cursor)
    }
    pub(super) fn cursor(&self, key: &CursorKey) -> Result<Cursor> {
        let _lock = locked(&self.base().join("route.lock"))?;
        self.check_generation()?;
        self.reconcile_pending()?;
        self.cursor_unlocked(key)
    }
    fn decision_path(&self, handoff: &str) -> Result<PathBuf> {
        self.key_path("decisions", &handoff)
    }
    fn read_decision(&self, handoff: &str) -> Result<Option<Decision>> {
        let d: Option<Decision> = read(&self.decision_path(handoff)?)?;
        if d.is_none() && self.known("decisions", &handoff.to_owned())? {
            return Err(corrupt("known decision record absent"));
        }
        if d.as_ref()
            .is_some_and(|d| d.handoff != handoff || d.generation != self.generation)
        {
            return Err(corrupt("decision key or generation collision"));
        }
        if let Some(receipt) = d.as_ref().and_then(|d| d.receipt.as_ref()) {
            receipt.require_present(&self.root)?;
        }
        Ok(d)
    }
    fn reconcile_pending(&self) -> Result<()> {
        let pending_path = self.base().join("pending.json");
        let Some(pending): Option<Pending> = read(&pending_path)? else {
            return Ok(());
        };
        if pending.generation != self.generation
            || pending.decision.generation != self.generation
            || keyed(&pending.decision)? != pending.decision_sha256
            || pending.expected.key != pending.decision.key
        {
            return Err(corrupt("pending generation or decision digest"));
        }
        // The first cursor's known marker is fsynced after its decision and
        // before cursor publication. A crash in that gap is the only legal
        // marker-without-cursor state, and this exact pending decision repairs it.
        let current = if pending.expected.sequence == 0
            && self.key_path("cursors", &pending.expected.key)?.exists() == false
        {
            let marker = self.known("cursors", &pending.expected.key)?;
            if marker && self.read_decision(&pending.decision.handoff)?.is_none() {
                return Err(corrupt("cursor marker without decision"));
            }
            pending.expected.clone()
        } else {
            self.cursor_unlocked(&pending.expected.key)?
        };
        // A live receipt is the publication point. The index-owned decision
        // is recoverable only from this exact durable broker receipt. A crash
        // before receipt visibility leaves the cursor unchanged.
        if let Some(receipt) = &pending.decision.receipt {
            if !receipt.verify(&self.root)? {
                if self.read_decision(&pending.decision.handoff)?.is_some() {
                    return Err(corrupt("indexed decision lost its broker receipt"));
                }
                fs::remove_file(&pending_path)?;
                sync_dir(&self.base())?;
                return Ok(());
            }
            if self.read_decision(&pending.decision.handoff)?.is_none() {
                write_new(
                    &self.decision_path(&pending.decision.handoff)?,
                    &pending.decision,
                )?;
            }
        }
        let decision = self.read_decision(&pending.decision.handoff)?;
        if let Some(actual) = decision.as_ref() {
            if actual != &pending.decision {
                return Err(corrupt("pending decision readback mismatch"));
            }
        }
        let next = self.advanced(&pending.expected, &pending.decision)?;
        if current != pending.expected && current != next {
            return Err(corrupt("pending cursor prior/next mismatch"));
        }
        if decision.is_some() && current == pending.expected && next != current {
            self.mark_known("decisions", &pending.decision.handoff)?;
            self.mark_known("cursors", &current.key)?;
            write_atomic(&self.key_path("cursors", &current.key)?, &next)?;
        }
        if decision.is_some() {
            self.mark_known("decisions", &pending.decision.handoff)?;
        }
        // A missing decision means no sequence advance. A pin leaves the cursor
        // byte-for-byte unchanged even when its decision exists.
        fs::remove_file(&pending_path)?;
        sync_dir(&self.base())?;
        Ok(())
    }
    fn advanced(&self, old: &Cursor, decision: &Decision) -> Result<Cursor> {
        if decision.pin {
            if decision.sequence != 0 {
                return Err(corrupt("pin sequence is nonzero"));
            }
            return Ok(old.clone());
        }
        let next_sequence = old
            .sequence
            .checked_add(1)
            .ok_or(IndexError::Conflict("sequence overflow"))?;
        if decision.sequence != next_sequence {
            return Err(corrupt("noncontiguous decision sequence"));
        }
        Ok(Cursor {
            generation: self.generation.clone(),
            key: old.key.clone(),
            sequence: next_sequence,
            index: Some(decision.candidate_index),
            last_handoff: Some(decision.handoff.clone()),
        })
    }
    /// The decision is the immutable index-owned transaction artifact. The
    /// caller supplies exact identity; this does not create a live route grant.
    pub(super) fn commit_decision(
        &self,
        key: CursorKey,
        handoff: String,
        candidate_identity: String,
        candidate_index: usize,
        pin: bool,
        expected_sequence: u64,
    ) -> Result<Decision> {
        if key.model.is_empty()
            || key.config_sha256.is_empty()
            || handoff.is_empty()
            || candidate_identity.is_empty()
        {
            return Err(IndexError::Conflict("empty decision identity"));
        }
        let _lock = locked(&self.base().join("route.lock"))?;
        self.check_generation()?;
        self.reconcile_pending()?;
        if let Some(existing) = self.read_decision(&handoff)? {
            if !self.known("decisions", &handoff)? {
                return Err(corrupt("decision known-key marker absent"));
            }
            if existing.key != key
                || existing.candidate_identity != candidate_identity
                || existing.candidate_index != candidate_index
                || existing.pin != pin
            {
                return Err(IndexError::Conflict(
                    "handoff already has different decision",
                ));
            }
            return Ok(existing);
        }
        let current = self.cursor_unlocked(&key)?;
        if current.sequence != expected_sequence {
            return Err(IndexError::Conflict("cursor CAS mismatch"));
        }
        let decision = Decision {
            generation: self.generation.clone(),
            handoff,
            key,
            candidate_identity,
            candidate_index,
            pin,
            sequence: if pin {
                0
            } else {
                current
                    .sequence
                    .checked_add(1)
                    .ok_or(IndexError::Conflict("sequence overflow"))?
            },
            receipt: None,
        };
        let _next = self.advanced(&current, &decision)?;
        let pending_path = self.base().join("pending.json");
        write_new(
            &pending_path,
            &Pending {
                generation: self.generation.clone(),
                expected: current,
                decision: decision.clone(),
                decision_sha256: keyed(&decision)?,
            },
        )?;
        write_new(&self.decision_path(&decision.handoff)?, &decision)?;
        self.reconcile_pending()?;
        Ok(decision)
    }
    /// Publish one externally visible broker route receipt and its cursor as
    /// a single recoverable choice. The caller holds the broker's selection
    /// lock, computes `receipt_bytes` from its fully selected RouteDecision,
    /// and writes exactly those bytes in `publish`. Neither layer chooses a
    /// second candidate. Readback of a pending receipt publishes the matching
    /// index decision once, including after a process restart.
    pub(super) fn commit_live_decision(
        &self,
        key: CursorKey,
        handoff: String,
        candidate_identity: String,
        candidate_index: usize,
        pin: bool,
        expected_sequence: u64,
        receipt_path: String,
        receipt_bytes: &[u8],
        publish: impl FnOnce() -> io::Result<()>,
    ) -> Result<Decision> {
        if key.model.is_empty()
            || key.config_sha256.is_empty()
            || handoff.is_empty()
            || candidate_identity.is_empty()
        {
            return Err(IndexError::Conflict("empty decision identity"));
        }
        let receipt = Artifact {
            path: receipt_path,
            sha256: hash(receipt_bytes),
        };
        receipt.validate()?;
        let _lock = locked(&self.base().join("route.lock"))?;
        self.check_generation()?;
        self.reconcile_pending()?;
        if let Some(existing) = self.read_decision(&handoff)? {
            if !self.known("decisions", &handoff)? {
                return Err(corrupt("decision known-key marker absent"));
            }
            if existing.key != key
                || existing.candidate_identity != candidate_identity
                || existing.candidate_index != candidate_index
                || existing.pin != pin
                || existing.receipt.as_ref() != Some(&receipt)
            {
                return Err(IndexError::Conflict(
                    "handoff already has different decision",
                ));
            }
            return Ok(existing);
        }
        if receipt.verify(&self.root)? {
            return Err(IndexError::RebuildRequired(
                "broker route receipt exists without indexed decision",
            ));
        }
        let current = self.cursor_unlocked(&key)?;
        if current.sequence != expected_sequence {
            return Err(IndexError::Conflict("cursor CAS mismatch"));
        }
        let decision = Decision {
            generation: self.generation.clone(),
            handoff,
            key,
            candidate_identity,
            candidate_index,
            pin,
            sequence: if pin {
                0
            } else {
                current
                    .sequence
                    .checked_add(1)
                    .ok_or(IndexError::Conflict("sequence overflow"))?
            },
            receipt: Some(receipt),
        };
        self.advanced(&current, &decision)?;
        write_new(
            &self.base().join("pending.json"),
            &Pending {
                generation: self.generation.clone(),
                expected: current,
                decision: decision.clone(),
                decision_sha256: keyed(&decision)?,
            },
        )?;
        publish()?;
        self.reconcile_pending()?;
        Ok(decision)
    }
    pub(super) fn decision(&self, handoff: &str) -> Result<Option<Decision>> {
        let _lock = locked(&self.base().join("route.lock"))?;
        self.check_generation()?;
        self.reconcile_pending()?;
        let decision = self.read_decision(handoff)?;
        if decision.is_some() && !self.known("decisions", &handoff.to_owned())? {
            return Err(corrupt("decision known-key marker absent"));
        }
        Ok(decision)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Artifact {
    pub path: String,
    pub sha256: String,
}
impl Artifact {
    pub(super) fn from_existing(root: &Path, relative: &Path) -> Result<Self> {
        let path = relative
            .to_str()
            .ok_or_else(|| corrupt("non-UTF8 offline artifact path"))?
            .to_owned();
        let mut f = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(root.join(relative))?;
        if !f.metadata()?.is_file() {
            return Err(corrupt("offline artifact not regular"));
        }
        let mut hasher = Sha256::new();
        io::copy(&mut f, &mut hasher)?;
        let artifact = Self {
            path,
            sha256: format!("{:x}", hasher.finalize()),
        };
        artifact.require_present(root)?;
        Ok(artifact)
    }
    fn verify(&self, root: &Path) -> Result<bool> {
        let path = Path::new(&self.path);
        if path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
            || self.path.is_empty()
            || self.sha256.len() != 64
            || !self.sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(corrupt("invalid artifact reference"));
        }
        reader_open_attempt();
        let mut f = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(root.join(path))
        {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e.into()),
        };
        reader_opened();
        let before = f.metadata()?;
        if !before.is_file() {
            return Err(corrupt("artifact is not regular"));
        }
        let mut hasher = Sha256::new();
        io::copy(&mut f, &mut hasher)?;
        let after = f.metadata()?;
        if before.dev() != after.dev() || before.ino() != after.ino() || before.len() != after.len()
        {
            return Err(corrupt("artifact changed during readback"));
        }
        if format!("{:x}", hasher.finalize()) != self.sha256 {
            return Err(corrupt("artifact digest mismatch"));
        }
        Ok(true)
    }
    fn read_json<T: DeserializeOwned>(&self, root: &Path) -> Result<T> {
        self.validate()?;
        let path = root.join(&self.path);
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)?;
        let before = file.metadata()?;
        if !before.is_file() || before.len() > MAX_RECORD {
            return Err(corrupt("typed artifact invalid or oversize"));
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let after = file.metadata()?;
        if (before.dev(), before.ino(), before.len()) != (after.dev(), after.ino(), after.len())
            || hash(&bytes) != self.sha256
        {
            return Err(corrupt("typed artifact digest changed"));
        }
        serde_json::from_slice(&bytes).map_err(|e| corrupt(format!("typed artifact: {e}")))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ProviderGrant {
    pub decision_handoff: String,
    pub grant: Artifact,
    #[serde(default)]
    pub candidate: Option<Artifact>,
    pub consumed_k: Option<Artifact>,
    #[serde(default)]
    pub certified_q: Option<PhysicalQ>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum EffectKind {
    Quota,
    Auth,
    ManualQuota,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub(super) struct SourceKey {
    pub commands_sha256: String,
    pub environment_sha256: String,
}
impl SourceKey {
    fn valid(&self) -> bool {
        !self.commands_sha256.is_empty() && !self.environment_sha256.is_empty()
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct EffectIntent {
    pub kind: EffectKind,
    pub source: SourceKey,
    #[serde(default)]
    pub decision_handoff: String,
    #[serde(default)]
    pub route_source: Option<Artifact>,
    #[serde(default)]
    pub candidate: Option<Artifact>,
    pub intent: Artifact,
    #[serde(default)]
    pub reuse: Option<Artifact>,
    pub consumed_k: Option<Artifact>,
    #[serde(default)]
    pub certified_q: Option<PhysicalQ>,
    #[serde(default)]
    pub result: Option<Artifact>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct PhysicalQ {
    pub physical_k: Artifact,
    pub q: Artifact,
    pub terminal: Option<Artifact>,
    pub completed_unix_nanos: i64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct MarkerTimes {
    pub quota_rejection_nanos: Option<i64>,
    pub auth_rejection_nanos: Option<i64>,
    pub model_capacity_nanos: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct SourceQ {
    pub source: SourceKey,
    pub latest_quota_q: Option<PhysicalQ>,
    pub latest_auth_q: Option<PhysicalQ>,
}
/// Small, versioned read projection. The account audit record remains the
/// writer's exact retained history; only this record is opened by a compact
/// account read. Cardinality limits turn unusual fan-out into a refusal.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AccountHead {
    pub generation: String,
    pub physical_key: String,
    pub revision: u64,
    pub committed: bool,
    pub pending: BTreeMap<String, PendingHead>,
    pub sources: BTreeMap<String, SourceHead>,
    pub quota_rejection: Option<MarkerHead>,
    pub auth_rejection: Option<MarkerHead>,
    pub model_capacity: BTreeMap<String, MarkerHead>,
    pub marker_times: MarkerTimes,
    /// Old or malformed marker evidence cannot be assigned a safe scope.
    pub unknown_marker_scope: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PendingHead {
    pub announcement: Artifact,
    pub candidate: Option<Artifact>,
    pub physical_k: Option<Artifact>,
    pub source: Option<SourceKey>,
    pub model: Option<String>,
    pub config_sha256: Option<String>,
    pub decision_handoff: String,
    pub kind: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SourceHead {
    pub source: SourceKey,
    pub quota: Option<ObservationHead>,
    pub auth: Option<ObservationHead>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ObservationHead {
    pub q: PhysicalQ,
    pub result: Option<Artifact>,
    pub outcome: String,
    pub origin_model: Option<String>,
    pub origin_config_sha256: Option<String>,
    pub completed_unix_seconds: Option<i64>,
    pub windows: Vec<WindowHead>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WindowHead {
    pub used_percent: f64,
    pub resets_at: String,
    pub reset_unix_seconds: i64,
    pub remaining: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MarkerHead {
    pub q: PhysicalQ,
    pub model: String,
    pub config_sha256: String,
    pub outcome: String,
}
impl ObservationHead {
    /// Typed cache calculation only. This is not a route eligibility verdict.
    pub(super) fn quota_basis_points_at(&self, now: i64) -> Option<u32> {
        if self.outcome != "valid_windows" || self.windows.is_empty() {
            return None;
        }
        let completed = self.completed_unix_seconds?;
        if now < completed || now.checked_sub(completed)? >= 5 * 60 * 60 {
            return None;
        }
        let mut binding = u32::MAX;
        for window in &self.windows {
            if window.reset_unix_seconds <= now
                || window.used_percent >= 100.0
                || window.remaining == Some(0)
            {
                return None;
            }
            binding = binding.min(((100.0 - window.used_percent) * 100.0).round() as u32);
        }
        Some(binding)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Account {
    pub generation: String,
    pub physical_key: String,
    pub revision: u64,
    pub grants: BTreeMap<String, ProviderGrant>,
    pub effects: BTreeMap<String, EffectIntent>,
    pub observed_invocations: u64,
    pub markers: MarkerTimes,
    pub source_q: BTreeMap<String, SourceQ>,
    pub recent_failure_nanos: Vec<i64>,
}
impl AccountHead {
    fn from_account(root: &Path, account: &Account) -> Result<Self> {
        let mut head = Self {
            generation: account.generation.clone(),
            physical_key: account.physical_key.clone(),
            revision: account.revision,
            committed: true,
            pending: BTreeMap::new(),
            sources: BTreeMap::new(),
            quota_rejection: None,
            auth_rejection: None,
            model_capacity: BTreeMap::new(),
            marker_times: account.markers.clone(),
            unknown_marker_scope: false,
        };
        for (id, grant) in &account.grants {
            if let Some(q) = &grant.certified_q {
                let Some(terminal) = &q.terminal else {
                    return Err(corrupt("settled provider missing terminal"));
                };
                let record: serde_json::Value = match terminal.read_json(root) {
                    Ok(record) => record,
                    // Old opaque records can remain in the audit, but cannot
                    // authorize a scoped marker in the compact projection.
                    Err(_) => {
                        head.unknown_marker_scope = true;
                        continue;
                    }
                };
                let selection = &record["selection"];
                let model = selection["model"].as_str();
                let config = selection["config_sha256"].as_str();
                let physical = selection["account_identity"].as_str();
                let outcome = record["outcome"].as_str();
                if record["grant_id"].as_str() != Some(id)
                    || record["physical_q_sha256"].as_str() != Some(&q.q.sha256)
                    || record["physical_q_unix_nanos"].as_u64()
                        != u64::try_from(q.completed_unix_nanos).ok()
                    || physical != Some(account.physical_key.as_str())
                    || model.is_none()
                    || config.is_none()
                    || outcome.is_none()
                {
                    head.unknown_marker_scope = true;
                    continue;
                }
                let marker = MarkerHead {
                    q: q.clone(),
                    model: model.unwrap().to_owned(),
                    config_sha256: config.unwrap().to_owned(),
                    outcome: outcome.unwrap().to_owned(),
                };
                match marker.outcome.as_str() {
                    "quota_rejected" => replace_newer_marker(&mut head.quota_rejection, marker),
                    "auth_rejected" => replace_newer_marker(&mut head.auth_rejection, marker),
                    "model_at_capacity" => {
                        let key = keyed(&(&marker.model, &marker.config_sha256))?;
                        match head.model_capacity.entry(key) {
                            std::collections::btree_map::Entry::Vacant(entry) => {
                                entry.insert(marker);
                            }
                            std::collections::btree_map::Entry::Occupied(mut entry) => {
                                if entry.get().q.completed_unix_nanos
                                    <= marker.q.completed_unix_nanos
                                {
                                    entry.insert(marker);
                                }
                            }
                        }
                    }
                    "clean"
                    | "generic_failure"
                    | "cancelled"
                    | "unknown"
                    | "maybe_quota"
                    | "provider_unavailable"
                    | "rate_limited"
                    | "storage_contention" => {}
                    _ => head.unknown_marker_scope = true,
                }
            } else {
                let (model, config_sha256) = grant
                    .candidate
                    .as_ref()
                    .map(|artifact| artifact_scope(root, artifact))
                    .unwrap_or((None, None));
                head.pending.insert(
                    format!("grant:{id}"),
                    PendingHead {
                        announcement: grant.grant.clone(),
                        candidate: grant.candidate.clone(),
                        physical_k: grant.consumed_k.clone(),
                        source: None,
                        model,
                        config_sha256,
                        decision_handoff: grant.decision_handoff.clone(),
                        kind: "provider".into(),
                    },
                );
            }
        }
        for (id, effect) in &account.effects {
            if effect.certified_q.is_none() || effect.result.is_none() {
                if effect.reuse.is_none() && physical_effect(root, effect)? {
                    let (model, config_sha256) = if matches!(effect.kind, EffectKind::ManualQuota) {
                        let value = effect.intent.read_json::<serde_json::Value>(root).ok();
                        (
                            value
                                .as_ref()
                                .and_then(|v| v["request"]["model"].as_str())
                                .map(str::to_owned),
                            value
                                .as_ref()
                                .and_then(|v| v["request"]["config_sha256"].as_str())
                                .map(str::to_owned),
                        )
                    } else {
                        effect
                            .candidate
                            .as_ref()
                            .map(|artifact| artifact_scope(root, artifact))
                            .unwrap_or((None, None))
                    };
                    head.pending.insert(
                        format!("effect:{id}"),
                        PendingHead {
                            announcement: effect.intent.clone(),
                            candidate: effect.candidate.clone(),
                            physical_k: effect.consumed_k.clone(),
                            source: Some(effect.source.clone()),
                            model,
                            config_sha256,
                            decision_handoff: effect.decision_handoff.clone(),
                            kind: format!("{:?}", effect.kind),
                        },
                    );
                }
            }
            let Some(q) = &effect.certified_q else {
                continue;
            };
            let observation = typed_observation(root, &account.physical_key, id, effect, q)?;
            let digest = keyed(&effect.source)?;
            let source = head.sources.entry(digest).or_insert_with(|| SourceHead {
                source: effect.source.clone(),
                quota: None,
                auth: None,
            });
            if source.source != effect.source {
                return Err(corrupt("compact source collision"));
            }
            let target = match effect.kind {
                EffectKind::Quota | EffectKind::ManualQuota => &mut source.quota,
                EffectKind::Auth => &mut source.auth,
            };
            match target {
                Some(old)
                    if old.q.completed_unix_nanos == q.completed_unix_nanos && old.q != *q =>
                {
                    old.outcome = "unknown".into();
                    old.windows.clear();
                }
                Some(old) if old.q.completed_unix_nanos > q.completed_unix_nanos => {}
                _ => *target = Some(observation),
            }
        }
        if account.markers.quota_rejection_nanos
            != head
                .quota_rejection
                .as_ref()
                .map(|m| m.q.completed_unix_nanos)
            || account.markers.auth_rejection_nanos
                != head
                    .auth_rejection
                    .as_ref()
                    .map(|m| m.q.completed_unix_nanos)
            || account.markers.model_capacity_nanos
                != head
                    .model_capacity
                    .values()
                    .map(|m| m.q.completed_unix_nanos)
                    .max()
        {
            head.unknown_marker_scope = true;
        }
        if head.pending.len() > MAX_HEAD_PENDING
            || head.sources.len() > MAX_HEAD_SOURCES
            || head.model_capacity.len() > MAX_HEAD_SOURCES
        {
            return Err(IndexError::Conflict(
                "compact account head cardinality exceeded",
            ));
        }
        Ok(head)
    }
    fn validate_current(&self, root: &Path, generation: &str, key: &str) -> Result<()> {
        if !self.committed
            || self.generation != generation
            || self.physical_key != key
            || self.revision == 0
            || self.pending.len() > MAX_HEAD_PENDING
            || self.sources.len() > MAX_HEAD_SOURCES
            || self.model_capacity.len() > MAX_HEAD_SOURCES
        {
            return Err(IndexError::RebuildRequired(
                "compact account head incomplete",
            ));
        }
        for pending in self.pending.values() {
            pending.announcement.validate()?;
            if let Some(candidate) = &pending.candidate {
                candidate.require_present(root)?;
            }
            if let Some(k) = &pending.physical_k {
                pending.announcement.require_present(root)?;
                k.require_present(root)?;
            }
        }
        for (digest, source) in &self.sources {
            if !source.source.valid() || keyed(&source.source)? != *digest {
                return Err(corrupt("compact source key mismatch"));
            }
            for observation in [&source.quota, &source.auth].into_iter().flatten() {
                observation.q.verify(root)?;
                if let Some(result) = &observation.result {
                    result.require_present(root)?;
                }
                if observation.windows.len() > 64
                    || observation.windows.iter().any(|w| {
                        !w.used_percent.is_finite()
                            || !(0.0..=100.0).contains(&w.used_percent)
                            || DateTime::parse_from_rfc3339(&w.resets_at)
                                .map_or(true, |d| d.timestamp() != w.reset_unix_seconds)
                    })
                {
                    return Err(corrupt("compact typed window invalid"));
                }
            }
        }
        for marker in [&self.quota_rejection, &self.auth_rejection]
            .into_iter()
            .flatten()
        {
            marker.q.verify(root)?;
        }
        for marker in self.model_capacity.values() {
            marker.q.verify(root)?;
        }
        Ok(())
    }
}
fn replace_newer_marker(target: &mut Option<MarkerHead>, marker: MarkerHead) {
    if target
        .as_ref()
        .is_none_or(|old| old.q.completed_unix_nanos <= marker.q.completed_unix_nanos)
    {
        *target = Some(marker);
    }
}
fn artifact_scope(root: &Path, artifact: &Artifact) -> (Option<String>, Option<String>) {
    let Ok(value) = artifact.read_json::<serde_json::Value>(root) else {
        return (None, None);
    };
    (
        value["model"].as_str().map(str::to_owned),
        value["config_sha256"].as_str().map(str::to_owned),
    )
}
fn physical_effect(root: &Path, effect: &EffectIntent) -> Result<bool> {
    if !matches!(effect.kind, EffectKind::ManualQuota) {
        return Ok(true);
    }
    let value: serde_json::Value = match effect.intent.read_json(root) {
        Ok(value) => value,
        Err(_) => return Ok(true), // unknown intent is debt
    };
    if !value.is_object()
        || value.get("source_operation_id").is_none()
        || value.get("quota_script").is_none()
    {
        return Ok(true);
    }
    Ok(value["source_operation_id"].is_null() && !value["quota_script"].is_null())
}
fn typed_observation(
    root: &Path,
    physical_key: &str,
    id: &str,
    effect: &EffectIntent,
    q: &PhysicalQ,
) -> Result<ObservationHead> {
    let mut observation = ObservationHead {
        q: q.clone(),
        result: effect.result.clone(),
        outcome: "unknown".into(),
        origin_model: None,
        origin_config_sha256: None,
        completed_unix_seconds: None,
        windows: Vec::new(),
    };
    if matches!(effect.kind, EffectKind::ManualQuota) {
        if let Ok(value) = effect.intent.read_json::<serde_json::Value>(root) {
            observation.origin_model = value["request"]["model"].as_str().map(str::to_owned);
            observation.origin_config_sha256 = value["request"]["config_sha256"]
                .as_str()
                .map(str::to_owned);
        }
    } else if let Some(candidate) = &effect.candidate {
        if let Ok(value) = candidate.read_json::<serde_json::Value>(root) {
            observation.origin_model = value["model"].as_str().map(str::to_owned);
            observation.origin_config_sha256 = value["config_sha256"].as_str().map(str::to_owned);
        }
    }
    let Some(result) = &effect.result else {
        return Ok(observation);
    };
    let (state, outcome, completed, windows) = if matches!(effect.kind, EffectKind::ManualQuota) {
        let readback = super::manual_quota::indexed_physical_readback(root, id)?;
        if readback.operation_id != id || readback.physical_account_id != physical_key {
            return Err(corrupt("manual compact Q identity changed"));
        }
        (
            readback.state,
            readback.outcome,
            readback.completed_unix_seconds,
            readback.windows,
        )
    } else {
        let readback: oulipoly_kernel_broker::protocol::FreshAccountEffectReadback =
            match result.read_json(root) {
                Ok(value) => value,
                Err(_) => return Ok(observation),
            };
        if readback.effect_id != id {
            return Err(corrupt("effect compact Q identity changed"));
        }
        (
            readback.state,
            readback.outcome,
            readback.completed_unix_seconds,
            readback.windows,
        )
    };
    if state != "drained" {
        return Ok(observation);
    }
    observation.outcome = outcome.unwrap_or_else(|| "unknown".into());
    observation.completed_unix_seconds = completed;
    if observation.outcome == "valid_windows" {
        if windows.is_empty() || windows.len() > 64 {
            observation.outcome = "invalid".into();
            return Ok(observation);
        }
        for window in windows {
            let Ok(reset) = DateTime::parse_from_rfc3339(&window.resets_at) else {
                observation.outcome = "invalid".into();
                observation.windows.clear();
                return Ok(observation);
            };
            if !window.used_percent.is_finite() || !(0.0..=100.0).contains(&window.used_percent) {
                observation.outcome = "invalid".into();
                observation.windows.clear();
                return Ok(observation);
            }
            observation.windows.push(WindowHead {
                used_percent: window.used_percent,
                resets_at: window.resets_at,
                reset_unix_seconds: reset.timestamp(),
                remaining: window.remaining,
            });
        }
    }
    Ok(observation)
}
impl Index {
    fn account_unlocked(&self, key: &str) -> Result<Account> {
        let record: Option<Account> = read(&self.key_path("accounts", &key)?)?;
        if record.is_some() != self.known("accounts", &key.to_owned())? {
            return Err(corrupt("account record or known-key marker absent"));
        }
        let a = record.unwrap_or(Account {
            generation: self.generation.clone(),
            physical_key: key.into(),
            revision: 0,
            grants: BTreeMap::new(),
            effects: BTreeMap::new(),
            observed_invocations: 0,
            markers: MarkerTimes::default(),
            source_q: BTreeMap::new(),
            recent_failure_nanos: Vec::new(),
        });
        if a.generation != self.generation
            || a.physical_key != key
            || a.recent_failure_nanos.len() > MAX_RECENT_FAILURES
            || a.effects.values().any(|effect| !effect.source.valid())
            || a.source_q.iter().any(|(digest, value)| {
                !value.source.valid() || !keyed(&value.source).is_ok_and(|hash| hash == *digest)
            })
        {
            return Err(corrupt("account key, generation or failure bound"));
        }
        Ok(a)
    }
    pub(super) fn account(&self, key: &str) -> Result<Account> {
        let _lock = locked(&self.key_path("accounts", &format!("lock:{key}"))?)?;
        self.check_generation()?;
        let account = self.account_unlocked(key)?;
        // A missing prepared artifact is still announced debt. Once K is
        // recorded, grant/intent and K must read back exactly. No Q is inferred
        // from a filename; Q/terminal pointers here are externally certified.
        for grant in account.grants.values() {
            grant.grant.validate()?;
            if let Some(candidate) = &grant.candidate {
                candidate.require_present(&self.root)?;
            }
            if let Some(k) = &grant.consumed_k {
                grant.grant.require_present(&self.root)?;
                k.require_present(&self.root)?;
            } else {
                grant.grant.verify(&self.root)?;
            }
            if let Some(q) = &grant.certified_q {
                if grant.consumed_k.as_ref() != Some(&q.physical_k) || q.terminal.is_none() {
                    return Err(corrupt("certified provider Q/K or terminal mismatch"));
                }
                q.verify(&self.root)?;
            }
        }
        for effect in account.effects.values() {
            effect.intent.validate()?;
            if let Some(source) = &effect.route_source {
                source.require_present(&self.root)?;
            }
            if let Some(candidate) = &effect.candidate {
                candidate.require_present(&self.root)?;
            }
            if let Some(reference) = &effect.reuse {
                reference.require_present(&self.root)?;
            }
            if let Some(k) = &effect.consumed_k {
                effect.intent.require_present(&self.root)?;
                k.require_present(&self.root)?;
            } else {
                effect.intent.verify(&self.root)?;
            }
            if let Some(q) = &effect.certified_q {
                if effect.consumed_k.as_ref() != Some(&q.physical_k) || q.terminal.is_some() {
                    return Err(corrupt("certified effect Q/K mismatch"));
                }
                q.verify(&self.root)?;
            }
            if let Some(result) = &effect.result {
                if effect.certified_q.is_none() {
                    return Err(corrupt("effect result lacks Q"));
                }
                result.require_present(&self.root)?;
            }
        }
        for source in account.source_q.values() {
            if let Some(q) = &source.latest_quota_q {
                q.verify(&self.root)?;
            }
            if let Some(q) = &source.latest_auth_q {
                q.verify(&self.root)?;
            }
        }
        Ok(account)
    }
    pub(super) fn live_account_keys(&self) -> Result<Vec<String>> {
        self.check_generation()?;
        let mut keys = Vec::new();
        for entry in fs::read_dir(self.base().join("accounts"))? {
            let path = entry?.path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| corrupt("account entry name"))?;
            if !name.ends_with(".known.json") {
                continue;
            }
            let marker: KnownKey<String> =
                read(&path)?.ok_or_else(|| corrupt("account marker absent"))?;
            if marker.generation != self.generation
                || path != self.known_path("accounts", &marker.key)?
            {
                return Err(corrupt("account marker key changed"));
            }
            self.account(&marker.key)?;
            keys.push(marker.key);
        }
        let records = keys
            .iter()
            .map(|key| self.key_path("accounts", key))
            .collect::<Result<std::collections::HashSet<_>>>()?;
        for entry in fs::read_dir(self.base().join("accounts"))? {
            let path = entry?.path();
            if path.extension().is_some_and(|ext| ext == "json")
                && !path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.ends_with(".known.json"))
                && path.metadata()?.len() != 0
                && !records.contains(&path)
            {
                return Err(corrupt("unmarked account record"));
            }
        }
        Ok(keys)
    }
    /// One exact CAS. No silent field overwrite; only the defined state
    /// transitions below may be submitted. The caller retains the previous
    /// revision and must retry from readback after a conflict.
    pub(super) fn update_account(
        &self,
        key: &str,
        expected_revision: u64,
        update: AccountUpdate,
    ) -> Result<Account> {
        if key.is_empty() {
            return Err(IndexError::Conflict("empty physical account key"));
        }
        let _lock = locked(&self.key_path("accounts", &format!("lock:{key}"))?)?;
        self.check_generation()?;
        let mut a = self.account_unlocked(key)?;
        let previous_head: Option<AccountHead> = read(&self.key_path("heads", &key)?)?;
        if a.revision == 0 {
            if previous_head.is_some() {
                return Err(corrupt("compact head precedes account"));
            }
        } else if previous_head.as_ref().is_none_or(|h| {
            !h.committed
                || h.generation != self.generation
                || h.physical_key != key
                || h.revision != a.revision
        }) {
            return Err(IndexError::RebuildRequired(
                "compact head/account CAS incomplete",
            ));
        }
        if a.revision != expected_revision {
            return Err(IndexError::Conflict("account CAS mismatch"));
        }
        match update {
            AccountUpdate::AnnounceGrant { id, grant } => {
                if id.is_empty() || grant.decision_handoff.is_empty() || a.grants.contains_key(&id)
                {
                    return Err(IndexError::Conflict(
                        "grant identity already announced or empty",
                    ));
                }
                if grant.consumed_k.is_some() || grant.certified_q.is_some() {
                    return Err(IndexError::Conflict("announcement includes K"));
                }
                grant.grant.validate()?;
                a.grants.insert(id, grant);
            }
            AccountUpdate::ConsumeGrant { id, k } => {
                k.require_present(&self.root)?;
                let grant = a
                    .grants
                    .get_mut(&id)
                    .ok_or(IndexError::Conflict("grant absent"))?;
                grant.grant.require_present(&self.root)?;
                if grant.consumed_k.is_some() {
                    return Err(IndexError::Conflict("grant K already recorded"));
                }
                grant.consumed_k = Some(k);
                a.observed_invocations = a
                    .observed_invocations
                    .checked_add(1)
                    .ok_or(IndexError::Conflict("invocation overflow"))?;
            }
            AccountUpdate::SettleGrant {
                id,
                q,
                failed,
                marker,
            } => {
                let grant = a
                    .grants
                    .get_mut(&id)
                    .ok_or(IndexError::Conflict("grant absent"))?;
                if grant.certified_q.is_some() {
                    return Err(IndexError::Conflict("provider Q already certified"));
                }
                let k = grant
                    .consumed_k
                    .as_ref()
                    .ok_or(IndexError::Conflict("grant K absent"))?;
                if &q.physical_k != k || q.terminal.is_none() {
                    return Err(IndexError::Conflict("provider Q/K or terminal mismatch"));
                }
                q.verify(&self.root)?;
                // Keep the exact settled identity for admission readback. A
                // physical K with no indexed ID must never resemble a grant
                // whose Q was already projected. The record bound fails closed
                // before a later K if retained history grows too large.
                grant.certified_q = Some(q.clone());
                if let Some(kind) = marker {
                    let target = match kind {
                        TerminalMarkerKind::Quota => &mut a.markers.quota_rejection_nanos,
                        TerminalMarkerKind::Auth => &mut a.markers.auth_rejection_nanos,
                        TerminalMarkerKind::ModelCapacity => &mut a.markers.model_capacity_nanos,
                    };
                    *target = Some(target.unwrap_or(i64::MIN).max(q.completed_unix_nanos));
                }
                if failed {
                    a.add_failure(q.completed_unix_nanos)?;
                }
            }
            AccountUpdate::AnnounceEffect { id, effect } => {
                if id.is_empty()
                    || a.effects.contains_key(&id)
                    || effect.consumed_k.is_some()
                    || effect.certified_q.is_some()
                    || effect.result.is_some()
                {
                    return Err(IndexError::Conflict(
                        "effect identity already announced or includes K",
                    ));
                }
                effect.intent.validate()?;
                if let Some(source) = &effect.route_source {
                    source.require_present(&self.root)?;
                }
                if let Some(candidate) = &effect.candidate {
                    candidate.require_present(&self.root)?;
                }
                if let Some(reference) = &effect.reuse {
                    reference.validate()?;
                }
                if !effect.source.valid() {
                    return Err(IndexError::Conflict("effect source identity absent"));
                }
                a.effects.insert(id, effect);
            }
            AccountUpdate::ConsumeEffect { id, k } => {
                k.require_present(&self.root)?;
                let effect = a
                    .effects
                    .get_mut(&id)
                    .ok_or(IndexError::Conflict("effect absent"))?;
                effect.intent.require_present(&self.root)?;
                if effect.consumed_k.is_some() {
                    return Err(IndexError::Conflict("effect K already recorded"));
                }
                effect.consumed_k = Some(k);
            }
            AccountUpdate::SettleEffect {
                id,
                q,
                result,
                marker,
            } => {
                let effect = a
                    .effects
                    .get(&id)
                    .ok_or(IndexError::Conflict("effect absent"))?;
                if result.is_some() && (effect.certified_q.is_some() || effect.result.is_some()) {
                    return Err(IndexError::Conflict("effect Q/result already certified"));
                }
                let k = effect
                    .consumed_k
                    .as_ref()
                    .ok_or(IndexError::Conflict("effect K absent"))?;
                if &q.physical_k != k || q.terminal.is_some() {
                    return Err(IndexError::Conflict("effect Q/K mismatch"));
                }
                q.verify(&self.root)?;
                if let Some(result) = &result {
                    result.require_present(&self.root)?;
                }
                let kind = effect.kind.clone();
                let source = effect.source.clone();
                if let Some(result) = result {
                    let effect = a.effects.get_mut(&id).unwrap();
                    effect.certified_q = Some(q.clone());
                    effect.result = Some(result);
                } else {
                    // Unknown typed result is retained as the newest Q and
                    // remains debt. It cannot expose an older healthy source.
                    a.effects.get_mut(&id).unwrap().certified_q = Some(q.clone());
                }
                if marker == Some(true) {
                    a.mark(&kind, q.completed_unix_nanos);
                } else if marker == Some(false) {
                    let digest = keyed(&source)?;
                    let projection = a.source_q.entry(digest).or_insert_with(|| SourceQ {
                        source: source.clone(),
                        latest_quota_q: None,
                        latest_auth_q: None,
                    });
                    if projection.source != source {
                        return Err(corrupt("source-key collision"));
                    }
                    let target = match kind {
                        EffectKind::Quota | EffectKind::ManualQuota => {
                            &mut projection.latest_quota_q
                        }
                        EffectKind::Auth => &mut projection.latest_auth_q,
                    };
                    if target
                        .as_ref()
                        .is_none_or(|old| old.completed_unix_nanos <= q.completed_unix_nanos)
                    {
                        *target = Some(q);
                    }
                }
            }
            AccountUpdate::PruneFailures { before_nanos } => {
                a.recent_failure_nanos.retain(|time| *time >= before_nanos);
            }
        }
        a.revision = a
            .revision
            .checked_add(1)
            .ok_or(IndexError::Conflict("account revision overflow"))?;
        self.check_generation()?;
        let mut head = AccountHead::from_account(&self.root, &a)?;
        head.committed = false;
        write_atomic(&self.key_path("heads", &key)?, &head)?;
        self.mark_known("accounts", &key.to_owned())?;
        write_atomic(&self.key_path("accounts", &key)?, &a)?;
        head.committed = true;
        write_atomic(&self.key_path("heads", &key)?, &head)?;
        Ok(a)
    }
}
impl Artifact {
    fn validate(&self) -> Result<()> {
        let p = Path::new(&self.path);
        if self.path.is_empty()
            || p.components().any(|c| !matches!(c, Component::Normal(_)))
            || self.sha256.len() != 64
            || !self.sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(corrupt("invalid artifact reference"));
        }
        Ok(())
    }
    fn require_present(&self, root: &Path) -> Result<()> {
        if !self.verify(root)? {
            return Err(IndexError::Conflict("exact artifact not durable"));
        }
        Ok(())
    }
}
impl PhysicalQ {
    fn verify(&self, root: &Path) -> Result<()> {
        self.physical_k.require_present(root)?;
        self.q.require_present(root)?;
        if let Some(t) = &self.terminal {
            t.require_present(root)?;
        }
        Ok(())
    }
}
impl Account {
    fn add_failure(&mut self, nanos: i64) -> Result<()> {
        if self.recent_failure_nanos.len() == MAX_RECENT_FAILURES {
            return Err(IndexError::Conflict(
                "recent failure bound reached; eligibility unknown",
            ));
        }
        self.recent_failure_nanos.push(nanos);
        Ok(())
    }
    fn mark(&mut self, kind: &EffectKind, nanos: i64) {
        let target = match kind {
            EffectKind::Quota | EffectKind::ManualQuota => &mut self.markers.quota_rejection_nanos,
            EffectKind::Auth => &mut self.markers.auth_rejection_nanos,
        };
        *target = Some(target.unwrap_or(i64::MIN).max(nanos));
    }
}
#[derive(Clone, Debug)]
pub(super) enum AccountUpdate {
    AnnounceGrant {
        id: String,
        grant: ProviderGrant,
    },
    ConsumeGrant {
        id: String,
        k: Artifact,
    },
    SettleGrant {
        id: String,
        q: PhysicalQ,
        failed: bool,
        marker: Option<TerminalMarkerKind>,
    },
    AnnounceEffect {
        id: String,
        effect: EffectIntent,
    },
    ConsumeEffect {
        id: String,
        k: Artifact,
    },
    SettleEffect {
        id: String,
        q: PhysicalQ,
        result: Option<Artifact>,
        /// Some(true): typed rejection; Some(false): healthy source Q.
        /// None: exact known result without quota or auth authority.
        marker: Option<bool>,
    },
    PruneFailures {
        before_nanos: i64,
    },
}
#[derive(Clone, Debug)]
pub(super) enum TerminalMarkerKind {
    Quota,
    Auth,
    ModelCapacity,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fresh() -> (tempfile::TempDir, Index) {
        let temp = tempfile::tempdir().unwrap();
        let index = Index::create_fresh_genesis(
            temp.path(),
            GenesisAuthority::ConfirmedFreshEmptyDirectory,
        )
        .unwrap();
        (temp, index)
    }
    fn key() -> CursorKey {
        CursorKey {
            model: "m".into(),
            config_sha256: "config".into(),
        }
    }
    fn decision(index: &Index, handoff: &str, sequence: u64, pin: bool) -> Decision {
        Decision {
            generation: index.generation.clone(),
            handoff: handoff.into(),
            key: key(),
            candidate_identity: "physical-a".into(),
            candidate_index: 2,
            pin,
            sequence,
            receipt: None,
        }
    }
    fn pending(index: &Index, d: Decision) -> Pending {
        Pending {
            generation: index.generation.clone(),
            expected: index.cursor_unlocked(&d.key).unwrap(),
            decision_sha256: keyed(&d).unwrap(),
            decision: d,
        }
    }
    fn artifact(root: &Path, path: &str, bytes: &[u8]) -> Artifact {
        fs::write(root.join(path), bytes).unwrap();
        Artifact {
            path: path.into(),
            sha256: hash(bytes),
        }
    }
    fn future_artifact(path: &str, bytes: &[u8]) -> Artifact {
        Artifact {
            path: path.into(),
            sha256: hash(bytes),
        }
    }
    #[test]
    fn explicit_genesis_and_preindex_repair_required() {
        let temp = tempfile::tempdir().unwrap();
        assert!(matches!(
            Index::open(temp.path()),
            Err(IndexError::RebuildRequired(_))
        ));
        fs::write(temp.path().join("old-decision.json"), b"history").unwrap();
        assert!(matches!(
            Index::create_fresh_genesis(
                temp.path(),
                GenesisAuthority::ConfirmedFreshEmptyDirectory
            ),
            Err(IndexError::RebuildRequired(_))
        ));
        assert!(matches!(
            Index::open(temp.path()),
            Err(IndexError::RebuildRequired(_))
        ));
        let (empty, index) = fresh();
        assert_eq!(
            Index::open(empty.path()).unwrap().generation,
            index.generation
        );
        assert!(matches!(
            Index::create_fresh_genesis(
                empty.path(),
                GenesisAuthority::ConfirmedFreshEmptyDirectory
            ),
            Err(IndexError::RebuildRequired(_))
        ));
    }
    #[test]
    fn service_admission_creates_only_clean_genesis_and_preserves_old_wal() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("broker");
        fs::create_dir(&root).unwrap();
        let wal = temp.path().join("state.db-wal");
        fs::write(&wal, b"old WAL sentinel").unwrap();
        let lease = broker_admission_lease(&root).unwrap();
        let first = Index::admit_live_routes(&root, &lease).unwrap();
        assert_eq!(first.cursor(&key()).unwrap().sequence, 0);
        let generation = first.generation().to_owned();
        drop(lease);
        let lease = broker_admission_lease(&root).unwrap();
        assert_eq!(
            Index::admit_live_routes(&root, &lease)
                .unwrap()
                .generation(),
            generation
        );
        assert_eq!(fs::read(&wal).unwrap(), b"old WAL sentinel");
        drop(lease);

        let other = temp.path().join("old-broker");
        fs::create_dir(&other).unwrap();
        fs::write(other.join("old.route-selection.json"), b"old receipt").unwrap();
        let old_lease = broker_admission_lease(&other).unwrap();
        assert!(matches!(
            Index::admit_live_routes(&other, &old_lease),
            Err(IndexError::RebuildRequired(_))
        ));
        assert!(!other.join("index-v1/manifest.json").exists());
    }
    #[test]
    fn decision_reopen_pin_and_same_broker_threads_serialize() {
        let (temp, index) = fresh();
        let pinned = index
            .commit_decision(key(), "p".into(), "physical-a".into(), 2, true, 0)
            .unwrap();
        assert_eq!(pinned.sequence, 0);
        assert_eq!(index.cursor(&key()).unwrap().sequence, 0);
        let a = Index::open(temp.path()).unwrap();
        let b = Index::open(temp.path()).unwrap();
        let first = std::thread::spawn(move || {
            a.commit_decision(key(), "a".into(), "physical-a".into(), 2, false, 0)
        });
        let second = std::thread::spawn(move || {
            b.commit_decision(key(), "b".into(), "physical-a".into(), 2, false, 0)
        });
        let results = [first.join().unwrap(), second.join().unwrap()];
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        let reopened = Index::open(temp.path()).unwrap();
        assert_eq!(reopened.cursor(&key()).unwrap().sequence, 1);
        assert_eq!(reopened.decision("p").unwrap(), Some(pinned));
        let winner = results.into_iter().find_map(Result::ok).unwrap();
        assert_eq!(
            reopened.decision(&winner.handoff).unwrap(),
            Some(winner.clone())
        );
        assert_eq!(
            reopened
                .commit_decision(
                    key(),
                    winner.handoff.clone(),
                    "physical-a".into(),
                    2,
                    false,
                    0
                )
                .unwrap(),
            winner
        );
    }
    #[test]
    fn pending_crash_windows_reconcile_one_exact_decision() {
        // Before decision visibility: no advance, and the pending name clears.
        let (_temp, index) = fresh();
        let d = decision(&index, "before", 1, false);
        write_new(&index.base().join("pending.json"), &pending(&index, d)).unwrap();
        assert_eq!(index.cursor(&key()).unwrap().sequence, 0);
        assert!(!index.base().join("pending.json").exists());
        // Decision visible but cursor not published: exact readback advances.
        let d = decision(&index, "after", 1, false);
        write_new(
            &index.base().join("pending.json"),
            &pending(&index, d.clone()),
        )
        .unwrap();
        write_new(&index.decision_path(&d.handoff).unwrap(), &d).unwrap();
        assert_eq!(index.decision("after").unwrap(), Some(d.clone()));
        assert_eq!(index.cursor(&key()).unwrap().sequence, 1);
        // Cursor visible but pending remains: idempotent cleanup.
        let d2 = decision(&index, "published", 2, false);
        let p = pending(&index, d2.clone());
        write_new(&index.base().join("pending.json"), &p).unwrap();
        write_new(&index.decision_path(&d2.handoff).unwrap(), &d2).unwrap();
        let next = index.advanced(&p.expected, &d2).unwrap();
        write_atomic(&index.key_path("cursors", &key()).unwrap(), &next).unwrap();
        assert_eq!(index.cursor(&key()).unwrap(), next);
        assert!(!index.base().join("pending.json").exists());
    }
    #[test]
    fn live_receipt_and_cursor_share_one_exact_choice() {
        let (temp, index) = fresh();
        let body = b"selected physical-a, candidate 2\n";
        let path = temp.path().join("first.route-selection.json");
        let receipt = index
            .commit_live_decision(
                key(),
                "first".into(),
                "physical-a".into(),
                2,
                false,
                0,
                "first.route-selection.json".into(),
                body,
                || fs::write(&path, body),
            )
            .unwrap();
        assert_eq!(receipt.sequence, 1);
        let reopened = Index::open(temp.path()).unwrap();
        assert_eq!(reopened.cursor(&key()).unwrap().sequence, 1);
        assert_eq!(reopened.decision("first").unwrap(), Some(receipt.clone()));
        assert_eq!(
            reopened
                .commit_live_decision(
                    key(),
                    "first".into(),
                    "physical-a".into(),
                    2,
                    false,
                    0,
                    "first.route-selection.json".into(),
                    body,
                    || panic!("idempotent readback must not republish"),
                )
                .unwrap(),
            receipt
        );
        fs::write(&path, b"different candidate").unwrap();
        assert!(matches!(
            reopened.decision("first"),
            Err(IndexError::Corrupt(_))
        ));
        assert!(matches!(
            reopened.cursor(&key()),
            Err(IndexError::Corrupt(_))
        ));
    }
    #[test]
    fn live_receipt_crash_stages_reconcile_without_duplicate_advance() {
        let (temp, index) = fresh();
        let mut before = decision(&index, "before", 1, false);
        before.receipt = Some(future_artifact("before.route-selection.json", b"before"));
        write_new(&index.base().join("pending.json"), &pending(&index, before)).unwrap();
        assert_eq!(index.cursor(&key()).unwrap().sequence, 0);
        assert!(!index.base().join("pending.json").exists());

        let mut after = decision(&index, "after", 1, false);
        after.receipt = Some(future_artifact("after.route-selection.json", b"after"));
        write_new(
            &index.base().join("pending.json"),
            &pending(&index, after.clone()),
        )
        .unwrap();
        fs::write(temp.path().join("after.route-selection.json"), b"after").unwrap();
        let reopened = Index::open(temp.path()).unwrap();
        assert_eq!(reopened.cursor(&key()).unwrap().sequence, 1);
        assert_eq!(reopened.decision("after").unwrap(), Some(after));
        assert_eq!(reopened.cursor(&key()).unwrap().sequence, 1);

        let pin_bytes = b"pin";
        let pin_path = temp.path().join("pin.route-selection.json");
        let pin = reopened
            .commit_live_decision(
                key(),
                "pin".into(),
                "physical-a".into(),
                2,
                true,
                1,
                "pin.route-selection.json".into(),
                pin_bytes,
                || fs::write(&pin_path, pin_bytes),
            )
            .unwrap();
        assert_eq!(pin.sequence, 0);
        assert_eq!(reopened.cursor(&key()).unwrap().sequence, 1);
    }
    #[test]
    fn unindexed_route_receipt_requires_repair() {
        let (temp, index) = fresh();
        fs::write(temp.path().join("orphan.route-selection.json"), b"orphan").unwrap();
        assert!(matches!(
            index.commit_live_decision(
                key(),
                "orphan".into(),
                "physical-a".into(),
                2,
                false,
                0,
                "orphan.route-selection.json".into(),
                b"orphan",
                || panic!("unindexed receipt must not be republished"),
            ),
            Err(IndexError::RebuildRequired(_))
        ));
    }
    #[test]
    fn pending_mismatch_collision_and_generation_refuse() {
        let (_temp, index) = fresh();
        let d = decision(&index, "h", 1, false);
        write_new(
            &index.base().join("pending.json"),
            &pending(&index, d.clone()),
        )
        .unwrap();
        let mut wrong = d.clone();
        wrong.candidate_identity = "different".into();
        write_new(&index.decision_path("h").unwrap(), &wrong).unwrap();
        assert!(matches!(index.cursor(&key()), Err(IndexError::Corrupt(_))));
        let (_temp, index) = fresh();
        let other = CursorKey {
            model: "other".into(),
            config_sha256: "config".into(),
        };
        let mut cursor = index.cursor(&key()).unwrap();
        cursor.key = other;
        write_atomic(&index.key_path("cursors", &key()).unwrap(), &cursor).unwrap();
        assert!(matches!(index.cursor(&key()), Err(IndexError::Corrupt(_))));
        let manifest = Manifest {
            version: VERSION,
            generation: uuid::Uuid::new_v4().to_string(),
            generation_dir: false,
        };
        write_atomic(&index.base().join("manifest.json"), &manifest).unwrap();
        assert!(matches!(
            index.cursor(&key()),
            Err(IndexError::RebuildRequired(_))
        ));
    }
    #[test]
    fn provider_grant_effect_manual_unknown_q_and_cas_survive_reopen() {
        let (temp, index) = fresh();
        let grant = future_artifact("grant.json", b"grant");
        let a = index
            .update_account(
                "physical",
                0,
                AccountUpdate::AnnounceGrant {
                    id: "g".into(),
                    grant: ProviderGrant {
                        decision_handoff: "d".into(),
                        grant: grant.clone(),
                        candidate: None,
                        consumed_k: None,
                        certified_q: None,
                    },
                },
            )
            .unwrap();
        assert_eq!(a.revision, 1);
        assert!(
            !index.account("physical").unwrap().grants["g"]
                .grant
                .verify(temp.path())
                .unwrap()
        );
        assert!(matches!(
            index.update_account(
                "physical",
                0,
                AccountUpdate::SettleGrant {
                    id: "g".into(),
                    q: PhysicalQ {
                        physical_k: grant.clone(),
                        q: grant.clone(),
                        terminal: Some(grant.clone()),
                        completed_unix_nanos: 1
                    },
                    failed: true,
                    marker: Some(TerminalMarkerKind::Quota),
                }
            ),
            Err(IndexError::Conflict(_))
        ));
        let intent = future_artifact("manual-intent.json", b"intent");
        let a = index
            .update_account(
                "physical",
                1,
                AccountUpdate::AnnounceEffect {
                    id: "manual".into(),
                    effect: EffectIntent {
                        kind: EffectKind::ManualQuota,
                        source: SourceKey {
                            commands_sha256: "command".into(),
                            environment_sha256: "environment".into(),
                        },
                        decision_handoff: String::new(),
                        route_source: None,
                        candidate: None,
                        intent: intent.clone(),
                        reuse: None,
                        consumed_k: None,
                        certified_q: None,
                        result: None,
                    },
                },
            )
            .unwrap();
        assert_eq!(a.revision, 2);
        let reopened = Index::open(temp.path()).unwrap();
        assert_eq!(
            reopened.account("physical").unwrap().effects["manual"].intent,
            intent
        );
        fs::write(temp.path().join("grant.json"), b"grant").unwrap();
        fs::write(temp.path().join("manual-intent.json"), b"intent").unwrap();
        let k = artifact(temp.path(), "provider-k.json", b"K");
        let a = reopened
            .update_account(
                "physical",
                2,
                AccountUpdate::ConsumeGrant {
                    id: "g".into(),
                    k: k.clone(),
                },
            )
            .unwrap();
        assert_eq!(a.observed_invocations, 1);
        let mk = artifact(temp.path(), "manual-k.json", b"manual K");
        reopened
            .update_account(
                "physical",
                3,
                AccountUpdate::ConsumeEffect {
                    id: "manual".into(),
                    k: mk.clone(),
                },
            )
            .unwrap();
        let unresolved = Index::open(temp.path())
            .unwrap()
            .account("physical")
            .unwrap();
        assert!(unresolved.grants["g"].consumed_k.is_some());
        assert!(unresolved.effects["manual"].consumed_k.is_some());
        assert!(unresolved.source_q.is_empty()); // unknown Q retains both K references
        let mq = artifact(temp.path(), "manual-q.json", b"manual Q");
        let a = reopened
            .update_account(
                "physical",
                4,
                AccountUpdate::SettleEffect {
                    id: "manual".into(),
                    q: PhysicalQ {
                        physical_k: mk,
                        q: mq,
                        terminal: None,
                        completed_unix_nanos: 20,
                    },
                    result: None,
                    marker: Some(false),
                },
            )
            .unwrap();
        let source = SourceKey {
            commands_sha256: "command".into(),
            environment_sha256: "environment".into(),
        };
        assert_eq!(a.source_q[&keyed(&source).unwrap()].source, source);
        assert!(
            a.source_q[&keyed(&source).unwrap()]
                .latest_quota_q
                .is_some()
        );
        let pq = artifact(temp.path(), "provider-q.json", b"provider Q");
        let terminal = artifact(temp.path(), "terminal.json", b"terminal");
        let a = reopened
            .update_account(
                "physical",
                5,
                AccountUpdate::SettleGrant {
                    id: "g".into(),
                    q: PhysicalQ {
                        physical_k: k,
                        q: pq,
                        terminal: Some(terminal),
                        completed_unix_nanos: 10,
                    },
                    failed: true,
                    marker: Some(TerminalMarkerKind::Quota),
                },
            )
            .unwrap();
        assert_eq!(a.observed_invocations, 1);
        assert_eq!(a.recent_failure_nanos, vec![10]);
        assert_eq!(a.markers.quota_rejection_nanos, Some(10));
        assert!(a.grants["g"].certified_q.is_some());
        assert_eq!(
            a.source_q[&keyed(&source).unwrap()]
                .latest_quota_q
                .as_ref()
                .unwrap()
                .completed_unix_nanos,
            20,
            "an older provider terminal cannot replace newer source Q",
        );
        assert_eq!(
            Index::open(temp.path())
                .unwrap()
                .account("physical")
                .unwrap(),
            a
        );
    }
    #[test]
    fn account_collision_corruption_and_missing_artifact_refuse() {
        let (temp, index) = fresh();
        let a = index.account("one").unwrap();
        let mut wrong = a.clone();
        wrong.physical_key = "two".into();
        write_atomic(&index.key_path("accounts", &"one").unwrap(), &wrong).unwrap();
        assert!(matches!(index.account("one"), Err(IndexError::Corrupt(_))));
        let (_temp, index) = fresh();
        index
            .update_account(
                "one",
                0,
                AccountUpdate::AnnounceGrant {
                    id: "g".into(),
                    grant: ProviderGrant {
                        decision_handoff: "d".into(),
                        grant: future_artifact("grant", b"x"),
                        candidate: None,
                        consumed_k: None,
                        certified_q: None,
                    },
                },
            )
            .unwrap();
        let path = index.key_path("accounts", &"one").unwrap();
        fs::write(&path, b"{broken").unwrap();
        assert!(matches!(index.account("one"), Err(IndexError::Corrupt(_))));
        fs::remove_file(&path).unwrap();
        assert!(matches!(index.account("one"), Err(IndexError::Corrupt(_))));
        assert!(!temp.path().join("not-present").exists());
    }
    #[test]
    fn missing_committed_cursor_refuses_empty_reinterpretation() {
        let (_temp, index) = fresh();
        index
            .commit_decision(key(), "h".into(), "physical".into(), 0, false, 0)
            .unwrap();
        fs::remove_file(index.key_path("cursors", &key()).unwrap()).unwrap();
        assert!(matches!(index.cursor(&key()), Err(IndexError::Corrupt(_))));
        assert!(matches!(
            index.commit_decision(key(), "another".into(), "physical".into(), 0, false, 0),
            Err(IndexError::Corrupt(_))
        ));
    }
    #[test]
    fn effect_k_readback_markers_and_bounded_failures() {
        let (temp, index) = fresh();
        for (revision, id, kind) in [
            (0, "quota", EffectKind::Quota),
            (1, "auth", EffectKind::Auth),
        ] {
            index
                .update_account(
                    "physical",
                    revision,
                    AccountUpdate::AnnounceEffect {
                        id: id.into(),
                        effect: EffectIntent {
                            kind,
                            source: SourceKey {
                                commands_sha256: format!("{id}-source"),
                                environment_sha256: "env".into(),
                            },
                            decision_handoff: String::new(),
                            route_source: None,
                            candidate: None,
                            intent: future_artifact(&format!("{id}-intent"), b"I"),
                            reuse: None,
                            consumed_k: None,
                            certified_q: None,
                            result: None,
                        },
                    },
                )
                .unwrap();
        }
        let intent = artifact(temp.path(), "quota-intent", b"I");
        let k = artifact(temp.path(), "quota-k", b"K");
        index
            .update_account(
                "physical",
                2,
                AccountUpdate::ConsumeEffect {
                    id: "quota".into(),
                    k: k.clone(),
                },
            )
            .unwrap();
        assert_eq!(
            index.account("physical").unwrap().effects["quota"].intent,
            intent
        );
        fs::remove_file(temp.path().join("quota-k")).unwrap();
        assert!(matches!(
            index.account("physical"),
            Err(IndexError::Conflict(_))
        ));
        fs::write(temp.path().join("quota-k"), b"K").unwrap();
        let q = artifact(temp.path(), "quota-q", b"Q");
        index
            .update_account(
                "physical",
                3,
                AccountUpdate::SettleEffect {
                    id: "quota".into(),
                    q: PhysicalQ {
                        physical_k: k,
                        q,
                        terminal: None,
                        completed_unix_nanos: 40,
                    },
                    result: None,
                    marker: Some(true),
                },
            )
            .unwrap();
        let a = index.account("physical").unwrap();
        assert_eq!(a.markers.quota_rejection_nanos, Some(40));
        assert!(a.source_q.is_empty());
        assert!(a.effects.contains_key("auth"));
        let terminal = artifact(temp.path(), "terminal", b"T");
        let physical_k = artifact(temp.path(), "provider-k", b"K");
        let q = artifact(temp.path(), "provider-q", b"Q");
        index
            .update_account(
                "physical",
                4,
                AccountUpdate::AnnounceGrant {
                    id: "model-capacity".into(),
                    grant: ProviderGrant {
                        decision_handoff: "model-capacity-decision".into(),
                        grant: future_artifact("model-capacity-grant", b"G"),
                        candidate: None,
                        consumed_k: None,
                        certified_q: None,
                    },
                },
            )
            .unwrap();
        artifact(temp.path(), "model-capacity-grant", b"G");
        index
            .update_account(
                "physical",
                5,
                AccountUpdate::ConsumeGrant {
                    id: "model-capacity".into(),
                    k: physical_k.clone(),
                },
            )
            .unwrap();
        index
            .update_account(
                "physical",
                6,
                AccountUpdate::SettleGrant {
                    id: "model-capacity".into(),
                    q: PhysicalQ {
                        physical_k,
                        q,
                        terminal: Some(terminal),
                        completed_unix_nanos: 35,
                    },
                    failed: true,
                    marker: Some(TerminalMarkerKind::ModelCapacity),
                },
            )
            .unwrap();
        assert_eq!(
            Index::open(temp.path())
                .unwrap()
                .account("physical")
                .unwrap()
                .markers
                .model_capacity_nanos,
            Some(35)
        );
        artifact(temp.path(), "auth-intent", b"I");
        let auth_k = artifact(temp.path(), "auth-k", b"K");
        index
            .update_account(
                "physical",
                7,
                AccountUpdate::ConsumeEffect {
                    id: "auth".into(),
                    k: auth_k.clone(),
                },
            )
            .unwrap();
        let auth_q = artifact(temp.path(), "auth-q", b"Q");
        let a = index
            .update_account(
                "physical",
                8,
                AccountUpdate::SettleEffect {
                    id: "auth".into(),
                    q: PhysicalQ {
                        physical_k: auth_k,
                        q: auth_q,
                        terminal: None,
                        completed_unix_nanos: 50,
                    },
                    result: None,
                    marker: Some(false),
                },
            )
            .unwrap();
        let auth_source = SourceKey {
            commands_sha256: "auth-source".into(),
            environment_sha256: "env".into(),
        };
        assert!(
            a.source_q[&keyed(&auth_source).unwrap()]
                .latest_auth_q
                .is_some()
        );
        assert_eq!(a.source_q.len(), 1);
        let mut a = index.account("physical").unwrap();
        a.recent_failure_nanos = vec![1; MAX_RECENT_FAILURES];
        assert!(matches!(a.add_failure(2), Err(IndexError::Conflict(_))));
    }

    #[test]
    fn indexed_effect_older_certified_q_cannot_replace_newer_source_q() {
        let (temp, index) = fresh();
        let source = SourceKey {
            commands_sha256: "same-command".into(),
            environment_sha256: "same-environment".into(),
        };
        for (id, nanos) in [("newer", 50), ("older", 10)] {
            let account = index.account("physical").unwrap();
            index
                .update_account(
                    "physical",
                    account.revision,
                    AccountUpdate::AnnounceEffect {
                        id: id.into(),
                        effect: EffectIntent {
                            kind: EffectKind::Quota,
                            source: source.clone(),
                            decision_handoff: id.into(),
                            route_source: None,
                            candidate: None,
                            intent: future_artifact(&format!("{id}-intent"), b"intent"),
                            reuse: None,
                            consumed_k: None,
                            certified_q: None,
                            result: None,
                        },
                    },
                )
                .unwrap();
            artifact(temp.path(), &format!("{id}-intent"), b"intent");
            let k = artifact(temp.path(), &format!("{id}-k"), b"K");
            let account = index.account("physical").unwrap();
            index
                .update_account(
                    "physical",
                    account.revision,
                    AccountUpdate::ConsumeEffect {
                        id: id.into(),
                        k: k.clone(),
                    },
                )
                .unwrap();
            let q = artifact(temp.path(), &format!("{id}-q"), b"Q");
            let result = artifact(temp.path(), &format!("{id}-result"), b"result");
            let account = index.account("physical").unwrap();
            index
                .update_account(
                    "physical",
                    account.revision,
                    AccountUpdate::SettleEffect {
                        id: id.into(),
                        q: PhysicalQ {
                            physical_k: k,
                            q,
                            terminal: None,
                            completed_unix_nanos: nanos,
                        },
                        result: Some(result),
                        marker: Some(false),
                    },
                )
                .unwrap();
        }
        let account = index.account("physical").unwrap();
        assert_eq!(account.effects.len(), 2);
        assert!(
            account
                .effects
                .values()
                .all(|effect| effect.result.is_some())
        );
        assert_eq!(
            account.source_q[&keyed(&source).unwrap()]
                .latest_quota_q
                .as_ref()
                .unwrap()
                .completed_unix_nanos,
            50
        );
    }

    #[test]
    fn compact_typed_head_is_constant_over_settled_history_and_never_revives_old_q() {
        let (temp, _genesis) = fresh();
        let root = temp.path();
        let wal = root.join("old-state.db-wal");
        fs::write(&wal, b"old WAL sentinel").unwrap();
        let source = SourceKey {
            commands_sha256: "exact-command".into(),
            environment_sha256: "exact-env".into(),
        };
        let mut account = Account {
            generation: String::new(),
            physical_key: "physical".into(),
            revision: 0,
            grants: BTreeMap::new(),
            effects: BTreeMap::new(),
            observed_invocations: 0,
            markers: MarkerTimes::default(),
            source_q: BTreeMap::new(),
            recent_failure_nanos: Vec::new(),
        };
        fn add(
            root: &Path,
            account: &mut Account,
            source: &SourceKey,
            n: usize,
            outcome: &str,
            used: f64,
            model: &str,
        ) {
            let id = format!("effect-{n:03}");
            let intent = artifact(root, &format!("{id}.intent.json"), b"intent");
            let k = artifact(root, &format!("{id}.k.json"), b"K");
            let q = artifact(root, &format!("{id}.q.json"), b"Q");
            let candidate_bytes =
                serde_json::to_vec(&serde_json::json!({"model":model,"config_sha256":"config"}))
                    .unwrap();
            let candidate = artifact(root, &format!("{id}.candidate.json"), &candidate_bytes);
            let windows = if outcome == "valid_windows" && used == 100.0 {
                serde_json::json!([
                    {"used_percent":20.0,"resets_at":"2099-01-01T00:00:00Z","remaining":50},
                    {"used_percent":used,"resets_at":"2099-01-01T00:00:00Z","remaining":0}
                ])
            } else if outcome == "valid_windows" {
                serde_json::json!([{"used_percent":used,"resets_at":"2099-01-01T00:00:00Z","remaining":50}])
            } else {
                serde_json::json!([])
            };
            let result_bytes = serde_json::to_vec(&serde_json::json!({
                "effect_id":id,"state":"drained","outcome":outcome,"windows":windows,
                "completed_unix_seconds":100,"artifact":"exact","peer_effect_id":null,"peer_artifact":null
            })).unwrap();
            let result = artifact(root, &format!("{id}.result.json"), &result_bytes);
            account.effects.insert(
                id,
                EffectIntent {
                    kind: EffectKind::Quota,
                    source: source.clone(),
                    decision_handoff: String::new(),
                    route_source: None,
                    candidate: Some(candidate),
                    intent,
                    reuse: None,
                    consumed_k: Some(k.clone()),
                    certified_q: Some(PhysicalQ {
                        physical_k: k,
                        q,
                        terminal: None,
                        completed_unix_nanos: n as i64,
                    }),
                    result: Some(result),
                },
            );
        }
        fn publish(root: &Path, account: &Account) -> Index {
            let mut snapshot = OfflineSnapshot::default();
            snapshot.accounts.insert("physical".into(), account.clone());
            Index::publish_offline(root, snapshot).unwrap()
        }
        add(
            root,
            &mut account,
            &source,
            1,
            "valid_windows",
            20.0,
            "model-a",
        );
        let one = publish(root, &account);
        let (first, first_io) = {
            let _guard = ReaderIoGuard::start("compact-one");
            let head = one.compact_account("physical").unwrap();
            drop(_guard);
            (head, last_reader_io().unwrap())
        };
        let digest = keyed(&source).unwrap();
        assert_eq!(
            first.sources[&digest]
                .quota
                .as_ref()
                .unwrap()
                .quota_basis_points_at(100),
            Some(8000)
        );
        assert_eq!(
            first.sources[&digest]
                .quota
                .as_ref()
                .unwrap()
                .quota_basis_points_at(100 + 5 * 60 * 60),
            None
        );
        for n in 2..317 {
            add(
                root,
                &mut account,
                &source,
                n,
                "valid_windows",
                20.0,
                "model-a",
            );
        }
        add(root, &mut account, &source, 317, "failed", 0.0, "model-b");
        let failed = publish(root, &account).compact_account("physical").unwrap();
        assert_eq!(
            failed.sources[&digest].quota.as_ref().unwrap().outcome,
            "failed"
        );
        assert_eq!(
            failed.sources[&digest]
                .quota
                .as_ref()
                .unwrap()
                .quota_basis_points_at(100),
            None
        );
        add(root, &mut account, &source, 318, "empty", 0.0, "model-b");
        let empty = publish(root, &account).compact_account("physical").unwrap();
        assert_eq!(
            empty.sources[&digest].quota.as_ref().unwrap().outcome,
            "empty"
        );
        assert_eq!(
            empty.sources[&digest]
                .quota
                .as_ref()
                .unwrap()
                .quota_basis_points_at(100),
            None
        );
        add(
            root,
            &mut account,
            &source,
            319,
            "valid_windows",
            100.0,
            "model-b",
        );
        let exhausted = publish(root, &account).compact_account("physical").unwrap();
        let q = exhausted.sources[&digest].quota.as_ref().unwrap();
        assert_eq!(q.origin_model.as_deref(), Some("model-b"));
        assert_eq!(
            q.quota_basis_points_at(100),
            None,
            "any full window excludes immediately"
        );
        add(root, &mut account, &source, 320, "invalid", 0.0, "model-b");
        let many = publish(root, &account);
        let (latest, many_io) = {
            let _guard = ReaderIoGuard::start("compact-many");
            let head = many.compact_account("physical").unwrap();
            drop(_guard);
            (head, last_reader_io().unwrap())
        };
        let q = latest.sources[&digest].quota.as_ref().unwrap();
        assert_eq!(q.q.completed_unix_nanos, 320);
        assert_eq!(q.outcome, "invalid");
        assert_eq!(q.quota_basis_points_at(100), None);
        assert_eq!(
            many_io, first_io,
            "compact read must not reopen settled history"
        );
        assert_eq!(many_io.directory_entries, 0);
        assert!(many_io.opened > 0);
        assert_eq!(fs::read(&wal).unwrap(), b"old WAL sentinel");
        assert_eq!(
            Index::open(root)
                .unwrap()
                .compact_account("physical")
                .unwrap()
                .revision,
            latest.revision
        );
        assert!(matches!(
            one.compact_account("physical"),
            Err(IndexError::RebuildRequired(_))
        ));
        let intent = artifact(root, "effect-321.intent", b"pending");
        many.update_account(
            "physical",
            latest.revision,
            AccountUpdate::AnnounceEffect {
                id: "effect-321".into(),
                effect: EffectIntent {
                    kind: EffectKind::Quota,
                    source: source.clone(),
                    decision_handoff: String::new(),
                    route_source: None,
                    candidate: None,
                    intent,
                    reuse: None,
                    consumed_k: None,
                    certified_q: None,
                    result: None,
                },
            },
        )
        .unwrap();
        assert_eq!(many.compact_account("physical").unwrap().pending.len(), 1);
        assert!(matches!(
            many.route_reader_preflight("physical"),
            Err(IndexError::Conflict(_))
        ));
        fs::remove_file(many.key_path("heads", &"physical").unwrap()).unwrap();
        assert!(matches!(
            many.compact_account("physical"),
            Err(IndexError::RebuildRequired(_))
        ));
    }

    #[test]
    fn compact_head_announcements_are_debt_before_and_after_k() {
        let (temp, index) = fresh();
        let grant = artifact(temp.path(), "grant.json", b"grant");
        let account = index
            .update_account(
                "physical",
                0,
                AccountUpdate::AnnounceGrant {
                    id: "grant".into(),
                    grant: ProviderGrant {
                        decision_handoff: "decision".into(),
                        grant,
                        candidate: None,
                        consumed_k: None,
                        certified_q: None,
                    },
                },
            )
            .unwrap();
        assert_eq!(index.compact_account("physical").unwrap().pending.len(), 1);
        assert!(matches!(
            index.route_reader_preflight("physical"),
            Err(IndexError::Conflict(_))
        ));
        let k = artifact(temp.path(), "grant.k.json", b"K");
        index
            .update_account(
                "physical",
                account.revision,
                AccountUpdate::ConsumeGrant {
                    id: "grant".into(),
                    k,
                },
            )
            .unwrap();
        assert_eq!(index.compact_account("physical").unwrap().pending.len(), 1);
        let intent = artifact(temp.path(), "effect.intent.json", b"intent");
        let account = index.account("physical").unwrap();
        index
            .update_account(
                "physical",
                account.revision,
                AccountUpdate::AnnounceEffect {
                    id: "effect".into(),
                    effect: EffectIntent {
                        kind: EffectKind::Quota,
                        source: SourceKey {
                            commands_sha256: "command".into(),
                            environment_sha256: "env".into(),
                        },
                        decision_handoff: String::new(),
                        route_source: None,
                        candidate: None,
                        intent,
                        reuse: None,
                        consumed_k: None,
                        certified_q: None,
                        result: None,
                    },
                },
            )
            .unwrap();
        assert_eq!(index.compact_account("physical").unwrap().pending.len(), 2);
        let manual_intent = artifact(
            temp.path(),
            "manual.intent.json",
            br#"{"source_operation_id":null,"quota_script":"quota"}"#,
        );
        let account = index.account("physical").unwrap();
        index
            .update_account(
                "physical",
                account.revision,
                AccountUpdate::AnnounceEffect {
                    id: "manual".into(),
                    effect: EffectIntent {
                        kind: EffectKind::ManualQuota,
                        source: SourceKey {
                            commands_sha256: "manual-command".into(),
                            environment_sha256: "manual-env".into(),
                        },
                        decision_handoff: String::new(),
                        route_source: None,
                        candidate: None,
                        intent: manual_intent,
                        reuse: None,
                        consumed_k: None,
                        certified_q: None,
                        result: None,
                    },
                },
            )
            .unwrap();
        assert_eq!(index.compact_account("physical").unwrap().pending.len(), 3);
        let manual_k = artifact(temp.path(), "manual.k.json", b"K");
        let account = index.account("physical").unwrap();
        index
            .update_account(
                "physical",
                account.revision,
                AccountUpdate::ConsumeEffect {
                    id: "manual".into(),
                    k: manual_k,
                },
            )
            .unwrap();
        assert_eq!(index.compact_account("physical").unwrap().pending.len(), 3);
        let mut pending = index.compact_account("physical").unwrap();
        pending.committed = false;
        write_atomic(&index.key_path("heads", &"physical").unwrap(), &pending).unwrap();
        assert!(matches!(
            index.compact_account("physical"),
            Err(IndexError::RebuildRequired(_))
        ));
        assert!(matches!(
            index.update_account(
                "physical",
                5,
                AccountUpdate::PruneFailures { before_nanos: 0 }
            ),
            Err(IndexError::RebuildRequired(_))
        ));
    }

    #[test]
    fn compact_markers_keep_account_rejection_separate_from_model_capacity() {
        let (temp, _index) = fresh();
        let root = temp.path();
        let mut account = Account {
            generation: String::new(),
            physical_key: "physical".into(),
            revision: 0,
            grants: BTreeMap::new(),
            effects: BTreeMap::new(),
            observed_invocations: 3,
            markers: MarkerTimes {
                quota_rejection_nanos: Some(20),
                auth_rejection_nanos: Some(30),
                model_capacity_nanos: Some(10),
            },
            source_q: BTreeMap::new(),
            recent_failure_nanos: Vec::new(),
        };
        for (id, model, outcome, nanos) in [
            ("capacity", "model-a", "model_at_capacity", 10),
            ("quota", "model-b", "quota_rejected", 20),
            ("auth", "model-c", "auth_rejected", 30),
        ] {
            let grant = artifact(root, &format!("{id}.grant"), b"grant");
            let k = artifact(root, &format!("{id}.k"), b"K");
            let q = artifact(root, &format!("{id}.q"), b"Q");
            let terminal_bytes = serde_json::to_vec(&serde_json::json!({
                "grant_id":id,"selection":{"account_identity":"physical","model":model,"config_sha256":"config"},
                "physical_q_sha256":q.sha256,"physical_q_unix_nanos":nanos,"outcome":outcome
            })).unwrap();
            let terminal = artifact(root, &format!("{id}.terminal"), &terminal_bytes);
            account.grants.insert(
                id.into(),
                ProviderGrant {
                    decision_handoff: id.into(),
                    grant,
                    candidate: None,
                    consumed_k: Some(k.clone()),
                    certified_q: Some(PhysicalQ {
                        physical_k: k,
                        q,
                        terminal: Some(terminal),
                        completed_unix_nanos: nanos,
                    }),
                },
            );
        }
        let mut snapshot = OfflineSnapshot::default();
        snapshot.accounts.insert("physical".into(), account);
        let head = Index::publish_offline(root, snapshot)
            .unwrap()
            .compact_account("physical")
            .unwrap();
        assert!(!head.unknown_marker_scope);
        assert_eq!(head.quota_rejection.as_ref().unwrap().model, "model-b");
        assert_eq!(head.auth_rejection.as_ref().unwrap().model, "model-c");
        assert_eq!(head.model_capacity.len(), 1);
        assert_eq!(
            head.model_capacity.values().next().unwrap().model,
            "model-a"
        );
        assert_eq!(head.marker_times.quota_rejection_nanos, Some(20));
    }

    #[test]
    fn old_index_schema_requires_offline_rebuild() {
        let (temp, index) = fresh();
        let mut manifest: Manifest = read(&temp.path().join("index-v1/manifest.json"))
            .unwrap()
            .unwrap();
        manifest.version = 1;
        write_atomic(&temp.path().join("index-v1/manifest.json"), &manifest).unwrap();
        assert!(matches!(
            Index::open(temp.path()),
            Err(IndexError::RebuildRequired(_))
        ));
        assert!(matches!(
            index.route_reader_preflight("physical"),
            Err(IndexError::RebuildRequired(_))
        ));
    }
}
