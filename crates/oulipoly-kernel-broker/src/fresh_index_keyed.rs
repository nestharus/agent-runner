//! Non-activating candidate for per-physical-account keyed live state.
//!
//! A transaction changes only named keys and a constant-size root. Immutable
//! objects and chained audit records keep exact historical evidence. The root
//! rename commits; a durable intent makes pointer repair deterministic after
//! either side of that rename. Operations take this store's account lock;
//! integration must join it to broker route/admission lock order.
//! This module does not replace `Index::account` or authorize a route/K.
use super::{IndexError, Result, corrupt, keyed};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::cell::Cell;
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

// A single physical artifact/transaction can be rejected for resource safety.
// Neither an account nor its number of sources/effects has a byte/count cap.
const MAX_ITEM_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct IoCount {
    pub open_attempts: u64,
    pub opened: u64,
    pub directory_entries: u64,
    pub bytes_parsed: u64,
    pub bytes_written: u64,
}
thread_local! { static IO: Cell<Option<IoCount>> = const { Cell::new(None) }; }
fn count(f: impl FnOnce(&mut IoCount)) {
    IO.with(|cell| {
        if let Some(mut io) = cell.get() {
            f(&mut io);
            cell.set(Some(io));
        }
    });
}
pub(crate) fn measured<T>(run: impl FnOnce() -> T) -> (T, IoCount) {
    IO.with(|cell| {
        assert!(cell.get().is_none(), "nested keyed I/O measurement");
        cell.set(Some(IoCount::default()));
    });
    let result = run();
    (result, IO.with(|cell| cell.replace(None).unwrap()))
}
fn opened(result: &io::Result<File>) {
    count(|io| {
        io.open_attempts += 1;
        io.opened += u64::from(result.is_ok());
    });
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Sealed<T> {
    data: T,
    sha256: String,
}
fn read<T: DeserializeOwned + Serialize>(path: &Path) -> Result<Option<T>> {
    let result = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path);
    opened(&result);
    let file = match result {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !file.metadata()?.is_file() || file.metadata()?.len() > MAX_ITEM_BYTES {
        return Err(corrupt(format!("invalid keyed record {}", path.display())));
    }
    let mut bytes = Vec::new();
    file.take(MAX_ITEM_BYTES + 1).read_to_end(&mut bytes)?;
    count(|io| io.bytes_parsed += bytes.len() as u64);
    let sealed: Sealed<T> = serde_json::from_slice(&bytes)
        .map_err(|error| corrupt(format!("{}: {error}", path.display())))?;
    if keyed(&sealed.data)? != sealed.sha256 {
        return Err(corrupt(format!("keyed checksum {}", path.display())));
    }
    Ok(Some(sealed.data))
}
fn sync_dir(path: &Path) -> Result<()> {
    let result = File::open(path);
    opened(&result);
    result?.sync_all()?;
    Ok(())
}
fn write<T: Serialize>(path: &Path, value: &T, replace: bool) -> Result<()> {
    let bytes = serde_json::to_vec(&Sealed {
        data: value,
        sha256: keyed(value)?,
    })
    .map_err(|error| corrupt(error.to_string()))?;
    if bytes.len() as u64 > MAX_ITEM_BYTES {
        return Err(IndexError::Conflict(
            "keyed physical record exceeds byte safeguard",
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| corrupt("keyed parent absent"))?;
    let temp = parent.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let result = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp);
    opened(&result);
    let mut file = result?;
    let result = (|| -> Result<()> {
        file.write_all(&bytes)?;
        count(|io| io.bytes_written += bytes.len() as u64);
        file.sync_all()?;
        if replace {
            fs::rename(&temp, path)?;
        } else {
            fs::hard_link(&temp, path)?;
        }
        sync_dir(parent)?;
        if !replace {
            fs::remove_file(&temp)?;
            sync_dir(parent)?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Root {
    generation: String,
    account: String,
    revision: u64,
    pending_count: u64,
    audit_sha256: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Known {
    generation: String,
    account: String,
    class: String,
    key: String,
    birth_revision: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Object {
    generation: String,
    account: String,
    class: String,
    key: String,
    revision: u64,
    value: Option<Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Pointer {
    object: String,
    sha256: String,
    revision: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct PointerChange {
    class: String,
    key: String,
    pointer: Pointer,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Audit {
    generation: String,
    account: String,
    revision: u64,
    prior_sha256: Option<String>,
    changes: Vec<PointerChange>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Intent {
    before: Root,
    after: Root,
    audit: String,
    changes: Vec<PointerChange>,
}
#[derive(Clone, Debug)]
pub(super) struct Change {
    pub class: String,
    pub key: String,
    pub value: Option<Value>,
}
#[derive(Clone, Debug)]
pub(crate) struct KeyedAccountStore {
    path: PathBuf,
    generation: String,
    account: String,
}
impl KeyedAccountStore {
    /// Caller supplies an unused private directory under a frozen generation.
    pub(super) fn create(path: &Path, generation: &str, account: &str) -> Result<Self> {
        if generation.is_empty() || account.is_empty() {
            return Err(IndexError::Conflict("keyed generation or account empty"));
        }
        fs::create_dir(path)?;
        sync_dir(
            path.parent()
                .ok_or_else(|| corrupt("keyed parent absent"))?,
        )?;
        for class in ["objects", "pointers", "known", "history"] {
            fs::create_dir(path.join(class))?;
            sync_dir(&path.join(class))?;
        }
        let store = Self::open(path, generation, account)?;
        write(&store.path.join("root.json"), &store.empty_root(), false)?;
        sync_dir(path)?;
        Ok(store)
    }
    pub(super) fn open(path: &Path, generation: &str, account: &str) -> Result<Self> {
        let store = Self {
            path: path.to_owned(),
            generation: generation.into(),
            account: account.into(),
        };
        if store.path.join("root.json").exists() {
            store.root()?;
        }
        Ok(store)
    }
    fn empty_root(&self) -> Root {
        Root {
            generation: self.generation.clone(),
            account: self.account.clone(),
            revision: 0,
            pending_count: 0,
            audit_sha256: None,
        }
    }
    fn lock(&self) -> Result<File> {
        let result = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.path.join("lock"));
        opened(&result);
        let file = result?;
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
    fn root(&self) -> Result<Root> {
        let root: Root = read(&self.path.join("root.json"))?
            .ok_or(IndexError::RebuildRequired("keyed root absent"))?;
        if root.generation != self.generation || root.account != self.account {
            return Err(IndexError::RebuildRequired(
                "keyed generation/account differs",
            ));
        }
        Ok(root)
    }
    fn key_path(&self, class: &str, key: &str, directory: &str) -> Result<PathBuf> {
        Ok(self
            .path
            .join(directory)
            .join(format!("{}.json", keyed(&(class, key))?)))
    }
    fn known(&self, class: &str, key: &str) -> Result<Option<Known>> {
        let known: Option<Known> = read(&self.key_path(class, key, "known")?)?;
        if known.as_ref().is_some_and(|known| {
            known.generation != self.generation
                || known.account != self.account
                || known.class != class
                || known.key != key
        }) {
            return Err(corrupt("keyed known identity differs"));
        }
        Ok(known)
    }
    fn object(&self, change: &PointerChange) -> Result<Object> {
        let path = self
            .path
            .join("objects")
            .join(format!("{}.json", change.pointer.object));
        let object: Object = read(&path)?.ok_or_else(|| corrupt("keyed object absent"))?;
        if object.generation != self.generation
            || object.account != self.account
            || object.class != change.class
            || object.key != change.key
            || object.revision != change.pointer.revision
            || keyed(&object)? != change.pointer.sha256
        {
            return Err(corrupt("keyed object identity differs"));
        }
        Ok(object)
    }
    fn pointer(&self, class: &str, key: &str, root: &Root) -> Result<Option<Pointer>> {
        let pointer: Option<Pointer> = read(&self.key_path(class, key, "pointers")?)?;
        let known = self.known(class, key)?;
        if pointer.is_none()
            && known
                .as_ref()
                .is_some_and(|known| known.birth_revision <= root.revision)
        {
            return Err(corrupt("known keyed pointer absent"));
        }
        if pointer.is_some() && known.is_none() {
            return Err(corrupt("keyed pointer lacks known marker"));
        }
        if pointer
            .as_ref()
            .is_some_and(|pointer| pointer.revision > root.revision)
        {
            return Err(IndexError::RebuildRequired("keyed pointer ahead of root"));
        }
        Ok(pointer)
    }
    fn recover(&self) -> Result<Root> {
        let root = self.root()?;
        let intent: Option<Intent> = read(&self.path.join("intent.json"))?;
        let Some(intent) = intent else {
            return Ok(root);
        };
        if intent.before.generation != self.generation
            || intent.before.account != self.account
            || intent.after.generation != self.generation
            || intent.after.account != self.account
            || intent.after.revision
                != intent
                    .before
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| corrupt("keyed revision overflow"))?
        {
            return Err(corrupt("keyed intent identity differs"));
        }
        if root == intent.before {
            // Root rename was never durable. New immutable objects remain as
            // unreferenced audit evidence; no pointer has been changed.
        } else if root == intent.after {
            let audit: Audit = read(
                &self
                    .path
                    .join("history")
                    .join(format!("{}.json", intent.audit)),
            )?
            .ok_or_else(|| corrupt("keyed audit absent"))?;
            if keyed(&audit)? != intent.after.audit_sha256.clone().unwrap_or_default()
                || audit.generation != self.generation
                || audit.account != self.account
                || audit.revision != root.revision
                || audit.prior_sha256 != intent.before.audit_sha256
                || audit.changes != intent.changes
            {
                return Err(corrupt("keyed audit chain differs"));
            }
            for change in &intent.changes {
                self.object(change)?;
                if self.known(&change.class, &change.key)?.is_none() {
                    write(
                        &self.key_path(&change.class, &change.key, "known")?,
                        &Known {
                            generation: self.generation.clone(),
                            account: self.account.clone(),
                            class: change.class.clone(),
                            key: change.key.clone(),
                            birth_revision: root.revision,
                        },
                        false,
                    )?;
                }
                write(
                    &self.key_path(&change.class, &change.key, "pointers")?,
                    &change.pointer,
                    true,
                )?;
            }
        } else {
            return Err(IndexError::RebuildRequired("keyed root/intent CAS differs"));
        }
        fs::remove_file(self.path.join("intent.json"))?;
        sync_dir(&self.path)?;
        Ok(root)
    }
    pub(crate) fn summary(&self) -> Result<(u64, u64)> {
        let _lock = self.lock()?;
        let root = self.recover()?;
        Ok((root.revision, root.pending_count))
    }
    /// Full inventory is restricted to frozen service admission. Live reads
    /// use exact keys and never enumerate this directory.
    pub(crate) fn admission_keys(&self) -> Result<HashSet<(String, String)>> {
        let _lock = self.lock()?;
        let root = self.recover()?;
        let mut keys = HashSet::new();
        for entry in fs::read_dir(self.path.join("known"))? {
            count(|io| io.directory_entries += 1);
            let path = entry?.path();
            let known: Known =
                read(&path)?.ok_or_else(|| corrupt("keyed admission known marker absent"))?;
            if known.generation != self.generation
                || known.account != self.account
                || known.birth_revision > root.revision
                || path != self.key_path(&known.class, &known.key, "known")?
                || !keys.insert((known.class.clone(), known.key.clone()))
            {
                return Err(corrupt("keyed admission known marker differs"));
            }
            let pointer = self
                .pointer(&known.class, &known.key, &root)?
                .ok_or_else(|| corrupt("keyed admission pointer absent"))?;
            self.object(&PointerChange {
                class: known.class,
                key: known.key,
                pointer,
            })?;
        }
        Ok(keys)
    }
    pub(crate) fn get(&self, class: &str, key: &str) -> Result<Option<Value>> {
        let _lock = self.lock()?;
        let root = self.recover()?;
        self.get_unlocked(class, key, &root)
    }
    fn get_unlocked(&self, class: &str, key: &str, root: &Root) -> Result<Option<Value>> {
        let Some(pointer) = self.pointer(class, key, root)? else {
            return Ok(None);
        };
        Ok(self
            .object(&PointerChange {
                class: class.into(),
                key: key.into(),
                pointer,
            })?
            .value)
    }
    /// One account-locked compact read for a named physical source. The debt
    /// summary and source record share the same committed root revision.
    pub(crate) fn compact_source(&self, source: &str) -> Result<(u64, u64, Option<Value>)> {
        let _lock = self.lock()?;
        let root = self.recover()?;
        let value = self.get_unlocked("source", source, &root)?;
        Ok((root.revision, root.pending_count, value))
    }
    pub(super) fn commit(&self, expected_revision: u64, changes: Vec<Change>) -> Result<u64> {
        self.commit_inner(expected_revision, changes, None)
    }
    fn commit_inner(
        &self,
        expected_revision: u64,
        changes: Vec<Change>,
        stop: Option<CrashPoint>,
    ) -> Result<u64> {
        let _lock = self.lock()?;
        let before = self.recover()?;
        if before.revision != expected_revision {
            return Err(IndexError::Conflict("keyed account CAS mismatch"));
        }
        let next_revision = before
            .revision
            .checked_add(1)
            .ok_or(IndexError::Conflict("keyed revision overflow"))?;
        let mut pending_count = before.pending_count;
        let mut seen = HashSet::new();
        let mut pointers = Vec::new();
        for change in changes {
            if change.class.is_empty()
                || change.key.is_empty()
                || !seen.insert((change.class.clone(), change.key.clone()))
            {
                return Err(IndexError::Conflict("empty or duplicate keyed change"));
            }
            // Exact-key old state must be legible before replacement. A missing
            // known pointer cannot silently turn an old ID into a new ID.
            let old = self.pointer(&change.class, &change.key, &before)?;
            if change.class == "pending" {
                let old_present = old
                    .map(|pointer| {
                        self.object(&PointerChange {
                            class: change.class.clone(),
                            key: change.key.clone(),
                            pointer,
                        })
                        .map(|object| object.value.is_some())
                    })
                    .transpose()?
                    .unwrap_or(false);
                if !old_present && change.value.is_some() {
                    pending_count = pending_count
                        .checked_add(1)
                        .ok_or(IndexError::Conflict("keyed pending debt count overflow"))?;
                } else if old_present && change.value.is_none() {
                    pending_count = pending_count
                        .checked_sub(1)
                        .ok_or(IndexError::Conflict("keyed pending debt count underflow"))?;
                }
            }
            let object = Object {
                generation: self.generation.clone(),
                account: self.account.clone(),
                class: change.class.clone(),
                key: change.key.clone(),
                revision: next_revision,
                value: change.value,
            };
            let id = uuid::Uuid::new_v4().to_string();
            write(
                &self.path.join("objects").join(format!("{id}.json")),
                &object,
                false,
            )?;
            if stop == Some(CrashPoint::AfterObject) {
                return Err(IndexError::Conflict("simulated crash after keyed object"));
            }
            pointers.push(PointerChange {
                class: change.class,
                key: change.key,
                pointer: Pointer {
                    object: id,
                    sha256: keyed(&object)?,
                    revision: next_revision,
                },
            });
        }
        let audit = Audit {
            generation: self.generation.clone(),
            account: self.account.clone(),
            revision: next_revision,
            prior_sha256: before.audit_sha256.clone(),
            changes: pointers.clone(),
        };
        let audit_id = uuid::Uuid::new_v4().to_string();
        write(
            &self.path.join("history").join(format!("{audit_id}.json")),
            &audit,
            false,
        )?;
        let after = Root {
            revision: next_revision,
            pending_count,
            audit_sha256: Some(keyed(&audit)?),
            ..before.clone()
        };
        write(
            &self.path.join("intent.json"),
            &Intent {
                before,
                after: after.clone(),
                audit: audit_id,
                changes: pointers,
            },
            false,
        )?;
        if stop == Some(CrashPoint::BeforeRoot) || stop == Some(CrashPoint::AfterIntent) {
            return Err(IndexError::Conflict("simulated crash before keyed root"));
        }
        write(&self.path.join("root.json"), &after, true)?;
        if stop == Some(CrashPoint::AfterRoot) {
            return Err(IndexError::Conflict("simulated crash after keyed root"));
        }
        self.recover()?;
        Ok(next_revision)
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum CrashPoint {
    AfterObject,
    AfterIntent,
    BeforeRoot,
    AfterRoot,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(class: &str, key: &str, value: &str) -> Change {
        Change {
            class: class.into(),
            key: key.into(),
            value: Some(Value::String(value.into())),
        }
    }
    #[test]
    fn keyed_object_and_intent_crashes_do_not_publish_pending_debt() {
        for point in [CrashPoint::AfterObject, CrashPoint::AfterIntent] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("account");
            let store = KeyedAccountStore::create(&path, "generation", "physical").unwrap();
            assert!(
                store
                    .commit_inner(0, vec![change("pending", "abandoned", "K?")], Some(point))
                    .is_err()
            );
            let reopened = KeyedAccountStore::open(&path, "generation", "physical").unwrap();
            assert_eq!(reopened.summary().unwrap(), (0, 0));
            assert!(reopened.get("pending", "abandoned").unwrap().is_none());
            reopened
                .commit(0, vec![change("pending", "committed", "K?")])
                .unwrap();
            assert_eq!(reopened.summary().unwrap(), (1, 1));
            assert!(reopened.get("pending", "abandoned").unwrap().is_none());
        }
    }
    #[test]
    fn keyed_history_does_not_enter_compact_read_or_cas() {
        let temp = tempfile::tempdir().unwrap();
        let store =
            KeyedAccountStore::create(&temp.path().join("account"), "generation", "physical")
                .unwrap();
        store
            .commit(0, vec![change("source", "active", "newest-invalid-q")])
            .unwrap();
        let (_, read_one) = measured(|| store.compact_source("active").unwrap());
        let (_, write_one) = measured(|| {
            store
                .commit(1, vec![change("effect", "next0", "settled")])
                .unwrap()
        });
        let mut revision = 2;
        for n in 0..200 {
            revision = store
                .commit(
                    revision,
                    vec![
                        change("effect", &format!("settled-{n}"), "exact-K-Q-result"),
                        change("source", &format!("source-{n}"), "newest-invalid-q"),
                    ],
                )
                .unwrap();
        }
        let ((observed_revision, pending_count, source), read_many) =
            measured(|| store.compact_source("active").unwrap());
        let (_, write_many) = measured(|| {
            store
                .commit(revision, vec![change("effect", "next1", "settled")])
                .unwrap()
        });
        eprintln!("keyed compact read one/many: {read_one:?} / {read_many:?}");
        eprintln!("keyed writer CAS one/many: {write_one:?} / {write_many:?}");
        assert_eq!((observed_revision, pending_count), (revision, 0));
        assert_eq!(source, Some(Value::String("newest-invalid-q".into())));
        assert_eq!(read_one.open_attempts, read_many.open_attempts);
        assert_eq!(write_one.open_attempts, write_many.open_attempts);
        assert!(read_many.bytes_parsed <= read_one.bytes_parsed + 16);
        assert!(write_many.bytes_parsed <= write_one.bytes_parsed + 128);
        assert!(write_many.bytes_written <= write_one.bytes_written + 128);
        assert_eq!(read_many.directory_entries, 0);
        assert_eq!(write_many.directory_entries, 0);
    }

    #[test]
    fn keyed_crash_repair_and_exact_pending_debt() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("account");
        let store = KeyedAccountStore::create(&path, "generation", "physical").unwrap();
        assert!(
            store
                .commit_inner(
                    0,
                    vec![change("pending", "effect:a", "announced")],
                    Some(CrashPoint::BeforeRoot)
                )
                .is_err()
        );
        let reopened = KeyedAccountStore::open(&path, "generation", "physical").unwrap();
        assert_eq!(reopened.summary().unwrap(), (0, 0));
        assert_eq!(reopened.get("pending", "effect:a").unwrap(), None);
        reopened
            .commit(0, vec![change("source", "other", "older-Q")])
            .unwrap();
        assert_eq!(reopened.get("pending", "effect:a").unwrap(), None);
        reopened
            .commit(1, vec![change("pending", "effect:a", "announced")])
            .unwrap();
        assert!(
            reopened
                .commit_inner(
                    2,
                    vec![change("pending", "effect:a", "consumed-K")],
                    Some(CrashPoint::AfterRoot)
                )
                .is_err()
        );
        let reopened = KeyedAccountStore::open(&path, "generation", "physical").unwrap();
        assert_eq!(reopened.summary().unwrap(), (3, 1));
        assert_eq!(
            reopened.get("pending", "effect:a").unwrap(),
            Some(Value::String("consumed-K".into()))
        );
        assert!(
            reopened
                .commit_inner(
                    3,
                    vec![
                        Change {
                            class: "pending".into(),
                            key: "effect:a".into(),
                            value: None
                        },
                        change("source", "commands+environment", "newest-invalid-Q")
                    ],
                    Some(CrashPoint::AfterRoot)
                )
                .is_err()
        );
        let reopened = KeyedAccountStore::open(&path, "generation", "physical").unwrap();
        assert_eq!(reopened.summary().unwrap(), (4, 0));
        assert_eq!(reopened.get("pending", "effect:a").unwrap(), None);
        assert_eq!(
            reopened.get("source", "commands+environment").unwrap(),
            Some(Value::String("newest-invalid-Q".into()))
        );
        assert!(matches!(
            KeyedAccountStore::open(&path, "other-generation", "physical"),
            Err(IndexError::RebuildRequired(_))
        ));
        let pointer = reopened
            .key_path("source", "commands+environment", "pointers")
            .unwrap();
        fs::remove_file(pointer).unwrap();
        assert!(matches!(
            reopened.get("source", "commands+environment"),
            Err(IndexError::Corrupt(_))
        ));
    }

    #[test]
    fn keyed_pending_debt_has_no_account_cardinality_cap() {
        let temp = tempfile::tempdir().unwrap();
        let store =
            KeyedAccountStore::create(&temp.path().join("account"), "generation", "physical")
                .unwrap();
        let changes = (0..129)
            .map(|n| change("pending", &format!("effect:{n}"), "exact-announcement"))
            .collect();
        store.commit(0, changes).unwrap();
        assert_eq!(store.summary().unwrap(), (1, 129));
        assert_eq!(
            store.get("pending", "effect:128").unwrap(),
            Some(Value::String("exact-announcement".into()))
        );
        assert!(
            KeyedAccountStore::open(&temp.path().join("account"), "generation", "physical")
                .unwrap()
                .summary()
                .is_ok()
        );
    }
}
