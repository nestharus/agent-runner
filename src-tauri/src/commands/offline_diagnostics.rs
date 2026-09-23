//! Bounded, read-only diagnostic inspection at the earliest CLI boundary.

use oulipoly_state::diagnostic_recorder::{
    DiagnosticEvent, DiagnosticId, DiagnosticPhase, FlightRecorderReader, InspectionCoverage,
    InspectionIssue, JsonlPageCursor, SourceCoordinate, default_recorder_root,
    diagnostic_event_from_envelope, failure_discriminator,
};
use oulipoly_state::event_store::{
    CoverageIssue, Digest32, EventFamily, EventFilter, EventKind, EvidenceDiscoveryCursor,
    GenerationId, GenerationReadTarget, ReadLimits, TraceId, WriterInstanceId,
    discover_generation_read_targets_page, read_generation_diagnostic_page,
};
use oulipoly_state::longitudinal_metrics::{
    MetricQuery, MetricQueryReport, TraceQueryReport, default_event_store_root, query_metrics,
    query_trace,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;

const DISCOVERY_NODES: usize = 16_384;
const DISCOVERY_GENERATIONS: usize = 256;
const READ_RECORDS: usize = 20_000;
const READ_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DiagnosticSearchCursor {
    query_key: String,
    normal: Option<EvidenceDiscoveryCursor>,
    normal_page: Option<NormalPageCursor>,
    normal_done: bool,
    jsonl: Option<JsonlPageCursor>,
    jsonl_done: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NormalPageCursor {
    discovery_start: EvidenceDiscoveryCursor,
    targets_sha256: Digest32,
    target_index: usize,
    after_local_sequence: Option<i64>,
}

fn target_page_digest(targets: &[GenerationReadTarget]) -> Digest32 {
    let mut digest = Sha256::new();
    digest.update(b"oulipoly.diagnostic-target-page.v1\0");
    for target in targets {
        digest.update(target.writer_instance_id.as_bytes());
        digest.update(target.generation_id.as_bytes());
        digest.update(
            target
                .prepared_manifest_sha256
                .map_or([0; 32], |value| *value.as_bytes()),
        );
    }
    Digest32::from_bytes(digest.finalize().into())
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "snake_case", tag = "store")]
enum DiagnosticOrigin {
    EventGeneration {
        writer_instance_id: WriterInstanceId,
        generation_id: GenerationId,
        local_sequence: i64,
    },
    Jsonl {
        file: String,
        line: u64,
    },
}

#[derive(Serialize)]
struct UnifiedRecord {
    event: DiagnosticEvent,
    /// Kept for existing JSONL clients; normal records use `origins`.
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<SourceCoordinate>,
    immutable_sha256: Option<Digest32>,
    origins: Vec<DiagnosticOrigin>,
}

#[derive(Serialize)]
struct NormalCoverage {
    nodes_examined: usize,
    entries_examined: usize,
    partitions_examined: usize,
    discovery_more: bool,
    read_complete: bool,
    issues: Vec<String>,
    read_issues: Vec<CoverageIssue>,
}

#[derive(Serialize)]
struct UnifiedCoverage {
    #[serde(flatten)]
    jsonl: InspectionCoverage,
    normal: NormalCoverage,
    coverage_complete: bool,
    /// `recent.limit` is applied to this page; compare all pages before
    /// inferring a global newest failure when continuation is present.
    page_local_results: bool,
    deduplicated_records: usize,
    /// A page is only one bounded slice. Pass this value as `--cursor` to
    /// continue; an empty page with a cursor is not proof of no failure.
    next_cursor: Option<String>,
}

#[derive(Serialize)]
struct UnifiedReport {
    events: Vec<UnifiedRecord>,
    coverage: UnifiedCoverage,
    issues: Vec<InspectionIssue>,
}

#[derive(Serialize)]
struct CoalescedFailure {
    operation: String,
    resource: String,
    phase: DiagnosticPhase,
    primary_code: Option<String>,
    extended_code: Option<i32>,
    failure_discriminator: String,
    first_recorded_at: String,
    last_recorded_at: String,
    count: usize,
    diagnostic_ids: Vec<DiagnosticId>,
    sources: Vec<SourceCoordinate>,
    origins: Vec<DiagnosticOrigin>,
}

#[derive(Serialize)]
struct CoalescedReport<'a> {
    failures: Vec<CoalescedFailure>,
    coverage: &'a UnifiedCoverage,
    issues: &'a [InspectionIssue],
}

