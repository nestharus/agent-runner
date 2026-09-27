//! AGE-377's exact-generation destructive maintenance protocol.

const RETIREMENT_HASH_BUFFER_BYTES: usize = 64 * 1024;
const DEFAULT_VALIDATION_ROWS: u64 = 256;
const DEFAULT_VALIDATION_BYTES: u64 = 8 * 1024 * 1024;
const DEFAULT_UNLINK_ENTRIES: usize = 4;

use super::generation::{
    MAX_GENERATION_CONTROL_RECORD_BYTES, checkpoint_closed_generation_under_lease,
    seal_closed_generation_under_lease,
};
use super::maintenance_discovery::{
    DiscoveryClass, DiscoveryPhase, move_generation_class, read_exact_record,
    record_generation_phase,
};
use super::{
    DATABASE_FILE_NAME, Digest32, GenerationEligibilityFacts, GenerationError, GenerationId,
    GenerationMaintenanceTarget, PartitionLifecycleState, RetirementReceipt,
    SEALED_MANIFEST_FILE_NAME, SealedManifest, WriterInstanceId, WriterLayout,
    acquire_generation_maintenance_lease_at, id_hex, publish_retirement_receipt,
    read_prepared_manifest,
};
use crate::maintenance::{
    MaintenanceError, MaintenanceJobKind, MaintenanceJobLease, now_unix_micros,
};
use crate::retention::{
    GenerationRetirementAuthorization, RetentionPolicy, authorize_event_generation_retirement,
};
use rusqlite::OpenFlags;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventRetirementRequest {
    pub writer_instance_id: WriterInstanceId,
    pub generation_id: GenerationId,
    pub policy: RetentionPolicy,
    pub as_of_unix_micros: i64,
    pub retirement_epoch: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventRetirementWorkLimits {
    pub max_validation_rows: u64,
    pub max_validation_bytes: u64,
    pub max_unlink_entries: usize,
}

impl Default for EventRetirementWorkLimits {
    fn default() -> Self {
        Self {
            max_validation_rows: DEFAULT_VALIDATION_ROWS,
            max_validation_bytes: DEFAULT_VALIDATION_BYTES,
            max_unlink_entries: DEFAULT_UNLINK_ENTRIES,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventRetirementSlice {
    Retired { receipt_path: PathBuf },
    MoreWork { phase: &'static str },
    Preserved { reason: String },
    Cancelled,
}

/// Invoke AGE-376's bounded non-destructive index planner for one exact,
/// non-selected closed generation. Missing indexes remain fail-closed because
/// AGE-376 requires a repair copy and exposes no safe replacement publication.
pub(crate) fn audit_event_generation_indexes(
    event_store_root: &Path,
    writer: WriterInstanceId,
    generation: GenerationId,
    max_file_bytes: u64,
) -> Result<super::IndexRebuildPlan, EventRetirementError> {
    let target = maintenance_target(event_store_root, writer, generation)?;
    super::plan_local_index_rebuild(&target, max_file_bytes).map_err(|error| {
        EventRetirementError::Validation(format!("AGE-376 index plan refused target: {error}"))
    })
}

pub(crate) fn inspect_event_generation_catalog(
    event_store_root: &Path,
    writer: WriterInstanceId,
    generation: GenerationId,
    max_rows: u64,
    max_file_bytes: u64,
) -> Result<super::CatalogInventoryResult, EventRetirementError> {
    let target = maintenance_target(event_store_root, writer, generation)?;
    super::inspect_closed_generation_for_catalog(&target, max_rows, max_file_bytes).map_err(
        |error| {
            EventRetirementError::Validation(format!(
                "AGE-376 catalog inspection refused target: {error}"
            ))
        },
    )
}

fn maintenance_target(
    event_store_root: &Path,
    writer: WriterInstanceId,
    generation: GenerationId,
) -> Result<GenerationMaintenanceTarget, EventRetirementError> {
    let layout = WriterLayout::open_existing(event_store_root, *writer.as_bytes())?;
    if layout
        .read_head(|_, _| Ok(()))?
        .is_some_and(|head| head.record.generation_id == generation)
    {
        return Err(EventRetirementError::Inconsistent(
            "historical maintenance refused the selected head".to_string(),
        ));
    }
    let generation_directory = layout.generation_dir(generation);
    let prepared = read_prepared_manifest(&generation_directory)?;
    let (sealed, sealed_digest) = read_sealed_manifest(&generation_directory)?;
    let database = sealed
        .durable_files
        .iter()
        .find(|file| file.relative_path == DATABASE_FILE_NAME)
        .ok_or_else(|| EventRetirementError::Inconsistent("sealed database absent".into()))?;
    Ok(GenerationMaintenanceTarget {
        writer_instance_id: writer,
        generation_id: generation,
        generation_directory,
        lease_path: layout.generation_lease_path(generation),
        database_relative_name: DATABASE_FILE_NAME.to_string(),
        prepared_manifest_sha256: prepared.sha256()?,
        sealed_manifest_sha256: Some(sealed_digest),
        database_file_sha256: Some(database.sha256),
        state: PartitionLifecycleState::Closed,
    })
}

/// Execute at most one bounded validation or trash-unlink slice. The caller's
/// durable job lease is independent from the exact generation lease; both are
/// required while the source exists. Cancellation before the directory move
/// is immediate. Once the move starts, receipt publication is completed before
/// cancellation is observed so a cancelled worker never manufactures an
/// unreceipted absence.
pub fn execute_event_retirement_slice(
    event_store_root: &Path,
    request: &EventRetirementRequest,
    limits: EventRetirementWorkLimits,
    job: &mut MaintenanceJobLease,
) -> Result<EventRetirementSlice, EventRetirementError> {
    if limits.max_validation_rows == 0
        || limits.max_validation_bytes == 0
        || limits.max_unlink_entries == 0
        || request.as_of_unix_micros < 0
        || request.retirement_epoch < 0
    {
        return Err(EventRetirementError::InvalidBound);
    }
    let writer_hex = id_hex(request.writer_instance_id.as_bytes());
    let generation_hex = id_hex(request.generation_id.as_bytes());
    let expected_partition = format!("{writer_hex}/{generation_hex}");
    if job.checkpoint().key.kind != MaintenanceJobKind::EventRetirement
        || job.checkpoint().key.partition != expected_partition
    {
        return Err(EventRetirementError::IdentityMismatch);
    }

    let layout =
        WriterLayout::open_existing(event_store_root, *request.writer_instance_id.as_bytes())?;
    let original = layout.generation_dir(request.generation_id);
    let lease_path = layout.generation_lease_path(request.generation_id);
    let pending = event_store_root
        .join("retirement")
        .join("trash")
        .join(&writer_hex)
        .join(format!("{generation_hex}.pending"));
    let receipt_path = event_store_root
        .join("retirement")
        .join("receipts")
        .join(&writer_hex)
        .join(format!("{generation_hex}.json"));

    let receipt_on_disk = read_receipt_if_present(&receipt_path)?;
    if let Some(receipt) = receipt_on_disk.as_ref() {
        validate_receipt_request(receipt, request)?;
        if original.exists() {
            return Err(EventRetirementError::Inconsistent(
                "receipt and original generation both exist".to_string(),
            ));
        }
    }

    if receipt_on_disk.is_none() && !original.exists() {
        if pending.exists() {
            // A crash after the move resumes below from the exact deterministic
            // path; its manifests must reproduce a fresh AGE-372 approval.
            if !job.checkpoint().cursor.as_deref().is_some_and(|cursor| {
                cursor.starts_with("retire:destructive_ready:")
                    || cursor.starts_with("retire:pending:")
            }) {
                return Err(EventRetirementError::Inconsistent(
                    "pending trash lacks a durable pre-move checkpoint".to_string(),
                ));
            }
        } else {
            return Err(EventRetirementError::Inconsistent(
                "generation, pending trash, and retirement receipt are absent".to_string(),
            ));
        }
    } else if original.exists() && pending.exists() {
        return Err(EventRetirementError::Inconsistent(
            "original generation and pending trash both exist".to_string(),
        ));
    }

    let generation_lease = acquire_generation_maintenance_lease_at(&lease_path)?;
    let head = layout.read_head(|_, _| Ok(()))?;
    if head
        .as_ref()
        .is_some_and(|selected| selected.record.generation_id == request.generation_id)
    {
        return Ok(EventRetirementSlice::Preserved {
            reason: "selected_head".to_string(),
        });
    }
    if let Some(receipt) = receipt_on_disk {
        publish_catalog_retirement(event_store_root, &receipt)?;
        if job.is_cancelled()? {
            return Ok(EventRetirementSlice::Cancelled);
        }
        return unlink_pending_slice(&pending, &receipt_path, limits.max_unlink_entries, job);
    }
    if job.is_cancelled()? && original.exists() {
        return Ok(EventRetirementSlice::Cancelled);
    }

    let source = if original.exists() {
        &original
    } else {
        &pending
    };
    let prepared = read_prepared_manifest(source)?;
    if prepared.writer_instance_id != request.writer_instance_id
        || prepared.generation_id != request.generation_id
    {
        return Err(EventRetirementError::IdentityMismatch);
    }

    let mut sealed_record = if source.join(SEALED_MANIFEST_FILE_NAME).exists() {
        Some(read_sealed_manifest(source)?)
    } else {
        None
    };
    if original.exists() && sealed_record.is_none() {
        #[cfg(test)]
        run_prepare_hook();
        checkpoint_closed_generation_under_lease(source, &generation_lease)?;
        let sealed = seal_closed_generation_under_lease(source, false, &generation_lease)?;
        sealed_record = Some(sealed);
    }
    let (sealed, sealed_digest) = sealed_record
        .ok_or_else(|| EventRetirementError::Inconsistent("sealed manifest absent".into()))?;
    let proof = digest_text(sealed_digest.as_bytes());
    if original.exists()
        && !cursor_has_phase(
            job.checkpoint().cursor.as_deref(),
            "payload_validated",
            &proof,
        )
    {
        let after = validation_cursor(job.checkpoint().cursor.as_deref(), &proof)?;
        let validation = validate_payload_slice(source, after, limits)?;
        let cursor = validation
            .next_sequence
            .map(|value| format!("validate:{proof}:{value}"))
            .unwrap_or_else(|| format!("retire:payload_validated:{proof}"));
        job.record_progress(
            "payload_validation",
            Some(cursor),
            job.checkpoint()
                .units_completed
                .saturating_add(validation.rows_examined),
            job.checkpoint()
                .bytes_completed
                .saturating_add(validation.bytes_examined),
            now_unix_micros(),
        )?;
        if validation.next_sequence.is_some() {
            return Ok(EventRetirementSlice::MoreWork {
                phase: "payload_validation",
            });
        }
    }
    let database_identity = sealed
        .durable_files
        .iter()
        .find(|file| file.relative_path == DATABASE_FILE_NAME)
        .ok_or_else(|| {
            EventRetirementError::Inconsistent("sealed database identity absent".into())
        })?;
    if !cursor_at_or_after_file_validation(job.checkpoint().cursor.as_deref(), &proof) {
        validate_sealed_files(source, &sealed)?;
        job.record_progress(
            "sealed_files_validated",
            Some(format!("retire:files_validated:{proof}")),
            job.checkpoint().units_completed,
            job.checkpoint().bytes_completed,
            now_unix_micros(),
        )?;
    }
    let target = GenerationMaintenanceTarget {
        writer_instance_id: request.writer_instance_id,
        generation_id: request.generation_id,
        generation_directory: source.clone(),
        lease_path: lease_path.clone(),
        database_relative_name: DATABASE_FILE_NAME.to_string(),
        prepared_manifest_sha256: prepared.sha256()?,
        sealed_manifest_sha256: Some(sealed_digest),
        database_file_sha256: Some(database_identity.sha256),
        state: PartitionLifecycleState::Closed,
    };
    let facts = GenerationEligibilityFacts {
        writer_instance_id: request.writer_instance_id,
        generation_id: request.generation_id,
        state: PartitionLifecycleState::Closed,
        prepared_manifest_valid: true,
        sealed_manifest_valid: true,
        selected_head: false,
        hold_present: sealed.hold,
        corruption_present: false,
        unclassified_legacy_input: false,
        closed_at_unix_micros: Some(sealed.closed_at_unix_micros),
        max_ingested_at_unix_micros: sealed.max_ingested_at_unix_micros,
        row_count: Some(sealed.row_count),
    };
    let approval = match authorize_event_generation_retirement(
        &request.policy,
        &facts,
        &target,
        request.as_of_unix_micros,
    )? {
        GenerationRetirementAuthorization::Approved(approval) => approval,
        GenerationRetirementAuthorization::Preserve(decision) => {
            return Ok(EventRetirementSlice::Preserved {
                reason: serde_json::to_string(&decision)?,
            });
        }
    };
    let receipt = approval.retirement_receipt(request.retirement_epoch);

    // Re-read both slots immediately before the first destructive publication.
    let head = layout.read_head(|_, _| Ok(()))?;
    if head
        .as_ref()
        .is_some_and(|selected| selected.record.generation_id == request.generation_id)
    {
        return Ok(EventRetirementSlice::Preserved {
            reason: "selected_head".to_string(),
        });
    }

    if original.exists() {
        prepare_pending_parent(event_store_root, &writer_hex)?;
        job.record_progress(
            "destructive_ready",
            Some(format!("retire:destructive_ready:{proof}")),
            job.checkpoint().units_completed,
            job.checkpoint().bytes_completed,
            now_unix_micros(),
        )?;
        move_generation_class(
            event_store_root,
            request.writer_instance_id,
            request.generation_id,
            DiscoveryClass::Active,
            DiscoveryClass::Pending,
            DiscoveryPhase::Retiring,
        )
        .map_err(EventRetirementError::Discovery)?;
        #[cfg(test)]
        run_before_move_hook();
        let transition = job.lock_transition()?;
        if job.is_cancelled()? {
            move_generation_class(
                event_store_root,
                request.writer_instance_id,
                request.generation_id,
                DiscoveryClass::Pending,
                DiscoveryClass::Active,
                DiscoveryPhase::Closed,
            )
            .map_err(EventRetirementError::Discovery)?;
            return Ok(EventRetirementSlice::Cancelled);
        }
        #[cfg(test)]
        run_after_final_cancel_check_hook();
        // The transition gate remains held from the final cancellation read
        // through rename and parent sync. A cancellation request that returns
        // before this point necessarily won the gate and preserved the source;
        // one that waits observes a truthfully post-move transition.
        fs::rename(&original, &pending).map_err(|source| EventRetirementError::Io {
            operation: "move exact generation to pending trash",
            source,
        })?;
        sync_dir(original.parent().expect("generation path has parent"))?;
        sync_dir(pending.parent().expect("pending path has parent"))?;
        record_generation_phase(
            event_store_root,
            request.writer_instance_id,
            request.generation_id,
            DiscoveryPhase::PendingTrash,
        )
        .map_err(EventRetirementError::Discovery)?;
        drop(transition);
        #[cfg(test)]
        if run_after_move_hook() {
            return Err(EventRetirementError::Validation(
                "injected transient failure after directory move".into(),
            ));
        }
        job.record_progress(
            "moved_to_pending_trash",
            Some(format!("retire:pending:{proof}")),
            job.checkpoint().units_completed.saturating_add(1),
            job.checkpoint().bytes_completed,
            now_unix_micros(),
        )?;
    }
    let published = publish_retirement_receipt(event_store_root, &receipt)?;
    job.record_progress(
        "receipt_published",
        None,
        job.checkpoint().units_completed.saturating_add(1),
        job.checkpoint().bytes_completed,
        now_unix_micros(),
    )?;
    publish_catalog_retirement(event_store_root, &receipt)?;
    drop(generation_lease);
    if job.is_cancelled()? {
        return Ok(EventRetirementSlice::Cancelled);
    }
    unlink_pending_slice(&pending, &published, limits.max_unlink_entries, job)
}

struct ValidationSlice {
    rows_examined: u64,
    bytes_examined: u64,
    next_sequence: Option<i64>,
}

fn validate_payload_slice(
    generation_dir: &Path,
    after_sequence: i64,
    limits: EventRetirementWorkLimits,
) -> Result<ValidationSlice, EventRetirementError> {
    let connection = rusqlite::Connection::open_with_flags(
        generation_dir.join(DATABASE_FILE_NAME),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.pragma_update(None, "query_only", true)?;
    let mut statement = connection.prepare(
        "SELECT local_sequence,payload,payload_sha256,payload_bytes
           FROM events WHERE local_sequence>?1 ORDER BY local_sequence LIMIT ?2",
    )?;
    let limit = i64::try_from(limits.max_validation_rows.saturating_add(1)).unwrap_or(i64::MAX);
    let mut rows = statement.query(rusqlite::params![after_sequence, limit])?;
    let mut examined = 0;
    let mut bytes_examined = 0u64;
    let mut last = None;
    while let Some(row) = rows.next()? {
        let sequence: i64 = row.get(0)?;
        let payload: String = row.get(1)?;
        let expected_digest: Vec<u8> = row.get(2)?;
        let expected_bytes: i64 = row.get(3)?;
        if examined == limits.max_validation_rows {
            return Ok(ValidationSlice {
                rows_examined: examined,
                bytes_examined,
                next_sequence: last,
            });
        }
        let payload_bytes = payload.as_bytes();
        let next_bytes = bytes_examined.saturating_add(payload_bytes.len() as u64);
        // The byte budget is a resumable slice boundary, never a correctness
        // ceiling. Always admit one exact row so an individually large valid
        // payload cannot be rejected forever.
        if examined > 0 && next_bytes > limits.max_validation_bytes {
            return Ok(ValidationSlice {
                rows_examined: examined,
                bytes_examined,
                next_sequence: last,
            });
        }
        bytes_examined = next_bytes;
        if usize::try_from(expected_bytes).ok() != Some(payload_bytes.len())
            || Sha256::digest(payload_bytes).as_slice() != expected_digest
        {
            return Err(EventRetirementError::Validation(
                "payload length/digest validation failed".to_string(),
            ));
        }
        examined += 1;
        last = Some(sequence);
    }
    Ok(ValidationSlice {
        rows_examined: examined,
        bytes_examined,
        next_sequence: None,
    })
}

fn validation_cursor(cursor: Option<&str>, proof: &str) -> Result<i64, EventRetirementError> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    if cursor_has_phase(Some(cursor), "payload_validated", proof) {
        return Ok(i64::MAX);
    }
    let (cursor_proof, sequence) = cursor
        .strip_prefix("validate:")
        .and_then(|value| value.rsplit_once(':'))
        .ok_or_else(|| EventRetirementError::Validation("unexpected retirement cursor".into()))?;
    if cursor_proof != proof {
        return Err(EventRetirementError::Validation(
            "retirement cursor proof does not match the sealed generation".into(),
        ));
    }
    sequence
        .parse()
        .map_err(|_| EventRetirementError::Validation("invalid validation cursor".into()))
}

fn cursor_has_phase(cursor: Option<&str>, phase: &str, proof: &str) -> bool {
    cursor.is_some_and(|cursor| {
        cursor == format!("retire:{phase}:{proof}")
            || cursor == format!("retire:files_validated:{proof}")
            || cursor == format!("retire:destructive_ready:{proof}")
            || cursor == format!("retire:pending:{proof}")
    })
}

fn cursor_at_or_after_file_validation(cursor: Option<&str>, proof: &str) -> bool {
    cursor.is_some_and(|cursor| {
        cursor == format!("retire:files_validated:{proof}")
            || cursor == format!("retire:destructive_ready:{proof}")
            || cursor == format!("retire:pending:{proof}")
    })
}

fn digest_text(bytes: &[u8]) -> String {
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use fmt::Write as _;
        let _ = write!(value, "{byte:02x}");
    }
    value
}

fn read_sealed_manifest(
    generation_dir: &Path,
) -> Result<(SealedManifest, Digest32), EventRetirementError> {
    let path = generation_dir.join(SEALED_MANIFEST_FILE_NAME);
    let bytes = read_bounded(&path, MAX_GENERATION_CONTROL_RECORD_BYTES)?;
    let sealed: SealedManifest = serde_json::from_slice(&bytes)?;
    sealed.validate()?;
    let mut canonical = serde_json::to_vec(&sealed)?;
    canonical.push(b'\n');
    if canonical != bytes {
        return Err(EventRetirementError::Validation(
            "sealed manifest is not canonical".to_string(),
        ));
    }
    Ok((sealed, Digest32::sha256(&bytes)))
}

fn validate_sealed_files(
    generation_dir: &Path,
    sealed: &SealedManifest,
) -> Result<(), EventRetirementError> {
    for identity in &sealed.durable_files {
        let path = generation_dir.join(&identity.relative_path);
        let metadata = fs::metadata(&path).map_err(|source| EventRetirementError::Io {
            operation: "inspect sealed generation file",
            source,
        })?;
        if metadata.len() != identity.bytes || hash_file(&path, identity.bytes)? != identity.sha256
        {
            return Err(EventRetirementError::IdentityMismatch);
        }
    }
    Ok(())
}

pub(crate) fn archive_event_generation_discovery(
    event_store_root: &Path,
    writer: WriterInstanceId,
    generation: GenerationId,
    phase: DiscoveryPhase,
) -> Result<(), EventRetirementError> {
    let Some((class, _)) = read_exact_record(event_store_root, writer, generation)
        .map_err(EventRetirementError::Discovery)?
    else {
        return Err(EventRetirementError::Discovery(
            "exact generation discovery journal is absent".to_string(),
        ));
    };
    if class == DiscoveryClass::Archive {
        record_generation_phase(event_store_root, writer, generation, phase)
            .map_err(EventRetirementError::Discovery)
    } else {
        move_generation_class(
            event_store_root,
            writer,
            generation,
            class,
            DiscoveryClass::Archive,
            phase,
        )
        .map_err(EventRetirementError::Discovery)
    }
}

fn read_receipt_if_present(path: &Path) -> Result<Option<RetirementReceipt>, EventRetirementError> {
    let bytes = match read_bounded(path, MAX_GENERATION_CONTROL_RECORD_BYTES) {
        Ok(bytes) => bytes,
        Err(EventRetirementError::Io { source, .. })
            if source.kind() == io::ErrorKind::NotFound =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    let receipt: RetirementReceipt = serde_json::from_slice(&bytes)?;
    receipt.validate()?;
    if receipt.canonical_bytes()? != bytes {
        return Err(EventRetirementError::Validation(
            "retirement receipt is not canonical".to_string(),
        ));
    }
    Ok(Some(receipt))
}

fn validate_receipt_request(
    receipt: &RetirementReceipt,
    request: &EventRetirementRequest,
) -> Result<(), EventRetirementError> {
    if receipt.writer_instance_id != request.writer_instance_id
        || receipt.generation_id != request.generation_id
        || receipt.retention_policy_version != request.policy.version
    {
        return Err(EventRetirementError::IdentityMismatch);
    }
    Ok(())
}

fn prepare_pending_parent(
    event_store_root: &Path,
    writer_hex: &str,
) -> Result<(), EventRetirementError> {
    let retirement = event_store_root.join("retirement");
    let trash = retirement.join("trash");
    let writer = trash.join(writer_hex);
    for directory in [&retirement, &trash, &writer] {
        fs::create_dir_all(directory).map_err(|source| EventRetirementError::Io {
            operation: "create pending trash directory",
            source,
        })?;
        set_private_directory(directory)?;
    }
    sync_dir(&writer)?;
    sync_dir(&trash)?;
    sync_dir(&retirement)
}

fn publish_catalog_retirement(
    event_store_root: &Path,
    receipt: &RetirementReceipt,
) -> Result<(), EventRetirementError> {
    let catalog = event_store_root.join("catalog");
    let retired = catalog.join("retired");
    let writer = retired.join(id_hex(receipt.writer_instance_id.as_bytes()));
    for directory in [&catalog, &retired, &writer] {
        fs::create_dir_all(directory).map_err(|source| EventRetirementError::Io {
            operation: "create derived catalog retirement directory",
            source,
        })?;
        set_private_directory(directory)?;
    }
    let name = format!("{}.json", id_hex(receipt.generation_id.as_bytes()));
    create_once_equal(&writer, &name, &receipt.canonical_bytes()?)?;
    sync_dir(&writer)?;
    sync_dir(&retired)?;
    sync_dir(&catalog)
}

fn unlink_pending_slice(
    pending: &Path,
    receipt_path: &Path,
    max_entries: usize,
    job: &mut MaintenanceJobLease,
) -> Result<EventRetirementSlice, EventRetirementError> {
    if !pending.exists() {
        return Ok(EventRetirementSlice::Retired {
            receipt_path: receipt_path.to_path_buf(),
        });
    }
    let allowed = [
        DATABASE_FILE_NAME.to_string(),
        format!("{DATABASE_FILE_NAME}-wal"),
        format!("{DATABASE_FILE_NAME}-shm"),
        super::PREPARED_MANIFEST_FILE_NAME.to_string(),
        SEALED_MANIFEST_FILE_NAME.to_string(),
    ];
    let mut entries = fs::read_dir(pending)
        .map_err(|source| EventRetirementError::Io {
            operation: "enumerate pending retirement trash",
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| EventRetirementError::Io {
            operation: "read pending retirement trash entry",
            source,
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    if entries.iter().any(|entry| {
        let name = entry.file_name().to_string_lossy().into_owned();
        !allowed.contains(&name) || !entry.file_type().is_ok_and(|kind| kind.is_file())
    }) {
        return Err(EventRetirementError::Inconsistent(
            "pending trash contains an unexpected entry".to_string(),
        ));
    }
    let mut removed = 0u64;
    let mut removed_bytes = 0u64;
    for entry in entries.into_iter().take(max_entries) {
        removed_bytes =
            removed_bytes.saturating_add(entry.metadata().map(|value| value.len()).unwrap_or(0));
        fs::remove_file(entry.path()).map_err(|source| EventRetirementError::Io {
            operation: "unlink pending retirement entry",
            source,
        })?;
        removed += 1;
    }
    sync_dir(pending)?;
    let remaining = fs::read_dir(pending)
        .map_err(|source| EventRetirementError::Io {
            operation: "check pending retirement completion",
            source,
        })?
        .next()
        .transpose()
        .map_err(|source| EventRetirementError::Io {
            operation: "read pending retirement completion",
            source,
        })?;
    job.record_progress(
        "trash_unlink",
        Some("trash".to_string()),
        job.checkpoint().units_completed.saturating_add(removed),
        job.checkpoint()
            .bytes_completed
            .saturating_add(removed_bytes),
        now_unix_micros(),
    )?;
    if remaining.is_some() {
        return Ok(EventRetirementSlice::MoreWork {
            phase: "trash_unlink",
        });
    }
    fs::remove_dir(pending).map_err(|source| EventRetirementError::Io {
        operation: "remove empty pending retirement directory",
        source,
    })?;
    if let Some(parent) = pending.parent() {
        sync_dir(parent)?;
    }
    Ok(EventRetirementSlice::Retired {
        receipt_path: receipt_path.to_path_buf(),
    })
}

fn create_once_equal(
    directory: &Path,
    name: &str,
    bytes: &[u8],
) -> Result<(), EventRetirementError> {
    let target = directory.join(name);
    match fs::read(&target) {
        Ok(existing) if existing == bytes => return Ok(()),
        Ok(_) => return Err(EventRetirementError::PublicationConflict(target)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(EventRetirementError::Io {
                operation: "inspect create-once maintenance publication",
                source,
            });
        }
    }
    let temp = directory.join(format!(".{name}.{}.tmp", Uuid::new_v4().simple()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)
        .map_err(|source| EventRetirementError::Io {
            operation: "create maintenance publication temp",
            source,
        })?;
    file.write_all(bytes)
        .map_err(|source| EventRetirementError::Io {
            operation: "write maintenance publication temp",
            source,
        })?;
    file.sync_all().map_err(|source| EventRetirementError::Io {
        operation: "sync maintenance publication temp",
        source,
    })?;
    drop(file);
    match fs::hard_link(&temp, &target) {
        Ok(()) => {
            fs::remove_file(&temp).map_err(|source| EventRetirementError::Io {
                operation: "remove maintenance publication temp",
                source,
            })?;
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let existing = fs::read(&target).map_err(|source| EventRetirementError::Io {
                operation: "read racing maintenance publication",
                source,
            })?;
            let _ = fs::remove_file(&temp);
            if existing != bytes {
                return Err(EventRetirementError::PublicationConflict(target));
            }
        }
        Err(source) => {
            let _ = fs::remove_file(&temp);
            return Err(EventRetirementError::Io {
                operation: "publish create-once maintenance record",
                source,
            });
        }
    }
    sync_dir(directory)
}

fn read_bounded(path: &Path, max_bytes: usize) -> Result<Vec<u8>, EventRetirementError> {
    let mut file = File::open(path).map_err(|source| EventRetirementError::Io {
        operation: "open bounded maintenance record",
        source,
    })?;
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take(
            u64::try_from(max_bytes)
                .unwrap_or(u64::MAX)
                .saturating_add(1),
        )
        .read_to_end(&mut bytes)
        .map_err(|source| EventRetirementError::Io {
            operation: "read bounded maintenance record",
            source,
        })?;
    if bytes.len() > max_bytes {
        return Err(EventRetirementError::Validation(
            "maintenance control record exceeds byte bound".to_string(),
        ));
    }
    Ok(bytes)
}

fn hash_file(path: &Path, expected_bytes: u64) -> Result<Digest32, EventRetirementError> {
    let mut file = File::open(path).map_err(|source| EventRetirementError::Io {
        operation: "open sealed file for validation",
        source,
    })?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; RETIREMENT_HASH_BUFFER_BYTES];
    let mut total = 0u64;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|source| EventRetirementError::Io {
                operation: "read sealed file for validation",
                source,
            })?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        if total > expected_bytes {
            return Err(EventRetirementError::IdentityMismatch);
        }
        digest.update(&buffer[..read]);
    }
    if total != expected_bytes {
        return Err(EventRetirementError::IdentityMismatch);
    }
    Ok(Digest32::from_bytes(digest.finalize().into()))
}

fn sync_dir(path: &Path) -> Result<(), EventRetirementError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| EventRetirementError::Io {
            operation: "sync maintenance directory",
            source,
        })
}

#[cfg(unix)]
fn set_private_directory(path: &Path) -> Result<(), EventRetirementError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|source| {
        EventRetirementError::Io {
            operation: "set maintenance directory permissions",
            source,
        }
    })
}

#[cfg(not(unix))]
fn set_private_directory(_path: &Path) -> Result<(), EventRetirementError> {
    Ok(())
}

#[derive(Debug)]
pub enum EventRetirementError {
    Io {
        operation: &'static str,
        source: io::Error,
    },
    Sqlite(rusqlite::Error),
    Json(serde_json::Error),
    Generation(GenerationError),
    Maintenance(MaintenanceError),
    Policy(crate::retention::RetentionPolicyError),
    Discovery(String),
    InvalidBound,
    IdentityMismatch,
    Validation(String),
    Inconsistent(String),
    PublicationConflict(PathBuf),
}

impl fmt::Display for EventRetirementError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { operation, source } => write!(formatter, "{operation}: {source}"),
            Self::Sqlite(error) => write!(formatter, "retirement SQLite error: {error}"),
            Self::Json(error) => write!(formatter, "retirement JSON error: {error}"),
            Self::Generation(error) => error.fmt(formatter),
            Self::Maintenance(error) => error.fmt(formatter),
            Self::Policy(error) => error.fmt(formatter),
            Self::Discovery(error) => {
                write!(formatter, "event retirement discovery error: {error}")
            }
            Self::InvalidBound => {
                write!(formatter, "event retirement work limits must be non-zero")
            }
            Self::IdentityMismatch => write!(formatter, "event retirement exact identity mismatch"),
            Self::Validation(reason) => {
                write!(formatter, "event retirement validation failed: {reason}")
            }
            Self::Inconsistent(reason) => write!(
                formatter,
                "event retirement state is inconsistent: {reason}"
            ),
            Self::PublicationConflict(path) => write!(
                formatter,
                "event retirement publication conflicts at {}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for EventRetirementError {}

impl From<rusqlite::Error> for EventRetirementError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Sqlite(value)
    }
}
impl From<serde_json::Error> for EventRetirementError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}
impl From<GenerationError> for EventRetirementError {
    fn from(value: GenerationError) -> Self {
        Self::Generation(value)
    }
}
impl From<MaintenanceError> for EventRetirementError {
    fn from(value: MaintenanceError) -> Self {
        Self::Maintenance(value)
    }
}
impl From<crate::retention::RetentionPolicyError> for EventRetirementError {
    fn from(value: crate::retention::RetentionPolicyError) -> Self {
        Self::Policy(value)
    }
}

