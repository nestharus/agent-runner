use super::EVENT_SCHEMA_VERSION;
use super::envelope::{
    CorrelationId, Digest32, EventCorrelations, EventEnvelopeV1, EventFamily, EventId, EventKind,
    GenerationId, LegacyProvenance, PayloadCodec, ProcessInstanceId, ProducerIdentity,
    SessionCorrelationDigest, SpanId, SupervisorAuthorityId, TraceId, WriterInstanceId,
};
use super::schema::{
    AppendTicket, GenerationMetadata, ReconciliationOutcome, read_generation_metadata,
    verify_generation_schema,
};
use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OpenFlags, params_from_iter};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const EVENT_SELECT_COLUMNS: &str = "local_sequence, event_id, schema_version, family, kind,
    recorded_at_unix_micros, ingested_at_unix_micros, producer_sequence,
    writer_instance_id, process_instance_id, process_root_id,
    parent_process_instance_id, supervisor_authority_id,
    trace_id, span_id, parent_span_id, invocation_uuid,
    session_correlation_sha256, payload_codec, payload, payload_sha256,
    payload_bytes, immutable_sha256, legacy_provenance_json, retry_of_generation_id";

#[derive(Debug, Clone)]
pub struct ReadLimits {
    max_partitions: usize,
    max_records: usize,
    max_payload_bytes: usize,
}

impl ReadLimits {
    pub fn new(
        max_partitions: usize,
        max_records: usize,
        max_payload_bytes: usize,
    ) -> Result<Self, &'static str> {
        if max_partitions == 0 || max_records == 0 || max_payload_bytes == 0 {
            return Err("event read bounds must be non-zero");
        }
        Ok(Self {
            max_partitions,
            max_records,
            max_payload_bytes,
        })
    }

    pub const fn max_partitions(&self) -> usize {
        self.max_partitions
    }

    pub const fn max_records(&self) -> usize {
        self.max_records
    }

    pub const fn max_payload_bytes(&self) -> usize {
        self.max_payload_bytes
    }
}

#[derive(Debug, Clone, Default)]
pub struct EventFilter {
    pub recorded_at_or_after_unix_micros: Option<i64>,
    pub recorded_before_unix_micros: Option<i64>,
    pub family: Option<EventFamily>,
    pub kind: Option<EventKind>,
    pub process_root_id: Option<ProcessInstanceId>,
    pub process_instance_id: Option<ProcessInstanceId>,
    pub supervisor_authority_id: Option<SupervisorAuthorityId>,
    pub invocation_uuid: Option<CorrelationId>,
    pub session_correlation_sha256: Option<SessionCorrelationDigest>,
    pub trace_id: Option<TraceId>,
    pub span_id: Option<SpanId>,
    pub event_id: Option<EventId>,
}

#[derive(Debug, Clone)]
pub struct GenerationReadTarget {
    pub writer_instance_id: WriterInstanceId,
    pub generation_id: GenerationId,
    pub database_path: PathBuf,
    /// External lease file for this exact generation. Readers never create it.
    pub lease_path: PathBuf,
    /// Deterministic AGE-377 pending-trash location for absence classification.
    pub pending_trash_path: Option<PathBuf>,
    /// A receipt is not trusted merely because this path exists. Until the
    /// generation protocol validates its contents, absence remains incomplete.
    pub retirement_receipt_path: Option<PathBuf>,
    pub prepared_manifest_sha256: Option<Digest32>,
    pub sealed_manifest_sha256: Option<Digest32>,
    pub catalog_watermark: Option<Digest32>,
}

/// One bounded traversal of the sharded discovery journal. This never opens a
/// correctness database and never performs an unbounded writer-directory
/// enumeration. `coverage` is incomplete whenever the supplied node/entry
/// budget did not cover the journal or any leaf could not be interpreted.
#[derive(Debug, Clone)]
pub struct BoundedDiscovery {
    pub targets: Vec<GenerationReadTarget>,
    pub coverage: DiscoveryCoverage,
    pub issues: Vec<String>,
    pub nodes_examined: usize,
    pub entries_examined: usize,
    pub more: bool,
}

