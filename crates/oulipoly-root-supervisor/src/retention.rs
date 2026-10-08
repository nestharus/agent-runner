//! Retained Bash output: every byte an accepted Bash run writes is kept by
//! this root's owner, within stated bounds, so its requesting harness can
//! read it back in full after any inline prefix, and can record that it
//! accepted exactly those bytes.
//!
//! * While the owner relays a run's combined output, it also appends each
//!   byte, in order, to `output/<work>` in the root's private store (`0700`
//!   directory, `0600` file, owner-written only). Nothing a requester
//!   supplies names a path: a run is selected only by its work id within
//!   this root.
//! * Bounds: at most [`Limits::per_run`] bytes per run and
//!   [`Limits::per_root`] bytes for the whole root. Fresh-owner retention keeps
//!   a prefix of the stream; once a bound or a write failure stops it,
//!   nothing later is kept, and the loss is recorded with its reason and
//!   the received count. Kept bytes are never claimed as the whole output
//!   unless they are.
//! * Seal: when the stream ends (or is found ended by a later owner), the
//!   file is cut to the kept length and synced with its directory, its
//!   SHA-256 computed from the file itself, and one `bash_output` row
//!   committed (`complete` only when every received byte was kept, the
//!   stream closed and nothing was lost; otherwise `partial` with its
//!   losses). The run's `end` is reported only after the seal, with the
//!   sealed identity, with file persistence conditional on successful sync (a sync failure is
//!   explicitly partial, not a power-crash durability claim). Failed
//!   seals are recorded as `unsealed` loss when the store is writable;
//!   otherwise a successor revisits resolved work missing a seal.
//! * An owner that takes over a run an earlier owner was relaying appends
//!   to the earlier file. Bytes the earlier owner read but had not written
//!   may be missing at that offset: recorded as an `owner-changed` loss,
//!   never hidden. A run found ended with no seal is sealed with what the
//!   file holds, as `partial`.
//! * Identity: `rv1o:<root_id>:<work>:<bytes>:<sha256>`, the root, the
//!   Bash work (its one launch: a Bash run is never relaunched), the kept
//!   byte count and their hash.
//! * Retention lasts as long as the root's store. Nothing here prunes it;
//!   retiring the store removes it.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::store::{OutputRecord, Store};

/// Directory, inside the root's store, holding retained output.
pub(crate) const DIR: &str = "output";
/// Largest range one read request is answered with.
pub(crate) const MAX_READ: u64 = 256 * 1024;
/// Identity prefix (root v1 output).
pub(crate) const IDENTITY_PREFIX: &str = "rv1o";

/// Retention bounds.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    pub(crate) per_run: u64,
    pub(crate) per_root: u64,
}

/// The bounds this owner applies.
pub(crate) const LIMITS: Limits = Limits {
    per_run: 64 * 1024 * 1024,
    per_root: 512 * 1024 * 1024,
};

/// What this root has retained, against its bound.
pub(crate) struct Budget(Mutex<(Limits, u64)>);

impl Budget {
    pub(crate) fn new(limits: Limits, used: u64) -> Self {
        Self(Mutex::new((limits, used)))
    }

    #[cfg(test)]
    pub(crate) fn set_limits(&self, limits: Limits) {
        self.0.lock().expect("budget").0 = limits;
    }

    fn limits(&self) -> Limits {
        self.0.lock().expect("budget").0
    }

    /// Grants up to `want` more bytes for the root.
    fn grant(&self, want: u64) -> u64 {
        let mut budget = self.0.lock().expect("budget");
        let granted = want.min(budget.0.per_root.saturating_sub(budget.1));
        budget.1 += granted;
        granted
    }

    fn release(&self, bytes: u64) {
        let mut budget = self.0.lock().expect("budget");
        budget.1 = budget.1.saturating_sub(bytes);
    }
}

pub(crate) fn path(store_dir: &Path, work: i64) -> PathBuf {
    store_dir.join(DIR).join(work.to_string())
}

