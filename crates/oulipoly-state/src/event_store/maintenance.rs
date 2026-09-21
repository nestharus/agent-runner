//! Non-destructive metadata and work boundaries consumed by AGE-372 policy
//! and AGE-377 detached maintenance. This module deliberately does not
//! schedule work, acquire singleton jobs, quarantine, compact, or delete.

use super::schema::{GenerationState, REQUIRED_INDEXES, read_generation_metadata};
use super::{Digest32, GenerationId, WriterInstanceId};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::path::PathBuf;

pub const CATALOG_FORMAT_VERSION: i64 = 1;
pub const CATALOG_BLOOM_BYTES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartitionLifecycleState {
    Prepared,
    Writable,
    Closed,
    RetirementInProgress,
    Retired,
    Missing,
    Corrupt,
    Quarantined,
}

/// Immutable facts supplied to retention policy. The policy decision and its
/// cutoff/version belong to AGE-372; this type grants no deletion authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationEligibilityFacts {
    pub writer_instance_id: WriterInstanceId,
    pub generation_id: GenerationId,
    pub state: PartitionLifecycleState,
    pub prepared_manifest_valid: bool,
    pub sealed_manifest_valid: bool,
    pub selected_head: bool,
    pub hold_present: bool,
    pub corruption_present: bool,
    pub unclassified_legacy_input: bool,
    pub closed_at_unix_micros: Option<i64>,
    pub max_ingested_at_unix_micros: Option<i64>,
    /// Required to distinguish a proven empty generation from missing
    /// max-ingestion evidence for a nonempty generation.
    pub row_count: Option<u64>,
}