pub fn discover_generation_read_targets(
    event_store_root: &Path,
    max_nodes: usize,
    max_entries: usize,
) -> Result<BoundedDiscovery, String> {
    let batch = super::maintenance_discovery::read_evidence_batch(
        event_store_root,
        max_nodes,
        max_entries,
    )?;
    let mut issues = batch.issues;
    let entries_examined = batch.entries_examined;
    let mut targets = Vec::with_capacity(batch.entries.len());
    let mut watermark = Sha256::new();
    watermark.update(b"oulipoly.event-discovery-read.v1\0");

    for entry in batch.entries {
        watermark.update(entry.writer.as_bytes());
        watermark.update(entry.generation.as_bytes());
        let Some(record) = entry.record else {
            issues.push(
                entry.issue.unwrap_or_else(|| {
                    "discovery leaf has no readable selected record".to_string()
                }),
            );
            continue;
        };
        if let Some(issue) = entry.issue {
            issues.push(issue);
        }
        if let Some(digest) = record.prepared_manifest_sha256 {
            watermark.update(digest.as_bytes());
        }
        let writer = super::id_hex(entry.writer.as_bytes());
        let generation = super::id_hex(entry.generation.as_bytes());
        let writer_dir = event_store_root.join("writers").join(&writer);
        targets.push(GenerationReadTarget {
            writer_instance_id: entry.writer,
            generation_id: entry.generation,
            database_path: writer_dir
                .join("generations")
                .join(&generation)
                .join(super::DATABASE_FILE_NAME),
            lease_path: writer_dir.join("leases").join(format!("{generation}.lock")),
            pending_trash_path: Some(
                event_store_root
                    .join("retirement")
                    .join("trash")
                    .join(&writer)
                    .join(format!("{generation}.pending")),
            ),
            retirement_receipt_path: Some(
                event_store_root
                    .join("retirement")
                    .join("receipts")
                    .join(&writer)
                    .join(format!("{generation}.json")),
            ),
            prepared_manifest_sha256: record.prepared_manifest_sha256,
            sealed_manifest_sha256: None,
            catalog_watermark: None,
        });
    }
    targets.sort_by_key(|target| (target.writer_instance_id, target.generation_id));
    let incomplete = batch.more || !issues.is_empty();
    let coverage = if incomplete {
        DiscoveryCoverage::Incomplete {
            reason: format!(
                "bounded discovery more={} issues={} nodes={} entries={}",
                batch.more,
                issues.len(),
                batch.nodes_examined,
                entries_examined
            ),
        }
    } else {
        DiscoveryCoverage::Complete {
            discovery_watermark: Digest32::from_bytes(watermark.finalize().into()),
        }
    };
    Ok(BoundedDiscovery {
        targets,
        coverage,
        issues,
        nodes_examined: batch.nodes_examined,
        entries_examined,
        more: batch.more,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryCoverage {
    /// One exact physical address; no catalog/discovery completeness claim is
    /// needed or implied.
    ExactAddress,
    Complete {
        discovery_watermark: Digest32,
    },
    Incomplete {
        reason: String,
    },
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadRecord {
    pub generation_id: GenerationId,
    pub local_sequence: i64,
    pub immutable_sha256: Digest32,
    pub envelope: EventEnvelopeV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartitionWatermark {
    pub writer_instance_id: WriterInstanceId,
    pub generation_id: GenerationId,
    pub high_local_sequence: Option<i64>,
    pub snapshot_row_count: u64,
    pub max_ingested_at_unix_micros: Option<i64>,
    pub prepared_manifest_sha256: Option<Digest32>,
    pub catalog_watermark: Option<Digest32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageIssueKind {
    MissingPartition,
    RetirementInProgress,
    Retired,
    RetirementReceiptUnvalidated,
    RetirementInconsistent,
    CorruptPartition,
    UnknownSchemaVersion,
    GenerationIdentityMismatch,
    ManifestCoverageUnknown,
    PartitionLimitReached,
    RecordLimitReached,
    PayloadByteLimitReached,
    PayloadDigestMismatch,
    ImmutableDigestMismatch,
    DuplicateLogicalEvent,
    IdentityConflict,
    MissingTraceParent,
    InvalidFilter,
    DiscoveryCoverageIncomplete,
    LeaseUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageIssue {
    pub kind: CoverageIssueKind,
    pub writer_instance_id: Option<WriterInstanceId>,
    pub generation_id: Option<GenerationId>,
    pub event_id: Option<EventId>,
    pub detail: String,
    pub compromises_completeness: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoundedRead {
    pub records: Vec<ReadRecord>,
    pub watermarks: Vec<PartitionWatermark>,
    pub issues: Vec<CoverageIssue>,
    pub coverage_complete: bool,
    pub partitions_examined: usize,
    pub partitions_omitted: usize,
    pub records_returned: usize,
    pub payload_bytes_returned: usize,
    pub payload_bytes_examined: usize,
    pub discovery_watermark: Option<Digest32>,
}

impl BoundedRead {
    fn empty() -> Self {
        Self {
            records: Vec::new(),
            watermarks: Vec::new(),
            issues: Vec::new(),
            coverage_complete: true,
            partitions_examined: 0,
            partitions_omitted: 0,
            records_returned: 0,
            payload_bytes_returned: 0,
            payload_bytes_examined: 0,
            discovery_watermark: None,
        }
    }

    fn issue(&mut self, issue: CoverageIssue) {
        if issue.compromises_completeness {
            self.coverage_complete = false;
        }
        self.issues.push(issue);
    }
}

pub fn read_generation(
    target: &GenerationReadTarget,
    filter: &EventFilter,
    limits: &ReadLimits,
) -> BoundedRead {
    read_with_coverage(
        std::slice::from_ref(target),
        &DiscoveryCoverage::ExactAddress,
        filter,
        limits,
    )
}

/// Read independent per-generation snapshots under explicit aggregate bounds.
/// A broken generation becomes an issue while healthy generations remain in
/// the response; an empty complete result is possible only when every selected
/// partition was successfully examined.
pub fn read_generations(
    targets: &[GenerationReadTarget],
    filter: &EventFilter,
    limits: &ReadLimits,
) -> BoundedRead {
    read_with_coverage(targets, &DiscoveryCoverage::Unknown, filter, limits)
}

pub fn read_discovered_generations(
    targets: &[GenerationReadTarget],
    discovery: &DiscoveryCoverage,
    filter: &EventFilter,
    limits: &ReadLimits,
) -> BoundedRead {
    read_with_coverage(targets, discovery, filter, limits)
}

fn read_with_coverage(
    targets: &[GenerationReadTarget],
    discovery: &DiscoveryCoverage,
    filter: &EventFilter,
    limits: &ReadLimits,
) -> BoundedRead {
    let mut result = BoundedRead::empty();
    match discovery {
        DiscoveryCoverage::ExactAddress => {}
        DiscoveryCoverage::Complete {
            discovery_watermark,
        } => {
            result.discovery_watermark = Some(*discovery_watermark);
        }
        DiscoveryCoverage::Incomplete { reason } => result.issue(CoverageIssue {
            kind: CoverageIssueKind::DiscoveryCoverageIncomplete,
            writer_instance_id: None,
            generation_id: None,
            event_id: None,
            detail: reason.clone(),
            compromises_completeness: true,
        }),
        DiscoveryCoverage::Unknown => result.issue(CoverageIssue {
            kind: CoverageIssueKind::DiscoveryCoverageIncomplete,
            writer_instance_id: None,
            generation_id: None,
            event_id: None,
            detail:
                "candidate partitions were supplied without a complete discovery/catalog watermark"
                    .to_string(),
            compromises_completeness: true,
        }),
    }
    if filter
        .recorded_at_or_after_unix_micros
        .is_some_and(|value| value < 0)
        || filter
            .recorded_before_unix_micros
            .is_some_and(|value| value < 0)
        || matches!(
            (
                filter.recorded_at_or_after_unix_micros,
                filter.recorded_before_unix_micros
            ),
            (Some(start), Some(end)) if start > end
        )
        || (filter.span_id.is_some() && filter.trace_id.is_none())
        || (filter.kind.is_some() && filter.family.is_none())
        || (filter.process_instance_id.is_some() && filter.process_root_id.is_none())
    {
        result.issue(CoverageIssue {
            kind: CoverageIssueKind::InvalidFilter,
            writer_instance_id: None,
            generation_id: None,
            event_id: None,
            detail: "invalid event range or unsupported unanchored kind/span/process filter"
                .to_string(),
            compromises_completeness: true,
        });
        return result;
    }

    let examined = targets.len().min(limits.max_partitions);
    result.partitions_omitted = targets.len().saturating_sub(examined);
    if result.partitions_omitted > 0 {
        result.issue(CoverageIssue {
            kind: CoverageIssueKind::PartitionLimitReached,
            writer_instance_id: None,
            generation_id: None,
            event_id: None,
            detail: format!("{} candidate partitions omitted", result.partitions_omitted),
            compromises_completeness: true,
        });
    }

    for target in targets.iter().take(examined) {
        result.partitions_examined += 1;
        if result.records.len() >= limits.max_records {
            result.issue(target_issue(
                target,
                CoverageIssueKind::RecordLimitReached,
                "aggregate record limit reached",
                true,
            ));
            break;
        }
        if result.payload_bytes_examined >= limits.max_payload_bytes {
            result.issue(target_issue(
                target,
                CoverageIssueKind::PayloadByteLimitReached,
                "aggregate payload-byte limit reached",
                true,
            ));
            break;
        }
        let record_budget = limits.max_records - result.records.len();
        let byte_budget = limits.max_payload_bytes - result.payload_bytes_examined;
        read_one(target, filter, record_budget, byte_budget, &mut result);
    }

    result.records.sort_by(|left, right| {
        (
            left.envelope.recorded_at_unix_micros,
            left.envelope.event_id,
        )
            .cmp(&(
                right.envelope.recorded_at_unix_micros,
                right.envelope.event_id,
            ))
    });
    let mut identities = HashMap::<EventId, (Digest32, usize)>::new();
    let mut deduplicated = Vec::with_capacity(result.records.len());
    let records = std::mem::take(&mut result.records);
    for record in records {
        if let Some((digest, _)) = identities.get(&record.envelope.event_id) {
            let equal = *digest == record.immutable_sha256;
            result.issue(CoverageIssue {
                kind: if equal {
                    CoverageIssueKind::DuplicateLogicalEvent
                } else {
                    CoverageIssueKind::IdentityConflict
                },
                writer_instance_id: Some(record.envelope.producer.writer_instance_id),
                generation_id: Some(record.generation_id),
                event_id: Some(record.envelope.event_id),
                detail: if equal {
                    "equal logical event appeared in more than one partition".to_string()
                } else {
                    "event ID appeared with unequal immutable identities".to_string()
                },
                compromises_completeness: !equal,
            });
            continue;
        }
        identities.insert(
            record.envelope.event_id,
            (record.immutable_sha256, deduplicated.len()),
        );
        deduplicated.push(record);
    }
    result.records = deduplicated;

    if let Some(trace_id) = filter.trace_id {
        let spans = result
            .records
            .iter()
            .filter(|record| record.envelope.correlations.trace_id == Some(trace_id))
            .filter_map(|record| record.envelope.correlations.span_id)
            .collect::<HashSet<_>>();
        let missing = result
            .records
            .iter()
            .filter(|record| record.envelope.correlations.trace_id == Some(trace_id))
            .filter_map(|record| {
                let parent = record.envelope.correlations.parent_span_id?;
                (!spans.contains(&parent))
                    .then_some((record.generation_id, record.envelope.event_id))
            })
            .collect::<Vec<_>>();
        for (generation_id, event_id) in missing {
            result.issue(CoverageIssue {
                kind: CoverageIssueKind::MissingTraceParent,
                writer_instance_id: None,
                generation_id: Some(generation_id),
                event_id: Some(event_id),
                detail: "trace parent is not present in the bounded result".to_string(),
                compromises_completeness: true,
            });
        }
    }
    result.records_returned = result.records.len();
    result.payload_bytes_returned = result
        .records
        .iter()
        .filter_map(|record| usize::try_from(record.envelope.payload_bytes).ok())
        .sum();
    result
}

fn read_one(
    target: &GenerationReadTarget,
    filter: &EventFilter,
    record_budget: usize,
    byte_budget: usize,
    result: &mut BoundedRead,
) {
    let source_exists = target.database_path.is_file();
    let source_directory_exists = target
        .database_path
        .parent()
        .is_some_and(|path| path.is_dir());
    let pending_exists = target
        .pending_trash_path
        .as_ref()
        .is_some_and(|path| path.exists());
    let receipt_exists = target
        .retirement_receipt_path
        .as_ref()
        .is_some_and(|path| path.exists());
    if source_directory_exists && (pending_exists || receipt_exists) {
        result.issue(target_issue(
            target,
            CoverageIssueKind::RetirementInconsistent,
            "source generation conflicts with pending-trash or receipt state",
            true,
        ));
        return;
    }
    let is_generation_database_path = target
        .database_path
        .file_name()
        .is_some_and(|name| name == std::ffi::OsStr::new(super::DATABASE_FILE_NAME));
    if source_directory_exists && is_generation_database_path && !source_exists {
        result.issue(target_issue(
            target,
            CoverageIssueKind::CorruptPartition,
            "generation directory exists but its database is missing or not a regular file",
            true,
        ));
        return;
    }
    if !target.database_path.is_file() {
        if receipt_exists {
            match validate_retirement_receipt(target) {
                Ok(()) => {
                    result.issue(target_issue(
                        target,
                        CoverageIssueKind::Retired,
                        "generation absence is covered by its validated retirement receipt",
                        false,
                    ));
                    return;
                }
                Err(error) => {
                    result.issue(target_issue(
                        target,
                        CoverageIssueKind::RetirementReceiptUnvalidated,
                        &error,
                        true,
                    ));
                    return;
                }
            }
        }
        let (kind, detail) = match (pending_exists, receipt_exists) {
            (true, false) => (
                CoverageIssueKind::RetirementInProgress,
                "generation is at its exact pending-trash path without a receipt",
            ),
            (_, true) => unreachable!("receipt returned above"),
            (false, false) => (
                CoverageIssueKind::MissingPartition,
                "expected generation database is missing without retirement proof",
            ),
        };
        result.issue(target_issue(target, kind, detail, true));
        return;
    }
    let _lease = match super::acquire_generation_reader_lease_at(&target.lease_path) {
        Ok(lease) => lease,
        Err(error) => {
            result.issue(target_issue(
                target,
                CoverageIssueKind::LeaseUnavailable,
                &format!("generation shared lease unavailable: {error}"),
                true,
            ));
            return;
        }
    };
    if !target.database_path.is_file()
        || target
            .pending_trash_path
            .as_ref()
            .is_some_and(|path| path.exists())
    {
        result.issue(target_issue(
            target,
            CoverageIssueKind::RetirementInProgress,
            "generation location changed while acquiring its shared lease",
            true,
        ));
        return;
    }
    let generation_directory = match target.database_path.parent() {
        Some(path) => path,
        None => {
            result.issue(target_issue(
                target,
                CoverageIssueKind::CorruptPartition,
                "generation database has no parent directory",
                true,
            ));
            return;
        }
    };
    let manifest = match super::read_prepared_manifest(generation_directory) {
        Ok(manifest) => manifest,
        Err(error) => {
            result.issue(target_issue(
                target,
                CoverageIssueKind::CorruptPartition,
                &format!("prepared manifest validation failed: {error}"),
                true,
            ));
            return;
        }
    };
    let actual_manifest_sha256 = match manifest.sha256() {
        Ok(digest) => digest,
        Err(error) => {
            result.issue(target_issue(
                target,
                CoverageIssueKind::CorruptPartition,
                &format!("prepared manifest digest failed: {error}"),
                true,
            ));
            return;
        }
    };
    if manifest.generation_id != target.generation_id
        || manifest.writer_instance_id != target.writer_instance_id
        || target
            .prepared_manifest_sha256
            .is_some_and(|expected| expected != actual_manifest_sha256)
    {
        result.issue(target_issue(
            target,
            CoverageIssueKind::GenerationIdentityMismatch,
            "prepared manifest differs from exact target identity/digest",
            true,
        ));
        return;
    }
    let mut connection = match open_read_only(&target.database_path) {
        Ok(connection) => connection,
        Err(error) => {
            result.issue(target_issue(
                target,
                CoverageIssueKind::CorruptPartition,
                &format!("cannot open generation: {error}"),
                true,
            ));
            return;
        }
    };
    if let Err(error) = verify_generation_schema(&connection, None) {
        result.issue(target_issue(
            target,
            CoverageIssueKind::CorruptPartition,
            &format!("schema validation failed: {error}"),
            true,
        ));
        return;
    }
    let transaction = match connection.transaction() {
        Ok(transaction) => transaction,
        Err(error) => {
            result.issue(target_issue(
                target,
                CoverageIssueKind::CorruptPartition,
                &format!("cannot begin read snapshot: {error}"),
                true,
            ));
            return;
        }
    };
    let metadata = match read_generation_metadata(&transaction) {
        Ok(metadata) => metadata,
        Err(error) => {
            result.issue(target_issue(
                target,
                CoverageIssueKind::CorruptPartition,
                &format!("metadata read failed: {error}"),
                true,
            ));
            return;
        }
    };
    if metadata.generation_id != target.generation_id
        || metadata.writer_instance_id != target.writer_instance_id
        || metadata.schema_version != manifest.schema_version
        || metadata.predecessor_generation_id != manifest.predecessor_generation_id
        || metadata.process_instance_id != manifest.producer.process_instance_id
        || metadata.process_root_id != manifest.producer.process_root_id
        || metadata.parent_process_instance_id != manifest.producer.parent_process_instance_id
        || metadata.supervisor_authority_id != manifest.producer.supervisor_authority_id
        || metadata.native_process != manifest.producer.native_process
        || metadata.created_at_unix_micros != manifest.created_at_unix_micros
        || metadata.head_epoch != manifest.head_epoch
    {
        result.issue(target_issue(
            target,
            CoverageIssueKind::GenerationIdentityMismatch,
            "database metadata differs from exact target identity",
            true,
        ));
        return;
    }
    let watermark: rusqlite::Result<(Option<i64>, i64, Option<i64>)> = transaction.query_row(
        "SELECT MAX(local_sequence), COUNT(*), MAX(ingested_at_unix_micros) FROM events",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    );
    let (high_local_sequence, row_count, max_ingested) = match watermark {
        Ok(value) => value,
        Err(error) => {
            result.issue(target_issue(
                target,
                CoverageIssueKind::CorruptPartition,
                &format!("watermark read failed: {error}"),
                true,
            ));
            return;
        }
    };
    result.watermarks.push(PartitionWatermark {
        writer_instance_id: target.writer_instance_id,
        generation_id: target.generation_id,
        high_local_sequence,
        snapshot_row_count: u64::try_from(row_count).unwrap_or(0),
        max_ingested_at_unix_micros: max_ingested,
        prepared_manifest_sha256: Some(actual_manifest_sha256),
        catalog_watermark: target.catalog_watermark,
    });

    let (sql, parameters) = build_query(filter, record_budget.saturating_add(1));
    let mut statement = match transaction.prepare(&sql) {
        Ok(statement) => statement,
        Err(error) => {
            result.issue(target_issue(
                target,
                CoverageIssueKind::CorruptPartition,
                &format!("event query prepare failed: {error}"),
                true,
            ));
            return;
        }
    };
    let mut rows = match statement.query(params_from_iter(parameters.iter())) {
        Ok(rows) => rows,
        Err(error) => {
            result.issue(target_issue(
                target,
                CoverageIssueKind::CorruptPartition,
                &format!("event query failed: {error}"),
                true,
            ));
            return;
        }
    };
    let mut partition_records = 0usize;
    let mut partition_payload_bytes = 0usize;
    loop {
        let row = match rows.next() {
            Ok(Some(row)) => row,
            Ok(None) => break,
            Err(error) => {
                result.issue(target_issue(
                    target,
                    CoverageIssueKind::CorruptPartition,
                    &format!("event row read failed: {error}"),
                    true,
                ));
                break;
            }
        };
        if partition_records >= record_budget {
            result.issue(target_issue(
                target,
                CoverageIssueKind::RecordLimitReached,
                "record limit omitted matching rows",
                true,
            ));
            break;
        }
        let schema_version: i64 = match row.get(2) {
            Ok(value) => value,
            Err(error) => {
                result.issue(target_issue(
                    target,
                    CoverageIssueKind::CorruptPartition,
                    &format!("schema version read failed: {error}"),
                    true,
                ));
                continue;
            }
        };
        if schema_version != EVENT_SCHEMA_VERSION {
            let event_id = row
                .get::<_, Vec<u8>>(1)
                .ok()
                .and_then(|bytes| EventId::from_slice(&bytes).ok());
            result.issue(CoverageIssue {
                kind: CoverageIssueKind::UnknownSchemaVersion,
                writer_instance_id: Some(target.writer_instance_id),
                generation_id: Some(target.generation_id),
                event_id,
                detail: format!(
                    "event schema version {schema_version} is discoverable but not decoded"
                ),
                compromises_completeness: true,
            });
            continue;
        }
        match decode_record(row, &metadata) {
            Ok(record) => {
                let bytes = usize::try_from(record.envelope.payload_bytes).unwrap_or(usize::MAX);
                if partition_payload_bytes.saturating_add(bytes) > byte_budget {
                    result.issue(target_issue(
                        target,
                        CoverageIssueKind::PayloadByteLimitReached,
                        "payload byte limit omitted matching rows",
                        true,
                    ));
                    break;
                }
                partition_payload_bytes += bytes;
                result.payload_bytes_examined += bytes;
                result.records.push(record);
                partition_records += 1;
            }
            Err((kind, event_id, detail)) => result.issue(CoverageIssue {
                kind,
                writer_instance_id: Some(target.writer_instance_id),
                generation_id: Some(target.generation_id),
                event_id,
                detail,
                compromises_completeness: true,
            }),
        }
    }
}

fn build_query(filter: &EventFilter, limit: usize) -> (String, Vec<SqlValue>) {
    let mut sql = format!("SELECT {EVENT_SELECT_COLUMNS} FROM events WHERE 1 = 1");
    let mut values = Vec::new();
    macro_rules! scalar {
        ($field:expr, $column:literal) => {
            if let Some(value) = $field {
                sql.push_str(concat!(" AND ", $column, " = ?"));
                values.push(SqlValue::Integer(value));
            }
        };
    }
    macro_rules! blob {
        ($field:expr, $column:literal) => {
            if let Some(value) = $field {
                sql.push_str(concat!(" AND ", $column, " = ?"));
                values.push(SqlValue::Blob(value.as_bytes().to_vec()));
            }
        };
    }
    if let Some(value) = filter.recorded_at_or_after_unix_micros {
        sql.push_str(" AND recorded_at_unix_micros >= ?");
        values.push(SqlValue::Integer(value));
    }
    if let Some(value) = filter.recorded_before_unix_micros {
        sql.push_str(" AND recorded_at_unix_micros < ?");
        values.push(SqlValue::Integer(value));
    }
    scalar!(filter.family.map(|value| value as i64), "family");
    if let Some(value) = &filter.kind {
        sql.push_str(" AND kind = ?");
        values.push(SqlValue::Text(value.as_str().to_string()));
    }
    blob!(filter.process_root_id, "process_root_id");
    blob!(filter.process_instance_id, "process_instance_id");
    blob!(filter.supervisor_authority_id, "supervisor_authority_id");
    blob!(filter.invocation_uuid, "invocation_uuid");
    blob!(
        filter.session_correlation_sha256,
        "session_correlation_sha256"
    );
    blob!(filter.trace_id, "trace_id");
    blob!(filter.span_id, "span_id");
    blob!(filter.event_id, "event_id");
    sql.push_str(" ORDER BY recorded_at_unix_micros, event_id LIMIT ?");
    values.push(SqlValue::Integer(i64::try_from(limit).unwrap_or(i64::MAX)));
    (sql, values)
}

fn decode_record(
    row: &rusqlite::Row<'_>,
    metadata: &GenerationMetadata,
) -> Result<ReadRecord, (CoverageIssueKind, Option<EventId>, String)> {
    let get_blob =
        |index| -> Result<Vec<u8>, String> { row.get(index).map_err(|error| error.to_string()) };
    let event_id_bytes =
        get_blob(1).map_err(|error| (CoverageIssueKind::CorruptPartition, None, error))?;
    let event_id = EventId::from_slice(&event_id_bytes)
        .map_err(|error| (CoverageIssueKind::CorruptPartition, None, error.to_string()))?;
    let decode = || -> Result<ReadRecord, String> {
        let writer = WriterInstanceId::from_slice(&get_blob(8)?).map_err(|e| e.to_string())?;
        let process = ProcessInstanceId::from_slice(&get_blob(9)?).map_err(|e| e.to_string())?;
        let root = ProcessInstanceId::from_slice(&get_blob(10)?).map_err(|e| e.to_string())?;
        let optional_id = |index| -> Result<Option<Vec<u8>>, String> {
            row.get(index).map_err(|error| error.to_string())
        };
        let parent = optional_id(11)?
            .as_deref()
            .map(ProcessInstanceId::from_slice)
            .transpose()
            .map_err(|e| e.to_string())?;
        let supervisor = optional_id(12)?
            .as_deref()
            .map(SupervisorAuthorityId::from_slice)
            .transpose()
            .map_err(|e| e.to_string())?;
        if writer != metadata.writer_instance_id
            || process != metadata.process_instance_id
            || root != metadata.process_root_id
            || parent != metadata.parent_process_instance_id
            || supervisor != metadata.supervisor_authority_id
        {
            return Err("row producer identity differs from generation metadata".to_string());
        }
        let trace_id = optional_id(13)?
            .as_deref()
            .map(TraceId::from_slice)
            .transpose()
            .map_err(|e| e.to_string())?;
        let span_id = optional_id(14)?
            .as_deref()
            .map(SpanId::from_slice)
            .transpose()
            .map_err(|e| e.to_string())?;
        let parent_span_id = optional_id(15)?
            .as_deref()
            .map(SpanId::from_slice)
            .transpose()
            .map_err(|e| e.to_string())?;
        let invocation_uuid = optional_id(16)?
            .as_deref()
            .map(CorrelationId::from_slice)
            .transpose()
            .map_err(|e| e.to_string())?;
        let session_correlation_sha256 = optional_id(17)?
            .as_deref()
            .map(SessionCorrelationDigest::from_slice)
            .transpose()
            .map_err(|e| e.to_string())?;
        let legacy_json: Option<String> = row.get(23).map_err(|e| e.to_string())?;
        let legacy_provenance = legacy_json
            .as_deref()
            .map(serde_json::from_str::<LegacyProvenance>)
            .transpose()
            .map_err(|e| e.to_string())?;
        let retry = optional_id(24)?
            .as_deref()
            .map(GenerationId::from_slice)
            .transpose()
            .map_err(|e| e.to_string())?;
        let payload_sha = Digest32::from_slice(&get_blob(20)?).map_err(|e| e.to_string())?;
        let stored_immutable = Digest32::from_slice(&get_blob(22)?).map_err(|e| e.to_string())?;
        let envelope = EventEnvelopeV1 {
            event_id,
            schema_version: row.get(2).map_err(|e| e.to_string())?,
            family: EventFamily::from_i64(row.get(3).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?,
            kind: EventKind::from_stored(row.get(4).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?,
            recorded_at_unix_micros: row.get(5).map_err(|e| e.to_string())?,
            ingested_at_unix_micros: row.get(6).map_err(|e| e.to_string())?,
            producer_sequence: row.get(7).map_err(|e| e.to_string())?,
            producer: ProducerIdentity {
                writer_instance_id: writer,
                process_instance_id: process,
                process_root_id: root,
                parent_process_instance_id: parent,
                supervisor_authority_id: supervisor,
                native_process: metadata.native_process.clone(),
            },
            correlations: EventCorrelations {
                trace_id,
                span_id,
                parent_span_id,
                invocation_uuid,
                session_correlation_sha256,
            },
            payload_codec: PayloadCodec::from_i64(row.get(18).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?,
            payload: row.get(19).map_err(|e| e.to_string())?,
            payload_sha256: payload_sha,
            payload_bytes: row.get(21).map_err(|e| e.to_string())?,
            legacy_provenance,
            retry_of_generation_id: retry,
        };
        envelope.validate().map_err(|e| e.to_string())?;
        if envelope.immutable_digest().map_err(|e| e.to_string())? != stored_immutable {
            return Err("stored immutable digest does not match decoded envelope".to_string());
        }
        Ok(ReadRecord {
            generation_id: metadata.generation_id,
            local_sequence: row.get(0).map_err(|e| e.to_string())?,
            immutable_sha256: stored_immutable,
            envelope,
        })
    };
    decode().map_err(|detail| {
        let kind = if detail.contains("payload digest") {
            CoverageIssueKind::PayloadDigestMismatch
        } else if detail.contains("immutable digest") {
            CoverageIssueKind::ImmutableDigestMismatch
        } else {
            CoverageIssueKind::CorruptPartition
        };
        (kind, Some(event_id), detail)
    })
}

pub fn reconcile_exact_generation(
    database_path: &Path,
    lease_path: &Path,
    ticket: &AppendTicket,
) -> ReconciliationOutcome {
    let _lease = match super::acquire_generation_reader_lease_at(lease_path) {
        Ok(lease) => lease,
        Err(error) => {
            return ReconciliationOutcome::Unknown {
                reason: format!("exact generation shared lease unavailable: {error}"),
            };
        }
    };
    reconcile_exact_generation_while_leased(database_path, ticket)
}

pub(crate) fn reconcile_exact_generation_while_leased(
    database_path: &Path,
    ticket: &AppendTicket,
) -> ReconciliationOutcome {
    if !database_path.is_file() {
        return ReconciliationOutcome::Unknown {
            reason: "exact generation database is missing".to_string(),
        };
    }
    let generation_directory = match database_path.parent() {
        Some(path) => path,
        None => {
            return ReconciliationOutcome::Unknown {
                reason: "exact generation path has no parent".to_string(),
            };
        }
    };
    let manifest = match super::read_prepared_manifest(generation_directory) {
        Ok(manifest) => manifest,
        Err(error) => {
            return ReconciliationOutcome::Unknown {
                reason: format!("exact generation manifest is invalid: {error}"),
            };
        }
    };
    if manifest.generation_id != ticket.generation_id
        || manifest.writer_instance_id != ticket.writer_instance_id
    {
        return ReconciliationOutcome::Unknown {
            reason: "exact generation manifest identity does not match append ticket".to_string(),
        };
    }
    let connection = match open_read_only(database_path) {
        Ok(connection) => connection,
        Err(error) => {
            return ReconciliationOutcome::Unknown {
                reason: format!("exact generation cannot be opened: {error}"),
            };
        }
    };
    if let Err(error) = verify_generation_schema(&connection, None) {
        return ReconciliationOutcome::Unknown {
            reason: format!("exact generation schema is not healthy: {error}"),
        };
    }
    let metadata = match read_generation_metadata(&connection) {
        Ok(metadata) => metadata,
        Err(error) => {
            return ReconciliationOutcome::Unknown {
                reason: format!("exact generation metadata is unreadable: {error}"),
            };
        }
    };
    if metadata.generation_id != ticket.generation_id
        || metadata.writer_instance_id != ticket.writer_instance_id
        || metadata.schema_version != manifest.schema_version
        || metadata.predecessor_generation_id != manifest.predecessor_generation_id
        || metadata.process_instance_id != manifest.producer.process_instance_id
        || metadata.process_root_id != manifest.producer.process_root_id
        || metadata.parent_process_instance_id != manifest.producer.parent_process_instance_id
        || metadata.supervisor_authority_id != manifest.producer.supervisor_authority_id
        || metadata.native_process != manifest.producer.native_process
        || metadata.created_at_unix_micros != manifest.created_at_unix_micros
        || metadata.head_epoch != manifest.head_epoch
    {
        return ReconciliationOutcome::Unknown {
            reason: "exact generation identity does not match append ticket".to_string(),
        };
    }
    let quick_check: Result<String, _> =
        connection.query_row("PRAGMA quick_check(1)", [], |row| row.get(0));
    if !matches!(quick_check.as_deref(), Ok("ok")) {
        return ReconciliationOutcome::Unknown {
            reason: format!("exact generation quick_check failed: {quick_check:?}"),
        };
    }
    let mut statement = match connection.prepare(&format!(
        "SELECT {EVENT_SELECT_COLUMNS} FROM events WHERE event_id = ?1"
    )) {
        Ok(statement) => statement,
        Err(error) => {
            return ReconciliationOutcome::Unknown {
                reason: format!("exact event lookup prepare failed: {error}"),
            };
        }
    };
    let mut rows = match statement.query([ticket.event_id.as_bytes().as_slice()]) {
        Ok(rows) => rows,
        Err(error) => {
            return ReconciliationOutcome::Unknown {
                reason: format!("exact event lookup failed: {error}"),
            };
        }
    };
    match rows.next() {
        Ok(Some(row)) => match decode_record(row, &metadata) {
            Ok(record) if record.immutable_sha256 == ticket.immutable_sha256 => {
                ReconciliationOutcome::AlreadyCommitted {
                    local_sequence: record.local_sequence,
                }
            }
            Ok(record) => ReconciliationOutcome::IdentityConflict {
                stored_immutable_sha256: record.immutable_sha256,
            },
            Err((_, _, reason)) => ReconciliationOutcome::Unknown {
                reason: format!("exact stored row failed validation: {reason}"),
            },
        },
        Ok(None) => ReconciliationOutcome::AbsentHealthy,
        Err(error) => ReconciliationOutcome::Unknown {
            reason: format!("exact event lookup failed: {error}"),
        },
    }
}

fn open_read_only(path: &Path) -> rusqlite::Result<Connection> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.pragma_update(None, "query_only", true)?;
    Ok(connection)
}

fn validate_retirement_receipt(target: &GenerationReadTarget) -> Result<(), String> {
    let path = target
        .retirement_receipt_path
        .as_ref()
        .ok_or_else(|| "retirement receipt path was not supplied".to_string())?;
    let bytes = super::generation::read_bounded_control_file(path)
        .map_err(|error| format!("retirement receipt read failed: {error}"))?;
    let receipt: super::RetirementReceipt = serde_json::from_slice(&bytes)
        .map_err(|error| format!("retirement receipt JSON is invalid: {error}"))?;
    receipt
        .validate()
        .map_err(|error| format!("retirement receipt validation failed: {error}"))?;
    if receipt.writer_instance_id != target.writer_instance_id
        || receipt.generation_id != target.generation_id
        || target.prepared_manifest_sha256 != Some(receipt.prepared_manifest_sha256)
        || target.sealed_manifest_sha256 != Some(receipt.sealed_manifest_sha256)
        || receipt
            .canonical_bytes()
            .map_err(|error| error.to_string())?
            != bytes
    {
        return Err(
            "retirement receipt identity/digests/canonical bytes differ from target".to_string(),
        );
    }
    Ok(())
}

fn target_issue(
    target: &GenerationReadTarget,
    kind: CoverageIssueKind,
    detail: &str,
    compromises_completeness: bool,
) -> CoverageIssue {
    CoverageIssue {
        kind,
        writer_instance_id: Some(target.writer_instance_id),
        generation_id: Some(target.generation_id),
        event_id: None,
        detail: detail.to_string(),
        compromises_completeness,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_store::{
        AppendDisposition, Digest32, EventCorrelations, EventFamily, EventKind, GenerationSource,
        NativeProcessIdentity, NewEventV1, PayloadNormalizationPolicy, PreparedManifest,
        ProcessInstanceId, ProducerIdentity, WriterInstanceId, WriterLayout, append_batch,
        initialize_generation_schema, mark_generation_writable,
    };
    use rusqlite::Connection;
    use serde_json::json;
    use std::fs;

    struct Fixture {
        _root: tempfile::TempDir,
        target: GenerationReadTarget,
    }

    fn producer(writer: u8) -> ProducerIdentity {
        ProducerIdentity {
            writer_instance_id: WriterInstanceId::from_bytes([writer; 16]),
            process_instance_id: ProcessInstanceId::from_bytes([writer + 1; 16]),
            process_root_id: ProcessInstanceId::from_bytes([writer + 2; 16]),
            parent_process_instance_id: None,
            supervisor_authority_id: Some(SupervisorAuthorityId::from_bytes([writer + 3; 16])),
            native_process: Some(NativeProcessIdentity {
                os_pid: i64::from(writer) + 100,
                os_boot_id_sha256: Digest32::from_bytes([writer + 4; 32]),
                os_pid_starttime_ticks: i64::from(writer) + 200,
            }),
        }
    }

    fn event(writer: u8, id: u8, sequence: i64, trace: bool) -> EventEnvelopeV1 {
        EventEnvelopeV1::normalize(
            NewEventV1 {
                event_id: EventId::from_bytes([id; 16]),
                family: EventFamily::Trace,
                kind: EventKind::registered("trace.phase").unwrap(),
                recorded_at_unix_micros: 100 + sequence,
                producer_sequence: sequence,
                producer: producer(writer),
                correlations: if trace {
                    EventCorrelations {
                        trace_id: Some(TraceId::from_bytes([9; 16])),
                        span_id: Some(SpanId::from_bytes([id; 16])),
                        ..EventCorrelations::default()
                    }
                } else {
                    EventCorrelations::default()
                },
                payload: json!({"value": sequence}),
                legacy_provenance: None,
                retry_of_generation_id: None,
            },
            &PayloadNormalizationPolicy::registered(&["value"]).unwrap(),
            200 + sequence,
        )
        .unwrap()
    }

    fn fixture(writer: u8, generation: u8, events: &[EventEnvelopeV1]) -> Fixture {
        let root = tempfile::tempdir().unwrap();
        let layout = WriterLayout::create(root.path(), [writer; 16]).unwrap();
        let generation_id = GenerationId::from_bytes([generation; 16]);
        let manifest = PreparedManifest {
            format_version: super::super::EVENT_STORE_FORMAT_VERSION,
            schema_version: EVENT_SCHEMA_VERSION,
            writer_instance_id: WriterInstanceId::from_bytes([writer; 16]),
            generation_id,
            predecessor_generation_id: None,
            database_relative_path: super::super::DATABASE_FILE_NAME.to_string(),
            created_at_unix_micros: 1,
            head_epoch: 0,
            source: GenerationSource::NativeProcess,
            producer: producer(writer),
        };
        let published = layout
            .publish_prepared_generation(
                &manifest,
                |path| {
                    let mut connection = Connection::open(path)?;
                    initialize_generation_schema(&mut connection, &manifest.generation_metadata()?)
                        .map_err(|error| {
                            super::super::GenerationError::Validation(error.to_string())
                        })?;
                    Ok(())
                },
                |path, _| {
                    let connection = Connection::open(path)?;
                    connection.pragma_update(None, "synchronous", "FULL")?;
                    super::super::verify_generation_schema(
                        &connection,
                        Some(&manifest.generation_metadata()?),
                    )
                    .map_err(|error| super::super::GenerationError::Validation(error.to_string()))
                },
            )
            .unwrap();
        drop(layout.acquire_generation_lease(generation_id).unwrap());
        let mut connection = Connection::open(&published.database_path).unwrap();
        connection
            .pragma_update(None, "synchronous", "FULL")
            .unwrap();
        mark_generation_writable(&connection).unwrap();
        if !events.is_empty() {
            let dispositions = append_batch(&mut connection, generation_id, events).unwrap();
            assert!(
                dispositions
                    .iter()
                    .all(|value| matches!(value, AppendDisposition::Committed { .. }))
            );
        }
        drop(connection);
        let target = GenerationReadTarget {
            writer_instance_id: WriterInstanceId::from_bytes([writer; 16]),
            generation_id,
            database_path: published.database_path,
            lease_path: layout.writer_dir().join("leases").join(format!(
                "{}.lock",
                super::super::id_hex(generation_id.as_bytes())
            )),
            pending_trash_path: None,
            retirement_receipt_path: None,
            prepared_manifest_sha256: Some(published.prepared_manifest_sha256),
            sealed_manifest_sha256: None,
            catalog_watermark: None,
        };
        Fixture {
            _root: root,
            target,
        }
    }

    fn limits(records: usize) -> ReadLimits {
        ReadLimits::new(16, records, 1024 * 1024).unwrap()
    }

    #[test]
    fn exact_reader_filters_and_reports_snapshot_high_water() {
        let first = event(1, 10, 0, true);
        let second = event(1, 11, 1, true);
        let fixture = fixture(1, 7, &[first.clone(), second]);
        let result = read_generation(
            &fixture.target,
            &EventFilter {
                family: Some(EventFamily::Trace),
                kind: Some(EventKind::registered("trace.phase").unwrap()),
                trace_id: Some(TraceId::from_bytes([9; 16])),
                event_id: Some(first.event_id),
                ..EventFilter::default()
            },
            &limits(10),
        );
        assert!(result.coverage_complete, "issues: {:?}", result.issues);
        assert_eq!(result.records.len(), 1);
        assert_eq!(result.watermarks[0].high_local_sequence, Some(2));
        assert_eq!(result.watermarks[0].snapshot_row_count, 2);
    }

    #[test]
    fn missing_and_corrupt_partitions_do_not_hide_healthy_results() {
        let healthy = fixture(2, 8, &[event(2, 20, 0, false)]);
        let corrupt = fixture(3, 9, &[event(3, 21, 0, false)]);
        fs::write(&corrupt.target.database_path, b"not sqlite").unwrap();
        let missing = GenerationReadTarget {
            writer_instance_id: WriterInstanceId::from_bytes([4; 16]),
            generation_id: GenerationId::from_bytes([10; 16]),
            database_path: corrupt._root.path().join("missing.sqlite3"),
            lease_path: corrupt._root.path().join("missing.lock"),
            pending_trash_path: None,
            retirement_receipt_path: None,
            prepared_manifest_sha256: Some(Digest32::from_bytes([1; 32])),
            sealed_manifest_sha256: None,
            catalog_watermark: None,
        };
        let result = read_discovered_generations(
            &[healthy.target.clone(), corrupt.target.clone(), missing],
            &DiscoveryCoverage::Complete {
                discovery_watermark: Digest32::from_bytes([2; 32]),
            },
            &EventFilter::default(),
            &limits(10),
        );
        assert_eq!(result.records.len(), 1);
        assert!(!result.coverage_complete);
        assert!(
            result
                .issues
                .iter()
                .any(|issue| issue.kind == CoverageIssueKind::CorruptPartition)
        );
        assert!(
            result
                .issues
                .iter()
                .any(|issue| issue.kind == CoverageIssueKind::MissingPartition)
        );
    }

    #[test]
    fn bounds_unknown_schema_and_discovery_are_explicit() {
        let first = event(5, 30, 0, false);
        let second = event(5, 31, 1, false);
        let fixture = fixture(5, 11, &[first, second]);
        let bounded = read_generation(&fixture.target, &EventFilter::default(), &limits(1));
        assert_eq!(bounded.records.len(), 1);
        assert!(
            bounded
                .issues
                .iter()
                .any(|issue| issue.kind == CoverageIssueKind::RecordLimitReached)
        );

        let connection = Connection::open(&fixture.target.database_path).unwrap();
        connection
            .execute(
                "UPDATE events SET schema_version = 2 WHERE local_sequence = 1",
                [],
            )
            .unwrap();
        drop(connection);
        let unknown = read_generation(&fixture.target, &EventFilter::default(), &limits(10));
        assert!(
            unknown
                .issues
                .iter()
                .any(|issue| issue.kind == CoverageIssueKind::UnknownSchemaVersion)
        );
        assert!(!unknown.coverage_complete);

        let undiscovered = read_generations(&[], &EventFilter::default(), &limits(10));
        assert!(!undiscovered.coverage_complete);
        assert!(
            undiscovered
                .issues
                .iter()
                .any(|issue| issue.kind == CoverageIssueKind::DiscoveryCoverageIncomplete)
        );
    }

    #[test]
    fn exact_reconciliation_validates_full_row_and_not_just_stored_digest() {
        let original = event(6, 40, 0, false);
        let fixture = fixture(6, 12, std::slice::from_ref(&original));
        let ticket = AppendTicket::new(fixture.target.generation_id, &original).unwrap();
        assert!(matches!(
            reconcile_exact_generation(
                &fixture.target.database_path,
                &fixture.target.lease_path,
                &ticket,
            ),
            ReconciliationOutcome::AlreadyCommitted { local_sequence: 1 }
        ));
        let connection = Connection::open(&fixture.target.database_path).unwrap();
        connection
            .execute("UPDATE events SET payload = '{\"value\":9}'", [])
            .unwrap();
        drop(connection);
        assert!(matches!(
            reconcile_exact_generation(
                &fixture.target.database_path,
                &fixture.target.lease_path,
                &ticket,
            ),
            ReconciliationOutcome::Unknown { .. }
        ));
    }

    #[test]
    fn equal_cross_partition_event_is_deduplicated_but_reported() {
        let logical = event(7, 50, 0, false);
        let first = fixture(7, 13, std::slice::from_ref(&logical));
        let second = fixture(7, 14, std::slice::from_ref(&logical));
        let result = read_discovered_generations(
            &[first.target, second.target],
            &DiscoveryCoverage::Complete {
                discovery_watermark: Digest32::from_bytes([3; 32]),
            },
            &EventFilter::default(),
            &limits(10),
        );
        assert_eq!(result.records.len(), 1);
        assert!(
            result
                .issues
                .iter()
                .any(|issue| issue.kind == CoverageIssueKind::DuplicateLogicalEvent)
        );
        assert!(result.coverage_complete);
    }
}