pub(crate) fn identity(root_id: &str, work: i64, bytes: u64, sha256: &str) -> String {
    format!("{IDENTITY_PREFIX}:{root_id}:{work}:{bytes}:{sha256}")
}

/// A sealed record as the requester sees it.
pub(crate) fn record_json(root_id: &str, work: i64, record: &OutputRecord) -> Value {
    json!({
        "state": record.state,
        "root_id": root_id,
        "work": work,
        "bytes": record.retained,
        "sha256": record.sha256,
        "received": record.received,
        "losses": serde_json::from_str::<Value>(&record.losses).unwrap_or(Value::Null),
        "identity": if record.state == "unsealed" { Value::Null } else { json!(identity(root_id, work, record.retained, &record.sha256)) },
    })
}

/// Keeps one run's output while it is relayed, then seals it.
pub(crate) struct Retainer<'a> {
    work: i64,
    path: PathBuf,
    file: Option<File>,
    /// Bytes no longer kept (a bound or a write failure stopped retention).
    stopped: bool,
    /// Bytes in the file: this owner's writes plus any an earlier owner left.
    retained: u64,
    /// Bytes this owner read from the stream; `None` once an earlier owner
    /// read part of it.
    received: Option<u64>,
    losses: Vec<Value>,
    budget: &'a Budget,
    /// An earlier owner already sealed this run: nothing more is kept.
    sealed: Option<OutputRecord>,
    unsealable: Option<String>,
}

impl<'a> Retainer<'a> {
    /// A fresh run: its file must not exist yet.
    pub(crate) fn start(store_dir: &Path, work: i64, budget: &'a Budget) -> Self {
        let path = path(store_dir, work);
        let mut retainer = Self::empty(work, path, budget);
        match open(&retainer.path, true) {
            Ok(file) => retainer.file = Some(file),
            Err(error) => {
                retainer.stop(json!({ "reason": "open-failed", "detail": error.to_string() }))
            }
        }
        retainer
    }

    /// A run an earlier owner accepted: continues its file, recording the
    /// possible gap at the takeover offset. `sealed` is that owner's seal,
    /// if it made one: then nothing more is kept.
    pub(crate) fn resume(
        store_dir: &Path,
        work: i64,
        budget: &'a Budget,
        sealed: Option<OutputRecord>,
    ) -> Self {
        let path = path(store_dir, work);
        let mut retainer = Self::empty(work, path, budget);
        if sealed.is_some() {
            retainer.sealed = sealed;
            return retainer;
        }
        retainer.received = None;
        match open(&retainer.path, false) {
            Ok(file) => {
                let at = match file.metadata() {
                    Ok(meta) => meta.len(),
                    Err(error) => {
                        retainer.unsealable =
                            Some(format!("stat-failed: {error}; kept length unknown"));
                        return retainer;
                    }
                };
                retainer.retained = at;
                retainer.file = Some(file);
                retainer.losses.push(json!({
                    "reason": "owner-changed",
                    "at": at,
                    "meaning": "output an earlier owner read but had not kept may be missing here",
                }));
            }
            Err(error) => {
                retainer.stop(json!({ "reason": "open-failed", "detail": error.to_string() }));
            }
        }
        retainer
    }

    fn empty(work: i64, path: PathBuf, budget: &'a Budget) -> Self {
        Self {
            work,
            path,
            file: None,
            stopped: false,
            retained: 0,
            received: Some(0),
            losses: Vec::new(),
            budget,
            sealed: None,
            unsealable: None,
        }
    }

    pub(crate) fn deny_unknown_budget(&mut self) {
        self.stop(json!({ "reason": "budget-unknown", "at": self.retained }));
    }

    fn stop(&mut self, loss: Value) {
        if !self.stopped {
            self.stopped = true;
            self.losses.push(loss);
        }
    }