#[derive(Serialize)]
struct RecentOutput<'a> {
    command: &'static str,
    limit: usize,
    recent: &'a UnifiedReport,
    coalesced: CoalescedReport<'a>,
}

#[derive(Serialize)]
struct TraceOutput<'a> {
    command: &'static str,
    diagnostic_id: &'a str,
    trace: &'a UnifiedReport,
}

#[derive(Serialize)]
struct MetricsOutput<'a> {
    command: &'static str,
    minutes: u64,
    metrics: &'a MetricQueryReport,
}

#[derive(Serialize)]
struct EventTraceOutput<'a> {
    command: &'static str,
    trace_id: &'a str,
    trace: &'a TraceQueryReport,
}

pub(crate) fn run_recent(limit: usize, json: bool, cursor: Option<&str>) -> Result<i32, String> {
    let report = search(None, true, cursor)?;
    let mut report = report;
    if report.events.len() > limit {
        report.events.drain(..report.events.len() - limit);
    }
    let output = RecentOutput {
        command: "diagnostics recent",
        limit,
        coalesced: CoalescedReport {
            failures: coalesce(&report.events),
            coverage: &report.coverage,
            issues: &report.issues,
        },
        recent: &report,
    };
    render_output(&output, json, |output| {
        println!("diagnostics recent limit={}", output.limit);
        if !output.recent.coverage.coverage_complete {
            println!("coverage: partial; any empty result applies only to examined sources");
        }
        render_human_section("recent failures", output.recent)?;
        render_human_section("coalesced failures", &output.coalesced)
    })?;
    Ok(0)
}

pub(crate) fn run_trace(
    diagnostic_id: &str,
    json: bool,
    cursor: Option<&str>,
) -> Result<i32, String> {
    let parsed = diagnostic_id
        .parse::<DiagnosticId>()
        .map_err(|error| format!("invalid diagnostic ID {diagnostic_id:?}: {error}"))?;
    let report = search(Some(&parsed), false, cursor)?;
    let output = TraceOutput {
        command: "diagnostics trace",
        diagnostic_id,
        trace: &report,
    };
    render_output(&output, json, |output| {
        println!("diagnostics trace diagnostic_id={}", output.diagnostic_id);
        if !output.trace.coverage.coverage_complete {
            println!("coverage: partial; any empty result applies only to examined sources");
        }
        render_human_section("trace", output.trace)
    })?;
    Ok(0)
}

fn search(
    diagnostic_id: Option<&DiagnosticId>,
    failures_only: bool,
    cursor: Option<&str>,
) -> Result<UnifiedReport, String> {
    search_with_bounds(
        &default_event_store_root()?,
        &default_recorder_root()?,
        diagnostic_id,
        failures_only,
        cursor,
        READ_RECORDS,
        READ_BYTES,
    )
}