impl GenerationEligibilityFacts {
    /// Returns the later known close/ingestion timestamp only when every
    /// structural prerequisite for policy evaluation is proven. `None` is a
    /// fail-closed "insufficient facts", never an ineligible/eligible verdict.
    pub fn retention_reference_unix_micros(&self) -> Option<i64> {
        if self.state != PartitionLifecycleState::Closed
            || !self.prepared_manifest_valid
            || !self.sealed_manifest_valid
            || self.selected_head
            || self.hold_present
            || self.corruption_present
            || self.unclassified_legacy_input
        {
            return None;
        }
        let closed = self.closed_at_unix_micros?;
        match (self.row_count?, self.max_ingested_at_unix_micros) {
            (0, None) => Some(closed),
            (_, Some(max_ingested)) => Some(closed.max(max_ingested)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationMaintenanceTarget {
    pub writer_instance_id: WriterInstanceId,
    pub generation_id: GenerationId,
    pub generation_directory: PathBuf,
    pub lease_path: PathBuf,
    pub database_relative_name: String,
    pub prepared_manifest_sha256: Digest32,
    pub sealed_manifest_sha256: Option<Digest32>,
    /// Final database digest supplied by a validated sealed manifest. Catalog
    /// inventory does not rescan an unbounded database file to manufacture it.
    pub database_file_sha256: Option<Digest32>,
    pub state: PartitionLifecycleState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceOperation {
    CatalogBuild,
    OrphanAudit,
    IntegrityScan,
    IndexRebuild,
    RepairCopy,
    Compaction,
    Quarantine,
    Retirement,
}

/// Resume cursor for bounded detached scans. It is an input/output DTO only;
/// AGE-376 never creates a background worker from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RebuildCursor {
    pub operation: MaintenanceOperation,
    pub writer_instance_id: WriterInstanceId,
    pub generation_id: GenerationId,
    pub after_local_sequence: i64,
    pub source_manifest_sha256: Digest32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogPartitionEntry {
    pub writer_instance_id: WriterInstanceId,
    pub generation_id: GenerationId,
    pub prepared_manifest_sha256: Digest32,
    pub sealed_manifest_sha256: Option<Digest32>,
    pub file_identity_sha256: Digest32,
    pub schema_version: i64,
    pub min_recorded_at_unix_micros: Option<i64>,
    pub max_recorded_at_unix_micros: Option<i64>,
    pub min_ingested_at_unix_micros: Option<i64>,
    pub max_ingested_at_unix_micros: Option<i64>,
    pub family_bitset: u64,
    pub row_count: u64,
    /// Opaque, versioned exclusion-only filters. A negative Bloom result may
    /// skip a partition; a positive result must still query the local index.
    pub trace_bloom_v1: Vec<u8>,
    pub invocation_bloom_v1: Vec<u8>,
    pub process_bloom_v1: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogManifestWatermark {
    pub writer_instance_id: WriterInstanceId,
    pub generation_id: GenerationId,
    pub prepared_manifest_sha256: Digest32,
}

/// Contents of one create-once `CURRENT.0` / `CURRENT.1` slot. AGE-377 owns
/// file publication; AGE-376 owns deterministic encoding and validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogCurrentRecord {
    pub format_version: i64,
    pub epoch: i64,
    pub catalog_file_name: String,
    pub catalog_file_sha256: Digest32,
    pub manifest_watermarks: Vec<CatalogManifestWatermark>,
    pub checksum_sha256: Digest32,
}

impl CatalogCurrentRecord {
    pub fn checked(
        epoch: i64,
        catalog_file_name: String,
        catalog_file_sha256: Digest32,
        mut manifest_watermarks: Vec<CatalogManifestWatermark>,
    ) -> Result<Self, MaintenanceInspectionError> {
        if epoch < 0 || !valid_catalog_file_name(&catalog_file_name) {
            return Err(MaintenanceInspectionError::InvalidCatalogCurrent(
                "invalid epoch or catalog filename".to_string(),
            ));
        }
        manifest_watermarks.sort_by_key(|item| (item.writer_instance_id, item.generation_id));
        let mut value = Self {
            format_version: CATALOG_FORMAT_VERSION,
            epoch,
            catalog_file_name,
            catalog_file_sha256,
            manifest_watermarks,
            checksum_sha256: Digest32::from_bytes([0; 32]),
        };
        value.checksum_sha256 = value.expected_checksum()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), MaintenanceInspectionError> {
        if self.format_version != CATALOG_FORMAT_VERSION
            || self.epoch < 0
            || !valid_catalog_file_name(&self.catalog_file_name)
            || self.manifest_watermarks.windows(2).any(|items| {
                (items[0].writer_instance_id, items[0].generation_id)
                    >= (items[1].writer_instance_id, items[1].generation_id)
            })
            || self.expected_checksum()? != self.checksum_sha256
        {
            return Err(MaintenanceInspectionError::InvalidCatalogCurrent(
                "catalog current record failed identity/order/checksum validation".to_string(),
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, MaintenanceInspectionError> {
        self.validate()?;
        let mut bytes = serde_json::to_vec(self)
            .map_err(|error| MaintenanceInspectionError::Json(error.to_string()))?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    fn expected_checksum(&self) -> Result<Digest32, MaintenanceInspectionError> {
        #[derive(Serialize)]
        struct Checksum<'a> {
            format_version: i64,
            epoch: i64,
            catalog_file_name: &'a str,
            catalog_file_sha256: Digest32,
            manifest_watermarks: &'a [CatalogManifestWatermark],
        }
        let encoded = serde_json::to_vec(&Checksum {
            format_version: self.format_version,
            epoch: self.epoch,
            catalog_file_name: &self.catalog_file_name,
            catalog_file_sha256: self.catalog_file_sha256,
            manifest_watermarks: &self.manifest_watermarks,
        })
        .map_err(|error| MaintenanceInspectionError::Json(error.to_string()))?;
        let mut digest = Sha256::new();
        digest.update(b"oulipoly.event-catalog-current.v1\0");
        digest.update(encoded);
        Ok(Digest32::from_bytes(digest.finalize().into()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogCurrentSlot {
    Zero,
    One,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogCurrentIssue {
    MissingSlot {
        slot: CatalogCurrentSlot,
    },
    InvalidSlot {
        slot: CatalogCurrentSlot,
        reason: String,
    },
    PublicationIncomplete {
        invalid_slot: CatalogCurrentSlot,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogCurrentSelection {
    pub selected_slot: Option<CatalogCurrentSlot>,
    pub selected: Option<CatalogCurrentRecord>,
    pub issues: Vec<CatalogCurrentIssue>,
}

/// Validate exactly the two bounded `CURRENT.0/.1` slot contents. It never
/// enumerates catalog files. Equal-epoch disagreement is a hard conflict; a
/// torn higher epoch leaves the older slot selected with explicit incomplete
/// publication coverage. The caller-supplied byte ceiling is checked before
/// either slot is decoded.
pub fn select_catalog_current_slots(
    slot_zero: Option<&[u8]>,
    slot_one: Option<&[u8]>,
    max_slot_bytes: usize,
) -> Result<CatalogCurrentSelection, MaintenanceInspectionError> {
    if max_slot_bytes == 0 {
        return Err(MaintenanceInspectionError::InvalidBound);
    }
    if [slot_zero, slot_one]
        .into_iter()
        .flatten()
        .any(|bytes| bytes.len() > max_slot_bytes)
    {
        return Err(MaintenanceInspectionError::WorkLimitReached {
            bytes_examined: u64::try_from(max_slot_bytes).unwrap_or(u64::MAX),
        });
    }
    enum SlotRead {
        Missing,
        Valid(CatalogCurrentRecord),
        Invalid {
            epoch_hint: Option<i64>,
            reason: String,
        },
    }
    fn read(bytes: Option<&[u8]>) -> SlotRead {
        let Some(bytes) = bytes else {
            return SlotRead::Missing;
        };
        let epoch_hint = serde_json::from_slice::<serde_json::Value>(bytes)
            .ok()
            .and_then(|value| value.get("epoch")?.as_i64());
        match serde_json::from_slice::<CatalogCurrentRecord>(bytes) {
            Ok(record) => match record.validate().and_then(|()| {
                (record.canonical_bytes()? == bytes)
                    .then_some(())
                    .ok_or_else(|| {
                        MaintenanceInspectionError::InvalidCatalogCurrent(
                            "slot is not canonically encoded".to_string(),
                        )
                    })
            }) {
                Ok(()) => SlotRead::Valid(record),
                Err(error) => SlotRead::Invalid {
                    epoch_hint,
                    reason: error.to_string(),
                },
            },
            Err(error) => SlotRead::Invalid {
                epoch_hint,
                reason: error.to_string(),
            },
        }
    }

    let reads = [read(slot_zero), read(slot_one)];
    let slots = [CatalogCurrentSlot::Zero, CatalogCurrentSlot::One];
    let mut issues = Vec::new();
    let mut valid = Vec::new();
    for (slot, read) in slots.into_iter().zip(reads) {
        match read {
            SlotRead::Missing => issues.push(CatalogCurrentIssue::MissingSlot { slot }),
            SlotRead::Invalid { epoch_hint, reason } => {
                issues.push(CatalogCurrentIssue::InvalidSlot { slot, reason });
                valid.push((slot, None, epoch_hint));
            }
            SlotRead::Valid(record) => valid.push((slot, Some(record), None)),
        }
    }
    let mut candidates = valid
        .iter()
        .filter_map(|(slot, record, _)| record.clone().map(|record| (*slot, record)))
        .collect::<Vec<_>>();
    candidates.sort_by_key(|(_, record)| record.epoch);
    if candidates.len() == 2
        && candidates[0].1.epoch == candidates[1].1.epoch
        && candidates[0].1 != candidates[1].1
    {
        return Err(MaintenanceInspectionError::InvalidCatalogCurrent(
            "equal catalog epochs disagree".to_string(),
        ));
    }
    let selected = candidates.pop();
    if let Some((_, selected_record)) = &selected {
        for (slot, record, epoch_hint) in &valid {
            if record.is_none() && epoch_hint.is_some_and(|epoch| epoch > selected_record.epoch) {
                issues.push(CatalogCurrentIssue::PublicationIncomplete {
                    invalid_slot: *slot,
                });
            }
        }
    }
    if selected.is_some() {
        issues.retain(|issue| !matches!(issue, CatalogCurrentIssue::MissingSlot { .. }));
    }
    Ok(CatalogCurrentSelection {
        selected_slot: selected.as_ref().map(|(slot, _)| *slot),
        selected: selected.map(|(_, record)| record),
        issues,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogInventoryResult {
    Entry(Box<CatalogPartitionEntry>),
    WorkLimitReached { rows_examined: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexRebuildAction {
    None,
    ReindexClosedGeneration,
    RepairCopyRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexRebuildPlan {
    pub writer_instance_id: WriterInstanceId,
    pub generation_id: GenerationId,
    pub missing_indexes: Vec<String>,
    pub action: IndexRebuildAction,
}

/// Inspect at most `max_rows` records of a closed generation and build the
/// derived catalog entry. If there is one more row, no partial entry is
/// returned; AGE-377 receives a resumable work-limit result instead.
pub fn inspect_closed_generation_for_catalog(
    target: &GenerationMaintenanceTarget,
    max_rows: u64,
    max_file_bytes: u64,
) -> Result<CatalogInventoryResult, MaintenanceInspectionError> {
    if max_rows == 0 || max_file_bytes == 0 {
        return Err(MaintenanceInspectionError::InvalidBound);
    }
    let (_lease, connection, metadata, sealed) =
        open_validated_closed_target(target, max_file_bytes)?;
    let database_sha256 = sealed
        .durable_files
        .iter()
        .find(|file| file.relative_path == target.database_relative_name)
        .ok_or(MaintenanceInspectionError::UnsealedGeneration)?
        .sha256;
    let mut statement = connection.prepare(
        "SELECT family, recorded_at_unix_micros, ingested_at_unix_micros,
                trace_id, invocation_uuid, process_instance_id
           FROM events ORDER BY local_sequence LIMIT ?1",
    )?;
    let sql_limit = i64::try_from(max_rows.saturating_add(1)).unwrap_or(i64::MAX);
    let mut rows = statement.query([sql_limit])?;
    let mut row_count = 0u64;
    let mut min_recorded = None;
    let mut max_recorded = None;
    let mut min_ingested = None;
    let mut max_ingested = None;
    let mut family_bitset = 0u64;
    let mut trace_bloom = vec![0; CATALOG_BLOOM_BYTES];
    let mut invocation_bloom = vec![0; CATALOG_BLOOM_BYTES];
    let mut process_bloom = vec![0; CATALOG_BLOOM_BYTES];
    while let Some(row) = rows.next()? {
        row_count += 1;
        if row_count > max_rows {
            return Ok(CatalogInventoryResult::WorkLimitReached {
                rows_examined: row_count,
            });
        }
        let family: i64 = row.get(0)?;
        let recorded: i64 = row.get(1)?;
        let ingested: i64 = row.get(2)?;
        family_bitset |= 1u64
            .checked_shl(u32::try_from(family).unwrap_or(63))
            .unwrap_or(0);
        min_recorded = Some(min_recorded.map_or(recorded, |value: i64| value.min(recorded)));
        max_recorded = Some(max_recorded.map_or(recorded, |value: i64| value.max(recorded)));
        min_ingested = Some(min_ingested.map_or(ingested, |value: i64| value.min(ingested)));
        max_ingested = Some(max_ingested.map_or(ingested, |value: i64| value.max(ingested)));
        if let Some(value) = row.get::<_, Option<Vec<u8>>>(3)? {
            bloom_insert(&mut trace_bloom, &value);
        }
        if let Some(value) = row.get::<_, Option<Vec<u8>>>(4)? {
            bloom_insert(&mut invocation_bloom, &value);
        }
        bloom_insert(&mut process_bloom, &row.get::<_, Vec<u8>>(5)?);
    }
    let high_water = connection.query_row("SELECT MAX(local_sequence) FROM events", [], |row| {
        row.get::<_, Option<i64>>(0)
    })?;
    if sealed.row_count != row_count
        || sealed.high_water_local_sequence
            != high_water.and_then(|value| u64::try_from(value).ok())
        || sealed.min_recorded_at_unix_micros != min_recorded
        || sealed.max_recorded_at_unix_micros != max_recorded
        || sealed.min_ingested_at_unix_micros != min_ingested
        || sealed.max_ingested_at_unix_micros != max_ingested
    {
        return Err(MaintenanceInspectionError::InvalidGeneration(
            "sealed manifest row/time watermarks differ from SQLite".to_string(),
        ));
    }
    Ok(CatalogInventoryResult::Entry(Box::new(
        CatalogPartitionEntry {
            writer_instance_id: target.writer_instance_id,
            generation_id: target.generation_id,
            prepared_manifest_sha256: target.prepared_manifest_sha256,
            sealed_manifest_sha256: target.sealed_manifest_sha256,
            file_identity_sha256: database_sha256,
            schema_version: metadata.schema_version,
            min_recorded_at_unix_micros: min_recorded,
            max_recorded_at_unix_micros: max_recorded,
            min_ingested_at_unix_micros: min_ingested,
            max_ingested_at_unix_micros: max_ingested,
            family_bitset,
            row_count,
            trace_bloom_v1: trace_bloom,
            invocation_bloom_v1: invocation_bloom,
            process_bloom_v1: process_bloom,
        },
    )))
}

/// Verify only index presence and return a non-destructive action plan. Missing
/// indexes on a healthy closed database may be rebuilt by AGE-377 with REINDEX;
/// any live/corrupt target requires a repair copy instead of in-place writes.
pub fn plan_local_index_rebuild(
    target: &GenerationMaintenanceTarget,
    max_file_bytes: u64,
) -> Result<IndexRebuildPlan, MaintenanceInspectionError> {
    if max_file_bytes == 0 {
        return Err(MaintenanceInspectionError::InvalidBound);
    }
    let (_lease, connection, _metadata, _sealed) =
        open_validated_closed_target(target, max_file_bytes)?;
    let mut missing_indexes = Vec::new();
    for index in REQUIRED_INDEXES {
        let present: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_index_list('events') WHERE name = ?1)",
            [index],
            |row| row.get(0),
        )?;
        if !present {
            missing_indexes.push((*index).to_string());
        }
    }
    let action = if missing_indexes.is_empty() {
        IndexRebuildAction::None
    } else {
        IndexRebuildAction::RepairCopyRequired
    };
    Ok(IndexRebuildPlan {
        writer_instance_id: target.writer_instance_id,
        generation_id: target.generation_id,
        missing_indexes,
        action,
    })
}

fn valid_catalog_file_name(value: &str) -> bool {
    let Some(epoch) = value
        .strip_prefix("catalog-")
        .and_then(|value| value.strip_suffix(".sqlite3"))
    else {
        return false;
    };
    !epoch.is_empty() && epoch.bytes().all(|byte| byte.is_ascii_digit())
}

fn bloom_insert(bloom: &mut [u8], value: &[u8]) {
    let digest = Sha256::digest(value);
    for pair in digest[..8].chunks_exact(2) {
        let bit = usize::from(u16::from_be_bytes([pair[0], pair[1]])) % (bloom.len() * 8);
        bloom[bit / 8] |= 1 << (bit % 8);
    }
}

fn open_read_only(path: &std::path::Path) -> Result<Connection, MaintenanceInspectionError> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.pragma_update(None, "query_only", true)?;
    Ok(connection)
}

fn open_validated_closed_target(
    target: &GenerationMaintenanceTarget,
    max_file_bytes: u64,
) -> Result<
    (
        super::GenerationReaderLease,
        Connection,
        super::GenerationMetadata,
        super::SealedManifest,
    ),
    MaintenanceInspectionError,
> {
    if target.state != PartitionLifecycleState::Closed
        || target.database_relative_name != super::DATABASE_FILE_NAME
    {
        return Err(MaintenanceInspectionError::UnsealedGeneration);
    }
    let lease = super::acquire_generation_reader_lease_at(&target.lease_path)
        .map_err(|error| MaintenanceInspectionError::Generation(error.to_string()))?;
    let prepared = super::read_prepared_manifest(&target.generation_directory)
        .map_err(|error| MaintenanceInspectionError::Generation(error.to_string()))?;
    let prepared_digest = prepared
        .sha256()
        .map_err(|error| MaintenanceInspectionError::Generation(error.to_string()))?;
    if prepared.writer_instance_id != target.writer_instance_id
        || prepared.generation_id != target.generation_id
        || prepared_digest != target.prepared_manifest_sha256
    {
        return Err(MaintenanceInspectionError::IdentityMismatch);
    }
    let sealed_path = target
        .generation_directory
        .join(super::SEALED_MANIFEST_FILE_NAME);
    let sealed_bytes = super::generation::read_bounded_control_file(&sealed_path)
        .map_err(|error| MaintenanceInspectionError::Generation(error.to_string()))?;
    let sealed: super::SealedManifest = serde_json::from_slice(&sealed_bytes)
        .map_err(|error| MaintenanceInspectionError::Json(error.to_string()))?;
    sealed
        .validate()
        .map_err(|error| MaintenanceInspectionError::Generation(error.to_string()))?;
    let mut canonical_sealed = serde_json::to_vec(&sealed)
        .map_err(|error| MaintenanceInspectionError::Json(error.to_string()))?;
    canonical_sealed.push(b'\n');
    let sealed_digest = Digest32::sha256(&canonical_sealed);
    if canonical_sealed != sealed_bytes
        || target.sealed_manifest_sha256 != Some(sealed_digest)
        || sealed.writer_instance_id != target.writer_instance_id
        || sealed.generation_id != target.generation_id
        || sealed.prepared_manifest_sha256 != prepared_digest
    {
        return Err(MaintenanceInspectionError::IdentityMismatch);
    }
    let database_identity = sealed
        .durable_files
        .iter()
        .find(|file| file.relative_path == super::DATABASE_FILE_NAME)
        .ok_or(MaintenanceInspectionError::UnsealedGeneration)?;
    if target
        .database_file_sha256
        .is_some_and(|digest| digest != database_identity.sha256)
    {
        return Err(MaintenanceInspectionError::IdentityMismatch);
    }
    let mut bytes_examined = 0u64;
    for identity in &sealed.durable_files {
        bytes_examined = bytes_examined.saturating_add(identity.bytes);
        if bytes_examined > max_file_bytes {
            return Err(MaintenanceInspectionError::WorkLimitReached { bytes_examined });
        }
        let path = target.generation_directory.join(&identity.relative_path);
        let metadata = std::fs::metadata(&path)
            .map_err(|error| MaintenanceInspectionError::Generation(error.to_string()))?;
        if metadata.len() != identity.bytes
            || hash_file_bounded(&path, identity.bytes)? != identity.sha256
        {
            return Err(MaintenanceInspectionError::IdentityMismatch);
        }
    }
    let database_path = target
        .generation_directory
        .join(&target.database_relative_name);
    if !database_path.is_file() {
        return Err(MaintenanceInspectionError::InvalidGeneration(
            "sealed database is missing".to_string(),
        ));
    }
    let connection = open_read_only(&database_path)?;
    let metadata = read_generation_metadata(&connection)
        .map_err(|error| MaintenanceInspectionError::InvalidGeneration(error.to_string()))?;
    if metadata.state != GenerationState::Closed
        || metadata.generation_id != target.generation_id
        || metadata.writer_instance_id != target.writer_instance_id
        || metadata.schema_version != prepared.schema_version
        || metadata.predecessor_generation_id != prepared.predecessor_generation_id
        || metadata.process_instance_id != prepared.producer.process_instance_id
        || metadata.process_root_id != prepared.producer.process_root_id
        || metadata.parent_process_instance_id != prepared.producer.parent_process_instance_id
        || metadata.supervisor_authority_id != prepared.producer.supervisor_authority_id
        || metadata.native_process != prepared.producer.native_process
        || metadata.created_at_unix_micros != prepared.created_at_unix_micros
        || metadata.head_epoch != prepared.head_epoch
    {
        return Err(MaintenanceInspectionError::IdentityMismatch);
    }
    Ok((lease, connection, metadata, sealed))
}

fn hash_file_bounded(
    path: &std::path::Path,
    expected_bytes: u64,
) -> Result<Digest32, MaintenanceInspectionError> {
    use std::io::Read;
    const BUFFER_BYTES: usize = 64 * 1024;
    let mut file = std::fs::File::open(path)
        .map_err(|error| MaintenanceInspectionError::Generation(error.to_string()))?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; BUFFER_BYTES];
    let mut read_total = 0u64;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| MaintenanceInspectionError::Generation(error.to_string()))?;
        if read == 0 {
            break;
        }
        read_total = read_total.saturating_add(read as u64);
        if read_total > expected_bytes {
            return Err(MaintenanceInspectionError::IdentityMismatch);
        }
        digest.update(&buffer[..read]);
    }
    if read_total != expected_bytes {
        return Err(MaintenanceInspectionError::IdentityMismatch);
    }
    Ok(Digest32::from_bytes(digest.finalize().into()))
}

#[derive(Debug)]
pub enum MaintenanceInspectionError {
    Sqlite(rusqlite::Error),
    InvalidGeneration(String),
    Generation(String),
    IdentityMismatch,
    UnsealedGeneration,
    InvalidBound,
    WorkLimitReached { bytes_examined: u64 },
    InvalidCatalogCurrent(String),
    Json(String),
}

impl fmt::Display for MaintenanceInspectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => {
                write!(formatter, "maintenance inspection SQLite error: {error}")
            }
            Self::InvalidGeneration(error) => write!(formatter, "invalid generation: {error}"),
            Self::Generation(error) => write!(formatter, "generation protocol error: {error}"),
            Self::IdentityMismatch => write!(formatter, "maintenance target identity mismatch"),
            Self::UnsealedGeneration => write!(
                formatter,
                "maintenance target is not a sealed closed generation"
            ),
            Self::InvalidBound => {
                write!(formatter, "maintenance inspection bound must be non-zero")
            }
            Self::WorkLimitReached { bytes_examined } => write!(
                formatter,
                "maintenance inspection file-byte limit reached after {bytes_examined} bytes"
            ),
            Self::InvalidCatalogCurrent(error) => {
                write!(formatter, "invalid catalog current record: {error}")
            }
            Self::Json(error) => write!(formatter, "catalog JSON error: {error}"),
        }
    }
}

impl std::error::Error for MaintenanceInspectionError {}

impl From<rusqlite::Error> for MaintenanceInspectionError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Sqlite(value)
    }
}

/// Isolated fixture handed to AGE-372/AGE-377 tests. It carries exact source
/// facts and expected output paths but performs no filesystem mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaintenanceFixture {
    pub target: GenerationMaintenanceTarget,
    pub eligibility: GenerationEligibilityFacts,
    pub catalog_entry: Option<CatalogPartitionEntry>,
    pub replacement_generation_id: Option<GenerationId>,
    pub quarantine_destination: Option<PathBuf>,
    pub pending_trash_destination: Option<PathBuf>,
    pub receipt_destination: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_store::{
        EventCorrelations, EventEnvelopeV1, EventFamily, EventId, EventKind, GenerationSource,
        NativeProcessIdentity, NewEventV1, PayloadNormalizationPolicy, PreparedManifest,
        ProcessInstanceId, ProducerIdentity, SupervisorAuthorityId, WriterLayout, append_batch,
        close_generation, initialize_generation_schema, mark_generation_writable,
        seal_closed_generation,
    };
    use rusqlite::Connection;
    use serde_json::json;

    struct ClosedFixture {
        _root: tempfile::TempDir,
        target: GenerationMaintenanceTarget,
    }

    fn producer() -> ProducerIdentity {
        ProducerIdentity {
            writer_instance_id: WriterInstanceId::from_bytes([8; 16]),
            process_instance_id: ProcessInstanceId::from_bytes([9; 16]),
            process_root_id: ProcessInstanceId::from_bytes([10; 16]),
            parent_process_instance_id: None,
            supervisor_authority_id: Some(SupervisorAuthorityId::from_bytes([11; 16])),
            native_process: Some(NativeProcessIdentity {
                os_pid: 80,
                os_boot_id_sha256: Digest32::from_bytes([12; 32]),
                os_pid_starttime_ticks: 90,
            }),
        }
    }

    fn event(id: u8, sequence: i64) -> EventEnvelopeV1 {
        EventEnvelopeV1::normalize(
            NewEventV1 {
                event_id: EventId::from_bytes([id; 16]),
                family: EventFamily::Maintenance,
                kind: EventKind::registered("maintenance.fixture").unwrap(),
                recorded_at_unix_micros: 10 + sequence,
                producer_sequence: sequence,
                producer: producer(),
                correlations: EventCorrelations::default(),
                payload: json!({"value": sequence}),
                legacy_provenance: None,
                retry_of_generation_id: None,
            },
            &PayloadNormalizationPolicy::registered(&["value"]).unwrap(),
            20 + sequence,
        )
        .unwrap()
    }

    fn closed_fixture(drop_index: bool) -> ClosedFixture {
        let root = tempfile::tempdir().unwrap();
        let layout = WriterLayout::create(root.path(), [8; 16]).unwrap();
        let generation_id = GenerationId::from_bytes([13; 16]);
        let manifest = PreparedManifest {
            format_version: super::super::EVENT_STORE_FORMAT_VERSION,
            schema_version: super::super::EVENT_SCHEMA_VERSION,
            writer_instance_id: WriterInstanceId::from_bytes([8; 16]),
            generation_id,
            predecessor_generation_id: None,
            database_relative_path: super::super::DATABASE_FILE_NAME.to_string(),
            created_at_unix_micros: 1,
            head_epoch: 0,
            source: GenerationSource::NativeProcess,
            producer: producer(),
        };
        let published = layout
            .publish_prepared_generation(
                &manifest,
                |path| {
                    let mut connection = Connection::open(path)?;
                    initialize_generation_schema(&mut connection, &manifest.generation_metadata()?)
                        .map_err(|error| {
                            super::super::GenerationError::Validation(error.to_string())
                        })
                },
                |_, _| Ok(()),
            )
            .unwrap();
        drop(layout.acquire_generation_lease(generation_id).unwrap());
        let mut connection = Connection::open(&published.database_path).unwrap();
        connection
            .pragma_update(None, "synchronous", "FULL")
            .unwrap();
        mark_generation_writable(&connection).unwrap();
        append_batch(&mut connection, generation_id, &[event(1, 0), event(2, 1)]).unwrap();
        if drop_index {
            connection
                .execute_batch("DROP INDEX events_trace_span_idx")
                .unwrap();
        }
        close_generation(&mut connection, 100, GenerationId::from_bytes([14; 16])).unwrap();
        drop(connection);
        let (sealed, sealed_digest) =
            seal_closed_generation(&published.generation_dir, false).unwrap();
        let database_digest = sealed
            .durable_files
            .iter()
            .find(|file| file.relative_path == super::super::DATABASE_FILE_NAME)
            .unwrap()
            .sha256;
        ClosedFixture {
            _root: root,
            target: GenerationMaintenanceTarget {
                writer_instance_id: WriterInstanceId::from_bytes([8; 16]),
                generation_id,
                generation_directory: published.generation_dir,
                lease_path: layout.writer_dir().join("leases").join(format!(
                    "{}.lock",
                    super::super::id_hex(generation_id.as_bytes())
                )),
                database_relative_name: super::super::DATABASE_FILE_NAME.to_string(),
                prepared_manifest_sha256: published.prepared_manifest_sha256,
                sealed_manifest_sha256: Some(sealed_digest),
                database_file_sha256: Some(database_digest),
                state: PartitionLifecycleState::Closed,
            },
        }
    }

    #[test]
    fn eligibility_facts_fail_closed_without_every_required_fact() {
        let mut facts = GenerationEligibilityFacts {
            writer_instance_id: WriterInstanceId::from_bytes([1; 16]),
            generation_id: GenerationId::from_bytes([2; 16]),
            state: PartitionLifecycleState::Closed,
            prepared_manifest_valid: true,
            sealed_manifest_valid: true,
            selected_head: false,
            hold_present: false,
            corruption_present: false,
            unclassified_legacy_input: false,
            closed_at_unix_micros: Some(10),
            max_ingested_at_unix_micros: Some(20),
            row_count: Some(1),
        };
        assert_eq!(facts.retention_reference_unix_micros(), Some(20));
        facts.hold_present = true;
        assert_eq!(facts.retention_reference_unix_micros(), None);
        facts.hold_present = false;
        facts.max_ingested_at_unix_micros = None;
        assert_eq!(facts.retention_reference_unix_micros(), None);
        facts.row_count = Some(0);
        assert_eq!(facts.retention_reference_unix_micros(), Some(10));
    }

    #[test]
    fn catalog_inventory_is_bounded_and_derived_from_validated_closed_generation() {
        let fixture = closed_fixture(false);
        assert!(matches!(
            inspect_closed_generation_for_catalog(&fixture.target, 1, 128 * 1024 * 1024).unwrap(),
            CatalogInventoryResult::WorkLimitReached { rows_examined: 2 }
        ));
        let entry =
            match inspect_closed_generation_for_catalog(&fixture.target, 10, 128 * 1024 * 1024)
                .unwrap()
            {
                CatalogInventoryResult::Entry(entry) => entry,
                other => panic!("unexpected inventory: {other:?}"),
            };
        assert_eq!(entry.row_count, 2);
        assert_ne!(entry.family_bitset, 0);
        assert_eq!(entry.trace_bloom_v1.len(), CATALOG_BLOOM_BYTES);
        assert!(matches!(
            inspect_closed_generation_for_catalog(&fixture.target, 10, 1),
            Err(MaintenanceInspectionError::WorkLimitReached { .. })
        ));
    }

    #[test]
    fn index_plan_is_non_destructive_and_missing_index_requires_repair_copy() {
        let healthy = closed_fixture(false);
        let healthy_plan = plan_local_index_rebuild(&healthy.target, 128 * 1024 * 1024).unwrap();
        assert_eq!(healthy_plan.action, IndexRebuildAction::None);
        let missing = closed_fixture(true);
        let missing_plan = plan_local_index_rebuild(&missing.target, 128 * 1024 * 1024).unwrap();
        assert_eq!(missing_plan.action, IndexRebuildAction::RepairCopyRequired);
        assert_eq!(missing_plan.missing_indexes, vec!["events_trace_span_idx"]);
    }

    #[test]
    fn catalog_current_checksum_and_watermark_order_are_validated() {
        let mut current = CatalogCurrentRecord::checked(
            3,
            "catalog-3.sqlite3".to_string(),
            Digest32::from_bytes([1; 32]),
            vec![
                CatalogManifestWatermark {
                    writer_instance_id: WriterInstanceId::from_bytes([2; 16]),
                    generation_id: GenerationId::from_bytes([2; 16]),
                    prepared_manifest_sha256: Digest32::from_bytes([2; 32]),
                },
                CatalogManifestWatermark {
                    writer_instance_id: WriterInstanceId::from_bytes([1; 16]),
                    generation_id: GenerationId::from_bytes([1; 16]),
                    prepared_manifest_sha256: Digest32::from_bytes([1; 32]),
                },
            ],
        )
        .unwrap();
        current.validate().unwrap();
        assert!(
            current.manifest_watermarks[0].writer_instance_id
                < current.manifest_watermarks[1].writer_instance_id
        );
        let first_bytes = current.canonical_bytes().unwrap();
        let second = CatalogCurrentRecord::checked(
            4,
            "catalog-4.sqlite3".to_string(),
            Digest32::from_bytes([4; 32]),
            Vec::new(),
        )
        .unwrap();
        let selected = select_catalog_current_slots(
            Some(&first_bytes),
            Some(&second.canonical_bytes().unwrap()),
            64 * 1024,
        )
        .unwrap();
        assert_eq!(selected.selected.unwrap().epoch, 4);

        let torn_higher = br#"{"epoch":5"#;
        let selected =
            select_catalog_current_slots(Some(&first_bytes), Some(torn_higher), 64 * 1024).unwrap();
        assert_eq!(selected.selected.unwrap().epoch, 3);
        assert!(
            selected
                .issues
                .iter()
                .any(|issue| matches!(issue, CatalogCurrentIssue::InvalidSlot { .. }))
        );
        assert!(matches!(
            select_catalog_current_slots(Some(&first_bytes), None, 1),
            Err(MaintenanceInspectionError::WorkLimitReached { .. })
        ));

        current.epoch += 1;
        assert!(current.validate().is_err());
    }
}