    /// Keeps what the bounds allow of the next relayed chunk.
    pub(crate) fn take(&mut self, bytes: &[u8]) {
        if self.sealed.is_some() {
            return;
        }
        let len = bytes.len() as u64;
        if let Some(received) = self.received.as_mut() {
            *received += len;
        }
        if self.stopped {
            return;
        }
        let Some(file) = self.file.as_mut() else {
            return;
        };
        let limits = self.budget.limits();
        let run_room = limits.per_run.saturating_sub(self.retained);
        let want = len.min(run_room);
        let granted = self.budget.grant(want);
        let keep = usize::try_from(granted).unwrap_or(usize::MAX);
        let mut written = 0usize;
        let mut failure = None;
        while written < keep {
            match file.write(&bytes[written..keep]) {
                Ok(0) => {
                    failure = Some("write returned zero".to_owned());
                    break;
                }
                Ok(n) => written += n,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => {
                    failure = Some(error.to_string());
                    break;
                }
            }
        }
        self.retained += written as u64;
        self.budget.release(granted - written as u64);
        if let Some(detail) = failure {
            self.stop(json!({ "reason": "write-failed", "detail": detail, "at": self.retained }));
        } else if granted < len {
            let reason = if want < len {
                "per-run-bound"
            } else {
                "per-root-bound"
            };
            let bound = if want < len {
                limits.per_run
            } else {
                limits.per_root
            };
            self.stop(json!({ "reason": reason, "bound": bound, "at": self.retained }));
        }
    }

    /// Seals what was kept (see the module docs) and returns the identity
    /// the requester is told, or why there is none (`unsealed`). `stream`
    /// is the relay's own output state.
    pub(crate) fn seal(self, stream: &Value, store: &Mutex<Store>, root_id: &str) -> Value {
        self.seal_recorded(stream, store, root_id).0
    }

    /// Whether this run's retention began with an earlier owner.
    pub(crate) fn taken_over(&self) -> bool {
        self.received.is_none() || self.sealed.is_some()
    }

    /// [`Self::seal`], and whether a seal record (any state) is in the store.
    pub(crate) fn seal_recorded(
        mut self,
        stream: &Value,
        store: &Mutex<Store>,
        root_id: &str,
    ) -> (Value, bool) {
        if let Some(record) = self.sealed.take() {
            return (record_json(root_id, self.work, &record), true);
        }
        if let Some(reason) = self.unsealable.take() {
            return self.failed_seal(&reason, store, root_id);
        }
        if stream["state"] != "closed" {
            self.losses.push(json!({
                "reason": "stream-not-closed",
                "state": stream["state"],
                "detail": stream["reason"],
            }));
        }
        if let Some(received) = self.received
            && received > self.retained
            && !self.stopped
        {
            // Every loss above stops retention; this is a guard, not a path.
            self.losses
                .push(json!({ "reason": "not-kept", "at": self.retained }));
        }
        let file = match self.file.take() {
            Some(file) => file,
            None => match open(&self.path, false) {
                Ok(file) => file,
                Err(error) => {
                    return self.failed_seal(&format!("open-failed: {error}"), store, root_id);
                }
            },
        };
        let durable = file
            .set_len(self.retained)
            .and_then(|()| file.sync_all())
            .and_then(|()| File::open(self.path.parent().expect("output dir"))?.sync_all());
        if let Err(error) = durable {
            self.losses
                .push(json!({ "reason": "sync-failed", "detail": error.to_string() }));
        }
        let sha256 = match hash_prefix(&self.path, self.retained) {
            Ok(sha256) => sha256,
            Err(error) => {
                return self.failed_seal(&format!("hash-failed: {error}"), store, root_id);
            }
        };
        let record = OutputRecord {
            received: self.received,
            retained: self.retained,
            sha256,
            state: if self.losses.is_empty() {
                "complete".to_owned()
            } else {
                "partial".to_owned()
            },
            losses: Value::Array(self.losses).to_string(),
        };
        match store
            .lock()
            .expect("store lock")
            .seal_output(self.work, &record)
        {
            Ok(()) => (record_json(root_id, self.work, &record), true),
            Err(error) => (unsealed(error.label()), false),
        }
    }
    fn failed_seal(mut self, reason: &str, store: &Mutex<Store>, root_id: &str) -> (Value, bool) {
        self.losses
            .push(json!({ "reason": "seal-failed", "detail": reason }));
        let record = OutputRecord {
            received: self.received,
            retained: self.retained,
            sha256: String::new(),
            state: "unsealed".into(),
            losses: Value::Array(self.losses).to_string(),
        };
        match store
            .lock()
            .expect("store lock")
            .seal_output(self.work, &record)
        {
            Ok(()) => (record_json(root_id, self.work, &record), true),
            Err(error) => (unsealed(error.label()), false),
        }
    }
}

