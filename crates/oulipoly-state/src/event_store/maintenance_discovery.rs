//! Sharded, non-authoritative discovery journals for detached maintenance.
//!
//! Every generation owns one small checksummed journal beneath a fixed-fanout
//! identity trie. There is no shared database, append log, or mutable catalog
//! on the producer publication path. Manifests, heads, generation leases,
//! AGE-372 approval, and retirement receipts remain the only effect authority.

use super::{Digest32, GenerationId, NativeProcessIdentity, PreparedManifest, WriterInstanceId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

const FORMAT_VERSION: u32 = 1;
const DIRECTORY: &str = "maintenance-discovery-v1";
const ACTIVE: &str = "active";
const PENDING: &str = "pending";
const ARCHIVE: &str = "archive";
const IDENTITY_BYTES: usize = 32;
const MAX_DIRECTORY_FANOUT: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DiscoveryClass {
    Active,
    Pending,
    Archive,
}

impl DiscoveryClass {
    fn directory(self) -> &'static str {
        match self {
            Self::Active => ACTIVE,
            Self::Pending => PENDING,
            Self::Archive => ARCHIVE,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DiscoveryPhase {
    Intent,
    Staging,
    Publishing,
    Prepared,
    Selected,
    Closed,
    Retiring,
    PendingTrash,
    OrphanPending,
    PreservedDeadHead,
    Retired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DiscoveryRecord {
    format_version: u32,
    sequence: u64,
    pub(crate) writer: WriterInstanceId,
    pub(crate) generation: GenerationId,
    pub(crate) phase: DiscoveryPhase,
    /// Exact basename below the writer's staging directory. It is published
    /// before that directory may be created.
    pub(crate) staging_name: Option<String>,
    pub(crate) prepared_manifest_sha256: Option<Digest32>,
    pub(crate) producer_native_process: Option<NativeProcessIdentity>,
    checksum_sha256: String,
}

impl DiscoveryRecord {
    fn from_manifest(
        manifest: &PreparedManifest,
        staging_name: Option<String>,
    ) -> Result<Self, String> {
        let mut record = Self {
            format_version: FORMAT_VERSION,
            sequence: 0,
            writer: manifest.writer_instance_id,
            generation: manifest.generation_id,
            phase: DiscoveryPhase::Intent,
            staging_name,
            prepared_manifest_sha256: Some(manifest.sha256().map_err(|error| error.to_string())?),
            producer_native_process: manifest.producer.native_process.clone(),
            checksum_sha256: String::new(),
        };
        record.checksum_sha256 = record.expected_checksum()?;
        record.validate()?;
        Ok(record)
    }

    fn legacy(
        writer: WriterInstanceId,
        generation: GenerationId,
        phase: DiscoveryPhase,
        staging_name: Option<String>,
        manifest: Option<&PreparedManifest>,
    ) -> Result<Self, String> {
        let mut record = Self {
            format_version: FORMAT_VERSION,
            sequence: 0,
            writer,
            generation,
            phase,
            staging_name,
            prepared_manifest_sha256: manifest
                .map(PreparedManifest::sha256)
                .transpose()
                .map_err(|error| error.to_string())?,
            producer_native_process: manifest
                .and_then(|value| value.producer.native_process.clone()),
            checksum_sha256: String::new(),
        };
        record.checksum_sha256 = record.expected_checksum()?;
        record.validate()?;
        Ok(record)
    }

    fn validate(&self) -> Result<(), String> {
        if self.format_version != FORMAT_VERSION
            || self.writer.is_nil()
            || self.generation.is_nil()
            || self.staging_name.as_ref().is_some_and(|name| {
                name.is_empty()
                    || name.len() > 128
                    || name.contains('/')
                    || name.contains('\\')
                    || name == "."
                    || name == ".."
            })
            || self.expected_checksum()? != self.checksum_sha256
        {
            return Err("invalid maintenance discovery record".to_string());
        }
        if let Some(native) = &self.producer_native_process {
            native.validate().map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    fn expected_checksum(&self) -> Result<String, String> {
        #[derive(Serialize)]
        struct Body<'a> {
            format_version: u32,
            sequence: u64,
            writer: WriterInstanceId,
            generation: GenerationId,
            phase: DiscoveryPhase,
            staging_name: &'a Option<String>,
            prepared_manifest_sha256: Option<Digest32>,
            producer_native_process: &'a Option<NativeProcessIdentity>,
        }
        let bytes = serde_json::to_vec(&Body {
            format_version: self.format_version,
            sequence: self.sequence,
            writer: self.writer,
            generation: self.generation,
            phase: self.phase,
            staging_name: &self.staging_name,
            prepared_manifest_sha256: self.prepared_manifest_sha256,
            producer_native_process: &self.producer_native_process,
        })
        .map_err(|error| error.to_string())?;
        let mut digest = Sha256::new();
        digest.update(b"oulipoly.maintenance-discovery.v1\0");
        digest.update(bytes);
        Ok(hex(&digest.finalize()))
    }

    fn canonical_bytes(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let mut bytes = serde_json::to_vec(self).map_err(|error| error.to_string())?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    /// Stable identity of the publication operation. Sequence and derived
    /// phase are deliberately excluded so active/pending/archive retries bind
    /// to the same immutable producer intent.
    pub(crate) fn operation_sha256(&self) -> Result<Digest32, String> {
        #[derive(Serialize)]
        struct Operation<'a> {
            writer: WriterInstanceId,
            generation: GenerationId,
            staging_name: &'a Option<String>,
            prepared_manifest_sha256: Option<Digest32>,
            producer_native_process: &'a Option<NativeProcessIdentity>,
        }
        let bytes = serde_json::to_vec(&Operation {
            writer: self.writer,
            generation: self.generation,
            staging_name: &self.staging_name,
            prepared_manifest_sha256: self.prepared_manifest_sha256,
            producer_native_process: &self.producer_native_process,
        })
        .map_err(|error| error.to_string())?;
        let mut digest = Sha256::new();
        digest.update(b"oulipoly.maintenance-discovery-operation.v1\0");
        digest.update(bytes);
        Ok(Digest32::from_bytes(digest.finalize().into()))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DiscoveryCursor {
    /// Hex path of the last trie node visited, not merely the last valid leaf.
    /// This lets corrupt/incomplete leaf construction advance under a raw-node
    /// budget without rescanning an unbounded prefix.
    pending_after_node: Option<String>,
    active_after_node: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiscoveryEntry {
    pub(crate) writer: WriterInstanceId,
    pub(crate) generation: GenerationId,
    pub(crate) class: DiscoveryClass,
    pub(crate) record: Option<DiscoveryRecord>,
    pub(crate) issue: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiscoveryBatch {
    pub(crate) entries: Vec<DiscoveryEntry>,
    pub(crate) issues: Vec<String>,
    pub(crate) cursor: DiscoveryCursor,
    pub(crate) more: bool,
    pub(crate) nodes_examined: usize,
    pub(crate) entries_examined: usize,
}

/// Publish the exact scheduling intent before any staging directory is created.
/// Failure is safe to return because no generation-side effect has begun.
pub(crate) fn publish_generation_intent(
    root: &Path,
    manifest: &PreparedManifest,
    staging_name: &str,
) -> Result<(), String> {
    let record = DiscoveryRecord::from_manifest(manifest, Some(staging_name.to_string()))?;
    let leaf = ensure_leaf(
        root,
        DiscoveryClass::Active,
        manifest.writer_instance_id,
        manifest.generation_id,
    )?;
    let result = match read_selected_record(&leaf)? {
        Some(existing)
            if existing.writer == record.writer
                && existing.generation == record.generation
                && existing.staging_name == record.staging_name
                && existing.prepared_manifest_sha256 == record.prepared_manifest_sha256 =>
        {
            Ok(())
        }
        Some(_) => Err("maintenance discovery intent identity conflict".to_string()),
        None => {
            write_record(&leaf, None, &record)?;
            Ok(())
        }
    };
    #[cfg(test)]
    if result.is_ok() {
        run_generation_intent_hook(root, record.writer, record.generation);
    }
    result
}

#[cfg(test)]
type GenerationIntentHook =
    std::sync::Arc<dyn Fn(&Path, WriterInstanceId, GenerationId) + Send + Sync>;

#[cfg(test)]
fn generation_intent_hook() -> &'static std::sync::Mutex<Option<GenerationIntentHook>> {
    static HOOK: std::sync::OnceLock<std::sync::Mutex<Option<GenerationIntentHook>>> =
        std::sync::OnceLock::new();
    HOOK.get_or_init(|| std::sync::Mutex::new(None))
}

#[cfg(test)]
fn run_generation_intent_hook(root: &Path, writer: WriterInstanceId, generation: GenerationId) {
    let hook = generation_intent_hook().lock().unwrap().clone();
    if let Some(hook) = hook {
        hook(root, writer, generation);
    }
}

#[cfg(test)]
pub(crate) fn with_generation_intent_hook<T>(
    hook: GenerationIntentHook,
    operation: impl FnOnce() -> T,
) -> T {
    *generation_intent_hook().lock().unwrap() = Some(hook);
    let result = operation();
    *generation_intent_hook().lock().unwrap() = None;
    result
}

/// Register a pre-boundary exact path found by the detached migration cursor.
/// This is scheduling evidence only and never changes generation authority.
pub(crate) fn register_legacy_generation(
    root: &Path,
    writer: WriterInstanceId,
    generation: GenerationId,
    phase: DiscoveryPhase,
    staging_name: Option<String>,
    manifest: Option<&PreparedManifest>,
) -> Result<(), String> {
    if let Some((_class, leaf)) = find_leaf(root, writer, generation) {
        if read_selected_record(&leaf)?.is_some() {
            return Ok(());
        }
        let record = DiscoveryRecord::legacy(writer, generation, phase, staging_name, manifest)?;
        write_record(&leaf, selected_slot(&leaf)?, &record)?;
        return Ok(());
    }
    let record = DiscoveryRecord::legacy(writer, generation, phase, staging_name, manifest)?;
    let leaf = ensure_leaf(root, DiscoveryClass::Active, writer, generation)?;
    if read_selected_record(&leaf)?.is_none() {
        write_record(&leaf, None, &record)?;
    }
    Ok(())
}

/// Archive a fully identified leaf that never published a valid initial slot
/// and has no generation/staging/pending artifact. The trie identity is enough
/// to discharge this derived-only pre-effect crash residue; valid journals
/// must use the ordinary phase/class transition protocol.
pub(crate) fn archive_invalid_absent_leaf(
    root: &Path,
    class: DiscoveryClass,
    writer: WriterInstanceId,
    generation: GenerationId,
) -> Result<(), String> {
    let source = leaf_path(root, class, writer, generation);
    if read_selected_record(&source)?.is_some() {
        return Err("valid discovery leaf requires an ordinary class transition".to_string());
    }
    if class == DiscoveryClass::Archive {
        return Ok(());
    }
    let destination = ensure_leaf_parent(root, DiscoveryClass::Archive, writer, generation)?;
    if destination.exists() {
        return Err("invalid discovery archive destination already exists".to_string());
    }
    fs::rename(&source, &destination).map_err(|error| error.to_string())?;
    sync_dir(source.parent().expect("discovery source has parent"))?;
    sync_dir(
        destination
            .parent()
            .expect("discovery destination has parent"),
    )
}

/// Advance one exact journal in place. Once intent publication has succeeded,
/// callers may treat a later update failure as an evidence gap: the preceding
/// record still names both exact identity and staging/final recovery paths.
pub(crate) fn record_generation_phase(
    root: &Path,
    writer: WriterInstanceId,
    generation: GenerationId,
    phase: DiscoveryPhase,
) -> Result<(), String> {
    let (_, leaf) = find_leaf(root, writer, generation)
        .ok_or_else(|| "maintenance discovery record is absent".to_string())?;
    let mut record = read_selected_record(&leaf)?
        .ok_or_else(|| "maintenance discovery record has no valid slot".to_string())?;
    record.sequence = record.sequence.saturating_add(1);
    record.phase = phase;
    record.checksum_sha256 = record.expected_checksum()?;
    let active = selected_slot(&leaf)?;
    write_record(&leaf, active, &record)?;
    Ok(())
}

/// Move an exact journal between active, pending, and archive tries. The phase
/// update is durable before the directory move; either location is therefore a
/// complete recovery source at every crash boundary.
pub(crate) fn move_generation_class(
    root: &Path,
    writer: WriterInstanceId,
    generation: GenerationId,
    from: DiscoveryClass,
    to: DiscoveryClass,
    phase: DiscoveryPhase,
) -> Result<(), String> {
    let source = leaf_path(root, from, writer, generation);
    let destination = leaf_path(root, to, writer, generation);
    if !source.is_dir() {
        if destination.is_dir() {
            record_generation_phase(root, writer, generation, phase)?;
            return Ok(());
        }
        return Err("maintenance discovery source and destination are absent".to_string());
    }
    record_generation_phase(root, writer, generation, phase)?;
    ensure_leaf_parent(root, to, writer, generation)?;
    match fs::rename(&source, &destination) {
        Ok(()) => {
            sync_dir(source.parent().expect("discovery leaf has parent"))?;
            sync_dir(destination.parent().expect("discovery leaf has parent"))
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let source_record = read_selected_record(&source)?;
            let destination_record = read_selected_record(&destination)?;
            if source_record == destination_record && source_record.is_some() {
                Ok(())
            } else {
                Err("maintenance discovery class publication conflict".to_string())
            }
        }
        Err(error) => Err(format!("move maintenance discovery class: {error}")),
    }
}

/// A formerly selected generation can have a derived archived marker while
/// its authoritative head is still live. Once it is authoritatively closed,
/// restore it to the active closed-work class before scheduling historical
/// work.
pub(crate) fn record_closed_generation(
    root: &Path,
    writer: WriterInstanceId,
    generation: GenerationId,
) -> Result<(), String> {
    let Some((class, _)) = find_leaf(root, writer, generation) else {
        return Err("maintenance discovery record is absent".to_string());
    };
    if class == DiscoveryClass::Archive {
        move_generation_class(
            root,
            writer,
            generation,
            DiscoveryClass::Archive,
            DiscoveryClass::Active,
            DiscoveryPhase::Closed,
        )
    } else {
        record_generation_phase(root, writer, generation, DiscoveryPhase::Closed)
    }
}

pub(crate) fn read_exact_record(
    root: &Path,
    writer: WriterInstanceId,
    generation: GenerationId,
) -> Result<Option<(DiscoveryClass, DiscoveryRecord)>, String> {
    let Some((class, leaf)) = find_leaf(root, writer, generation) else {
        return Ok(None);
    };
    Ok(read_selected_record(&leaf)?.map(|record| (class, record)))
}

/// Read a bounded number of trie nodes and exact leaves. Pending recovery is
/// always served before active work, with independent durable cursors.
pub(crate) fn read_batch(
    root: &Path,
    previous: &DiscoveryCursor,
    max_nodes: usize,
    max_entries: usize,
) -> Result<DiscoveryBatch, String> {
    if max_nodes == 0 || max_entries == 0 {
        return Err("maintenance discovery limits must be non-zero".to_string());
    }
    let mut cursor = previous.clone();
    let mut entries = Vec::new();
    let mut issues = Vec::new();
    let mut remaining_nodes = max_nodes;

    let pending = walk_class(
        root,
        DiscoveryClass::Pending,
        cursor.pending_after_node.as_deref(),
        remaining_nodes,
        max_entries,
    )?;
    remaining_nodes = remaining_nodes.saturating_sub(pending.nodes_examined);
    entries.extend(pending.entries);
    issues.extend(pending.issues);
    cursor.pending_after_node = pending.last_node;
    if pending.more || entries.len() == max_entries {
        return Ok(DiscoveryBatch {
            entries_examined: entries.len(),
            entries,
            issues,
            cursor,
            more: true,
            nodes_examined: max_nodes - remaining_nodes,
        });
    }
    cursor.pending_after_node = None;
    if remaining_nodes == 0 {
        return Ok(DiscoveryBatch {
            entries_examined: entries.len(),
            entries,
            issues,
            cursor,
            more: true,
            nodes_examined: max_nodes,
        });
    }

    let active = walk_class(
        root,
        DiscoveryClass::Active,
        cursor.active_after_node.as_deref(),
        remaining_nodes,
        max_entries - entries.len(),
    )?;
    remaining_nodes = remaining_nodes.saturating_sub(active.nodes_examined);
    entries.extend(active.entries);
    issues.extend(active.issues);
    cursor.active_after_node = active.last_node;
    if !active.more {
        cursor.active_after_node = None;
    }
    Ok(DiscoveryBatch {
        entries_examined: entries.len(),
        entries,
        issues,
        cursor,
        more: active.more,
        nodes_examined: max_nodes - remaining_nodes,
    })
}

/// Read the ordinary maintenance candidates plus archived selected heads whose
/// dead producers were preserved fail-closed. Archive traversal shares the
/// caller's node and entry budgets. If unrelated terminal archive records use
/// that budget before every preserved head is examined, `more` remains true so
/// evidence readers report incomplete coverage instead of complete emptiness.
///
/// This is deliberately separate from [`read_batch`]: detached maintenance
/// must not reacquire terminal archived work, while bounded evidence discovery
/// must continue to account for preserved selected heads.
pub(crate) fn read_evidence_batch(
    root: &Path,
    max_nodes: usize,
    max_entries: usize,
) -> Result<DiscoveryBatch, String> {
    let mut batch = read_batch(root, &DiscoveryCursor::default(), max_nodes, max_entries)?;
    if batch.more {
        return Ok(batch);
    }

    let remaining_nodes = max_nodes.saturating_sub(batch.nodes_examined);
    let remaining_entries = max_entries.saturating_sub(batch.entries_examined);
    if remaining_nodes == 0 || remaining_entries == 0 {
        batch.more = true;
        return Ok(batch);
    }

    let archive = walk_class(
        root,
        DiscoveryClass::Archive,
        None,
        remaining_nodes,
        remaining_entries,
    )?;
    batch.nodes_examined = batch.nodes_examined.saturating_add(archive.nodes_examined);
    batch.entries_examined = batch.entries_examined.saturating_add(archive.entries.len());
    batch.issues.extend(archive.issues);
    batch
        .entries
        .extend(archive.entries.into_iter().filter(|entry| {
            entry.issue.is_some()
                || entry
                    .record
                    .as_ref()
                    .is_none_or(|record| record.phase == DiscoveryPhase::PreservedDeadHead)
        }));
    batch.more = archive.more;
    Ok(batch)
}

struct WalkBatch {
    entries: Vec<DiscoveryEntry>,
    issues: Vec<String>,
    last_node: Option<String>,
    more: bool,
    nodes_examined: usize,
}

struct Walker {
    class: DiscoveryClass,
    after: Option<Vec<u8>>,
    max_nodes: usize,
    max_entries: usize,
    nodes_examined: usize,
    entries: Vec<DiscoveryEntry>,
    issues: Vec<String>,
    last_node: Option<Vec<u8>>,
    stopped: bool,
}

fn walk_class(
    root: &Path,
    class: DiscoveryClass,
    after: Option<&str>,
    max_nodes: usize,
    max_entries: usize,
) -> Result<WalkBatch, String> {
    let after = after.map(decode_node).transpose()?;
    let class_root = discovery_root(root).join(class.directory());
    if !class_root.is_dir() {
        return Ok(WalkBatch {
            entries: Vec::new(),
            issues: Vec::new(),
            last_node: None,
            more: false,
            nodes_examined: 0,
        });
    }
    let mut walker = Walker {
        class,
        after,
        max_nodes,
        max_entries,
        nodes_examined: 0,
        entries: Vec::new(),
        issues: Vec::new(),
        last_node: None,
        stopped: false,
    };
    let mut prefix = Vec::with_capacity(IDENTITY_BYTES);
    walk_directory(&class_root, &mut prefix, &mut walker)?;
    Ok(WalkBatch {
        entries: walker.entries,
        issues: walker.issues,
        last_node: walker.last_node.map(|value| hex(&value)),
        more: walker.stopped,
        nodes_examined: walker.nodes_examined,
    })
}

fn walk_directory(
    directory: &Path,
    prefix: &mut Vec<u8>,
    walker: &mut Walker,
) -> Result<(), String> {
    if walker.stopped {
        return Ok(());
    }
    if prefix.len() == IDENTITY_BYTES {
        if walker
            .after
            .as_ref()
            .is_some_and(|after| after.as_slice() == prefix.as_slice())
        {
            return Ok(());
        }
        let writer = WriterInstanceId::from_bytes(
            prefix[..16]
                .try_into()
                .expect("fixed discovery writer identity"),
        );
        let generation = GenerationId::from_bytes(
            prefix[16..]
                .try_into()
                .expect("fixed discovery generation identity"),
        );
        let record = read_selected_record(directory);
        let (record, issue) = match record {
            Ok(Some(record)) if record.writer == writer && record.generation == generation => {
                (Some(record), None)
            }
            Ok(Some(_)) => (
                None,
                Some("discovery leaf record identity mismatch".to_string()),
            ),
            Ok(None) => (
                None,
                Some("discovery leaf has no valid state slot".to_string()),
            ),
            Err(error) => (None, Some(error)),
        };
        walker.entries.push(DiscoveryEntry {
            writer,
            generation,
            class: walker.class,
            record,
            issue,
        });
        if walker.entries.len() == walker.max_entries {
            walker.stopped = true;
        }
        return Ok(());
    }

    let mut children = Vec::new();
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("read maintenance discovery trie: {error}"))?;
    let mut raw_entries = 0usize;
    for entry in entries {
        raw_entries += 1;
        if raw_entries > MAX_DIRECTORY_FANOUT {
            walker.issues.push(format!(
                "discovery trie fanout exceeds 256 below {}",
                hex(prefix)
            ));
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                walker
                    .issues
                    .push(format!("read maintenance discovery entry: {error}"));
                continue;
            }
        };
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "non-UTF8 maintenance discovery component".to_string())?;
        let value = match decode_component(&name) {
            Ok(value) => value,
            Err(error) => {
                walker.issues.push(format!(
                    "invalid discovery component below {}: {error}",
                    hex(prefix)
                ));
                continue;
            }
        };
        let is_directory = match entry.file_type() {
            Ok(file_type) => file_type.is_dir(),
            Err(error) => {
                walker
                    .issues
                    .push(format!("inspect maintenance discovery entry: {error}"));
                continue;
            }
        };
        children.push((value, entry.path(), is_directory));
    }
    children.sort_by_key(|(value, _, _)| *value);
    for (value, path, is_directory) in children {
        let relation = relation_to_after(prefix, value, walker.after.as_deref());
        if relation == NodeRelation::Before {
            continue;
        }
        prefix.push(value);
        if relation == NodeRelation::Fresh {
            if walker.nodes_examined == walker.max_nodes {
                walker.stopped = true;
                prefix.pop();
                break;
            }
            walker.nodes_examined += 1;
            walker.last_node = Some(prefix.clone());
            if walker.nodes_examined == walker.max_nodes && prefix.len() < IDENTITY_BYTES {
                walker.stopped = true;
                prefix.pop();
                break;
            }
        }
        if is_directory {
            if let Err(error) = walk_directory(&path, prefix, walker) {
                walker.issues.push(format!(
                    "unreadable discovery subtree at {}: {error}",
                    hex(prefix)
                ));
            }
        } else {
            walker.issues.push(format!(
                "discovery trie node is not a directory at {}",
                hex(prefix)
            ));
        }
        prefix.pop();
        if walker.stopped {
            break;
        }
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NodeRelation {
    Before,
    OnCursor,
    Fresh,
}

fn relation_to_after(prefix: &[u8], child: u8, after: Option<&[u8]>) -> NodeRelation {
    let Some(after) = after else {
        return NodeRelation::Fresh;
    };
    let depth = prefix.len();
    if depth < after.len() && prefix == &after[..depth] {
        return match child.cmp(&after[depth]) {
            std::cmp::Ordering::Less => NodeRelation::Before,
            std::cmp::Ordering::Equal => NodeRelation::OnCursor,
            std::cmp::Ordering::Greater => NodeRelation::Fresh,
        };
    }
    match prefix.cmp(&after[..prefix.len().min(after.len())]) {
        std::cmp::Ordering::Less => NodeRelation::Before,
        _ => NodeRelation::Fresh,
    }
}

fn discovery_root(root: &Path) -> PathBuf {
    root.join(DIRECTORY)
}

fn identity_components(writer: WriterInstanceId, generation: GenerationId) -> [u8; IDENTITY_BYTES] {
    let mut value = [0u8; IDENTITY_BYTES];
    value[..16].copy_from_slice(writer.as_bytes());
    value[16..].copy_from_slice(generation.as_bytes());
    value
}

fn leaf_path(
    root: &Path,
    class: DiscoveryClass,
    writer: WriterInstanceId,
    generation: GenerationId,
) -> PathBuf {
    let mut path = discovery_root(root).join(class.directory());
    for component in identity_components(writer, generation) {
        path.push(format!("{component:02x}"));
    }
    path
}

fn ensure_leaf_parent(
    root: &Path,
    class: DiscoveryClass,
    writer: WriterInstanceId,
    generation: GenerationId,
) -> Result<PathBuf, String> {
    let leaf = leaf_path(root, class, writer, generation);
    let parent = leaf.parent().expect("discovery leaf has parent");
    create_private_directories(root, parent)?;
    sync_dir(parent)?;
    Ok(leaf)
}

fn ensure_leaf(
    root: &Path,
    class: DiscoveryClass,
    writer: WriterInstanceId,
    generation: GenerationId,
) -> Result<PathBuf, String> {
    let leaf = ensure_leaf_parent(root, class, writer, generation)?;
    create_private_directories(root, &leaf)?;
    sync_dir(&leaf)?;
    sync_dir(leaf.parent().expect("discovery leaf has parent"))?;
    Ok(leaf)
}

fn find_leaf(
    root: &Path,
    writer: WriterInstanceId,
    generation: GenerationId,
) -> Option<(DiscoveryClass, PathBuf)> {
    for class in [
        DiscoveryClass::Pending,
        DiscoveryClass::Active,
        DiscoveryClass::Archive,
    ] {
        let leaf = leaf_path(root, class, writer, generation);
        if leaf.is_dir() {
            return Some((class, leaf));
        }
    }
    None
}

fn read_selected_record(directory: &Path) -> Result<Option<DiscoveryRecord>, String> {
    let mut valid = Vec::new();
    for slot in 0..=1 {
        let path = directory.join(format!("status.{slot}"));
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.to_string()),
        };
        let record: DiscoveryRecord = match serde_json::from_slice(&bytes) {
            Ok(record) => record,
            Err(_) => continue,
        };
        if record.validate().is_ok()
            && record
                .canonical_bytes()
                .is_ok_and(|canonical| canonical == bytes)
        {
            valid.push(record);
        }
    }
    valid.sort_by_key(|record| record.sequence);
    if valid.len() == 2 && valid[0].sequence == valid[1].sequence && valid[0] != valid[1] {
        return Err("maintenance discovery slots conflict".to_string());
    }
    Ok(valid.pop())
}

fn selected_slot(directory: &Path) -> Result<Option<usize>, String> {
    let selected = read_selected_record(directory)?;
    let Some(selected) = selected else {
        return Ok(None);
    };
    for slot in 0..=1 {
        let bytes = match fs::read(directory.join(format!("status.{slot}"))) {
            Ok(bytes) => bytes,
            Err(_) => continue,
        };
        if serde_json::from_slice::<DiscoveryRecord>(&bytes).is_ok_and(|record| record == selected)
        {
            return Ok(Some(slot));
        }
    }
    Ok(None)
}

fn write_record(
    directory: &Path,
    active_slot: Option<usize>,
    record: &DiscoveryRecord,
) -> Result<usize, String> {
    let slot = active_slot.map_or(0, |slot| 1 - slot);
    let name = format!("status.{slot}");
    let target = directory.join(&name);
    let temp = directory.join(format!(".{name}.{}.tmp", Uuid::new_v4().simple()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)
        .map_err(|error| error.to_string())?;
    file.write_all(&record.canonical_bytes()?)
        .map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    drop(file);
    if target.exists() {
        fs::remove_file(&target).map_err(|error| error.to_string())?;
        sync_dir(directory)?;
    }
    fs::rename(&temp, &target).map_err(|error| error.to_string())?;
    sync_dir(directory)?;
    Ok(slot)
}

fn decode_component(value: &str) -> Result<u8, String> {
    if value.len() != 2 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("invalid maintenance discovery trie component".to_string());
    }
    u8::from_str_radix(value, 16)
        .map_err(|_| "invalid maintenance discovery trie component".to_string())
}

fn decode_node(value: &str) -> Result<Vec<u8>, String> {
    if value.is_empty()
        || value.len() > IDENTITY_BYTES * 2
        || !value.len().is_multiple_of(2)
        || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("invalid maintenance discovery cursor".to_string());
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|chunk| {
            let text = std::str::from_utf8(chunk)
                .map_err(|_| "invalid maintenance discovery cursor".to_string())?;
            u8::from_str_radix(text, 16)
                .map_err(|_| "invalid maintenance discovery cursor".to_string())
        })
        .collect()
}

#[cfg(unix)]
fn create_private_directories(root: &Path, path: &Path) -> Result<(), String> {
    use std::os::unix::fs::DirBuilderExt;
    path.strip_prefix(root)
        .map_err(|_| "maintenance discovery path escaped its root".to_string())?;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder.create(path).map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn create_private_directories(root: &Path, path: &Path) -> Result<(), String> {
    path.strip_prefix(root)
        .map_err(|_| "maintenance discovery path escaped its root".to_string())?;
    fs::create_dir_all(path).map_err(|error| error.to_string())
}

fn sync_dir(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| error.to_string())
}

fn hex(bytes: &[u8]) -> String {
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use fmt::Write as _;
        let _ = write!(value, "{byte:02x}");
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(root: &Path, writer: u8, generation: u8) {
        register_legacy_generation(
            root,
            WriterInstanceId::from_bytes([writer; 16]),
            GenerationId::from_bytes([generation; 16]),
            DiscoveryPhase::Prepared,
            None,
            None,
        )
        .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn publication_sets_only_new_directory_permissions_and_archive_leaves_source_skeleton() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let discovery = discovery_root(root.path());
        let active = discovery.join(ACTIVE);
        fs::create_dir_all(&active).unwrap();
        fs::set_permissions(&discovery, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&active, fs::Permissions::from_mode(0o755)).unwrap();
        let writer = WriterInstanceId::from_bytes([11; 16]);
        let generation = GenerationId::from_bytes([12; 16]);
        register_legacy_generation(
            root.path(),
            writer,
            generation,
            DiscoveryPhase::Prepared,
            None,
            None,
        )
        .unwrap();

        assert_eq!(
            fs::metadata(&discovery).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(
            fs::metadata(&active).unwrap().permissions().mode() & 0o777,
            0o755
        );
        let leaf = leaf_path(root.path(), DiscoveryClass::Active, writer, generation);
        assert_eq!(
            fs::metadata(&leaf).unwrap().permissions().mode() & 0o777,
            0o700
        );

        move_generation_class(
            root.path(),
            writer,
            generation,
            DiscoveryClass::Active,
            DiscoveryClass::Archive,
            DiscoveryPhase::Retired,
        )
        .unwrap();
        assert!(active.join("0b").exists());
        assert!(leaf_path(root.path(), DiscoveryClass::Archive, writer, generation).is_dir());
        let batch = read_batch(root.path(), &DiscoveryCursor::default(), 64, 1).unwrap();
        assert!(batch.entries.is_empty());
        assert!(batch.nodes_examined <= 64);
    }

    #[test]
    fn authoritative_close_reactivates_an_archived_derived_record() {
        let root = tempfile::tempdir().unwrap();
        let writer = WriterInstanceId::from_bytes([8; 16]);
        let generation = GenerationId::from_bytes([18; 16]);
        record(root.path(), 8, 18);
        move_generation_class(
            root.path(),
            writer,
            generation,
            DiscoveryClass::Active,
            DiscoveryClass::Archive,
            DiscoveryPhase::Retired,
        )
        .unwrap();
        record_closed_generation(root.path(), writer, generation).unwrap();
        assert!(!leaf_path(root.path(), DiscoveryClass::Archive, writer, generation).exists());
        let (class, active) = read_exact_record(root.path(), writer, generation)
            .unwrap()
            .unwrap();
        assert_eq!(class, DiscoveryClass::Active);
        assert_eq!(active.phase, DiscoveryPhase::Closed);
    }

    #[test]
    fn evidence_discovery_includes_only_preserved_dead_heads_from_archive() {
        let root = tempfile::tempdir().unwrap();
        record(root.path(), 1, 11);
        record(root.path(), 2, 12);
        move_generation_class(
            root.path(),
            WriterInstanceId::from_bytes([1; 16]),
            GenerationId::from_bytes([11; 16]),
            DiscoveryClass::Active,
            DiscoveryClass::Archive,
            DiscoveryPhase::Retired,
        )
        .unwrap();
        move_generation_class(
            root.path(),
            WriterInstanceId::from_bytes([2; 16]),
            GenerationId::from_bytes([12; 16]),
            DiscoveryClass::Active,
            DiscoveryClass::Archive,
            DiscoveryPhase::PreservedDeadHead,
        )
        .unwrap();

        let maintenance = read_batch(root.path(), &DiscoveryCursor::default(), 256, 8).unwrap();
        assert!(maintenance.entries.is_empty());
        assert!(!maintenance.more);

        let evidence = read_evidence_batch(root.path(), 256, 8).unwrap();
        assert!(!evidence.more);
        assert_eq!(evidence.entries_examined, 2);
        assert_eq!(evidence.entries.len(), 1);
        assert_eq!(
            evidence.entries[0].writer,
            WriterInstanceId::from_bytes([2; 16])
        );
        assert_eq!(
            evidence.entries[0].record.as_ref().unwrap().phase,
            DiscoveryPhase::PreservedDeadHead
        );
    }

    #[test]
    fn archive_budget_exhaustion_is_incomplete_through_public_metric_query() {
        let root = tempfile::tempdir().unwrap();
        record(root.path(), 1, 11);
        record(root.path(), 2, 12);
        move_generation_class(
            root.path(),
            WriterInstanceId::from_bytes([1; 16]),
            GenerationId::from_bytes([11; 16]),
            DiscoveryClass::Active,
            DiscoveryClass::Archive,
            DiscoveryPhase::Retired,
        )
        .unwrap();
        move_generation_class(
            root.path(),
            WriterInstanceId::from_bytes([2; 16]),
            GenerationId::from_bytes([12; 16]),
            DiscoveryClass::Active,
            DiscoveryClass::Archive,
            DiscoveryPhase::PreservedDeadHead,
        )
        .unwrap();

        let evidence = read_evidence_batch(root.path(), 256, 1).unwrap();
        assert!(evidence.entries.is_empty());
        assert_eq!(evidence.entries_examined, 1);
        assert!(evidence.more);

        let discovery =
            crate::event_store::discover_generation_read_targets(root.path(), 256, 1).unwrap();
        assert!(discovery.targets.is_empty());
        assert!(discovery.more);
        assert!(matches!(
            discovery.coverage,
            crate::event_store::DiscoveryCoverage::Incomplete { .. }
        ));

        let mut query = crate::longitudinal_metrics::MetricQuery::recent(1).unwrap();
        query.discovery_max_nodes = 256;
        query.discovery_max_entries = 1;
        let report = crate::longitudinal_metrics::query_metrics(root.path(), &query).unwrap();
        assert!(!report.coverage_complete);
        assert!(report.discovery_issues.is_empty());
        assert!(report.read_issues.iter().any(|issue| matches!(
            issue.kind,
            crate::event_store::CoverageIssueKind::DiscoveryCoverageIncomplete
        )));
    }

    #[test]
    fn trie_cursor_bounds_raw_nodes_and_pending_preempts_active() {
        let root = tempfile::tempdir().unwrap();
        record(root.path(), 1, 11);
        record(root.path(), 2, 12);
        record(root.path(), 3, 13);
        move_generation_class(
            root.path(),
            WriterInstanceId::from_bytes([3; 16]),
            GenerationId::from_bytes([13; 16]),
            DiscoveryClass::Active,
            DiscoveryClass::Pending,
            DiscoveryPhase::PendingTrash,
        )
        .unwrap();

        let mut cursor = DiscoveryCursor::default();
        let mut seen = Vec::new();
        for _ in 0..80 {
            let batch = read_batch(root.path(), &cursor, 64, 1).unwrap();
            if let Some(entry) = batch.entries.first() {
                seen.push((entry.class, entry.writer));
                if entry.class == DiscoveryClass::Pending {
                    move_generation_class(
                        root.path(),
                        entry.writer,
                        entry.generation,
                        DiscoveryClass::Pending,
                        DiscoveryClass::Archive,
                        DiscoveryPhase::Retired,
                    )
                    .unwrap();
                }
            }
            cursor = batch.cursor;
            if !batch.more && seen.len() == 3 {
                break;
            }
        }
        assert_eq!(seen[0].0, DiscoveryClass::Pending);
        assert_eq!(seen[1].1, WriterInstanceId::from_bytes([1; 16]));
        assert_eq!(seen[2].1, WriterInstanceId::from_bytes([2; 16]));
    }

    #[test]
    fn one_node_budget_advances_to_a_leaf_instead_of_rescanning_the_prefix() {
        let root = tempfile::tempdir().unwrap();
        let writer = WriterInstanceId::from_bytes([17; 16]);
        let generation = GenerationId::from_bytes([29; 16]);
        record(root.path(), 17, 29);
        let mut cursor = DiscoveryCursor::default();
        let mut observed = None;
        for slice in 0..=IDENTITY_BYTES {
            let batch = read_batch(root.path(), &cursor, 1, 1).unwrap();
            assert!(batch.nodes_examined <= 1);
            if let Some(entry) = batch.entries.into_iter().next() {
                observed = Some((slice, entry.writer, entry.generation));
                break;
            }
            assert!(batch.more);
            cursor = batch.cursor;
        }
        assert_eq!(observed, Some((IDENTITY_BYTES - 1, writer, generation)));
    }

    #[test]
    fn independent_leaf_unavailability_does_not_share_a_writer() {
        let root = tempfile::tempdir().unwrap();
        let blocked_writer = WriterInstanceId::from_bytes([1; 16]);
        let blocked_generation = GenerationId::from_bytes([2; 16]);
        let blocked = leaf_path(
            root.path(),
            DiscoveryClass::Active,
            blocked_writer,
            blocked_generation,
        );
        fs::create_dir_all(blocked.parent().unwrap()).unwrap();
        fs::write(&blocked, b"not a directory").unwrap();

        assert!(
            register_legacy_generation(
                root.path(),
                blocked_writer,
                blocked_generation,
                DiscoveryPhase::Prepared,
                None,
                None,
            )
            .is_err()
        );
        register_legacy_generation(
            root.path(),
            WriterInstanceId::from_bytes([9; 16]),
            GenerationId::from_bytes([10; 16]),
            DiscoveryPhase::Prepared,
            None,
            None,
        )
        .unwrap();

        let mut cursor = DiscoveryCursor::default();
        let mut saw_gap = false;
        let mut saw_healthy = false;
        for _ in 0..80 {
            let batch = read_batch(root.path(), &cursor, 1, 1).unwrap();
            saw_gap |= !batch.issues.is_empty();
            saw_healthy |= batch.entries.iter().any(|entry| {
                entry.writer == WriterInstanceId::from_bytes([9; 16])
                    && entry.generation == GenerationId::from_bytes([10; 16])
            });
            cursor = batch.cursor;
            if saw_gap && saw_healthy {
                break;
            }
        }
        assert!(saw_gap);
        assert!(
            saw_healthy,
            "blocked shard must not make the scan non-advancing"
        );
    }
}