#[cfg(test)]
thread_local! {
    static BEFORE_MOVE_HOOK: std::cell::RefCell<Option<Box<dyn FnMut()>>> =
        const { std::cell::RefCell::new(None) };
    static AFTER_FINAL_CANCEL_CHECK_HOOK: std::cell::RefCell<Option<Box<dyn FnMut()>>> =
        const { std::cell::RefCell::new(None) };
    static AFTER_MOVE_HOOK: std::cell::RefCell<Option<Box<dyn FnMut()>>> =
        const { std::cell::RefCell::new(None) };
    static PREPARE_HOOK: std::cell::RefCell<Option<Box<dyn FnMut()>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn run_before_move_hook() {
    BEFORE_MOVE_HOOK.with(|slot| {
        if let Some(mut hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(test)]
fn run_after_final_cancel_check_hook() {
    AFTER_FINAL_CANCEL_CHECK_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().as_mut() {
            hook();
        }
    });
}

#[cfg(test)]
fn run_after_move_hook() -> bool {
    AFTER_MOVE_HOOK.with(|slot| {
        if let Some(mut hook) = slot.borrow_mut().take() {
            hook();
            true
        } else {
            false
        }
    })
}

#[cfg(test)]
fn run_prepare_hook() {
    PREPARE_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().as_mut() {
            hook();
        }
    });
}