fn search_with_bounds(
    event_root: &Path,
    recorder_root: &Path,
    diagnostic_id: Option<&DiagnosticId>,
    failures_only: bool,
    cursor: Option<&str>,
    read_records: usize,
    read_bytes: usize,
) -> Result<UnifiedReport, String> {
    ReadLimits::new(1, read_records, read_bytes).map_err(str::to_string)?;
    let previous = cursor
        .map(serde_json::from_str::<DiagnosticSearchCursor>)
        .transpose()
        .map_err(|error| format!("invalid diagnostic search cursor: {error}"))?
        .unwrap_or_default();
    let query_key = diagnostic_id.map_or_else(|| "recent".to_string(), |id| format!("trace:{id}"));
    if cursor.is_some() && previous.query_key != query_key {
        return Err("diagnostic search cursor belongs to a different query".to_string());
    }
    let mut records = Vec::<UnifiedRecord>::new();
    let mut issues = Vec::<InspectionIssue>::new();
    let mut normal = NormalCoverage {
        nodes_examined: 0,
        entries_examined: 0,
        partitions_examined: 0,
        discovery_more: false,
        read_complete: true,
        issues: Vec::new(),
        read_issues: Vec::new(),
    };
    let mut next = DiagnosticSearchCursor {
        query_key,
        normal_done: previous.normal_done,
        jsonl_done: previous.jsonl_done,
        ..DiagnosticSearchCursor::default()
    };
    if !previous.normal_done {
        let discovery_start = previous.normal_page.as_ref().map_or_else(
            || previous.normal.clone().unwrap_or_default(),
            |page| page.discovery_start.clone(),
        );
        let discovery = discover_generation_read_targets_page(
            event_root,
            &discovery_start,
            DISCOVERY_NODES,
            DISCOVERY_GENERATIONS,
        )?;
        normal.nodes_examined = discovery.nodes_examined;
        normal.entries_examined = discovery.entries_examined;
        normal.discovery_more = discovery.more;
        normal.issues = discovery.issues;
        let digest = target_page_digest(&discovery.targets);
        if previous
            .normal_page
            .as_ref()
            .is_some_and(|page| page.targets_sha256 != digest)
        {
            return Err("normal discovery page changed; diagnostic cursor invalidated".to_string());
        }
        let trace_id = diagnostic_id
            .map(|id| uuid::Uuid::parse_str(&id.to_string()).map(TraceId::from))
            .transpose()
            .map_err(|error| error.to_string())?;
        let filter = EventFilter {
            family: Some(EventFamily::Diagnostic),
            kind: Some(
                EventKind::registered("diagnostic.observation")
                    .map_err(|error| error.to_string())?,
            ),
            trace_id,
            ..EventFilter::default()
        };
        let mut index = previous
            .normal_page
            .as_ref()
            .map_or(0, |page| page.target_index);
        if index >= discovery.targets.len() && previous.normal_page.is_some() {
            return Err("normal discovery page no longer contains cursor target".to_string());
        }
        let mut after = previous
            .normal_page
            .as_ref()
            .and_then(|page| page.after_local_sequence);
        let mut rows_left = read_records;
        let mut bytes_left = read_bytes;
        while index < discovery.targets.len() && rows_left > 0 && bytes_left > 0 {
            let byte_budget_before = bytes_left;
            let limits = ReadLimits::new(1, rows_left, bytes_left).map_err(str::to_string)?;
            let page =
                read_generation_diagnostic_page(&discovery.targets[index], &filter, &limits, after);
            let first_row_needs_fresh_budget = page.next_local_sequence.is_none()
                && byte_budget_before < read_bytes
                && page.read.issues.iter().any(|issue| {
                    issue.kind == oulipoly_state::event_store::CoverageIssueKind::PayloadByteLimitReached
                });
            normal.partitions_examined += page.read.partitions_examined;
            normal.read_complete &= page.read.coverage_complete;
            normal.read_issues.extend(page.read.issues);
            rows_left -= page.rows_examined;
            bytes_left -= page.read.payload_bytes_examined;
            for record in page.read.records {
                match diagnostic_event_from_envelope(&record.envelope) {
                    Ok(Some(event)) => records.push(UnifiedRecord {
                        event,
                        source: None,
                        immutable_sha256: Some(record.immutable_sha256),
                        origins: vec![DiagnosticOrigin::EventGeneration {
                            writer_instance_id: record.envelope.producer.writer_instance_id,
                            generation_id: record.generation_id,
                            local_sequence: record.local_sequence,
                        }],
                    }),
                    Ok(None) => {}
                    Err(error) => {
                        normal.read_complete = false;
                        normal
                            .issues
                            .push(format!("diagnostic envelope projection failed: {error}"));
                    }
                }
            }
            if let Some(sequence) = page.next_local_sequence {
                next.normal_page = Some(NormalPageCursor {
                    discovery_start: discovery_start.clone(),
                    targets_sha256: digest,
                    target_index: index,
                    after_local_sequence: Some(sequence),
                });
                break;
            }
            if first_row_needs_fresh_budget {
                next.normal_page = Some(NormalPageCursor {
                    discovery_start: discovery_start.clone(),
                    targets_sha256: digest,
                    target_index: index,
                    after_local_sequence: after,
                });
                break;
            }
            index += 1;
            after = None;
        }
        if next.normal_page.is_none() && index < discovery.targets.len() {
            next.normal_page = Some(NormalPageCursor {
                discovery_start,
                targets_sha256: digest,
                target_index: index,
                after_local_sequence: None,
            });
        }
        if next.normal_page.is_none() {
            next.normal = discovery.next_cursor;
        }
        next.normal_done = next.normal_page.is_none() && next.normal.is_none();
    }
    let mut jsonl_coverage = InspectionCoverage::default();
    let mut jsonl_complete = previous.jsonl_done;
    if !previous.jsonl_done {
        let page = FlightRecorderReader::new(recorder_root).inspect_page(previous.jsonl.as_ref());
        jsonl_complete = page.coverage_complete;
        next.jsonl = page.next_cursor;
        next.jsonl_done = next.jsonl.is_none();
        jsonl_coverage = page.report.coverage;
        issues.extend(page.report.issues);
        for record in page.report.events {
            records.push(UnifiedRecord {
                event: record.event,
                source: Some(record.source.clone()),
                immutable_sha256: None,
                origins: vec![DiagnosticOrigin::Jsonl {
                    file: record.source.file,
                    line: record.source.line,
                }],
            });
        }
    }
    records.sort_by(|left, right| {
        left.event
            .recorded_at
            .cmp(&right.event.recorded_at)
            .then_with(|| left.event.event_id.cmp(&right.event.event_id))
    });
    let mut by_id = HashMap::<String, Vec<usize>>::new();
    let mut deduplicated: Vec<UnifiedRecord> = Vec::new();
    let mut duplicate_count = 0;
    for mut record in records {
        let id = record.event.event_id.to_string();
        if let Some(indices) = by_id.get(&id) {
            let equal_index = indices.iter().copied().find(|&index| {
                let existing = &deduplicated[index];
                existing.event == record.event
                    && match (existing.immutable_sha256, record.immutable_sha256) {
                        (Some(left), Some(right)) => left == right,
                        _ => true,
                    }
            });
            if let Some(index) = equal_index {
                let existing = &mut deduplicated[index];
                duplicate_count += 1;
                if existing.source.is_none() {
                    existing.source = record.source.clone();
                }
                existing.origins.append(&mut record.origins);
                if existing.immutable_sha256.is_none() {
                    existing.immutable_sha256 = record.immutable_sha256;
                }
            } else {
                issues.push(InspectionIssue {
                    source: record.source.clone(),
                    kind: "identity_conflict".to_string(),
                    message: format!("event ID {id} has unequal diagnostic content"),
                });
                by_id
                    .get_mut(&id)
                    .expect("existing identity")
                    .push(deduplicated.len());
                deduplicated.push(record);
            }
        } else {
            by_id.insert(id, vec![deduplicated.len()]);
            deduplicated.push(record);
        }
    }
    deduplicated.retain(|record| {
        diagnostic_id.is_none_or(|id| &record.event.diagnostic_id == id)
            && (!failures_only
                || matches!(
                    record.event.phase,
                    DiagnosticPhase::Failed | DiagnosticPhase::Contention
                )
                || record.event.observation.sqlite_failure.is_some())
    });
    let continuation = (!next.normal_done || !next.jsonl_done)
        .then(|| serde_json::to_string(&next))
        .transpose()
        .map_err(|error| error.to_string())?;
    let coverage_complete = cursor.is_none()
        && continuation.is_none()
        && normal.read_complete
        && normal.issues.is_empty()
        && jsonl_complete
        && issues.is_empty();
    let page_local_results = cursor.is_some() || continuation.is_some();
    if !coverage_complete && deduplicated.is_empty() {
        issues.push(InspectionIssue {
            source: None,
            kind: "incomplete_search_no_match".to_string(),
            message:
                "no matching event found in examined sources; continue or target remaining sources"
                    .to_string(),
        });
    }
    Ok(UnifiedReport {
        events: deduplicated,
        coverage: UnifiedCoverage {
            jsonl: jsonl_coverage,
            normal,
            coverage_complete,
            page_local_results,
            deduplicated_records: duplicate_count,
            next_cursor: continuation,
        },
        issues,
    })
}

