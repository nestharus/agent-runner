//! Inert broker-owned evidence index substrate. The live fresh route and all
//! provider/effect writers still use their existing files; this module is not
//! called by them. Its records cannot certify provider Q by themselves.
//!
//! Lock order for a future same-broker cutover: route lock, then account locks
//! in sorted physical-key order. Grant/effect admission takes only its account
//! lock. PID1 Q takes no index lock. Never acquire route lock from an account
//! lock. Each operation here takes its own required lock; a caller holding an
//! account lock must not call a route operation.
//! `index-v1/route.lock` is distinct from the current live
//! `route-selection.lock`; a future cutover must join their authority before
//! either can admit a decision.
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

const VERSION: u32 = 1;
const MAX_RECORD: u64 = 4 * 1024 * 1024;
const MAX_RECENT_FAILURES: usize = 256;

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
type Result<T> = std::result::Result<T, IndexError>;
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
    let file = match File::open(path) {
        Ok(f) => f,
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
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    generation: String,
}
#[derive(Clone, Debug)]
pub(super) struct Index {
    root: PathBuf,
    generation: String,
}
/// The caller must hold an admission freeze for this previously unused broker
/// directory. The empty-directory check below is a second, local guard.
pub(super) enum GenesisAuthority {
    ConfirmedFreshEmptyDirectory,
}
impl Index {
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
        for name in ["cursors", "accounts", "decisions"] {
            fs::create_dir(base.join(name))?;
        }
        for name in ["cursors", "accounts", "decisions"] {
            sync_dir(&base.join(name))?;
        }
        sync_dir(&base)?;
        let manifest = Manifest {
            version: VERSION,
            generation: uuid::Uuid::new_v4().to_string(),
        };
        write_new(&base.join("manifest.json"), &manifest)?;
        sync_dir(root)?;
        Ok(Self {
            root: root.to_owned(),
            generation: manifest.generation,
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
        for name in ["cursors", "accounts", "decisions"] {
            if !root.join("index-v1").join(name).is_dir() {
                return Err(IndexError::RebuildRequired("index storage missing"));
            }
        }
        Ok(Self {
            root: root.to_owned(),
            generation: manifest.generation,
        })
    }
    fn base(&self) -> PathBuf {
        self.root.join("index-v1")
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
    sequence: u64,
    index: Option<usize>,
    last_handoff: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Decision {
    generation: String,
    handoff: String,
    key: CursorKey,
    candidate_identity: String,
    candidate_index: usize,
    pin: bool,
    sequence: u64,
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
        if d.as_ref()
            .is_some_and(|d| d.handoff != handoff || d.generation != self.generation)
        {
            return Err(corrupt("decision key or generation collision"));
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
            self.mark_known("cursors", &current.key)?;
            write_atomic(&self.key_path("cursors", &current.key)?, &next)?;
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
    pub(super) fn decision(&self, handoff: &str) -> Result<Option<Decision>> {
        let _lock = locked(&self.base().join("route.lock"))?;
        self.check_generation()?;
        self.reconcile_pending()?;
        self.read_decision(handoff)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Artifact {
    pub path: String,
    pub sha256: String,
}
impl Artifact {
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
        let mut f = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(root.join(path))
        {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e.into()),
        };
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
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ProviderGrant {
    pub decision_handoff: String,
    pub grant: Artifact,
    pub consumed_k: Option<Artifact>,
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
    pub intent: Artifact,
    pub consumed_k: Option<Artifact>,
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
            if let Some(k) = &grant.consumed_k {
                grant.grant.require_present(&self.root)?;
                k.require_present(&self.root)?;
            } else {
                grant.grant.verify(&self.root)?;
            }
        }
        for effect in account.effects.values() {
            effect.intent.validate()?;
            if let Some(k) = &effect.consumed_k {
                effect.intent.require_present(&self.root)?;
                k.require_present(&self.root)?;
            } else {
                effect.intent.verify(&self.root)?;
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
                if grant.consumed_k.is_some() {
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
            AccountUpdate::SettleGrant { id, q, failed } => {
                let grant = a
                    .grants
                    .get(&id)
                    .ok_or(IndexError::Conflict("grant absent"))?;
                let k = grant
                    .consumed_k
                    .as_ref()
                    .ok_or(IndexError::Conflict("grant K absent"))?;
                if &q.physical_k != k || q.terminal.is_none() {
                    return Err(IndexError::Conflict("provider Q/K or terminal mismatch"));
                }
                q.verify(&self.root)?;
                a.grants.remove(&id);
                if failed {
                    a.add_failure(q.completed_unix_nanos)?;
                }
            }
            AccountUpdate::AnnounceEffect { id, effect } => {
                if id.is_empty() || a.effects.contains_key(&id) || effect.consumed_k.is_some() {
                    return Err(IndexError::Conflict(
                        "effect identity already announced or includes K",
                    ));
                }
                effect.intent.validate()?;
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
            AccountUpdate::SettleEffect { id, q, marker } => {
                let effect = a
                    .effects
                    .get(&id)
                    .ok_or(IndexError::Conflict("effect absent"))?;
                let k = effect
                    .consumed_k
                    .as_ref()
                    .ok_or(IndexError::Conflict("effect K absent"))?;
                if &q.physical_k != k || q.terminal.is_some() {
                    return Err(IndexError::Conflict("effect Q/K mismatch"));
                }
                q.verify(&self.root)?;
                let kind = effect.kind.clone();
                let source = effect.source.clone();
                a.effects.remove(&id);
                if marker {
                    a.mark(&kind, q.completed_unix_nanos);
                } else {
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
            AccountUpdate::RecordTerminalMarker { kind, q } => {
                if q.terminal.is_none() {
                    return Err(IndexError::Conflict(
                        "terminal marker lacks terminal record",
                    ));
                }
                q.verify(&self.root)?;
                let target = match kind {
                    TerminalMarkerKind::Quota => &mut a.markers.quota_rejection_nanos,
                    TerminalMarkerKind::Auth => &mut a.markers.auth_rejection_nanos,
                    TerminalMarkerKind::ModelCapacity => &mut a.markers.model_capacity_nanos,
                };
                *target = Some(target.unwrap_or(i64::MIN).max(q.completed_unix_nanos));
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
        self.mark_known("accounts", &key.to_owned())?;
        write_atomic(&self.key_path("accounts", &key)?, &a)?;
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
        marker: bool,
    },
    RecordTerminalMarker {
        kind: TerminalMarkerKind,
        q: PhysicalQ,
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
                        consumed_k: None,
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
                AccountUpdate::RecordTerminalMarker {
                    kind: TerminalMarkerKind::Quota,
                    q: PhysicalQ {
                        physical_k: grant.clone(),
                        q: grant.clone(),
                        terminal: None,
                        completed_unix_nanos: 1
                    }
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
                        intent: intent.clone(),
                        consumed_k: None,
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
                    marker: false,
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
                },
            )
            .unwrap();
        assert_eq!(a.observed_invocations, 1);
        assert_eq!(a.recent_failure_nanos, vec![10]);
        assert!(a.grants.is_empty());
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
                        consumed_k: None,
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
                            intent: future_artifact(&format!("{id}-intent"), b"I"),
                            consumed_k: None,
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
                    marker: true,
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
                AccountUpdate::RecordTerminalMarker {
                    kind: TerminalMarkerKind::ModelCapacity,
                    q: PhysicalQ {
                        physical_k,
                        q,
                        terminal: Some(terminal),
                        completed_unix_nanos: 35,
                    },
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
                5,
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
                6,
                AccountUpdate::SettleEffect {
                    id: "auth".into(),
                    q: PhysicalQ {
                        physical_k: auth_k,
                        q: auth_q,
                        terminal: None,
                        completed_unix_nanos: 50,
                    },
                    marker: false,
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
}