/// Charge all physical retained files before any new/recovered relay grants.
/// Failure denies fresh storage, rather than silently assuming an empty root.
pub(crate) fn initial_usage(store_dir: &Path) -> io::Result<u64> {
    let entries = match fs::read_dir(store_dir.join(DIR)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let mut used = 0u64;
    for entry in entries {
        let entry = entry?;
        let meta = fs::symlink_metadata(entry.path())?;
        if !meta.is_file() || entry.file_name().to_string_lossy().parse::<i64>().is_err() {
            return Err(io::Error::other("unexpected retained storage entry"));
        }
        used = used
            .checked_add(meta.len())
            .ok_or_else(|| io::Error::other("retained budget overflow"))?;
    }
    Ok(used)
}

fn unsealed(reason: &str) -> Value {
    json!({ "state": "unsealed", "reason": reason })
}

/// Opens (creating its private directory) a run's retention file for
/// appending; `fresh` requires that it not exist yet.
fn open(path: &Path, fresh: bool) -> io::Result<File> {
    let dir = path.parent().expect("output dir");
    match fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {
            if let Some(store) = dir.parent() {
                File::open(store)?.sync_all()?;
            }
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let mut options = OpenOptions::new();
    options
        .read(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW);
    if fresh {
        options.create_new(true);
    } else {
        options.create(true);
    }
    options.open(path)
}

/// SHA-256 of the first `len` bytes of `path`; fails if it holds fewer.
pub(crate) fn hash_prefix(path: &Path, len: u64) -> io::Result<String> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let mut hasher = Sha256::new();
    let mut left = len;
    let mut buf = vec![0u8; 64 * 1024];
    while left > 0 {
        let want = usize::try_from(left.min(buf.len() as u64)).unwrap_or(buf.len());
        let read = file.read(&mut buf[..want])?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "retained bytes missing",
            ));
        }
        hasher.update(&buf[..read]);
        left -= read as u64;
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Reads `[offset, offset + length)` of a sealed run, clipped to what was
/// retained and to [`MAX_READ`]. Fails if the file no longer holds the
/// retained bytes there.
pub(crate) fn read_range(
    store_dir: &Path,
    work: i64,
    retained: u64,
    sha256: &str,
    offset: u64,
    length: u64,
) -> io::Result<Vec<u8>> {
    let take = length.min(MAX_READ).min(retained.saturating_sub(offset));
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path(store_dir, work))?;
    if file.metadata()?.len() < retained {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "retained length changed",
        ));
    }
    // Capture the requested slice during the same read whose full hash is
    // verified. A separate seek/read after hashing could return changed bytes.
    let mut hasher = Sha256::new();
    let mut out = Vec::with_capacity(take as usize);
    let mut at = 0u64;
    let mut buf = [0u8; 64 * 1024];
    while at < retained {
        let want = (retained - at).min(buf.len() as u64) as usize;
        file.read_exact(&mut buf[..want])?;
        hasher.update(&buf[..want]);
        let start = offset.max(at);
        let end = (offset + take).min(at + want as u64);
        if start < end {
            out.extend_from_slice(&buf[(start - at) as usize..(end - at) as usize]);
        }
        at += want as u64;
    }
    let actual = format!("{:x}", hasher.finalize());
    if actual != sha256 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "retained bytes changed",
        ));
    }
    Ok(out)
}