fn coalesce(records: &[UnifiedRecord]) -> Vec<CoalescedFailure> {
    let mut groups = Vec::<CoalescedFailure>::new();
    let mut by_key = HashMap::<
        (
            String,
            String,
            DiagnosticPhase,
            Option<String>,
            Option<i32>,
            String,
        ),
        usize,
    >::new();
    for record in records {
        let event = &record.event;
        let failure = event.observation.sqlite_failure.as_ref();
        let primary_code = failure.and_then(|value| value.primary_code.clone());
        let extended_code = failure.and_then(|value| value.extended_code);
        let discriminator = failure_discriminator(event);
        let key = (
            event.operation.clone(),
            event.resource.clone(),
            event.phase,
            primary_code.clone(),
            extended_code,
            discriminator.clone(),
        );
        let index = by_key.get(&key).copied();
        let group = if let Some(index) = index {
            &mut groups[index]
        } else {
            by_key.insert(key, groups.len());
            groups.push(CoalescedFailure {
                operation: event.operation.clone(),
                resource: event.resource.clone(),
                phase: event.phase,
                primary_code,
                extended_code,
                failure_discriminator: discriminator,
                first_recorded_at: event.recorded_at.clone(),
                last_recorded_at: event.recorded_at.clone(),
                count: 0,
                diagnostic_ids: Vec::new(),
                sources: Vec::new(),
                origins: Vec::new(),
            });
            groups.last_mut().expect("just pushed")
        };
        group.count += 1;
        group.last_recorded_at = event.recorded_at.clone();
        if !group.diagnostic_ids.contains(&event.diagnostic_id) {
            group.diagnostic_ids.push(event.diagnostic_id.clone());
        }
        if let Some(source) = &record.source {
            group.sources.push(source.clone());
        }
        group.origins.extend(record.origins.iter().cloned());
    }
    groups
}

