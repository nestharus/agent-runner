//! Bounded union of partition reads and already-normalized exact legacy input.
//!
//! This module performs no discovery, filesystem access, import, scheduling,
//! or maintenance. Callers supply a completed [`BoundedRead`] and explicit
//! legacy-source results obtained through the offline importer boundary.

use super::{
    BoundedRead, Digest32, EventEnvelopeV1, EventFilter, EventId, GenerationId,
    LegacyImportCheckpoint, LegacyImportCoverage, LegacyImportReceipt, ReadLimits,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactLegacyRead {
    pub legacy_source_id: Digest32,
    pub envelopes: Vec<EventEnvelopeV1>,
    pub coverage: LegacyImportCoverage,
    pub checkpoint: Option<LegacyImportCheckpoint>,
    pub receipt: Option<LegacyImportReceipt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "source")]
pub enum UnionRecordOrigin {
    Partition {
        generation_id: GenerationId,
        local_sequence: i64,
    },
    Legacy {
        legacy_source_id: Digest32,
        complete_line_byte_offset: u64,
        complete_record_ordinal: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnionReadRecord {
    pub immutable_sha256: Digest32,
    pub envelope: EventEnvelopeV1,
    pub origins: Vec<UnionRecordOrigin>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyUnionCoverage {
    pub legacy_source_id: Digest32,
    pub coverage: LegacyImportCoverage,
    pub checkpoint: Option<LegacyImportCheckpoint>,
    pub receipt: Option<LegacyImportReceipt>,
    pub envelopes_supplied: usize,
    pub envelopes_examined: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnionCoverageIssueKind {
    PartitionCoverageIncomplete,
    LegacyCoverageIncomplete,
    LegacySourceLimitReached,
    RecordLimitReached,
    PayloadByteLimitReached,
    InvalidFilter,
    InvalidEnvelope,
    LegacySourceIdentityMismatch,
    DuplicateLogicalEvent,
    IdentityConflict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnionCoverageIssue {
    pub kind: UnionCoverageIssueKind,
    pub event_id: Option<EventId>,
    pub legacy_source_id: Option<Digest32>,
    pub detail: String,
    pub compromises_completeness: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoundedUnionRead {
    pub records: Vec<UnionReadRecord>,
    /// Preserves partition watermarks, discovery state, and per-generation
    /// issues without pretending the union reader rediscovered partitions.
    pub partition: BoundedRead,
    pub legacy_sources: Vec<LegacyUnionCoverage>,
    pub issues: Vec<UnionCoverageIssue>,
    pub coverage_complete: bool,
    pub legacy_sources_omitted: usize,
    pub candidate_records_examined: usize,
    pub candidate_records_omitted: usize,
    pub records_returned: usize,
    pub payload_bytes_returned: usize,
    pub duplicate_records: usize,
}

/// Unions one already-bounded partition result with caller-addressed legacy
/// sources. The same limits bound aggregate sources, candidate records, and
/// returned payload; hitting a bound is an explicit incomplete result.
pub fn union_bounded_read(
    partition: BoundedRead,
    legacy: &[ExactLegacyRead],
    filter: &EventFilter,
    limits: &ReadLimits,
) -> BoundedUnionRead {
    let mut result = BoundedUnionRead {
        coverage_complete: partition.coverage_complete,
        records: Vec::new(),
        legacy_sources: Vec::new(),
        issues: Vec::new(),
        legacy_sources_omitted: 0,
        candidate_records_examined: 0,
        candidate_records_omitted: 0,
        records_returned: 0,
        payload_bytes_returned: 0,
        duplicate_records: 0,
        partition,
    };
    if !result.partition.coverage_complete {
        result.issue(
            UnionCoverageIssueKind::PartitionCoverageIncomplete,
            None,
            None,
            "partition read reported incomplete coverage",
            true,
        );
    }
    if !valid_filter(filter) {
        result.issue(
            UnionCoverageIssueKind::InvalidFilter,
            None,
            None,
            "invalid event range or unsupported unanchored kind/span/process filter",
            true,
        );
        return result;
    }

    let remaining_sources = limits
        .max_partitions()
        .saturating_sub(result.partition.partitions_examined);
    let selected_legacy = legacy.len().min(remaining_sources);
    result.legacy_sources_omitted = legacy.len().saturating_sub(selected_legacy);
    if result.legacy_sources_omitted > 0 {
        result.issue(
            UnionCoverageIssueKind::LegacySourceLimitReached,
            None,
            None,
            &format!(
                "{} exact legacy sources omitted by the aggregate source bound",
                result.legacy_sources_omitted
            ),
            true,
        );
    }

    let candidate_total = result.partition.records.len().saturating_add(
        legacy
            .iter()
            .take(selected_legacy)
            .fold(0_usize, |total, source| {
                total.saturating_add(source.envelopes.len())
            }),
    );
    let mut by_event = HashMap::<EventId, (usize, Digest32)>::new();
    let mut by_digest = HashMap::<Digest32, usize>::new();
    let partition_candidates = result.partition.records.clone();
    for record in partition_candidates {
        if !result.has_candidate_budget(limits, candidate_total) {
            break;
        }
        result.candidate_records_examined += 1;
        if record.envelope.validate().is_err()
            || record.envelope.immutable_digest().ok() != Some(record.immutable_sha256)
        {
            result.issue(
                UnionCoverageIssueKind::InvalidEnvelope,
                Some(record.envelope.event_id),
                None,
                "partition record failed envelope or immutable-digest validation",
                true,
            );
            continue;
        }
        if !matches_filter(&record.envelope, filter) {
            continue;
        }
        result.insert(
            record.envelope,
            record.immutable_sha256,
            UnionRecordOrigin::Partition {
                generation_id: record.generation_id,
                local_sequence: record.local_sequence,
            },
            None,
            limits,
            &mut by_event,
            &mut by_digest,
        );
    }

    for source in legacy.iter().take(selected_legacy) {
        let source_index = result.legacy_sources.len();
        result.legacy_sources.push(LegacyUnionCoverage {
            legacy_source_id: source.legacy_source_id,
            coverage: source.coverage.clone(),
            checkpoint: source.checkpoint.clone(),
            receipt: source.receipt.clone(),
            envelopes_supplied: source.envelopes.len(),
            envelopes_examined: 0,
        });
        if !source.coverage.complete {
            result.issue(
                UnionCoverageIssueKind::LegacyCoverageIncomplete,
                None,
                Some(source.legacy_source_id),
                &format!(
                    "legacy import coverage incomplete with {} recorded issues",
                    source.coverage.issues.len()
                ),
                true,
            );
        }
        if source
            .checkpoint
            .as_ref()
            .is_some_and(|checkpoint| checkpoint.legacy_source_id != source.legacy_source_id)
            || source
                .receipt
                .as_ref()
                .is_some_and(|receipt| receipt.legacy_source_id != source.legacy_source_id)
        {
            result.issue(
                UnionCoverageIssueKind::LegacySourceIdentityMismatch,
                None,
                Some(source.legacy_source_id),
                "checkpoint or receipt names a different exact legacy source",
                true,
            );
            continue;
        }
        for envelope in &source.envelopes {
            if !result.has_candidate_budget(limits, candidate_total) {
                break;
            }
            result.candidate_records_examined += 1;
            result.legacy_sources[source_index].envelopes_examined += 1;
            let Some(provenance) = envelope.legacy_provenance.as_ref() else {
                result.issue(
                    UnionCoverageIssueKind::LegacySourceIdentityMismatch,
                    Some(envelope.event_id),
                    Some(source.legacy_source_id),
                    "legacy input has no explicit legacy provenance",
                    true,
                );
                continue;
            };
            if provenance.legacy_source_id != source.legacy_source_id {
                result.issue(
                    UnionCoverageIssueKind::LegacySourceIdentityMismatch,
                    Some(envelope.event_id),
                    Some(source.legacy_source_id),
                    "legacy envelope provenance names a different exact source",
                    true,
                );
                continue;
            }
            if envelope.validate().is_err() {
                result.issue(
                    UnionCoverageIssueKind::InvalidEnvelope,
                    Some(envelope.event_id),
                    Some(source.legacy_source_id),
                    "legacy envelope failed validation",
                    true,
                );
                continue;
            }
            if !matches_filter(envelope, filter) {
                continue;
            }
            let digest = match envelope.immutable_digest() {
                Ok(digest) => digest,
                Err(_) => {
                    result.issue(
                        UnionCoverageIssueKind::InvalidEnvelope,
                        Some(envelope.event_id),
                        Some(source.legacy_source_id),
                        "legacy envelope immutable digest could not be computed",
                        true,
                    );
                    continue;
                }
            };
            result.insert(
                envelope.clone(),
                digest,
                UnionRecordOrigin::Legacy {
                    legacy_source_id: source.legacy_source_id,
                    complete_line_byte_offset: provenance.complete_line_byte_offset,
                    complete_record_ordinal: provenance.complete_record_ordinal,
                },
                Some(source.legacy_source_id),
                limits,
                &mut by_event,
                &mut by_digest,
            );
        }
    }

    result.candidate_records_omitted =
        candidate_total.saturating_sub(result.candidate_records_examined);
    result.records.sort_by_key(|record| {
        (
            record.envelope.recorded_at_unix_micros,
            record.envelope.event_id,
        )
    });
    result.records_returned = result.records.len();
    result
}

impl BoundedUnionRead {
    fn issue(
        &mut self,
        kind: UnionCoverageIssueKind,
        event_id: Option<EventId>,
        legacy_source_id: Option<Digest32>,
        detail: &str,
        compromises_completeness: bool,
    ) {
        if compromises_completeness {
            self.coverage_complete = false;
        }
        self.issues.push(UnionCoverageIssue {
            kind,
            event_id,
            legacy_source_id,
            detail: detail.to_string(),
            compromises_completeness,
        });
    }

    fn has_candidate_budget(&mut self, limits: &ReadLimits, candidate_total: usize) -> bool {
        if self.candidate_records_examined >= limits.max_records() {
            if !self
                .issues
                .iter()
                .any(|issue| issue.kind == UnionCoverageIssueKind::RecordLimitReached)
            {
                self.issue(
                    UnionCoverageIssueKind::RecordLimitReached,
                    None,
                    None,
                    &format!(
                        "{} candidate records omitted by the aggregate record bound",
                        candidate_total.saturating_sub(self.candidate_records_examined)
                    ),
                    true,
                );
            }
            return false;
        }
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn insert(
        &mut self,
        envelope: EventEnvelopeV1,
        digest: Digest32,
        origin: UnionRecordOrigin,
        legacy_source_id: Option<Digest32>,
        limits: &ReadLimits,
        by_event: &mut HashMap<EventId, (usize, Digest32)>,
        by_digest: &mut HashMap<Digest32, usize>,
    ) {
        if let Some((index, known_digest)) = by_event.get(&envelope.event_id).copied() {
            if known_digest == digest {
                if !self.records[index].origins.contains(&origin) {
                    self.records[index].origins.push(origin);
                }
                self.duplicate_records += 1;
                self.issue(
                    UnionCoverageIssueKind::DuplicateLogicalEvent,
                    Some(envelope.event_id),
                    legacy_source_id,
                    "equal event ID and immutable digest deduplicated",
                    false,
                );
            } else {
                self.issue(
                    UnionCoverageIssueKind::IdentityConflict,
                    Some(envelope.event_id),
                    legacy_source_id,
                    "equal event ID has a different immutable digest; conflicting record refused",
                    true,
                );
            }
            return;
        }
        if let Some(index) = by_digest.get(&digest).copied() {
            self.issue(
                UnionCoverageIssueKind::IdentityConflict,
                Some(envelope.event_id),
                legacy_source_id,
                &format!(
                    "immutable digest aliases a different event ID {}; record refused",
                    self.records[index].envelope.event_id
                ),
                true,
            );
            return;
        }
        let bytes = match usize::try_from(envelope.payload_bytes) {
            Ok(bytes) => bytes,
            Err(_) => {
                self.issue(
                    UnionCoverageIssueKind::InvalidEnvelope,
                    Some(envelope.event_id),
                    legacy_source_id,
                    "payload length cannot be represented by this reader",
                    true,
                );
                return;
            }
        };
        if self.payload_bytes_returned.saturating_add(bytes) > limits.max_payload_bytes() {
            self.issue(
                UnionCoverageIssueKind::PayloadByteLimitReached,
                Some(envelope.event_id),
                legacy_source_id,
                "aggregate returned-payload bound reached; record omitted",
                true,
            );
            return;
        }
        let index = self.records.len();
        let event_id = envelope.event_id;
        self.payload_bytes_returned += bytes;
        self.records.push(UnionReadRecord {
            immutable_sha256: digest,
            envelope,
            origins: vec![origin],
        });
        by_event.insert(event_id, (index, digest));
        by_digest.insert(digest, index);
    }
}

fn valid_filter(filter: &EventFilter) -> bool {
    filter
        .recorded_at_or_after_unix_micros
        .is_none_or(|value| value >= 0)
        && filter
            .recorded_before_unix_micros
            .is_none_or(|value| value >= 0)
        && !matches!(
            (
                filter.recorded_at_or_after_unix_micros,
                filter.recorded_before_unix_micros
            ),
            (Some(start), Some(end)) if start > end
        )
        && (filter.span_id.is_none() || filter.trace_id.is_some())
        && (filter.kind.is_none() || filter.family.is_some())
        && (filter.process_instance_id.is_none() || filter.process_root_id.is_some())
}

fn matches_filter(envelope: &EventEnvelopeV1, filter: &EventFilter) -> bool {
    filter
        .recorded_at_or_after_unix_micros
        .is_none_or(|start| envelope.recorded_at_unix_micros >= start)
        && filter
            .recorded_before_unix_micros
            .is_none_or(|end| envelope.recorded_at_unix_micros < end)
        && filter.family.is_none_or(|value| envelope.family == value)
        && filter
            .kind
            .as_ref()
            .is_none_or(|value| &envelope.kind == value)
        && filter
            .process_root_id
            .is_none_or(|value| envelope.producer.process_root_id == value)
        && filter
            .process_instance_id
            .is_none_or(|value| envelope.producer.process_instance_id == value)
        && filter
            .supervisor_authority_id
            .is_none_or(|value| envelope.producer.supervisor_authority_id == Some(value))
        && filter
            .invocation_uuid
            .is_none_or(|value| envelope.correlations.invocation_uuid == Some(value))
        && filter
            .session_correlation_sha256
            .is_none_or(|value| envelope.correlations.session_correlation_sha256 == Some(value))
        && filter
            .trace_id
            .is_none_or(|value| envelope.correlations.trace_id == Some(value))
        && filter
            .span_id
            .is_none_or(|value| envelope.correlations.span_id == Some(value))
        && filter
            .event_id
            .is_none_or(|value| envelope.event_id == value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_store::{
        EventCorrelations, EventFamily, EventKind, LegacyCoverageIssue, LegacyProvenance,
        LegacySyntheticField, NewEventV1, PayloadNormalizationPolicy, ProcessInstanceId,
        ProducerIdentity, WriterInstanceId,
    };
    use serde_json::json;

    fn envelope(event_id: EventId, source: Digest32, message: &str) -> EventEnvelopeV1 {
        EventEnvelopeV1::normalize(
            NewEventV1 {
                event_id,
                family: EventFamily::Log,
                kind: EventKind::registered("fixture.union").unwrap(),
                recorded_at_unix_micros: 10,
                producer_sequence: 1,
                producer: ProducerIdentity {
                    writer_instance_id: WriterInstanceId::from_bytes([2; 16]),
                    process_instance_id: ProcessInstanceId::from_bytes([3; 16]),
                    process_root_id: ProcessInstanceId::from_bytes([3; 16]),
                    parent_process_instance_id: None,
                    supervisor_authority_id: None,
                    native_process: None,
                },
                correlations: EventCorrelations::default(),
                payload: json!({"message": message}),
                legacy_provenance: Some(LegacyProvenance {
                    legacy_source_id: source,
                    complete_line_byte_offset: 0,
                    complete_record_ordinal: 0,
                    line_sha256: Digest32::sha256(message.as_bytes()),
                    original_schema: "fixture-v1".to_string(),
                    synthetic_fields: vec![LegacySyntheticField::WriterInstance],
                    unavailable_fields: Vec::new(),
                }),
                retry_of_generation_id: None,
            },
            &PayloadNormalizationPolicy::registered(&["message"]).unwrap(),
            11,
        )
        .unwrap()
    }

    fn partition(envelope: EventEnvelopeV1) -> BoundedRead {
        BoundedRead {
            records: vec![super::super::ReadRecord {
                generation_id: GenerationId::from_bytes([4; 16]),
                local_sequence: 1,
                immutable_sha256: envelope.immutable_digest().unwrap(),
                envelope,
            }],
            watermarks: Vec::new(),
            issues: Vec::new(),
            coverage_complete: true,
            partitions_examined: 1,
            partitions_omitted: 0,
            records_returned: 1,
            payload_bytes_returned: 0,
            payload_bytes_examined: 0,
            discovery_watermark: Some(Digest32::sha256(b"catalog")),
        }
    }

    fn complete_coverage() -> LegacyImportCoverage {
        LegacyImportCoverage {
            complete: true,
            bytes_observed: 10,
            complete_records_observed: 1,
            supported_records: 1,
            unsupported_records: 0,
            torn_records: 0,
            issues: Vec::new(),
        }
    }

    fn limits() -> ReadLimits {
        ReadLimits::new(4, 8, 4 * 1024).unwrap()
    }

    #[test]
    fn equal_event_id_and_digest_deduplicate_with_both_origins() {
        let source = Digest32::sha256(b"legacy-source");
        let event = envelope(EventId::from_bytes([1; 16]), source, "same");
        let legacy = ExactLegacyRead {
            legacy_source_id: source,
            envelopes: vec![event.clone()],
            coverage: complete_coverage(),
            checkpoint: None,
            receipt: None,
        };

        let result = union_bounded_read(
            partition(event),
            &[legacy],
            &EventFilter::default(),
            &limits(),
        );

        assert!(result.coverage_complete, "{:?}", result.issues);
        assert_eq!(result.records.len(), 1);
        assert_eq!(result.records[0].origins.len(), 2);
        assert_eq!(result.duplicate_records, 1);
        assert!(result.issues.iter().any(|issue| {
            issue.kind == UnionCoverageIssueKind::DuplicateLogicalEvent
                && !issue.compromises_completeness
        }));
    }

    #[test]
    fn equal_event_id_with_different_digest_is_refused_as_conflict() {
        let source = Digest32::sha256(b"legacy-source");
        let event_id = EventId::from_bytes([1; 16]);
        let partition_event = envelope(event_id, source, "partition");
        let legacy_event = envelope(event_id, source, "legacy-conflict");
        let legacy = ExactLegacyRead {
            legacy_source_id: source,
            envelopes: vec![legacy_event],
            coverage: complete_coverage(),
            checkpoint: None,
            receipt: None,
        };

        let result = union_bounded_read(
            partition(partition_event),
            &[legacy],
            &EventFilter::default(),
            &limits(),
        );

        assert!(!result.coverage_complete);
        assert_eq!(result.records.len(), 1);
        assert!(result.issues.iter().any(|issue| {
            issue.kind == UnionCoverageIssueKind::IdentityConflict && issue.compromises_completeness
        }));
    }

    #[test]
    fn incomplete_legacy_coverage_is_preserved_in_union_result() {
        let source = Digest32::sha256(b"torn-source");
        let legacy = ExactLegacyRead {
            legacy_source_id: source,
            envelopes: Vec::new(),
            coverage: LegacyImportCoverage {
                complete: false,
                bytes_observed: 7,
                complete_records_observed: 0,
                supported_records: 0,
                unsupported_records: 0,
                torn_records: 1,
                issues: vec![LegacyCoverageIssue::TornTail],
            },
            checkpoint: None,
            receipt: None,
        };
        let empty_partition = BoundedRead {
            records: Vec::new(),
            watermarks: Vec::new(),
            issues: Vec::new(),
            coverage_complete: true,
            partitions_examined: 0,
            partitions_omitted: 0,
            records_returned: 0,
            payload_bytes_returned: 0,
            payload_bytes_examined: 0,
            discovery_watermark: Some(Digest32::sha256(b"catalog")),
        };

        let result = union_bounded_read(
            empty_partition,
            &[legacy],
            &EventFilter::default(),
            &limits(),
        );

        assert!(!result.coverage_complete);
        assert_eq!(result.legacy_sources.len(), 1);
        assert_eq!(
            result.legacy_sources[0].coverage.issues,
            vec![LegacyCoverageIssue::TornTail]
        );
        assert!(result.issues.iter().any(|issue| {
            issue.kind == UnionCoverageIssueKind::LegacyCoverageIncomplete
                && issue.legacy_source_id == Some(source)
        }));
    }
}
