//! Bounded detached/offline import primitives for preserved JSONL evidence.
//!
//! Nothing in this module is scheduled by a root process. Callers must invoke
//! it from a detached/offline boundary and persist the returned checkpoint and
//! receipt under their own exact-source operation. A source is never deletion
//! eligible merely because import was attempted.

use super::{
    Digest32, EventCorrelations, EventEnvelopeV1, EventFamily, EventId, EventKind,
    LegacyProvenance, LegacySyntheticField, NativeProcessIdentity, NewEventV1,
    PayloadNormalizationPolicy, ProcessInstanceId, ProducerIdentity, SpanId, TraceId,
    WriterInstanceId,
};
use crate::diagnostic_recorder::{
    DIAGNOSTIC_SCHEMA_VERSION, DiagnosticEvent, DiagnosticRetentionStatus,
};
use crate::lifecycle_log::{lifecycle_payload_fields, normalize_lifecycle_record};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::path::Path;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegacyImportLimits {
    pub max_source_bytes: u64,
    pub max_complete_records: usize,
}

impl LegacyImportLimits {
    pub fn validate(self) -> Result<Self, LegacyImportError> {
        if self.max_source_bytes == 0 || self.max_complete_records == 0 {
            return Err(LegacyImportError::InvalidLimits);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyImportDisposition {
    Appended,
    AlreadyCommitted,
}

pub trait LegacyImportSink {
    fn append_imported(
        &mut self,
        envelope: &EventEnvelopeV1,
    ) -> Result<LegacyImportDisposition, LegacyImportSinkError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyImportSinkError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyCoverageIssue {
    ActiveSource,
    SourceByteLimit,
    RecordLimit,
    TornTail,
    UnsupportedRecord { byte_offset: u64, ordinal: u64 },
    SinkFailed { byte_offset: u64, ordinal: u64 },
    SourceChanged,
    SourceReplaced,
    CheckpointSourceMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyImportCoverage {
    pub complete: bool,
    pub bytes_observed: u64,
    pub complete_records_observed: u64,
    pub supported_records: u64,
    pub unsupported_records: u64,
    pub torn_records: u64,
    pub issues: Vec<LegacyCoverageIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyImportCheckpoint {
    pub legacy_source_id: Digest32,
    pub complete_line_byte_offset: u64,
    pub complete_record_ordinal: u64,
    pub prefix_sha256: Digest32,
    pub supported_records: u64,
    pub unsupported_records: u64,
    pub torn_records: u64,
    pub imported_at_unix_micros: i64,
    pub imported_event_ids: Vec<EventId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyImportReceipt {
    pub legacy_source_id: Digest32,
    pub source_bytes: u64,
    pub source_sha256: Digest32,
    pub completed_at_unix_micros: i64,
    pub checkpoint: LegacyImportCheckpoint,
    pub imported_event_digests: Vec<Digest32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyImportResult {
    pub coverage: LegacyImportCoverage,
    pub checkpoint: Option<LegacyImportCheckpoint>,
    /// Present only after full stable-source coverage and successful/idempotent
    /// sink results. Unsupported or torn input never receives a receipt.
    pub receipt: Option<LegacyImportReceipt>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyImportError {
    InvalidLimits,
    OpenFailed,
    MetadataFailed,
    ReadFailed,
}

/// Imports one exact closed source without directory discovery or scheduling.
/// `previous` stabilizes import time across retries; imported IDs are replayed
/// idempotently from the beginning so the sink remains the commit authority.
pub fn import_legacy_source(
    source: impl AsRef<Path>,
    limits: LegacyImportLimits,
    previous: Option<&LegacyImportCheckpoint>,
    sink: &mut dyn LegacyImportSink,
) -> Result<LegacyImportResult, LegacyImportError> {
    let limits = limits.validate()?;
    let source = source.as_ref();
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(source)
        .map_err(|_| LegacyImportError::OpenFailed)?;
    match <fs::File as fs4::FileExt>::try_lock(&file) {
        Ok(()) => {}
        Err(fs4::TryLockError::WouldBlock) => {
            return Ok(incomplete_without_checkpoint(
                LegacyCoverageIssue::ActiveSource,
            ));
        }
        Err(_) => return Err(LegacyImportError::OpenFailed),
    }
    let before = SourceIdentity::read(source, &file)?;
    if before.file_identity == FileIdentity::Replacement {
        return Ok(incomplete_without_checkpoint(
            LegacyCoverageIssue::SourceReplaced,
        ));
    }
    if before.len > limits.max_source_bytes {
        return Ok(incomplete_without_checkpoint(
            LegacyCoverageIssue::SourceByteLimit,
        ));
    }
    let capacity = usize::try_from(before.len).map_err(|_| LegacyImportError::ReadFailed)?;
    let mut bytes = Vec::with_capacity(capacity);
    file.by_ref()
        .take(limits.max_source_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| LegacyImportError::ReadFailed)?;
    if bytes.len() as u64 > limits.max_source_bytes {
        let mut result = incomplete_without_checkpoint(LegacyCoverageIssue::SourceByteLimit);
        result.coverage.bytes_observed = bytes.len() as u64;
        return Ok(result);
    }
    let after = SourceIdentity::read(source, &file)?;
    if after.file_identity == FileIdentity::Replacement {
        return Ok(incomplete_without_checkpoint(
            LegacyCoverageIssue::SourceReplaced,
        ));
    }
    if before.file_identity != after.file_identity {
        return Ok(incomplete_without_checkpoint(
            LegacyCoverageIssue::SourceReplaced,
        ));
    }
    if before.len != after.len
        || before.modified != after.modified
        || bytes.len() as u64 != before.len
    {
        return Ok(incomplete_without_checkpoint(
            LegacyCoverageIssue::SourceChanged,
        ));
    }

    let source_sha256 = Digest32::sha256(&bytes);
    let producer_material = parsed_producer_material(&bytes);
    let legacy_source_id = legacy_source_id(source_sha256, before.len, &producer_material);
    if previous.is_some_and(|checkpoint| checkpoint.legacy_source_id != legacy_source_id) {
        let mut result =
            incomplete_without_checkpoint(LegacyCoverageIssue::CheckpointSourceMismatch);
        result.coverage.bytes_observed = before.len;
        return Ok(result);
    }
    let imported_at = previous
        .map(|checkpoint| checkpoint.imported_at_unix_micros)
        .unwrap_or_else(|| Utc::now().timestamp_micros().max(0));

    let mut coverage = LegacyImportCoverage {
        complete: true,
        bytes_observed: before.len,
        complete_records_observed: 0,
        supported_records: 0,
        unsupported_records: 0,
        torn_records: 0,
        issues: Vec::new(),
    };
    let mut offset = 0_usize;
    let mut imported_event_ids = Vec::new();
    let mut imported_event_digests = Vec::new();
    for (ordinal, line) in bytes.split_inclusive(|byte| *byte == b'\n').enumerate() {
        if !line.ends_with(b"\n") {
            coverage.complete = false;
            coverage.torn_records = coverage.torn_records.saturating_add(1);
            coverage.issues.push(LegacyCoverageIssue::TornTail);
            break;
        }
        if ordinal >= limits.max_complete_records {
            coverage.complete = false;
            coverage.issues.push(LegacyCoverageIssue::RecordLimit);
            break;
        }
        coverage.complete_records_observed = coverage.complete_records_observed.saturating_add(1);
        let body = &line[..line.len() - 1];
        let line_sha256 = Digest32::sha256(body);
        let envelope = derive_legacy_envelope(
            body,
            legacy_source_id,
            offset as u64,
            ordinal as u64,
            line_sha256,
            imported_at,
        );
        match envelope {
            Ok(envelope) => match sink.append_imported(&envelope) {
                Ok(
                    LegacyImportDisposition::Appended | LegacyImportDisposition::AlreadyCommitted,
                ) => {
                    coverage.supported_records = coverage.supported_records.saturating_add(1);
                    imported_event_ids.push(envelope.event_id);
                    if let Ok(digest) = envelope.immutable_digest() {
                        imported_event_digests.push(digest);
                    }
                }
                Err(_) => {
                    coverage.complete = false;
                    coverage.issues.push(LegacyCoverageIssue::SinkFailed {
                        byte_offset: offset as u64,
                        ordinal: ordinal as u64,
                    });
                }
            },
            Err(()) => {
                coverage.complete = false;
                coverage.unsupported_records = coverage.unsupported_records.saturating_add(1);
                coverage
                    .issues
                    .push(LegacyCoverageIssue::UnsupportedRecord {
                        byte_offset: offset as u64,
                        ordinal: ordinal as u64,
                    });
            }
        }
        offset = offset.saturating_add(line.len());
    }

    let prefix_sha256 = Digest32::sha256(&bytes[..offset.min(bytes.len())]);
    let checkpoint = LegacyImportCheckpoint {
        legacy_source_id,
        complete_line_byte_offset: offset as u64,
        complete_record_ordinal: coverage.complete_records_observed,
        prefix_sha256,
        supported_records: coverage.supported_records,
        unsupported_records: coverage.unsupported_records,
        torn_records: coverage.torn_records,
        imported_at_unix_micros: imported_at,
        imported_event_ids,
    };
    let receipt = coverage.complete.then(|| LegacyImportReceipt {
        legacy_source_id,
        source_bytes: before.len,
        source_sha256,
        completed_at_unix_micros: Utc::now().timestamp_micros().max(0),
        checkpoint: checkpoint.clone(),
        imported_event_digests,
    });
    Ok(LegacyImportResult {
        coverage,
        checkpoint: Some(checkpoint),
        receipt,
    })
}

fn derive_legacy_envelope(
    line: &[u8],
    legacy_source_id: Digest32,
    byte_offset: u64,
    ordinal: u64,
    line_sha256: Digest32,
    imported_at: i64,
) -> Result<EventEnvelopeV1, ()> {
    if let Ok(mut existing) = serde_json::from_slice::<EventEnvelopeV1>(line) {
        existing.validate().map_err(|_| ())?;
        if existing.legacy_provenance.is_some() {
            return Err(());
        }
        existing.producer.writer_instance_id = derived_writer_id(legacy_source_id);
        existing.ingested_at_unix_micros = imported_at;
        existing.retry_of_generation_id = None;
        existing.legacy_provenance = Some(LegacyProvenance {
            legacy_source_id,
            complete_line_byte_offset: byte_offset,
            complete_record_ordinal: ordinal,
            line_sha256,
            original_schema: "event-envelope-v1".to_string(),
            synthetic_fields: vec![LegacySyntheticField::WriterInstance],
            unavailable_fields: Vec::new(),
        });
        existing.validate().map_err(|_| ())?;
        return Ok(existing);
    }
    if let Ok(event) = serde_json::from_slice::<DiagnosticEvent>(line) {
        return derive_diagnostic_envelope(
            event,
            legacy_source_id,
            byte_offset,
            ordinal,
            line_sha256,
            imported_at,
        );
    }
    derive_lifecycle_envelope(
        line,
        legacy_source_id,
        byte_offset,
        ordinal,
        line_sha256,
        imported_at,
    )
}

fn derive_diagnostic_envelope(
    event: DiagnosticEvent,
    legacy_source_id: Digest32,
    byte_offset: u64,
    ordinal: u64,
    line_sha256: Digest32,
    imported_at: i64,
) -> Result<EventEnvelopeV1, ()> {
    if !(1..=DIAGNOSTIC_SCHEMA_VERSION).contains(&event.schema_version) {
        return Err(());
    }
    if event.schema_version >= 2
        && (event.retention_eligible_at.as_deref() != Some(event.recorded_at.as_str())
            || event.retention_status != DiagnosticRetentionStatus::Eligible)
    {
        return Err(());
    }
    let preserved_event_id = parse_uuid(&event.event_id.to_string()).filter(|id| !id.is_nil());
    let event_id = preserved_event_id
        .map(EventId::from)
        .unwrap_or_else(|| derived_event_id(legacy_source_id, byte_offset, line_sha256));
    let process_uuid =
        parse_uuid(&event.process.producer_instance.to_string()).filter(|id| !id.is_nil());
    let process_instance_id = process_uuid
        .map(ProcessInstanceId::from)
        .unwrap_or_else(|| derived_process_id(legacy_source_id, &event));
    let writer_instance_id = derived_writer_id(legacy_source_id);
    let trace = parse_uuid(&event.diagnostic_id.to_string()).ok_or(())?;
    let span = parse_uuid(&event.span_id.to_string()).ok_or(())?;
    let parent = event
        .parent_span_id
        .as_ref()
        .map(|id| parse_uuid(&id.to_string()).map(SpanId::from).ok_or(()))
        .transpose()?;
    let recorded_at = DateTime::parse_from_rfc3339(&event.recorded_at)
        .map_err(|_| ())?
        .timestamp_micros();
    if recorded_at < 0 {
        return Err(());
    }
    let mut synthetic_fields = vec![
        LegacySyntheticField::WriterInstance,
        LegacySyntheticField::SelfRoot,
        LegacySyntheticField::ProducerSequence,
    ];
    if preserved_event_id.is_none() {
        synthetic_fields.push(LegacySyntheticField::EventId);
    }
    if process_uuid.is_none() {
        synthetic_fields.push(LegacySyntheticField::ProcessInstance);
    }
    let payload = json!({
        "operation": event.operation,
        "resource": event.resource,
        "lifecycle_phase": event.lifecycle_phase,
        "phase": event.phase,
        "elapsed_micros": event.elapsed_micros,
        "observation": event.observation,
        "diagnostic_correlations": event.correlations,
        "sqlite": event.sqlite,
    });
    let policy = PayloadNormalizationPolicy::registered(&[
        "operation",
        "resource",
        "lifecycle_phase",
        "phase",
        "elapsed_micros",
        "observation",
        "diagnostic_correlations",
        "sqlite",
    ])
    .map_err(|_| ())?;
    EventEnvelopeV1::normalize(
        NewEventV1 {
            event_id,
            family: EventFamily::Diagnostic,
            kind: EventKind::registered("legacy.diagnostic").map_err(|_| ())?,
            recorded_at_unix_micros: recorded_at,
            producer_sequence: i64::try_from(ordinal).map_err(|_| ())?,
            producer: ProducerIdentity {
                writer_instance_id,
                process_instance_id,
                process_root_id: process_instance_id,
                parent_process_instance_id: None,
                supervisor_authority_id: None,
                native_process: native_identity(&event),
            },
            correlations: EventCorrelations {
                trace_id: Some(TraceId::from(trace)),
                span_id: Some(SpanId::from(span)),
                parent_span_id: parent,
                invocation_uuid: None,
                session_correlation_sha256: None,
            },
            payload,
            legacy_provenance: Some(LegacyProvenance {
                legacy_source_id,
                complete_line_byte_offset: byte_offset,
                complete_record_ordinal: ordinal,
                line_sha256,
                original_schema: "age369-diagnostic-v1".to_string(),
                synthetic_fields,
                unavailable_fields: vec![
                    "invocation_uuid".to_string(),
                    "session_correlation".to_string(),
                    "process_parent".to_string(),
                ],
            }),
            retry_of_generation_id: None,
        },
        &policy,
        imported_at,
    )
    .map_err(|_| ())
}

fn derive_lifecycle_envelope(
    line: &[u8],
    legacy_source_id: Digest32,
    byte_offset: u64,
    ordinal: u64,
    line_sha256: Digest32,
    imported_at: i64,
) -> Result<EventEnvelopeV1, ()> {
    let mut value = serde_json::from_slice::<Value>(line).map_err(|_| ())?;
    let object = value.as_object_mut().ok_or(())?;
    if let Some(legacy_timestamp) = object.remove("timestamp") {
        if let Some(recorded_at) = object.get("recorded_at") {
            if recorded_at != &legacy_timestamp {
                return Err(());
            }
        } else {
            object.insert("recorded_at".to_string(), legacy_timestamp);
        }
    }
    let normalized = normalize_lifecycle_record(&value).map_err(|_| ())?;
    let process_instance_id =
        derived_id16(b"oulipoly.legacy-process.v1\0", legacy_source_id.as_bytes());
    let policy =
        PayloadNormalizationPolicy::registered(lifecycle_payload_fields()).map_err(|_| ())?;
    EventEnvelopeV1::normalize(
        NewEventV1 {
            event_id: derived_event_id(legacy_source_id, byte_offset, line_sha256),
            family: EventFamily::Log,
            kind: normalized.kind,
            recorded_at_unix_micros: normalized.recorded_at_unix_micros,
            producer_sequence: i64::try_from(ordinal).map_err(|_| ())?,
            producer: ProducerIdentity {
                writer_instance_id: derived_writer_id(legacy_source_id),
                process_instance_id,
                process_root_id: process_instance_id,
                parent_process_instance_id: None,
                supervisor_authority_id: None,
                native_process: None,
            },
            correlations: normalized.correlations,
            payload: normalized.payload,
            legacy_provenance: Some(LegacyProvenance {
                legacy_source_id,
                complete_line_byte_offset: byte_offset,
                complete_record_ordinal: ordinal,
                line_sha256,
                original_schema: "lifecycle-log-v1".to_string(),
                synthetic_fields: vec![
                    LegacySyntheticField::EventId,
                    LegacySyntheticField::WriterInstance,
                    LegacySyntheticField::ProcessInstance,
                    LegacySyntheticField::SelfRoot,
                    LegacySyntheticField::ProducerSequence,
                ],
                unavailable_fields: vec!["process_parent".to_string()],
            }),
            retry_of_generation_id: None,
        },
        &policy,
        imported_at,
    )
    .map_err(|_| ())
}

fn parsed_producer_material(bytes: &[u8]) -> Vec<u8> {
    for line in bytes.split(|byte| *byte == b'\n') {
        if let Ok(event) = serde_json::from_slice::<EventEnvelopeV1>(line) {
            let native = event.producer.native_process.as_ref();
            return format!(
                "{}:{}:{}:{}:{}",
                event.producer.process_instance_id,
                native.map(|identity| identity.os_pid).unwrap_or(0),
                native
                    .map(|identity| identity.os_pid_starttime_ticks)
                    .unwrap_or(0),
                native
                    .map(|identity| hex(identity.os_boot_id_sha256.as_bytes()))
                    .unwrap_or_else(|| "unknown".to_string()),
                event.producer.writer_instance_id,
            )
            .into_bytes();
        }
        if let Ok(event) = serde_json::from_slice::<DiagnosticEvent>(line) {
            return format!(
                "{}:{}:{}:{}",
                event.process.producer_instance,
                event.process.os_pid,
                event.process.os_pid_starttime_ticks.unwrap_or(0),
                event.process.os_boot_id.as_deref().unwrap_or("unknown")
            )
            .into_bytes();
        }
    }
    b"producer-identity-unavailable".to_vec()
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn legacy_source_id(source_sha256: Digest32, len: u64, producer: &[u8]) -> Digest32 {
    let mut digest = Sha256::new();
    digest.update(b"oulipoly.legacy-source.v1\0");
    digest.update(source_sha256.as_bytes());
    digest.update(len.to_le_bytes());
    digest.update(producer);
    Digest32::from_bytes(digest.finalize().into())
}

fn derived_event_id(source: Digest32, offset: u64, line_sha256: Digest32) -> EventId {
    let mut digest = Sha256::new();
    digest.update(b"oulipoly.legacy-event.v1\0");
    digest.update(source.as_bytes());
    digest.update(offset.to_le_bytes());
    digest.update(line_sha256.as_bytes());
    EventId::from_bytes(digest.finalize()[..16].try_into().unwrap_or([0; 16]))
}

fn derived_writer_id(source: Digest32) -> WriterInstanceId {
    WriterInstanceId::from_bytes(
        derived_id16(b"oulipoly.legacy-writer.v1\0", source.as_bytes())
            .as_bytes()
            .to_owned(),
    )
}

fn derived_process_id(source: Digest32, event: &DiagnosticEvent) -> ProcessInstanceId {
    let material = format!(
        "{}:{}:{}",
        event.process.os_pid,
        event.process.os_pid_starttime_ticks.unwrap_or(0),
        event.process.os_boot_id.as_deref().unwrap_or("unknown")
    );
    let mut digest = Sha256::new();
    digest.update(b"oulipoly.legacy-process.v1\0");
    digest.update(source.as_bytes());
    digest.update(material.as_bytes());
    ProcessInstanceId::from_bytes(digest.finalize()[..16].try_into().unwrap_or([0; 16]))
}

fn derived_id16(domain: &[u8], value: &[u8]) -> ProcessInstanceId {
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update(value);
    ProcessInstanceId::from_bytes(digest.finalize()[..16].try_into().unwrap_or([0; 16]))
}

fn native_identity(event: &DiagnosticEvent) -> Option<NativeProcessIdentity> {
    let boot = event.process.os_boot_id.as_ref()?;
    let starttime = event
        .process
        .os_pid_starttime_ticks
        .filter(|value| *value > 0)?;
    (event.process.os_pid > 0).then(|| NativeProcessIdentity {
        os_pid: event.process.os_pid,
        os_boot_id_sha256: Digest32::sha256(boot.as_bytes()),
        os_pid_starttime_ticks: starttime,
    })
}

fn parse_uuid(value: &str) -> Option<Uuid> {
    Uuid::parse_str(value).ok()
}

fn incomplete_without_checkpoint(issue: LegacyCoverageIssue) -> LegacyImportResult {
    LegacyImportResult {
        coverage: LegacyImportCoverage {
            complete: false,
            bytes_observed: 0,
            complete_records_observed: 0,
            supported_records: 0,
            unsupported_records: 0,
            torn_records: 0,
            issues: vec![issue],
        },
        checkpoint: None,
        receipt: None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceIdentity {
    len: u64,
    file_identity: FileIdentity,
    modified: Option<std::time::SystemTime>,
}

impl SourceIdentity {
    fn read(path: &Path, file: &fs::File) -> Result<Self, LegacyImportError> {
        let descriptor = file
            .metadata()
            .map_err(|_| LegacyImportError::MetadataFailed)?;
        let pathname = fs::metadata(path).map_err(|_| LegacyImportError::MetadataFailed)?;
        let descriptor_identity = FileIdentity::from_metadata(&descriptor);
        let pathname_identity = FileIdentity::from_metadata(&pathname);
        if descriptor_identity != pathname_identity {
            return Ok(Self {
                len: descriptor.len(),
                file_identity: FileIdentity::Replacement,
                modified: descriptor.modified().ok(),
            });
        }
        Ok(Self {
            len: descriptor.len(),
            file_identity: descriptor_identity,
            modified: descriptor.modified().ok(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FileIdentity {
    #[cfg(unix)]
    Unix {
        device: u64,
        inode: u64,
    },
    #[cfg(not(unix))]
    Portable {
        len: u64,
        modified_nanos: u128,
    },
    Replacement,
}

impl FileIdentity {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self::Unix {
                device: metadata.dev(),
                inode: metadata.ino(),
            }
        }
        #[cfg(not(unix))]
        {
            let modified_nanos = metadata
                .modified()
                .ok()
                .and_then(|value| value.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
                .map(|value| value.as_nanos())
                .unwrap_or(0);
            Self::Portable {
                len: metadata.len(),
                modified_nanos,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic_recorder::{
        DiagnosticId, DiagnosticPhase, EventId as LegacyEventId, OutcomeCertainty,
        PhaseObservation, ProducerInstanceId, RecorderProcessIdentity, SpanId as LegacySpanId,
    };
    use std::collections::BTreeMap;
    use std::io::Write;

    #[derive(Default)]
    struct CollectingSink {
        events: Vec<EventEnvelopeV1>,
    }

    impl LegacyImportSink for CollectingSink {
        fn append_imported(
            &mut self,
            envelope: &EventEnvelopeV1,
        ) -> Result<LegacyImportDisposition, LegacyImportSinkError> {
            self.events.push(envelope.clone());
            Ok(LegacyImportDisposition::Appended)
        }
    }

    fn fixture_event() -> DiagnosticEvent {
        DiagnosticEvent {
            schema_version: DIAGNOSTIC_SCHEMA_VERSION,
            event_id: LegacyEventId::new(),
            diagnostic_id: DiagnosticId::new(),
            span_id: LegacySpanId::new(),
            parent_span_id: None,
            recorded_at: "2026-09-20T12:34:56.123456Z".to_string(),
            retention_eligible_at: Some("2026-09-20T12:34:56.123456Z".to_string()),
            retention_status: DiagnosticRetentionStatus::Eligible,
            elapsed_micros: 9,
            process: RecorderProcessIdentity {
                os_pid: 42,
                parent_pid: Some(41),
                os_boot_id: Some("boot-fixture".to_string()),
                os_pid_starttime_ticks: Some(7),
                producer_instance: ProducerInstanceId::new(),
            },
            operation: "fixture".to_string(),
            resource: "state_sqlite".to_string(),
            lifecycle_phase: Some("test".to_string()),
            phase: DiagnosticPhase::Failed,
            observation: PhaseObservation {
                certainty: OutcomeCertainty::StartedUnknown,
                causes: vec!["token=must-not-survive /private/path".to_string()],
                ..PhaseObservation::default()
            },
            correlations: BTreeMap::new(),
            sqlite: None,
        }
    }

    fn limits() -> LegacyImportLimits {
        LegacyImportLimits {
            max_source_bytes: 1024 * 1024,
            max_complete_records: 16,
        }
    }

    #[test]
    fn deterministic_import_preserves_event_id_and_retry_digest() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("closed.jsonl");
        let event = fixture_event();
        let mut encoded = serde_json::to_vec(&event).unwrap();
        encoded.push(b'\n');
        fs::write(&source, encoded).unwrap();

        let mut first_sink = CollectingSink::default();
        let first = import_legacy_source(&source, limits(), None, &mut first_sink).unwrap();
        assert!(first.coverage.complete, "{:?}", first.coverage);
        assert!(first.receipt.is_some());
        assert_eq!(first_sink.events.len(), 1);
        assert_eq!(
            first_sink.events[0].event_id.to_string(),
            event.event_id.to_string()
        );
        assert!(!first_sink.events[0].payload.contains("must-not-survive"));
        assert!(!first_sink.events[0].payload.contains("/private/path"));

        let mut retry_sink = CollectingSink::default();
        let retry = import_legacy_source(
            &source,
            limits(),
            first.checkpoint.as_ref(),
            &mut retry_sink,
        )
        .unwrap();
        assert_eq!(
            retry.checkpoint.as_ref().unwrap().legacy_source_id,
            first.checkpoint.as_ref().unwrap().legacy_source_id
        );
        assert_eq!(
            retry_sink.events[0].immutable_digest().unwrap(),
            first_sink.events[0].immutable_digest().unwrap()
        );
    }

    #[test]
    fn version_one_fallback_import_preserves_prefanout_logical_identity() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("closed-v1.jsonl");
        let process = ProcessInstanceId::from_bytes([2; 16]);
        let native = EventEnvelopeV1::normalize(
            NewEventV1 {
                event_id: EventId::from_bytes([1; 16]),
                family: EventFamily::Log,
                kind: EventKind::registered("fallback.notice").unwrap(),
                recorded_at_unix_micros: 10,
                producer_sequence: 7,
                producer: ProducerIdentity {
                    writer_instance_id: WriterInstanceId::from_bytes([3; 16]),
                    process_instance_id: process,
                    process_root_id: process,
                    parent_process_instance_id: None,
                    supervisor_authority_id: None,
                    native_process: Some(NativeProcessIdentity {
                        os_pid: 42,
                        os_boot_id_sha256: Digest32::from_bytes([4; 32]),
                        os_pid_starttime_ticks: 9,
                    }),
                },
                correlations: EventCorrelations::default(),
                payload: json!({"value": 4}),
                legacy_provenance: None,
                retry_of_generation_id: None,
            },
            &PayloadNormalizationPolicy::registered(&["value"]).unwrap(),
            11,
        )
        .unwrap();
        let original_digest = native.immutable_digest().unwrap();
        let mut encoded = serde_json::to_vec(&native).unwrap();
        encoded.push(b'\n');
        fs::write(&source, encoded).unwrap();

        let mut sink = CollectingSink::default();
        let result = import_legacy_source(&source, limits(), None, &mut sink).unwrap();
        assert!(result.coverage.complete, "{:?}", result.coverage);
        assert_eq!(sink.events.len(), 1);
        assert_eq!(sink.events[0].event_id, native.event_id);
        assert_eq!(sink.events[0].producer_sequence, native.producer_sequence);
        assert_ne!(
            sink.events[0].producer.writer_instance_id,
            native.producer.writer_instance_id
        );
        assert!(sink.events[0].legacy_provenance.is_some());
        assert_eq!(sink.events[0].immutable_digest().unwrap(), original_digest);
    }

    #[test]
    fn active_and_torn_sources_are_explicitly_incomplete() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("active.jsonl");
        let mut encoded = serde_json::to_vec(&fixture_event()).unwrap();
        encoded.push(b'\n');
        encoded.extend_from_slice(b"{\"torn\":");
        fs::write(&source, &encoded).unwrap();

        let active = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&source)
            .unwrap();
        <fs::File as fs4::FileExt>::try_lock(&active).unwrap();
        let mut sink = CollectingSink::default();
        let result = import_legacy_source(&source, limits(), None, &mut sink).unwrap();
        assert_eq!(
            result.coverage.issues,
            vec![LegacyCoverageIssue::ActiveSource]
        );
        assert!(result.receipt.is_none());
        drop(active);

        let result = import_legacy_source(&source, limits(), None, &mut sink).unwrap();
        assert_eq!(result.coverage.supported_records, 1);
        assert_eq!(result.coverage.torn_records, 1);
        assert!(
            result
                .coverage
                .issues
                .contains(&LegacyCoverageIssue::TornTail)
        );
        assert!(result.receipt.is_none());
    }

    #[test]
    fn lifecycle_without_trustworthy_timestamp_remains_preserved_unsupported() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("lifecycle.jsonl");
        let record = json!({
            "event_name": "invocation.started",
            "invocation_uuid": Uuid::new_v4().to_string(),
            "provider_source": null,
            "chain_id": null,
            "session_id": "raw-session",
            "latency_us": 1,
            "operation_result": "ok",
            "error_chain": null,
            "invocation_row_id": 1,
            "model": "fixture",
            "provider": "fixture",
            "parent_invocation_uuid": null
        });
        let mut file = fs::File::create(&source).unwrap();
        serde_json::to_writer(&mut file, &record).unwrap();
        file.write_all(b"\n").unwrap();
        drop(file);

        let mut sink = CollectingSink::default();
        let result = import_legacy_source(&source, limits(), None, &mut sink).unwrap();
        assert!(!result.coverage.complete);
        assert_eq!(result.coverage.unsupported_records, 1);
        assert!(result.receipt.is_none());
        assert!(sink.events.is_empty());

        let renamed = directory.path().join("renamed-lifecycle.jsonl");
        fs::rename(&source, &renamed).unwrap();
        let renamed_result = import_legacy_source(&renamed, limits(), None, &mut sink).unwrap();
        assert_eq!(
            renamed_result.checkpoint.as_ref().unwrap().legacy_source_id,
            result.checkpoint.as_ref().unwrap().legacy_source_id
        );
    }

    #[test]
    fn unavailable_producer_source_identity_is_stable_across_rename() {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("unknown.jsonl");
        fs::write(&original, b"{\"unknown_schema\":true}\n").unwrap();

        let mut sink = CollectingSink::default();
        let first = import_legacy_source(&original, limits(), None, &mut sink).unwrap();
        let renamed = directory.path().join("renamed.jsonl");
        fs::rename(&original, &renamed).unwrap();
        let second = import_legacy_source(&renamed, limits(), None, &mut sink).unwrap();

        assert_eq!(
            first.checkpoint.as_ref().unwrap().legacy_source_id,
            second.checkpoint.as_ref().unwrap().legacy_source_id
        );
        assert_eq!(first.coverage.unsupported_records, 1);
        assert_eq!(second.coverage.unsupported_records, 1);
        assert!(first.receipt.is_none());
        assert!(second.receipt.is_none());
    }
}