pub(crate) fn run_metrics(minutes: u64, json: bool) -> Result<i32, String> {
    let query = MetricQuery::recent(minutes)?;
    let metrics = query_metrics(&default_event_store_root()?, &query)?;
    let output = MetricsOutput {
        command: "diagnostics metrics",
        minutes,
        metrics: &metrics,
    };
    render_output(&output, json, |output| {
        println!("diagnostics metrics minutes={}", output.minutes);
        render_human_section("metrics", output.metrics)
    })?;
    Ok(0)
}

pub(crate) fn run_event_trace(trace_id: &str, json: bool) -> Result<i32, String> {
    let parsed = uuid::Uuid::parse_str(trace_id)
        .map(TraceId::from)
        .map_err(|error| format!("invalid event trace ID {trace_id:?}: {error}"))?;
    let limits = ReadLimits::new(256, READ_RECORDS, READ_BYTES).map_err(str::to_string)?;
    let trace = query_trace(&default_event_store_root()?, parsed, &limits)?;
    let output = EventTraceOutput {
        command: "diagnostics event-trace",
        trace_id,
        trace: &trace,
    };
    render_output(&output, json, |output| {
        println!("diagnostics event-trace trace_id={}", output.trace_id);
        render_human_section("event trace", output.trace)
    })?;
    Ok(0)
}