#[cfg(test)]
fn with_before_move_hook<T>(hook: impl FnMut() + 'static, operation: impl FnOnce() -> T) -> T {
    BEFORE_MOVE_HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
    let result = operation();
    BEFORE_MOVE_HOOK.with(|slot| *slot.borrow_mut() = None);
    result
}

#[cfg(test)]
fn with_after_final_cancel_check_hook<T>(
    hook: impl FnMut() + 'static,
    operation: impl FnOnce() -> T,
) -> T {
    AFTER_FINAL_CANCEL_CHECK_HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
    let result = operation();
    AFTER_FINAL_CANCEL_CHECK_HOOK.with(|slot| *slot.borrow_mut() = None);
    result
}

#[cfg(test)]
fn with_after_move_hook<T>(hook: impl FnMut() + 'static, operation: impl FnOnce() -> T) -> T {
    AFTER_MOVE_HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
    let result = operation();
    AFTER_MOVE_HOOK.with(|slot| *slot.borrow_mut() = None);
    result
}

#[cfg(test)]
fn with_prepare_hook<T>(hook: impl FnMut() + 'static, operation: impl FnOnce() -> T) -> T {
    PREPARE_HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
    let result = operation();
    PREPARE_HOOK.with(|slot| *slot.borrow_mut() = None);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_store::{
        EventCorrelations, EventEnvelopeV1, EventFamily, EventId, EventKind, GenerationSource,
        HeadRecord, NativeProcessIdentity, NewEventV1, PayloadNormalizationPolicy,
        PreparedManifest, ProcessInstanceId, ProducerIdentity, SupervisorAuthorityId, append_batch,
        close_generation, initialize_generation_schema, mark_generation_writable,
    };
    use crate::maintenance::{
        AcquireMaintenanceJob, MaintenanceJobKey, MaintenanceJobStore, MaintenanceRunState,
        MaintenanceWorkerIdentity,
    };
    use rusqlite::Connection;
    use serde_json::json;

    const OLD_CLOSE: i64 = 100;

    struct Fixture {
        data_root: tempfile::TempDir,
        event_root: PathBuf,
        layout: WriterLayout,
        old: GenerationId,
        successor: GenerationId,
        request: EventRetirementRequest,
    }

    fn producer(writer: WriterInstanceId) -> ProducerIdentity {
        ProducerIdentity {
            writer_instance_id: writer,
            process_instance_id: ProcessInstanceId::from_bytes([2; 16]),
            process_root_id: ProcessInstanceId::from_bytes([3; 16]),
            parent_process_instance_id: None,
            supervisor_authority_id: Some(SupervisorAuthorityId::from_bytes([4; 16])),
            native_process: Some(NativeProcessIdentity {
                os_pid: 10,
                os_boot_id_sha256: Digest32::from_bytes([5; 32]),
                os_pid_starttime_ticks: 20,
            }),
        }
    }

    fn manifest(
        writer: WriterInstanceId,
        generation: GenerationId,
        predecessor: Option<GenerationId>,
        epoch: i64,
    ) -> PreparedManifest {
        PreparedManifest {
            format_version: super::super::EVENT_STORE_FORMAT_VERSION,
            schema_version: super::super::EVENT_SCHEMA_VERSION,
            writer_instance_id: writer,
            generation_id: generation,
            predecessor_generation_id: predecessor,
            database_relative_path: DATABASE_FILE_NAME.to_string(),
            created_at_unix_micros: 1 + epoch,
            head_epoch: epoch,
            source: GenerationSource::NativeProcess,
            producer: producer(writer),
        }
    }

    fn event(writer: WriterInstanceId, value: u8, sequence: i64) -> EventEnvelopeV1 {
        EventEnvelopeV1::normalize(
            NewEventV1 {
                event_id: EventId::from_bytes([value; 16]),
                family: EventFamily::Maintenance,
                kind: EventKind::registered("maintenance.retirement_fixture").unwrap(),
                recorded_at_unix_micros: 10 + sequence,
                producer_sequence: sequence,
                producer: producer(writer),
                correlations: EventCorrelations::default(),
                payload: json!({"value": value}),
                legacy_provenance: None,
                retry_of_generation_id: None,
            },
            &PayloadNormalizationPolicy::registered(&["value"]).unwrap(),
            20 + sequence,
        )
        .unwrap()
    }

    impl Fixture {
        fn new(selected_old: bool, rows: u8) -> Self {
            let data_root = tempfile::tempdir().unwrap();
            let event_root = data_root.path().join("diagnostics/event-store-v1");
            let writer = WriterInstanceId::from_bytes([1; 16]);
            let old = GenerationId::from_bytes([6; 16]);
            let successor = GenerationId::from_bytes([7; 16]);
            let layout = WriterLayout::create(&event_root, *writer.as_bytes()).unwrap();
            let old_manifest = manifest(writer, old, None, 0);
            let old_published = layout
                .publish_prepared_generation(
                    &old_manifest,
                    |path| {
                        let mut db = Connection::open(path)?;
                        initialize_generation_schema(&mut db, &old_manifest.generation_metadata()?)
                            .map_err(|error| GenerationError::Validation(error.to_string()))
                    },
                    |_, _| Ok(()),
                )
                .unwrap();
            drop(layout.acquire_generation_lease(old).unwrap());
            let first_head =
                HeadRecord::new(0, writer, old, old_published.prepared_manifest_sha256).unwrap();
            let first_slot = layout.publish_head(None, &first_head).unwrap();
            let mut connection = Connection::open(&old_published.database_path).unwrap();
            connection
                .pragma_update(None, "synchronous", "FULL")
                .unwrap();
            mark_generation_writable(&connection).unwrap();
            let records = (0..rows)
                .map(|index| event(writer, index + 10, i64::from(index)))
                .collect::<Vec<_>>();
            append_batch(&mut connection, old, &records).unwrap();
            close_generation(&mut connection, OLD_CLOSE, successor).unwrap();
            drop(connection);

            if !selected_old {
                let successor_manifest = manifest(writer, successor, Some(old), 1);
                let successor_published = layout
                    .publish_prepared_generation(
                        &successor_manifest,
                        |path| {
                            let mut db = Connection::open(path)?;
                            initialize_generation_schema(
                                &mut db,
                                &successor_manifest.generation_metadata()?,
                            )
                            .map_err(|error| GenerationError::Validation(error.to_string()))
                        },
                        |_, _| Ok(()),
                    )
                    .unwrap();
                let successor_head = HeadRecord::new(
                    1,
                    writer,
                    successor,
                    successor_published.prepared_manifest_sha256,
                )
                .unwrap();
                layout
                    .publish_head(Some(first_slot), &successor_head)
                    .unwrap();
            }
            let as_of = OLD_CLOSE + crate::retention::DEFAULT_TERMINAL_RETENTION_MICROS;
            Self {
                data_root,
                event_root,
                layout,
                old,
                successor,
                request: EventRetirementRequest {
                    writer_instance_id: writer,
                    generation_id: old,
                    policy: RetentionPolicy::default(),
                    as_of_unix_micros: as_of,
                    retirement_epoch: 9,
                },
            }
        }

        fn store(&self) -> MaintenanceJobStore {
            MaintenanceJobStore::open(self.data_root.path()).unwrap()
        }

        fn key(&self) -> MaintenanceJobKey {
            MaintenanceJobKey::new(
                MaintenanceJobKind::EventRetirement,
                format!(
                    "{}/{}",
                    id_hex(self.request.writer_instance_id.as_bytes()),
                    id_hex(self.old.as_bytes())
                ),
            )
            .unwrap()
        }

        fn acquire(&self, epoch: i64) -> MaintenanceJobLease {
            let store = self.store();
            let owner = MaintenanceWorkerIdentity {
                worker_instance_id: Uuid::new_v4(),
                os_pid: 100 + epoch,
                os_boot_id: "test-boot".to_string(),
                os_pid_starttime_ticks: 1000 + epoch,
                schedule_basis: "test".to_string(),
                launch_id: None,
            };
            let AcquireMaintenanceJob::Acquired(lease) = store
                .acquire(self.key(), epoch, owner, 1_000 + epoch)
                .unwrap()
            else {
                panic!("job not acquired");
            };
            lease
        }

        fn original(&self) -> PathBuf {
            self.layout.generation_dir(self.old)
        }

        fn pending(&self) -> PathBuf {
            self.event_root
                .join("retirement/trash")
                .join(id_hex(self.request.writer_instance_id.as_bytes()))
                .join(format!("{}.pending", id_hex(self.old.as_bytes())))
        }

        fn receipt(&self) -> PathBuf {
            self.event_root
                .join("retirement/receipts")
                .join(id_hex(self.request.writer_instance_id.as_bytes()))
                .join(format!("{}.json", id_hex(self.old.as_bytes())))
        }
    }

    #[test]
    fn selected_head_is_refused_before_checkpoint_or_move() {
        let fixture = Fixture::new(true, 1);
        let mut job = fixture.acquire(9);
        let outcome = execute_event_retirement_slice(
            &fixture.event_root,
            &fixture.request,
            EventRetirementWorkLimits::default(),
            &mut job,
        )
        .unwrap();
        assert_eq!(
            outcome,
            EventRetirementSlice::Preserved {
                reason: "selected_head".to_string()
            }
        );
        assert!(fixture.original().is_dir());
        assert!(!fixture.pending().exists());
    }

    #[test]
    fn payload_cursor_is_fenced_to_the_exact_sealed_proof() {
        assert_eq!(
            validation_cursor(Some("validate:proof-a:7"), "proof-a").unwrap(),
            7
        );
        assert!(matches!(
            validation_cursor(Some("validate:proof-a:7"), "proof-b"),
            Err(EventRetirementError::Validation(_))
        ));
        assert!(matches!(
            validation_cursor(Some("retire:files_validated:proof-a"), "proof-b"),
            Err(EventRetirementError::Validation(_))
        ));
    }

    #[test]
    fn writer_or_reader_lease_prevents_destructive_worker() {
        let fixture = Fixture::new(false, 1);
        let shared = fixture
            .layout
            .acquire_generation_lease(fixture.old)
            .unwrap();
        let mut job = fixture.acquire(9);
        assert!(matches!(
            execute_event_retirement_slice(
                &fixture.event_root,
                &fixture.request,
                EventRetirementWorkLimits::default(),
                &mut job,
            ),
            Err(EventRetirementError::Generation(
                GenerationError::GenerationAlreadyOwned(_)
            ))
        ));
        assert!(fixture.original().is_dir());
        drop(shared);
    }

    #[test]
    fn bounded_validation_and_trash_unlink_resume_to_idempotent_receipt() {
        let fixture = Fixture::new(false, 3);
        let live_head_writer = fixture
            .layout
            .acquire_generation_lease(fixture.successor)
            .unwrap();
        let limits = EventRetirementWorkLimits {
            max_validation_rows: 1,
            // Smaller than one fixture payload: this remains a slice target,
            // not a permanent size rejection.
            max_validation_bytes: 1,
            max_unlink_entries: 1,
        };
        let epoch = 9;
        let mut slices = 0;
        let mut saw_validation_yield = false;
        let mut saw_unlink_yield = false;
        let preparations = std::rc::Rc::new(std::cell::Cell::new(0));
        let observed = std::rc::Rc::clone(&preparations);
        with_prepare_hook(
            move || observed.set(observed.get() + 1),
            || loop {
                let mut job = fixture.acquire(epoch);
                match execute_event_retirement_slice(
                    &fixture.event_root,
                    &fixture.request,
                    limits,
                    &mut job,
                )
                .unwrap()
                {
                    EventRetirementSlice::MoreWork {
                        phase: "payload_validation",
                    } => {
                        saw_validation_yield = true;
                        job.yield_run("payload_validation", "test resume", now_unix_micros())
                            .unwrap();
                    }
                    EventRetirementSlice::MoreWork {
                        phase: "trash_unlink",
                    } => {
                        saw_unlink_yield = true;
                        job.yield_run("trash_unlink", "test resume", now_unix_micros())
                            .unwrap();
                    }
                    EventRetirementSlice::Retired { receipt_path } => {
                        assert!(receipt_path.is_file());
                        job.finish(
                            MaintenanceRunState::Succeeded,
                            "retired",
                            "done",
                            now_unix_micros(),
                        )
                        .unwrap();
                        break;
                    }
                    other => panic!("unexpected outcome {other:?}"),
                }
                slices += 1;
                assert!(slices < 20);
            },
        );
        assert_eq!(
            preparations.get(),
            1,
            "checkpoint/seal proof is create-once"
        );
        assert!(saw_validation_yield);
        assert!(saw_unlink_yield);
        assert!(!fixture.original().exists());
        assert!(!fixture.pending().exists());
        let catalog_marker = fixture
            .event_root
            .join("catalog/retired")
            .join(id_hex(fixture.request.writer_instance_id.as_bytes()))
            .join(format!("{}.json", id_hex(fixture.old.as_bytes())));
        assert!(catalog_marker.is_file());
        drop(live_head_writer);

        let mut replay = fixture.acquire(31);
        let mut later_request = fixture.request.clone();
        later_request.retirement_epoch = 31;
        assert!(matches!(
            execute_event_retirement_slice(
                &fixture.event_root,
                &later_request,
                limits,
                &mut replay,
            )
            .unwrap(),
            EventRetirementSlice::Retired { .. }
        ));
    }

    #[test]
    fn cancellation_before_move_preserves_source_and_is_epoch_fenced() {
        let fixture = Fixture::new(false, 1);
        fixture
            .store()
            .request_cancellation(&fixture.key(), 9, 900)
            .unwrap();
        let mut job = fixture.acquire(9);
        assert_eq!(
            execute_event_retirement_slice(
                &fixture.event_root,
                &fixture.request,
                EventRetirementWorkLimits::default(),
                &mut job,
            )
            .unwrap(),
            EventRetirementSlice::Cancelled
        );
        assert!(fixture.original().exists());
        assert!(!fixture.pending().exists());
    }

    #[test]
    fn cancellation_published_during_validation_is_rechecked_before_move() {
        let fixture = Fixture::new(false, 1);
        let store = fixture.store();
        let key = fixture.key();
        let outcome = with_before_move_hook(
            move || {
                store
                    .request_cancellation(&key, 9, now_unix_micros())
                    .unwrap();
            },
            || {
                let mut job = fixture.acquire(9);
                execute_event_retirement_slice(
                    &fixture.event_root,
                    &fixture.request,
                    EventRetirementWorkLimits::default(),
                    &mut job,
                )
                .unwrap()
            },
        );
        assert_eq!(outcome, EventRetirementSlice::Cancelled);
        assert!(fixture.original().is_dir());
        assert!(!fixture.pending().exists());
    }

    #[test]
    fn cancellation_in_former_check_to_rename_window_is_linearized_after_move() {
        use std::sync::{Arc, Mutex, mpsc};

        let fixture = Fixture::new(false, 1);
        let store = fixture.store();
        let key = fixture.key();
        let (started_tx, started_rx) = mpsc::channel();
        let cancellation = Arc::new(Mutex::new(None));
        let cancellation_from_hook = Arc::clone(&cancellation);
        let outcome = with_after_final_cancel_check_hook(
            move || {
                let started_tx = started_tx.clone();
                let store = store.clone();
                let key = key.clone();
                let handle = std::thread::spawn(move || {
                    started_tx.send(()).unwrap();
                    store
                        .request_cancellation(&key, 9, now_unix_micros())
                        .unwrap();
                });
                *cancellation_from_hook.lock().unwrap() = Some(handle);
                started_rx.recv().unwrap();
            },
            || {
                let mut job = fixture.acquire(9);
                execute_event_retirement_slice(
                    &fixture.event_root,
                    &fixture.request,
                    EventRetirementWorkLimits::default(),
                    &mut job,
                )
                .unwrap()
            },
        );
        cancellation.lock().unwrap().take().unwrap().join().unwrap();

        assert!(!fixture.original().exists());
        assert!(fixture.receipt().is_file());
        assert!(matches!(
            outcome,
            EventRetirementSlice::Cancelled | EventRetirementSlice::Retired { .. }
        ));
    }

    #[test]
    fn returned_error_after_move_retains_epoch_cursor_for_receipt_recovery() {
        let fixture = Fixture::new(false, 1);
        let mut job = fixture.acquire(9);
        let error = with_after_move_hook(
            || {},
            || {
                execute_event_retirement_slice(
                    &fixture.event_root,
                    &fixture.request,
                    EventRetirementWorkLimits::default(),
                    &mut job,
                )
                .unwrap_err()
            },
        );
        assert!(!fixture.original().exists());
        assert!(fixture.pending().is_dir());
        crate::detached_maintenance::settle_event_retirement_error(&mut job, error.to_string())
            .unwrap();
        drop(job);
        let status = fixture
            .store()
            .read_status(&fixture.key())
            .unwrap()
            .unwrap();
        assert_eq!(status.state, MaintenanceRunState::Yielded);
        assert_eq!(status.opportunity_epoch, 9);
        assert!(
            status
                .cursor
                .as_deref()
                .is_some_and(|cursor| cursor.starts_with("retire:destructive_ready:"))
        );

        let mut recovery_slices = 0;
        loop {
            let mut recovery = fixture.acquire(9);
            match execute_event_retirement_slice(
                &fixture.event_root,
                &fixture.request,
                EventRetirementWorkLimits::default(),
                &mut recovery,
            )
            .unwrap()
            {
                EventRetirementSlice::Retired { .. } => break,
                EventRetirementSlice::MoreWork {
                    phase: "trash_unlink",
                } => recovery
                    .yield_run("trash_unlink", "recovery slice", now_unix_micros())
                    .unwrap(),
                outcome => panic!("unexpected recovery outcome {outcome:?}"),
            }
            recovery_slices += 1;
            assert!(recovery_slices < 4);
        }
        assert!(!fixture.pending().exists());
    }

    #[test]
    fn exact_partition_fence_rejects_different_generation() {
        let fixture = Fixture::new(false, 1);
        let store = fixture.store();
        let wrong = MaintenanceJobKey::new(
            MaintenanceJobKind::EventRetirement,
            format!(
                "{}/{}",
                id_hex(fixture.request.writer_instance_id.as_bytes()),
                id_hex(fixture.successor.as_bytes())
            ),
        )
        .unwrap();
        let owner = MaintenanceWorkerIdentity {
            worker_instance_id: Uuid::new_v4(),
            os_pid: 1,
            os_boot_id: "boot".into(),
            os_pid_starttime_ticks: 1,
            schedule_basis: "test".into(),
            launch_id: None,
        };
        let AcquireMaintenanceJob::Acquired(mut job) = store.acquire(wrong, 9, owner, 10).unwrap()
        else {
            panic!();
        };
        assert!(matches!(
            execute_event_retirement_slice(
                &fixture.event_root,
                &fixture.request,
                EventRetirementWorkLimits::default(),
                &mut job,
            ),
            Err(EventRetirementError::IdentityMismatch)
        ));
        assert!(fixture.original().exists());
    }

    #[test]
    fn restart_after_directory_move_finishes_receipt_and_unlink() {
        let fixture = Fixture::new(false, 1);
        let mut crashed_job = fixture.acquire(9);
        let source = fixture.original();
        let lease = fixture
            .layout
            .acquire_generation_maintenance_lease(fixture.old)
            .unwrap();
        checkpoint_closed_generation_under_lease(&source, &lease).unwrap();
        let (_, sealed_digest) =
            seal_closed_generation_under_lease(&source, false, &lease).unwrap();
        prepare_pending_parent(
            &fixture.event_root,
            &id_hex(fixture.request.writer_instance_id.as_bytes()),
        )
        .unwrap();
        crashed_job
            .record_progress(
                "destructive_ready",
                Some(format!(
                    "retire:destructive_ready:{}",
                    digest_text(sealed_digest.as_bytes())
                )),
                1,
                0,
                now_unix_micros(),
            )
            .unwrap();
        move_generation_class(
            &fixture.event_root,
            fixture.request.writer_instance_id,
            fixture.request.generation_id,
            DiscoveryClass::Active,
            DiscoveryClass::Pending,
            DiscoveryPhase::PendingTrash,
        )
        .unwrap();
        fs::rename(&source, fixture.pending()).unwrap();
        drop(lease);
        drop(crashed_job);
        assert!(!source.exists());
        assert!(fixture.pending().exists());

        let mut job = fixture.acquire(9);
        let outcome = execute_event_retirement_slice(
            &fixture.event_root,
            &fixture.request,
            EventRetirementWorkLimits {
                max_validation_rows: 10,
                max_validation_bytes: 128 * 1024 * 1024,
                max_unlink_entries: 10,
            },
            &mut job,
        )
        .unwrap();
        assert!(matches!(outcome, EventRetirementSlice::Retired { .. }));
        assert!(!fixture.pending().exists());
    }
}