fn render_output<T, F>(output: &T, json: bool, human: F) -> Result<(), String>
where
    T: Serialize,
    F: FnOnce(&T) -> Result<(), String>,
{
    if json {
        let rendered = serde_json::to_string_pretty(output)
            .map_err(|error| format!("Failed to serialize offline diagnostics JSON: {error}"))?;
        println!("{rendered}");
        return Ok(());
    }
    human(output)
}

fn render_human_section(label: &str, value: &impl Serialize) -> Result<(), String> {
    let rendered = serde_json::to_string_pretty(value)
        .map_err(|error| format!("Failed to render offline diagnostics {label}: {error}"))?;
    println!("{label}:");
    for line in rendered.lines() {
        println!("  {line}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_state::diagnostic_recorder::PhaseObservation;
    use oulipoly_state::event_store::{
        EventCorrelations, EventEnvelopeV1, EventId, EventWriterConfig, NativeProcessIdentity,
        NewEventV1, PayloadNormalizationPolicy, ProcessEventWriter, ProcessInstanceId,
        ProducerIdentity, SpanId,
    };

    const ID: &str = "11111111-1111-4111-8111-111111111111";

    fn add_generation(root: &Path, writer_byte: u8, phases: &[&str]) -> usize {
        let writer_id = WriterInstanceId::from_bytes([writer_byte; 16]);
        let process = ProcessInstanceId::from_bytes([21; 16]);
        let producer = ProducerIdentity {
            writer_instance_id: writer_id,
            process_instance_id: process,
            process_root_id: process,
            parent_process_instance_id: None,
            supervisor_authority_id: None,
            native_process: Some(NativeProcessIdentity {
                os_pid: 1234,
                os_boot_id_sha256: Digest32::from_bytes([8; 32]),
                os_pid_starttime_ticks: 100,
            }),
        };
        let writer =
            ProcessEventWriter::start(EventWriterConfig::native(root, producer.clone())).unwrap();
        let policy = PayloadNormalizationPolicy::registered(&[
            "operation",
            "resource",
            "lifecycle_phase",
            "phase",
            "elapsed_micros",
            "observation",
            "diagnostic_correlations",
            "database_identity",
        ])
        .unwrap();
        let trace_id = TraceId::from(uuid::Uuid::parse_str(ID).unwrap());
        let mut first_payload_bytes = 0;
        for (index, phase) in phases.iter().enumerate() {
            let event_byte = writer_byte * 16 + u8::try_from(index).unwrap();
            let micros = 1_700_000_000_000_000 + i64::from(event_byte);
            let event = EventEnvelopeV1::normalize(
                NewEventV1 {
                    event_id: EventId::from_bytes([event_byte; 16]),
                    family: EventFamily::Diagnostic,
                    kind: EventKind::registered("diagnostic.observation").unwrap(),
                    recorded_at_unix_micros: micros,
                    producer_sequence: i64::try_from(index).unwrap() + 1,
                    producer: producer.clone(),
                    correlations: EventCorrelations {
                        trace_id: Some(trace_id),
                        span_id: Some(SpanId::from_bytes([31; 16])),
                        ..EventCorrelations::default()
                    },
                    payload: serde_json::json!({
                        "operation": "fixture.bounded_search",
                        "resource": "state",
                        "lifecycle_phase": null,
                        "phase": phase,
                        "elapsed_micros": 1,
                        "observation": serde_json::to_value(PhaseObservation::started_unknown()).unwrap(),
                        "diagnostic_correlations": {},
                        "database_identity": null,
                    }),
                    legacy_provenance: None,
                    retry_of_generation_id: None,
                },
                &policy,
                micros,
            )
            .unwrap();
            if index == 0 {
                first_payload_bytes = usize::try_from(event.payload_bytes).unwrap();
            }
            writer.append(event).unwrap();
        }
        writer.shutdown().unwrap();
        first_payload_bytes
    }

    fn pages_with_bounds(
        root: &Path,
        id: Option<&DiagnosticId>,
        failures_only: bool,
        read_records: usize,
        read_bytes: usize,
    ) -> Vec<UnifiedReport> {
        std::fs::create_dir_all(root.join("empty-jsonl")).unwrap();
        let mut cursor = None;
        let mut reports = Vec::new();
        for _ in 0..8 {
            let report = search_with_bounds(
                root,
                &root.join("empty-jsonl"),
                id,
                failures_only,
                cursor.as_deref(),
                read_records,
                read_bytes,
            )
            .unwrap();
            cursor = report.coverage.next_cursor.clone();
            reports.push(report);
            if cursor.is_none() {
                break;
            }
        }
        assert!(cursor.is_none(), "bounded cursor failed to terminate");
        reports
    }

    fn pages(root: &Path, id: Option<&DiagnosticId>, failures_only: bool) -> Vec<UnifiedReport> {
        pages_with_bounds(root, id, failures_only, 2, 1024 * 1024)
    }

    #[test]
    fn bounded_rows_resume_inside_one_generation_for_recent_and_trace() {
        let root = tempfile::tempdir().unwrap();
        add_generation(
            root.path(),
            1,
            &["requested", "requested", "requested", "requested", "failed"],
        );
        let recent = pages(root.path(), None, true);
        assert!(recent.len() >= 3);
        assert!(recent[0].events.is_empty());
        assert!(
            recent[0]
                .coverage
                .normal
                .read_issues
                .iter()
                .any(|issue| issue.kind
                    == oulipoly_state::event_store::CoverageIssueKind::RecordLimitReached)
        );
        assert_eq!(
            recent
                .iter()
                .flat_map(|page| &page.events)
                .filter(|record| record.event.phase == DiagnosticPhase::Failed)
                .count(),
            1
        );
        let id = ID.parse::<DiagnosticId>().unwrap();
        assert!(
            search_with_bounds(
                root.path(),
                &root.path().join("empty-jsonl"),
                Some(&id),
                false,
                recent[0].coverage.next_cursor.as_deref(),
                2,
                1024 * 1024,
            )
            .is_err()
        );
        let trace = pages(root.path(), Some(&id), false);
        assert_eq!(trace.iter().map(|page| page.events.len()).sum::<usize>(), 5);
        assert!(
            trace
                .last()
                .unwrap()
                .events
                .iter()
                .any(|record| record.event.phase == DiagnosticPhase::Failed)
        );
    }

    #[test]
    fn bounded_rows_resume_at_later_generation_in_same_discovery_page() {
        let root = tempfile::tempdir().unwrap();
        add_generation(root.path(), 1, &["requested", "requested"]);
        add_generation(root.path(), 2, &["failed"]);
        let recent = pages(root.path(), None, true);
        assert_eq!(recent.len(), 2);
        assert!(recent[0].events.is_empty());
        assert_eq!(recent[1].events.len(), 1);
        let id = ID.parse::<DiagnosticId>().unwrap();
        let trace = pages(root.path(), Some(&id), false);
        assert_eq!(trace.len(), 2);
        assert_eq!(trace[0].events.len(), 2);
        assert_eq!(trace[1].events.len(), 1);
    }

    #[test]
    fn bounded_bytes_resume_inside_and_after_a_generation() {
        let root = tempfile::tempdir().unwrap();
        let payload_bytes = add_generation(root.path(), 1, &["requested", "requested", "failed"]);
        add_generation(root.path(), 2, &["failed"]);
        let recent = pages_with_bounds(root.path(), None, true, 10, payload_bytes + 1);
        assert!(recent.len() >= 3);
        assert!(
            recent[0]
                .coverage
                .normal
                .read_issues
                .iter()
                .any(|issue| issue.kind
                    == oulipoly_state::event_store::CoverageIssueKind::PayloadByteLimitReached)
        );
        assert_eq!(
            recent
                .iter()
                .flat_map(|page| &page.events)
                .filter(|record| record.event.phase == DiagnosticPhase::Failed)
                .count(),
            2
        );
    }
}
