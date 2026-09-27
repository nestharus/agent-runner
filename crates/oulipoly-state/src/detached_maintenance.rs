//! Bounded opportunity runner used only by the detached maintenance process.

const DEFAULT_DISCOVERY_NODES: usize = 128;
const DEFAULT_PARTITIONS_PER_RUN: usize = 4;
const DEFAULT_RETIREMENT_VALIDATION_ROWS: u64 = 256;
const DEFAULT_RETIREMENT_VALIDATION_BYTES: u64 = 8 * 1024 * 1024;
const DEFAULT_RETIREMENT_UNLINK_ENTRIES: usize = 4;

use crate::event_store::maintenance_discovery::{
    DiscoveryClass, DiscoveryCursor, DiscoveryEntry, DiscoveryPhase, archive_invalid_absent_leaf,
    move_generation_class, read_batch, record_closed_generation, register_legacy_generation,
};
use crate::event_store::{
    CatalogInventoryResult, EventRetirementRequest, EventRetirementSlice,
    EventRetirementWorkLimits, GenerationError, GenerationState, IndexRebuildAction, WriterLayout,
    archive_event_generation_discovery, audit_event_generation_indexes, digest_hex,
    execute_event_retirement_slice, id_hex, inspect_event_generation_catalog, parse_id_hex,
    read_generation_metadata, read_prepared_manifest,
};
use crate::maintenance::{
    AcquireMaintenanceJob, MaintenanceJobKey, MaintenanceJobKind, MaintenanceJobStore,
    MaintenanceRunState, MaintenanceWorkerIdentity, now_unix_micros,
};
use crate::retention::RetentionPolicy;
use crate::retention::{
    RetentionBatchCursor, RetentionBatchRequest, RetentionBatchStatus, RetentionFamily,
};
use crate::{StateDb, mailbox::MailboxDb};
use serde::{Deserialize, Serialize};
use std::ffi::{CStr, CString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use uuid::Uuid;

pub const EVENT_STORE_RELATIVE_PATH: &str = "diagnostics/event-store-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpportunityWorkLimits {
    pub max_discovery_nodes: usize,
    pub max_partitions_per_run: usize,
    pub retirement: EventRetirementWorkLimitsDto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventRetirementWorkLimitsDto {
    pub max_validation_rows: u64,
    pub max_validation_bytes: u64,
    pub max_unlink_entries: usize,
}

impl From<EventRetirementWorkLimitsDto> for EventRetirementWorkLimits {
    fn from(value: EventRetirementWorkLimitsDto) -> Self {
        Self {
            max_validation_rows: value.max_validation_rows,
            max_validation_bytes: value.max_validation_bytes,
            max_unlink_entries: value.max_unlink_entries,
        }
    }
}

impl Default for OpportunityWorkLimits {
    fn default() -> Self {
        Self {
            max_discovery_nodes: DEFAULT_DISCOVERY_NODES,
            max_partitions_per_run: DEFAULT_PARTITIONS_PER_RUN,
            retirement: EventRetirementWorkLimitsDto {
                max_validation_rows: DEFAULT_RETIREMENT_VALIDATION_ROWS,
                max_validation_bytes: DEFAULT_RETIREMENT_VALIDATION_BYTES,
                max_unlink_entries: DEFAULT_RETIREMENT_UNLINK_ENTRIES,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpportunityOutcome {
    Completed,
    DuplicateLive,
    AlreadyCompleted,
    Yielded,
    Cancelled,
    Preserved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoordinationRetentionOutcome {
    pub jobs_completed: usize,
    pub jobs_yielded: usize,
    pub duplicate_jobs: usize,
    pub gaps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetentionResume {
    as_of_unix_micros: i64,
    cursor: Option<RetentionBatchCursor>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EventOpportunityCursor {
    discovery: DiscoveryCursor,
    legacy: LegacyMigrationCursor,
    #[serde(default)]
    discovery_gap: bool,
    #[serde(default)]
    issues: Vec<String>,
}

const MAX_RETAINED_DISCOVERY_ISSUES: usize = 4;
const MAX_RETAINED_DISCOVERY_ISSUE_BYTES: usize = 160;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LegacyMigrationPhase {
    #[default]
    Generations,
    Staging,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyMigrationCursor {
    writer_cookie: i64,
    current_writer: Option<String>,
    child_cookie: i64,
    phase: LegacyMigrationPhase,
}

#[derive(Debug, Default)]
struct LegacyMigrationOutcome {
    more: bool,
    completed: bool,
    entries_examined: usize,
    issues: Vec<String>,
    root_error: bool,
}

pub fn run_event_store_opportunity(
    data_root: &Path,
    opportunity_epoch: i64,
    owner: MaintenanceWorkerIdentity,
    limits: OpportunityWorkLimits,
) -> Result<OpportunityOutcome, String> {
    if limits.max_discovery_nodes == 0
        || limits.max_partitions_per_run == 0
        || limits.retirement.max_unlink_entries == 0
    {
        return Err("detached maintenance opportunity limits must be non-zero".to_string());
    }
    let store = MaintenanceJobStore::open(data_root).map_err(|error| error.to_string())?;
    let scan_key = MaintenanceJobKey::new(MaintenanceJobKind::EventDiscovery, "event-store-v1")
        .map_err(|error| error.to_string())?;
    let mut scan = match acquire_resumable(&store, scan_key, opportunity_epoch, owner.clone())? {
        AcquireMaintenanceJob::DuplicateLive(_) => return Ok(OpportunityOutcome::DuplicateLive),
        AcquireMaintenanceJob::AlreadyTerminal(_) => {
            return Ok(OpportunityOutcome::AlreadyCompleted);
        }
        AcquireMaintenanceJob::Acquired(lease) => lease,
    };
    if scan.is_cancelled().map_err(|error| error.to_string())? {
        scan.finish(
            MaintenanceRunState::Cancelled,
            "cancelled",
            "opportunity cancellation observed",
            now_unix_micros(),
        )
        .map_err(|error| error.to_string())?;
        return Ok(OpportunityOutcome::Cancelled);
    }
    let event_root = data_root.join(EVENT_STORE_RELATIVE_PATH);
    if !event_root.exists() {
        scan.finish(
            MaintenanceRunState::Succeeded,
            "complete",
            "event store is absent",
            now_unix_micros(),
        )
        .map_err(|error| error.to_string())?;
        return Ok(OpportunityOutcome::Completed);
    }

    let mut cursor = scan
        .checkpoint()
        .cursor
        .as_deref()
        .map(serde_json::from_str::<EventOpportunityCursor>)
        .transpose()
        .map_err(|error| format!("invalid maintenance discovery cursor: {error}"))?
        .unwrap_or_default();
    let mut units_completed = scan.checkpoint().units_completed;
    // A separate bounded cursor admits pre-boundary flat writer/generation
    // layouts without sorting or materializing an unbounded directory.
    // There is deliberately no irreversible "legacy complete" marker: a
    // parent-version process may be live but have no writer namespace until a
    // later lazy recorder initialization. Each daily opportunity therefore
    // re-walks legacy state in detached bounded slices rather than suppressing
    // the only discovery route for a later legacy-only publication.
    let legacy = migrate_legacy_page(
        &event_root,
        &mut cursor.legacy,
        (limits.max_discovery_nodes / 4).max(1),
    );
    units_completed = units_completed.saturating_add(legacy.entries_examined as u64);
    for issue in legacy.issues {
        retain_discovery_issue(&mut cursor, issue);
    }
    let legacy_more = legacy.more && !legacy.root_error;
    if legacy.completed {
        cursor.legacy = LegacyMigrationCursor::default();
    }
    persist_discovery_progress(&mut scan, &cursor, "legacy_batch", units_completed)?;

    let effective_epoch = scan.checkpoint().opportunity_epoch;
    let mut remaining_nodes = limits.max_discovery_nodes;
    let mut partitions = 0usize;
    let mut discovery_more = false;
    while remaining_nodes > 0 && partitions < limits.max_partitions_per_run {
        let discovery = match read_batch(&event_root, &cursor.discovery, remaining_nodes, 1) {
            Ok(discovery) => discovery,
            Err(error) => {
                retain_discovery_issue(&mut cursor, format!("discovery root unavailable: {error}"));
                persist_discovery_progress(&mut scan, &cursor, "discovery_gap", units_completed)?;
                break;
            }
        };
        cursor.discovery = discovery.cursor;
        cursor.discovery_gap |= !discovery.issues.is_empty();
        for issue in discovery.issues {
            retain_discovery_issue(&mut cursor, issue);
        }
        remaining_nodes = remaining_nodes.saturating_sub(discovery.nodes_examined);
        units_completed = units_completed.saturating_add(discovery.nodes_examined as u64);
        discovery_more = discovery.more;
        if let Some(candidate) = discovery.entries.first() {
            partitions += 1;
            if let Err(error) = run_discovered_candidate(
                &store,
                &event_root,
                effective_epoch,
                owner.clone(),
                candidate,
                limits,
            ) {
                let reason = format!(
                    "candidate {}/{}: {error}",
                    id_hex(candidate.writer.as_bytes()),
                    id_hex(candidate.generation.as_bytes())
                );
                retain_discovery_issue(&mut cursor, reason.clone());
                if let Err(evidence_error) =
                    record_candidate_gap(&store, effective_epoch, owner.clone(), candidate, &reason)
                {
                    retain_discovery_issue(
                        &mut cursor,
                        format!("candidate gap evidence failed: {evidence_error}"),
                    );
                }
            }
        }
        persist_discovery_progress(&mut scan, &cursor, "discovery_candidate", units_completed)?;
        if !discovery_more || discovery.nodes_examined == 0 {
            break;
        }
    }
    if discovery_more || legacy_more {
        scan.yield_run(
            "yielded",
            "bounded discovery/migration budget reached with advancing cursor",
            now_unix_micros(),
        )
        .map_err(|error| error.to_string())?;
        Ok(OpportunityOutcome::Yielded)
    } else if cursor.discovery_gap || !cursor.issues.is_empty() {
        let detail = serde_json::to_string(&cursor.issues).map_err(|error| error.to_string())?;
        scan.finish(
            MaintenanceRunState::Preserved,
            "discovery_incomplete",
            detail,
            now_unix_micros(),
        )
        .map_err(|error| error.to_string())?;
        Ok(OpportunityOutcome::Preserved)
    } else {
        scan.finish(
            MaintenanceRunState::Succeeded,
            "complete",
            "bounded sharded discovery and legacy migration pass exhausted",
            now_unix_micros(),
        )
        .map_err(|error| error.to_string())?;
        Ok(OpportunityOutcome::Completed)
    }
}

fn persist_discovery_progress(
    scan: &mut crate::maintenance::MaintenanceJobLease,
    cursor: &EventOpportunityCursor,
    phase: &str,
    units_completed: u64,
) -> Result<(), String> {
    let next_cursor = serde_json::to_string(cursor).map_err(|error| error.to_string())?;
    scan.record_progress(
        phase,
        Some(next_cursor),
        units_completed,
        scan.checkpoint().bytes_completed,
        now_unix_micros(),
    )
    .map_err(|error| error.to_string())
}

fn retain_discovery_issue(cursor: &mut EventOpportunityCursor, issue: String) {
    cursor.discovery_gap = true;
    let issue = truncate_utf8(&issue, MAX_RETAINED_DISCOVERY_ISSUE_BYTES);
    if cursor.issues.last() == Some(&issue) {
        return;
    }
    if cursor.issues.len() == MAX_RETAINED_DISCOVERY_ISSUES {
        cursor.issues.remove(0);
    }
    cursor.issues.push(issue);
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

fn record_candidate_gap(
    store: &MaintenanceJobStore,
    opportunity_epoch: i64,
    owner: MaintenanceWorkerIdentity,
    candidate: &DiscoveryEntry,
    reason: &str,
) -> Result<(), String> {
    let key = MaintenanceJobKey::new(
        MaintenanceJobKind::EventDiscovery,
        format!(
            "event-store-v1/{}/{}",
            id_hex(candidate.writer.as_bytes()),
            id_hex(candidate.generation.as_bytes())
        ),
    )
    .map_err(|error| error.to_string())?;
    match acquire_resumable(store, key, opportunity_epoch, owner)? {
        AcquireMaintenanceJob::DuplicateLive(_) | AcquireMaintenanceJob::AlreadyTerminal(_) => {
            Ok(())
        }
        AcquireMaintenanceJob::Acquired(mut job) => job
            .finish(
                MaintenanceRunState::Preserved,
                "candidate_gap",
                truncate_utf8(reason, 1_024),
                now_unix_micros(),
            )
            .map_err(|error| error.to_string()),
    }
}

fn run_discovered_candidate(
    store: &MaintenanceJobStore,
    event_root: &Path,
    opportunity_epoch: i64,
    owner: MaintenanceWorkerIdentity,
    candidate: &DiscoveryEntry,
    limits: OpportunityWorkLimits,
) -> Result<(), String> {
    let partition = format!(
        "{}/{}",
        id_hex(candidate.writer.as_bytes()),
        id_hex(candidate.generation.as_bytes())
    );
    if candidate.class == DiscoveryClass::Pending
        && candidate
            .record
            .as_ref()
            .is_some_and(|record| record.phase == DiscoveryPhase::OrphanPending)
    {
        return run_orphan_classification(
            store,
            event_root,
            opportunity_epoch,
            owner,
            candidate,
            &partition,
            limits.retirement.max_unlink_entries,
        );
    }
    if candidate.class == DiscoveryClass::Pending {
        return run_event_retirement_candidate(
            store,
            event_root,
            opportunity_epoch,
            owner,
            candidate,
            &partition,
            limits,
        );
    }
    let layout = match WriterLayout::open_existing(event_root, *candidate.writer.as_bytes()) {
        Ok(layout) => layout,
        Err(_) => {
            return run_orphan_classification(
                store,
                event_root,
                opportunity_epoch,
                owner,
                candidate,
                &partition,
                limits.retirement.max_unlink_entries,
            );
        }
    };
    if candidate_is_selected(&layout, candidate.generation)? {
        return run_rotation_follow_up(
            store,
            event_root,
            opportunity_epoch,
            owner,
            candidate,
            &partition,
        );
    }
    let generation = layout.generation_dir(candidate.generation);
    let state = read_generation_state(&generation).ok();
    if state == Some(GenerationState::Closed) {
        run_event_retirement_candidate(
            store,
            event_root,
            opportunity_epoch,
            owner,
            candidate,
            &partition,
            limits,
        )
    } else {
        run_orphan_classification(
            store,
            event_root,
            opportunity_epoch,
            owner,
            candidate,
            &partition,
            limits.retirement.max_unlink_entries,
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn run_event_retirement_candidate(
    store: &MaintenanceJobStore,
    event_root: &Path,
    opportunity_epoch: i64,
    owner: MaintenanceWorkerIdentity,
    candidate: &DiscoveryEntry,
    partition: &str,
    limits: OpportunityWorkLimits,
) -> Result<(), String> {
    let key = MaintenanceJobKey::new(MaintenanceJobKind::EventRetirement, partition)
        .map_err(|error| error.to_string())?;
    let mut job = match acquire_resumable(store, key, opportunity_epoch, owner.clone())? {
        AcquireMaintenanceJob::DuplicateLive(_) => return Ok(()),
        AcquireMaintenanceJob::AlreadyTerminal(checkpoint)
            if checkpoint.state == MaintenanceRunState::Succeeded
                && checkpoint.phase == "retired" =>
        {
            return archive_event_generation_discovery(
                event_root,
                candidate.writer,
                candidate.generation,
                DiscoveryPhase::Retired,
            )
            .map_err(|error| format!("retry retired discovery archive: {error}"));
        }
        AcquireMaintenanceJob::AlreadyTerminal(_) => return Ok(()),
        AcquireMaintenanceJob::Acquired(lease) => lease,
    };
    let request = EventRetirementRequest {
        writer_instance_id: candidate.writer,
        generation_id: candidate.generation,
        policy: RetentionPolicy::default(),
        as_of_unix_micros: now_unix_micros(),
        retirement_epoch: job.checkpoint().opportunity_epoch,
    };
    match execute_event_retirement_slice(event_root, &request, limits.retirement.into(), &mut job) {
        Ok(EventRetirementSlice::Retired { .. }) => {
            job.finish(
                MaintenanceRunState::Succeeded,
                "retired",
                "receipt durable and pending trash unlinked",
                now_unix_micros(),
            )
            .map_err(|error| error.to_string())?;
            archive_event_generation_discovery(
                event_root,
                candidate.writer,
                candidate.generation,
                DiscoveryPhase::Retired,
            )
            .map_err(|error| error.to_string())
        }
        Ok(EventRetirementSlice::MoreWork { phase }) => {
            job.yield_run(
                phase,
                "bounded slice completed; exact cursor is resumable",
                now_unix_micros(),
            )
            .map_err(|error| error.to_string())?;
            drop(job);
            if candidate.class == DiscoveryClass::Active && phase == "payload_validation" {
                run_index_maintenance(
                    store,
                    event_root,
                    opportunity_epoch,
                    owner,
                    candidate,
                    partition,
                    u64::MAX,
                )?;
            }
            Ok(())
        }
        Ok(EventRetirementSlice::Preserved { reason }) => {
            job.finish(
                MaintenanceRunState::Preserved,
                "preserved",
                reason,
                now_unix_micros(),
            )
            .map_err(|error| error.to_string())?;
            drop(job);
            if candidate.class == DiscoveryClass::Active {
                run_index_maintenance(
                    store,
                    event_root,
                    opportunity_epoch,
                    owner.clone(),
                    candidate,
                    partition,
                    u64::MAX,
                )?;
                run_catalog_inspection(
                    store,
                    event_root,
                    opportunity_epoch,
                    owner,
                    candidate,
                    partition,
                    limits.retirement.max_validation_rows,
                    u64::MAX,
                )?;
            }
            Ok(())
        }
        Ok(EventRetirementSlice::Cancelled) => job
            .finish(
                MaintenanceRunState::Cancelled,
                "cancelled",
                "partition cancellation observed",
                now_unix_micros(),
            )
            .map_err(|error| error.to_string()),
        Err(error) => settle_event_retirement_error(&mut job, error.to_string()),
    }
}

pub(crate) fn settle_event_retirement_error(
    job: &mut crate::maintenance::MaintenanceJobLease,
    error: String,
) -> Result<(), String> {
    let phase = job.checkpoint().cursor.as_deref();
    if phase.is_some_and(|cursor| {
        cursor.starts_with("retire:destructive_ready:") || cursor.starts_with("retire:pending:")
    }) {
        job.yield_run("receipt_recovery_required", error, now_unix_micros())
            .map_err(|write_error| write_error.to_string())
    } else {
        job.finish(
            MaintenanceRunState::Failed,
            "failed",
            error,
            now_unix_micros(),
        )
        .map_err(|write_error| write_error.to_string())
    }
}

fn candidate_is_selected(
    layout: &WriterLayout,
    generation: crate::event_store::GenerationId,
) -> Result<bool, String> {
    Ok(layout
        .read_head(|_, _| Ok(()))
        .map_err(|error| error.to_string())?
        .is_some_and(|head| head.record.generation_id == generation))
}

fn read_generation_state(generation: &Path) -> Result<GenerationState, String> {
    let connection = rusqlite::Connection::open_with_flags(
        generation.join(crate::event_store::DATABASE_FILE_NAME),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
            | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|error| error.to_string())?;
    read_generation_metadata(&connection)
        .map(|metadata| metadata.state)
        .map_err(|error| error.to_string())
}

fn run_orphan_classification(
    store: &MaintenanceJobStore,
    event_root: &Path,
    opportunity_epoch: i64,
    owner: MaintenanceWorkerIdentity,
    candidate: &DiscoveryEntry,
    partition: &str,
    max_unlink_entries: usize,
) -> Result<(), String> {
    let key = MaintenanceJobKey::new(MaintenanceJobKind::OrphanClassification, partition)
        .map_err(|error| error.to_string())?;
    let mut job = match acquire_resumable(store, key, opportunity_epoch, owner)? {
        AcquireMaintenanceJob::DuplicateLive(_) | AcquireMaintenanceJob::AlreadyTerminal(_) => {
            return Ok(());
        }
        AcquireMaintenanceJob::Acquired(job) => job,
    };
    let layout = match WriterLayout::open_existing(event_root, *candidate.writer.as_bytes()) {
        Ok(layout) => layout,
        Err(error) => {
            return job
                .finish(
                    MaintenanceRunState::Preserved,
                    "writer_layout_missing",
                    error.to_string(),
                    now_unix_micros(),
                )
                .map_err(|write_error| write_error.to_string());
        }
    };
    let _generation_lease = match layout.acquire_generation_maintenance_lease(candidate.generation)
    {
        Ok(lease) => lease,
        Err(GenerationError::GenerationAlreadyOwned(_)) => {
            return job
                .yield_run(
                    "producer_or_reader_live",
                    "exact shared generation lease is still held",
                    now_unix_micros(),
                )
                .map_err(|error| error.to_string());
        }
        Err(error) => {
            return job
                .finish(
                    MaintenanceRunState::Preserved,
                    "lease_refused",
                    error.to_string(),
                    now_unix_micros(),
                )
                .map_err(|write_error| write_error.to_string());
        }
    };
    if candidate_is_selected(&layout, candidate.generation)? {
        return job
            .finish(
                MaintenanceRunState::Preserved,
                "selected_head_preserved",
                "orphan classification never changes a selected head",
                now_unix_micros(),
            )
            .map_err(|error| error.to_string());
    }

    let writer_hex = id_hex(candidate.writer.as_bytes());
    let generation_hex = id_hex(candidate.generation.as_bytes());
    let final_path = layout.generation_dir(candidate.generation);
    let pending = event_root
        .join("retirement/orphan-trash")
        .join(&writer_hex)
        .join(format!("{generation_hex}.pending"));
    let staging = candidate
        .record
        .as_ref()
        .and_then(|record| record.staging_name.as_deref())
        .map(|staging_name| layout.writer_dir().join("staging").join(staging_name));
    let pending_exists = pending.is_dir();
    let final_exists = final_path.is_dir();
    let staging_exists = staging.as_ref().is_some_and(|path| path.is_dir());
    if usize::from(pending_exists) + usize::from(final_exists) + usize::from(staging_exists) > 1 {
        return job
            .finish(
                MaintenanceRunState::Preserved,
                "orphan_source_conflict",
                "multiple staging/final/pending paths exist for one exact generation",
                now_unix_micros(),
            )
            .map_err(|error| error.to_string());
    }

    let existing_disposition = read_orphan_disposition(event_root, candidate)?;
    if !pending_exists && !final_exists && !staging_exists {
        if candidate.class == DiscoveryClass::Pending && existing_disposition.is_some() {
            job.finish(
                MaintenanceRunState::Succeeded,
                "orphan_discharged",
                "pending orphan was already fully unlinked before restart",
                now_unix_micros(),
            )
            .map_err(|error| error.to_string())?;
            move_generation_class(
                event_root,
                candidate.writer,
                candidate.generation,
                DiscoveryClass::Pending,
                DiscoveryClass::Archive,
                DiscoveryPhase::Retired,
            )?;
            return Ok(());
        }
        return archive_absent_orphan(event_root, candidate, &mut job);
    }

    let (source, source_kind) = if pending_exists {
        let disposition = existing_disposition.as_ref().ok_or_else(|| {
            "pending orphan has no create-once classification disposition".to_string()
        })?;
        (pending.clone(), disposition.source_kind)
    } else if final_exists {
        (final_path, OrphanSourceKind::FinalGeneration)
    } else {
        (
            staging.expect("staging source exists"),
            OrphanSourceKind::NamedStaging,
        )
    };

    let proof = match existing_disposition {
        Some(disposition) => match disposition.into_proof(candidate, source_kind) {
            Ok(proof) => {
                // Before the move, the source is still complete enough to
                // independently revalidate. Once it is pending, individual
                // known files may already be absent and the stable disposition
                // is deliberately the recovery authority.
                if source != pending {
                    match classify_orphan_artifact(&source, candidate, source_kind) {
                        Ok(current) if orphan_proof_matches(&proof, &current) => {}
                        Ok(_) => {
                            return job
                                .finish(
                                    MaintenanceRunState::Preserved,
                                    "orphan_disposition_conflict",
                                    "orphan source no longer matches its durable disposition",
                                    now_unix_micros(),
                                )
                                .map_err(|error| error.to_string());
                        }
                        Err(reason) => {
                            return job
                                .finish(
                                    MaintenanceRunState::Preserved,
                                    "orphan_disposition_conflict",
                                    reason,
                                    now_unix_micros(),
                                )
                                .map_err(|error| error.to_string());
                        }
                    }
                }
                proof
            }
            Err(reason) => {
                return job
                    .finish(
                        MaintenanceRunState::Preserved,
                        "orphan_disposition_conflict",
                        reason,
                        now_unix_micros(),
                    )
                    .map_err(|error| error.to_string());
            }
        },
        None => match classify_orphan_artifact(&source, candidate, source_kind) {
            Ok(proof) => proof,
            Err(reason) => {
                return job
                    .finish(
                        MaintenanceRunState::Preserved,
                        "orphan_not_safely_empty",
                        reason,
                        now_unix_micros(),
                    )
                    .map_err(|error| error.to_string());
            }
        },
    };
    match producer_is_dead(proof.producer.as_ref()) {
        Some(false) => {
            return job
                .yield_run(
                    "producer_live",
                    "prepared or staging owner remains exactly live",
                    now_unix_micros(),
                )
                .map_err(|error| error.to_string());
        }
        None => {
            return job
                .finish(
                    MaintenanceRunState::Preserved,
                    "producer_liveness_unknown",
                    "cannot prove prepared or staging owner dead",
                    now_unix_micros(),
                )
                .map_err(|error| error.to_string());
        }
        Some(true) => {}
    }
    publish_orphan_disposition(event_root, candidate, &proof)?;
    let operation = digest_hex(proof.operation_sha256.as_bytes());
    job.record_progress(
        "orphan_disposition_durable",
        Some(format!("orphan:disposition:{operation}")),
        job.checkpoint().units_completed,
        job.checkpoint().bytes_completed,
        now_unix_micros(),
    )
    .map_err(|error| error.to_string())?;
    #[cfg(test)]
    run_orphan_test_hook("disposition", &source);
    if source != pending {
        job.record_progress(
            "orphan_destructive_ready",
            Some(format!("orphan:destructive_ready:{operation}")),
            job.checkpoint().units_completed,
            job.checkpoint().bytes_completed,
            now_unix_micros(),
        )
        .map_err(|error| error.to_string())?;
        if candidate.class == DiscoveryClass::Active {
            move_generation_class(
                event_root,
                candidate.writer,
                candidate.generation,
                DiscoveryClass::Active,
                DiscoveryClass::Pending,
                DiscoveryPhase::OrphanPending,
            )?;
        }
        let transition = job.lock_transition().map_err(|error| error.to_string())?;
        if job.is_cancelled().map_err(|error| error.to_string())? {
            if candidate.class == DiscoveryClass::Active {
                move_generation_class(
                    event_root,
                    candidate.writer,
                    candidate.generation,
                    DiscoveryClass::Pending,
                    DiscoveryClass::Active,
                    candidate
                        .record
                        .as_ref()
                        .map_or(DiscoveryPhase::Staging, |record| record.phase),
                )?;
            }
            return job
                .finish(
                    MaintenanceRunState::Cancelled,
                    "cancelled",
                    "orphan cancellation won the transition gate",
                    now_unix_micros(),
                )
                .map_err(|error| error.to_string());
        }
        prepare_orphan_pending_parent(event_root, &writer_hex)?;
        fs::rename(&source, &pending).map_err(|error| error.to_string())?;
        sync_directory(source.parent().expect("orphan source has parent"))?;
        sync_directory(pending.parent().expect("orphan pending has parent"))?;
        drop(transition);
        job.record_progress(
            "orphan_moved",
            Some(format!("orphan:pending:{operation}")),
            job.checkpoint().units_completed.saturating_add(1),
            job.checkpoint().bytes_completed,
            now_unix_micros(),
        )
        .map_err(|error| error.to_string())?;
    }

    if job.is_cancelled().map_err(|error| error.to_string())? {
        return job
            .finish(
                MaintenanceRunState::Cancelled,
                "cancelled_after_move",
                "orphan disposition is durable; pending unlink remains resumable",
                now_unix_micros(),
            )
            .map_err(|error| error.to_string());
    }
    match unlink_orphan_slice(&pending, &proof, max_unlink_entries, &mut job)? {
        OrphanUnlinkSlice::More => {
            return job
                .yield_run(
                    "orphan_unlink",
                    "bounded orphan unlink slice completed with known files remaining",
                    now_unix_micros(),
                )
                .map_err(|error| error.to_string());
        }
        OrphanUnlinkSlice::Cancelled => {
            return job
                .finish(
                    MaintenanceRunState::Cancelled,
                    "cancelled_after_unlink",
                    "orphan disposition and partial unlink are durable and resumable",
                    now_unix_micros(),
                )
                .map_err(|error| error.to_string());
        }
        OrphanUnlinkSlice::Complete => {}
    }
    job.finish(
        MaintenanceRunState::Succeeded,
        "orphan_discharged",
        "proven-dead unselected empty prepared or incomplete staging artifact was dispositioned and unlinked",
        now_unix_micros(),
    )
    .map_err(|error| error.to_string())?;
    move_generation_class(
        event_root,
        candidate.writer,
        candidate.generation,
        DiscoveryClass::Pending,
        DiscoveryClass::Archive,
        DiscoveryPhase::Retired,
    )
}

struct OrphanProof {
    operation_sha256: crate::event_store::Digest32,
    discovery_operation_sha256: crate::event_store::Digest32,
    prepared_manifest_sha256: Option<crate::event_store::Digest32>,
    producer: Option<crate::event_store::NativeProcessIdentity>,
    source_kind: OrphanSourceKind,
}

fn orphan_proof_matches(left: &OrphanProof, right: &OrphanProof) -> bool {
    left.operation_sha256 == right.operation_sha256
        && left.discovery_operation_sha256 == right.discovery_operation_sha256
        && left.prepared_manifest_sha256 == right.prepared_manifest_sha256
        && left.producer == right.producer
        && left.source_kind == right.source_kind
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OrphanSourceKind {
    NamedStaging,
    FinalGeneration,
}

fn classify_orphan_artifact(
    source: &Path,
    candidate: &DiscoveryEntry,
    source_kind: OrphanSourceKind,
) -> Result<OrphanProof, String> {
    let record = candidate
        .record
        .as_ref()
        .ok_or_else(|| "orphan discovery record is invalid or absent".to_string())?;
    let discovery_operation_sha256 = record.operation_sha256()?;
    let operation_sha256 = orphan_operation_sha256(discovery_operation_sha256, source_kind);
    ensure_orphan_entries_known(source)?;
    let manifest_path = source.join(crate::event_store::PREPARED_MANIFEST_FILE_NAME);
    if manifest_path.is_file() {
        let manifest =
            validate_empty_prepared_orphan(source, candidate.writer, candidate.generation)?;
        let manifest_sha256 = manifest.sha256().map_err(|error| error.to_string())?;
        if record
            .prepared_manifest_sha256
            .is_some_and(|expected| manifest_sha256 != expected)
        {
            return Err("orphan manifest does not match discovery intent".to_string());
        }
        return Ok(OrphanProof {
            operation_sha256,
            discovery_operation_sha256,
            prepared_manifest_sha256: Some(manifest_sha256),
            producer: manifest.producer.native_process,
            source_kind,
        });
    }
    // The prepared manifest is create-once before the final-directory rename.
    // Its absence is dischargeable only for the unique staging path named by
    // the pre-effect intent, never for an unheaded final generation.
    if source_kind != OrphanSourceKind::NamedStaging {
        return Err("manifest-less final generation is preserved as unprovable".to_string());
    }
    Ok(OrphanProof {
        operation_sha256,
        discovery_operation_sha256,
        prepared_manifest_sha256: record.prepared_manifest_sha256,
        producer: record.producer_native_process.clone(),
        source_kind,
    })
}

fn orphan_operation_sha256(
    discovery: crate::event_store::Digest32,
    source_kind: OrphanSourceKind,
) -> crate::event_store::Digest32 {
    let mut bytes = Vec::with_capacity(33);
    bytes.extend_from_slice(discovery.as_bytes());
    bytes.push(match source_kind {
        OrphanSourceKind::NamedStaging => 1,
        OrphanSourceKind::FinalGeneration => 2,
    });
    crate::event_store::Digest32::sha256(&bytes)
}

fn ensure_orphan_entries_known(source: &Path) -> Result<(), String> {
    let allowed = [
        crate::event_store::DATABASE_FILE_NAME.to_string(),
        format!("{}-wal", crate::event_store::DATABASE_FILE_NAME),
        format!("{}-shm", crate::event_store::DATABASE_FILE_NAME),
        crate::event_store::PREPARED_MANIFEST_FILE_NAME.to_string(),
    ];
    let mut count = 0usize;
    for entry in fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        count += 1;
        let name = entry.file_name().to_string_lossy().into_owned();
        if count > allowed.len()
            || !allowed.contains(&name)
            || !entry.file_type().is_ok_and(|kind| kind.is_file())
        {
            return Err("orphan directory contains an unexpected entry".to_string());
        }
    }
    Ok(())
}

fn archive_absent_orphan(
    event_root: &Path,
    candidate: &DiscoveryEntry,
    job: &mut crate::maintenance::MaintenanceJobLease,
) -> Result<(), String> {
    job.finish(
        MaintenanceRunState::Succeeded,
        "absent_intent_discharged",
        "publication intent has no staging, final, or pending artifact",
        now_unix_micros(),
    )
    .map_err(|error| error.to_string())?;
    if candidate.record.is_some() {
        move_generation_class(
            event_root,
            candidate.writer,
            candidate.generation,
            candidate.class,
            DiscoveryClass::Archive,
            DiscoveryPhase::Retired,
        )?;
    } else {
        archive_invalid_absent_leaf(
            event_root,
            candidate.class,
            candidate.writer,
            candidate.generation,
        )?;
    }
    Ok(())
}

fn validate_empty_prepared_orphan(
    source: &Path,
    writer: crate::event_store::WriterInstanceId,
    generation: crate::event_store::GenerationId,
) -> Result<crate::event_store::PreparedManifest, String> {
    let manifest = read_prepared_manifest(source).map_err(|error| error.to_string())?;
    if manifest.writer_instance_id != writer || manifest.generation_id != generation {
        return Err("orphan manifest identity mismatch".to_string());
    }
    let connection = rusqlite::Connection::open_with_flags(
        source.join(crate::event_store::DATABASE_FILE_NAME),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
            | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|error| error.to_string())?;
    let metadata = read_generation_metadata(&connection).map_err(|error| error.to_string())?;
    let rows: i64 = connection
        .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if metadata.writer_instance_id != writer
        || metadata.generation_id != generation
        || metadata.state != GenerationState::Prepared
        || rows != 0
    {
        return Err("orphan is not an exact empty prepared generation".to_string());
    }
    Ok(manifest)
}

fn producer_is_dead(expected: Option<&crate::event_store::NativeProcessIdentity>) -> Option<bool> {
    let expected = expected?;
    match crate::pid_identity::observe_live_process_identity(expected.os_pid) {
        crate::pid_identity::ProcessIdentityObservation::ExactLive(live) => Some(
            live.os_pid != expected.os_pid
                || live.os_pid_starttime_ticks != expected.os_pid_starttime_ticks
                || crate::event_store::Digest32::sha256(live.os_boot_id.as_bytes())
                    != expected.os_boot_id_sha256,
        ),
        crate::pid_identity::ProcessIdentityObservation::Dead => Some(true),
        crate::pid_identity::ProcessIdentityObservation::Unsupported
        | crate::pid_identity::ProcessIdentityObservation::ReadError(_) => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OrphanDisposition {
    format_version: u32,
    writer_instance_id: crate::event_store::WriterInstanceId,
    generation_id: crate::event_store::GenerationId,
    operation_sha256: crate::event_store::Digest32,
    discovery_operation_sha256: crate::event_store::Digest32,
    prepared_manifest_sha256: Option<crate::event_store::Digest32>,
    producer: Option<crate::event_store::NativeProcessIdentity>,
    source_kind: OrphanSourceKind,
    reason: String,
}

impl OrphanDisposition {
    fn into_proof(
        self,
        candidate: &DiscoveryEntry,
        source_kind: OrphanSourceKind,
    ) -> Result<OrphanProof, String> {
        let record = candidate
            .record
            .as_ref()
            .ok_or_else(|| "pending orphan discovery record is invalid or absent".to_string())?;
        let discovery_operation_sha256 = record.operation_sha256()?;
        let operation_sha256 = orphan_operation_sha256(discovery_operation_sha256, source_kind);
        if self.format_version != 2
            || self.writer_instance_id != candidate.writer
            || self.generation_id != candidate.generation
            || self.source_kind != source_kind
            || self.discovery_operation_sha256 != discovery_operation_sha256
            || self.operation_sha256 != operation_sha256
            || self.reason != "proven_dead_unselected_empty_publication_artifact"
        {
            return Err("orphan disposition does not match exact immutable operation".to_string());
        }
        Ok(OrphanProof {
            operation_sha256,
            discovery_operation_sha256,
            prepared_manifest_sha256: self.prepared_manifest_sha256,
            producer: self.producer,
            source_kind,
        })
    }
}

fn publish_orphan_disposition(
    event_root: &Path,
    candidate: &DiscoveryEntry,
    proof: &OrphanProof,
) -> Result<(), String> {
    let writer = id_hex(candidate.writer.as_bytes());
    let generation = id_hex(candidate.generation.as_bytes());
    let directory = event_root
        .join("retirement/orphan-dispositions")
        .join(writer);
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let disposition = OrphanDisposition {
        format_version: 2,
        writer_instance_id: candidate.writer,
        generation_id: candidate.generation,
        operation_sha256: proof.operation_sha256,
        discovery_operation_sha256: proof.discovery_operation_sha256,
        prepared_manifest_sha256: proof.prepared_manifest_sha256,
        producer: proof.producer.clone(),
        source_kind: proof.source_kind,
        reason: "proven_dead_unselected_empty_publication_artifact".to_string(),
    };
    let mut bytes = serde_json::to_vec(&disposition).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    create_once_file(&directory, &format!("{generation}.json"), &bytes)?;
    sync_directory(&directory)
}

fn read_orphan_disposition(
    event_root: &Path,
    candidate: &DiscoveryEntry,
) -> Result<Option<OrphanDisposition>, String> {
    let path = event_root
        .join("retirement/orphan-dispositions")
        .join(id_hex(candidate.writer.as_bytes()))
        .join(format!("{}.json", id_hex(candidate.generation.as_bytes())));
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let disposition: OrphanDisposition =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    let mut canonical = serde_json::to_vec(&disposition).map_err(|error| error.to_string())?;
    canonical.push(b'\n');
    if canonical != bytes {
        return Err("orphan disposition bytes are not canonical".to_string());
    }
    Ok(Some(disposition))
}

fn prepare_orphan_pending_parent(event_root: &Path, writer: &str) -> Result<(), String> {
    let directory = event_root.join("retirement/orphan-trash").join(writer);
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    sync_directory(&directory)
}

enum OrphanUnlinkSlice {
    More,
    Complete,
    Cancelled,
}

fn unlink_orphan_slice(
    pending: &Path,
    proof: &OrphanProof,
    max_entries: usize,
    job: &mut crate::maintenance::MaintenanceJobLease,
) -> Result<OrphanUnlinkSlice, String> {
    let allowed = [
        crate::event_store::DATABASE_FILE_NAME.to_string(),
        format!("{}-wal", crate::event_store::DATABASE_FILE_NAME),
        format!("{}-shm", crate::event_store::DATABASE_FILE_NAME),
        crate::event_store::PREPARED_MANIFEST_FILE_NAME.to_string(),
    ];
    ensure_orphan_entries_known(pending)?;
    let operation = digest_hex(proof.operation_sha256.as_bytes());
    let mut removed = 0usize;
    for name in &allowed {
        let path = pending.join(name);
        match fs::remove_file(&path) {
            Ok(()) => {
                removed += 1;
                sync_directory(pending)?;
                #[cfg(test)]
                run_orphan_test_hook(&format!("unlink:{name}"), pending);
                job.record_progress(
                    "orphan_unlink",
                    Some(format!("orphan:unlink:{operation}:{name}")),
                    job.checkpoint().units_completed.saturating_add(1),
                    job.checkpoint().bytes_completed,
                    now_unix_micros(),
                )
                .map_err(|error| error.to_string())?;
                if job.is_cancelled().map_err(|error| error.to_string())? {
                    return Ok(OrphanUnlinkSlice::Cancelled);
                }
                if removed == max_entries {
                    ensure_orphan_entries_known(pending)?;
                    if fs::read_dir(pending)
                        .map_err(|error| error.to_string())?
                        .next()
                        .is_some()
                    {
                        return Ok(OrphanUnlinkSlice::More);
                    }
                    break;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    match fs::remove_dir(pending) {
        Ok(()) => {
            sync_directory(pending.parent().expect("orphan pending has parent"))?;
            Ok(OrphanUnlinkSlice::Complete)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(OrphanUnlinkSlice::Complete),
        Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => {
            Ok(OrphanUnlinkSlice::More)
        }
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
type OrphanTestHook = std::sync::Arc<dyn Fn(&str, &Path) + Send + Sync>;

#[cfg(test)]
thread_local! {
    static ORPHAN_TEST_HOOK: std::cell::RefCell<Option<OrphanTestHook>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
fn run_orphan_test_hook(stage: &str, path: &Path) {
    let hook = ORPHAN_TEST_HOOK.with(|slot| slot.borrow().clone());
    if let Some(hook) = hook {
        hook(stage, path);
    }
}

#[cfg(test)]
fn with_orphan_test_hook<T>(hook: OrphanTestHook, operation: impl FnOnce() -> T) -> T {
    ORPHAN_TEST_HOOK.with(|slot| {
        assert!(
            slot.borrow().is_none(),
            "orphan test hook is already installed"
        );
        *slot.borrow_mut() = Some(hook);
    });
    let result = operation();
    ORPHAN_TEST_HOOK.with(|slot| *slot.borrow_mut() = None);
    result
}

fn create_once_file(directory: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    let target = directory.join(name);
    match fs::read(&target) {
        Ok(existing) if existing == bytes => return Ok(()),
        Ok(_) => return Err("orphan disposition publication conflict".to_string()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    let temp = directory.join(format!(".{name}.{}.tmp", Uuid::new_v4().simple()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)
        .map_err(|error| error.to_string())?;
    file.write_all(bytes).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    drop(file);
    match fs::hard_link(&temp, &target) {
        Ok(()) => {
            fs::remove_file(&temp).map_err(|error| error.to_string())?;
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let existing = fs::read(&target).map_err(|error| error.to_string())?;
            let _ = fs::remove_file(&temp);
            if existing == bytes {
                Ok(())
            } else {
                Err("orphan disposition publication conflict".to_string())
            }
        }
        Err(error) => {
            let _ = fs::remove_file(&temp);
            Err(error.to_string())
        }
    }
}

fn sync_directory(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| error.to_string())
}

fn migrate_legacy_page(
    event_root: &Path,
    cursor: &mut LegacyMigrationCursor,
    max_entries: usize,
) -> LegacyMigrationOutcome {
    let mut outcome = LegacyMigrationOutcome::default();
    let writers = event_root.join("writers");
    if !writers.is_dir() {
        outcome.completed = true;
        return outcome;
    }
    let mut examined = 0usize;
    while examined < max_entries {
        if cursor.current_writer.is_none() {
            let page = match read_directory_page(&writers, cursor.writer_cookie, 1) {
                Ok(page) => page,
                Err(error) => {
                    outcome.root_error = true;
                    outcome.issues.push(format!(
                        "legacy writers enumeration at cookie {}: {error}",
                        cursor.writer_cookie
                    ));
                    outcome.entries_examined = examined;
                    return outcome;
                }
            };
            cursor.writer_cookie = page.next_cookie;
            let Some(name) = page.entries.into_iter().next() else {
                if page.exhausted {
                    cursor.writer_cookie = 0;
                    outcome.completed = true;
                    outcome.entries_examined = examined;
                    return outcome;
                }
                continue;
            };
            examined += 1;
            if parse_id_hex(&name).is_err() || !writers.join(&name).is_dir() {
                outcome.issues.push(format!(
                    "legacy writer entry is invalid or unavailable: {name}"
                ));
                continue;
            }
            cursor.current_writer = Some(name);
            cursor.child_cookie = 0;
            cursor.phase = LegacyMigrationPhase::Generations;
            if examined == max_entries {
                outcome.more = true;
                outcome.entries_examined = examined;
                return outcome;
            }
        }

        let writer_hex = cursor.current_writer.clone().expect("writer selected");
        let writer = match parse_id_hex(&writer_hex) {
            Ok(bytes) => crate::event_store::WriterInstanceId::from_bytes(bytes),
            Err(error) => {
                outcome
                    .issues
                    .push(format!("legacy writer identity {writer_hex}: {error}"));
                cursor.current_writer = None;
                cursor.child_cookie = 0;
                continue;
            }
        };
        let writer_dir = writers.join(&writer_hex);
        let child_dir = match cursor.phase {
            LegacyMigrationPhase::Generations => writer_dir.join("generations"),
            LegacyMigrationPhase::Staging => writer_dir.join("staging"),
        };
        if !child_dir.is_dir() {
            if cursor.phase == LegacyMigrationPhase::Generations {
                cursor.phase = LegacyMigrationPhase::Staging;
                cursor.child_cookie = 0;
                continue;
            }
            cursor.current_writer = None;
            cursor.phase = LegacyMigrationPhase::Generations;
            cursor.child_cookie = 0;
            continue;
        }
        let page = match read_directory_page(
            &child_dir,
            cursor.child_cookie,
            max_entries.saturating_sub(examined).max(1),
        ) {
            Ok(page) => page,
            Err(error) => {
                outcome.issues.push(format!(
                    "legacy {writer_hex}/{:?} enumeration at cookie {}: {error}",
                    cursor.phase, cursor.child_cookie
                ));
                cursor.child_cookie = 0;
                if cursor.phase == LegacyMigrationPhase::Generations {
                    cursor.phase = LegacyMigrationPhase::Staging;
                } else {
                    cursor.current_writer = None;
                    cursor.phase = LegacyMigrationPhase::Generations;
                }
                examined += 1;
                continue;
            }
        };
        cursor.child_cookie = page.next_cookie;
        for name in page.entries {
            examined += 1;
            match cursor.phase {
                LegacyMigrationPhase::Generations => {
                    if let Ok(bytes) = parse_id_hex(&name) {
                        let generation = crate::event_store::GenerationId::from_bytes(bytes);
                        let path = child_dir.join(&name);
                        let manifest = read_prepared_manifest(&path).ok();
                        if let Err(error) = register_legacy_generation(
                            event_root,
                            writer,
                            generation,
                            DiscoveryPhase::Prepared,
                            None,
                            manifest.as_ref(),
                        ) {
                            outcome.issues.push(format!(
                                "legacy generation {writer_hex}/{name} registration: {error}"
                            ));
                        }
                    } else {
                        outcome.issues.push(format!(
                            "legacy generation name {writer_hex}/{name} is invalid"
                        ));
                    }
                }
                LegacyMigrationPhase::Staging => {
                    let Some(generation_hex) = name.split('.').next() else {
                        continue;
                    };
                    if let Ok(bytes) = parse_id_hex(generation_hex) {
                        let generation = crate::event_store::GenerationId::from_bytes(bytes);
                        let path = child_dir.join(&name);
                        let manifest = read_prepared_manifest(&path).ok();
                        if let Err(error) = register_legacy_generation(
                            event_root,
                            writer,
                            generation,
                            DiscoveryPhase::Staging,
                            Some(name.clone()),
                            manifest.as_ref(),
                        ) {
                            outcome.issues.push(format!(
                                "legacy staging {writer_hex}/{generation_hex} registration: {error}"
                            ));
                        }
                    } else {
                        outcome.issues.push(format!(
                            "legacy staging generation {writer_hex}/{generation_hex} is invalid"
                        ));
                    }
                }
            }
        }
        if page.exhausted {
            cursor.child_cookie = 0;
            if cursor.phase == LegacyMigrationPhase::Generations {
                cursor.phase = LegacyMigrationPhase::Staging;
            } else {
                cursor.current_writer = None;
                cursor.phase = LegacyMigrationPhase::Generations;
            }
        }
        if examined == max_entries {
            outcome.more = true;
            outcome.entries_examined = examined;
            return outcome;
        }
    }
    outcome.more = true;
    outcome.entries_examined = examined;
    outcome
}

struct DirectoryPage {
    entries: Vec<String>,
    next_cookie: i64,
    exhausted: bool,
}

#[cfg(unix)]
fn read_directory_page(path: &Path, cookie: i64, limit: usize) -> Result<DirectoryPage, String> {
    use std::os::unix::ffi::OsStrExt;

    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| "directory path contains NUL".to_string())?;
    // SAFETY: the C string remains live until closed, readdir's name is copied
    // before the next call, and every successful opendir is paired with closedir.
    let directory = unsafe { libc::opendir(path.as_ptr()) };
    if directory.is_null() {
        return Err(io::Error::last_os_error().to_string());
    }
    if cookie > 0 {
        unsafe { libc::seekdir(directory, cookie as libc::c_long) };
    }
    let mut entries = Vec::new();
    let mut next_cookie = cookie;
    let mut exhausted = false;
    while entries.len() < limit {
        let entry = unsafe { libc::readdir(directory) };
        if entry.is_null() {
            exhausted = true;
            break;
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        next_cookie = unsafe { libc::telldir(directory) } as i64;
        if name == "." || name == ".." {
            continue;
        }
        entries.push(name);
    }
    unsafe { libc::closedir(directory) };
    Ok(DirectoryPage {
        entries,
        next_cookie,
        exhausted,
    })
}

#[cfg(not(unix))]
fn read_directory_page(path: &Path, cookie: i64, limit: usize) -> Result<DirectoryPage, String> {
    let start = usize::try_from(cookie.max(0)).map_err(|error| error.to_string())?;
    let mut iterator = fs::read_dir(path)
        .map_err(|error| error.to_string())?
        .skip(start);
    let mut entries = Vec::new();
    while entries.len() < limit {
        let Some(entry) = iterator.next() else {
            return Ok(DirectoryPage {
                entries,
                next_cookie: (start + limit) as i64,
                exhausted: true,
            });
        };
        entries.push(
            entry
                .map_err(|error| error.to_string())?
                .file_name()
                .to_string_lossy()
                .into_owned(),
        );
    }
    Ok(DirectoryPage {
        entries,
        next_cookie: (start + limit) as i64,
        exhausted: false,
    })
}

fn run_rotation_follow_up(
    store: &MaintenanceJobStore,
    event_root: &Path,
    opportunity_epoch: i64,
    owner: MaintenanceWorkerIdentity,
    candidate: &DiscoveryEntry,
    partition: &str,
) -> Result<(), String> {
    let key = MaintenanceJobKey::new(MaintenanceJobKind::RotationFollowUp, partition)
        .map_err(|error| error.to_string())?;
    let mut job = match acquire_resumable(store, key, opportunity_epoch, owner)? {
        AcquireMaintenanceJob::DuplicateLive(_) | AcquireMaintenanceJob::AlreadyTerminal(_) => {
            return Ok(());
        }
        AcquireMaintenanceJob::Acquired(job) => job,
    };
    let layout = WriterLayout::open_existing(event_root, *candidate.writer.as_bytes())
        .map_err(|error| error.to_string())?;
    let generation = layout.generation_dir(candidate.generation);
    let _generation_lease = match layout.acquire_generation_maintenance_lease(candidate.generation)
    {
        Ok(lease) => lease,
        Err(GenerationError::GenerationAlreadyOwned(_)) => {
            return job
                .yield_run(
                    "selected_head_owner_live",
                    "exact generation lease is still held by a producer or reader",
                    now_unix_micros(),
                )
                .map_err(|error| error.to_string());
        }
        Err(error) => {
            return job
                .finish(
                    MaintenanceRunState::Preserved,
                    "selected_head_lease_refused",
                    error.to_string(),
                    now_unix_micros(),
                )
                .map_err(|write_error| write_error.to_string());
        }
    };
    if !candidate_is_selected(&layout, candidate.generation)? {
        let state = read_generation_state(&generation)?;
        if state == GenerationState::Closed {
            record_closed_generation(event_root, candidate.writer, candidate.generation)?;
            return job
                .finish(
                    MaintenanceRunState::Succeeded,
                    "selected_head_rotated",
                    "authoritative head advanced and the closed generation returned to active historical work",
                    now_unix_micros(),
                )
                .map_err(|error| error.to_string());
        }
        return job
            .finish(
                MaintenanceRunState::Preserved,
                "selected_head_changed",
                format!(
                    "authoritative head advanced but generation state is {state:?}; no archive transition was published"
                ),
                now_unix_micros(),
            )
            .map_err(|error| error.to_string());
    }
    let prepared = read_prepared_manifest(&generation).map_err(|error| error.to_string())?;
    let mut archive_dead_selected = false;
    let status = match prepared.producer.native_process.as_ref() {
        Some(expected) => match crate::pid_identity::observe_live_process_identity(expected.os_pid)
        {
            crate::pid_identity::ProcessIdentityObservation::ExactLive(live)
                if live.os_pid == expected.os_pid
                    && live.os_pid_starttime_ticks == expected.os_pid_starttime_ticks
                    && crate::event_store::Digest32::sha256(live.os_boot_id.as_bytes())
                        == expected.os_boot_id_sha256 =>
            {
                "selected_head_owner_live"
            }
            crate::pid_identity::ProcessIdentityObservation::ExactLive(_)
            | crate::pid_identity::ProcessIdentityObservation::Dead => {
                archive_dead_selected = true;
                "selected_head_owner_dead_preserved"
            }
            crate::pid_identity::ProcessIdentityObservation::Unsupported
            | crate::pid_identity::ProcessIdentityObservation::ReadError(_) => {
                "selected_head_owner_unknown_preserved"
            }
        },
        None => "selected_head_has_no_exact_native_owner_preserved",
    };
    job.finish(
        MaintenanceRunState::Preserved,
        "selected_head_preserved",
        status,
        now_unix_micros(),
    )
    .map_err(|error| error.to_string())?;
    if archive_dead_selected {
        move_generation_class(
            event_root,
            candidate.writer,
            candidate.generation,
            DiscoveryClass::Active,
            DiscoveryClass::Archive,
            DiscoveryPhase::PreservedDeadHead,
        )?;
    }
    Ok(())
}

fn run_index_maintenance(
    store: &MaintenanceJobStore,
    event_root: &Path,
    opportunity_epoch: i64,
    owner: MaintenanceWorkerIdentity,
    candidate: &DiscoveryEntry,
    partition: &str,
    max_file_bytes: u64,
) -> Result<(), String> {
    let key = MaintenanceJobKey::new(MaintenanceJobKind::IndexRebuild, partition)
        .map_err(|error| error.to_string())?;
    let mut job = match acquire_resumable(store, key, opportunity_epoch, owner)? {
        AcquireMaintenanceJob::DuplicateLive(_) | AcquireMaintenanceJob::AlreadyTerminal(_) => {
            return Ok(());
        }
        AcquireMaintenanceJob::Acquired(job) => job,
    };
    match audit_event_generation_indexes(
        event_root,
        candidate.writer,
        candidate.generation,
        max_file_bytes,
    ) {
        Ok(plan) if plan.action == IndexRebuildAction::None => job
            .finish(
                MaintenanceRunState::Succeeded,
                "indexes_valid",
                "AGE-376 bounded index plan requires no repair",
                now_unix_micros(),
            )
            .map_err(|error| error.to_string()),
        Ok(plan) => job
            .finish(
                MaintenanceRunState::Preserved,
                "repair_copy_required",
                format!(
                    "AGE-376 refused in-place index mutation: {:?}:{}",
                    plan.action,
                    plan.missing_indexes.join(",")
                ),
                now_unix_micros(),
            )
            .map_err(|error| error.to_string()),
        Err(error) => job
            .finish(
                MaintenanceRunState::Preserved,
                "index_audit_refused",
                error.to_string(),
                now_unix_micros(),
            )
            .map_err(|write_error| write_error.to_string()),
    }
}

#[allow(clippy::too_many_arguments)]
fn run_catalog_inspection(
    store: &MaintenanceJobStore,
    event_root: &Path,
    opportunity_epoch: i64,
    owner: MaintenanceWorkerIdentity,
    candidate: &DiscoveryEntry,
    partition: &str,
    max_rows: u64,
    max_file_bytes: u64,
) -> Result<(), String> {
    let key = MaintenanceJobKey::new(MaintenanceJobKind::CatalogInspection, partition)
        .map_err(|error| error.to_string())?;
    let mut job = match acquire_resumable(store, key, opportunity_epoch, owner)? {
        AcquireMaintenanceJob::DuplicateLive(_) | AcquireMaintenanceJob::AlreadyTerminal(_) => {
            return Ok(());
        }
        AcquireMaintenanceJob::Acquired(job) => job,
    };
    match inspect_event_generation_catalog(
        event_root,
        candidate.writer,
        candidate.generation,
        max_rows,
        max_file_bytes,
    ) {
        Ok(CatalogInventoryResult::Entry(_)) => job
            .finish(
                MaintenanceRunState::Preserved,
                "catalog_publication_unsupported",
                "AGE-376 catalog entry validated; no authoritative catalog publication API exists",
                now_unix_micros(),
            )
            .map_err(|error| error.to_string()),
        Ok(CatalogInventoryResult::WorkLimitReached { rows_examined }) => job
            .finish(
                MaintenanceRunState::Preserved,
                "catalog_cursor_unsupported",
                format!(
                    "AGE-376 catalog inspection reached its {rows_examined}-row bound without a resumable cursor"
                ),
                now_unix_micros(),
            )
            .map_err(|error| error.to_string()),
        Err(error) => job
            .finish(
                MaintenanceRunState::Preserved,
                "catalog_inspection_refused",
                error.to_string(),
                now_unix_micros(),
            )
            .map_err(|write_error| write_error.to_string()),
    }
}

/// Drive AGE-372's existing cursor-bound State and mailbox retention APIs from
/// the detached process. Every family is an exact durable job/partition and
/// opens its historical writer only inside the child for one bounded batch.
pub fn run_coordination_retention_opportunity(
    data_root: &Path,
    opportunity_epoch: i64,
    owner: MaintenanceWorkerIdentity,
    batch_limit: usize,
) -> Result<CoordinationRetentionOutcome, String> {
    if batch_limit == 0 {
        return Err("coordination retention batch limit must be non-zero".to_string());
    }
    let store = MaintenanceJobStore::open(data_root).map_err(|error| error.to_string())?;
    let mut summary = CoordinationRetentionOutcome {
        jobs_completed: 0,
        jobs_yielded: 0,
        duplicate_jobs: 0,
        gaps: Vec::new(),
    };
    let state_path = data_root.join("state.db");
    if state_path.is_file() {
        for family in [
            RetentionFamily::Invocation,
            RetentionFamily::ProviderLogicalLaunch,
            RetentionFamily::CompletedTurn,
        ] {
            run_retention_family(
                &store,
                opportunity_epoch,
                owner.clone(),
                batch_limit,
                "state",
                family,
                || StateDb::open_historical_without_observation(&state_path),
                |database, request| database.run_retention_batch_without_observation(request),
                &mut summary,
            )?;
        }
    }
    let mailbox_path = data_root.join("pid-identity.db");
    if mailbox_path.is_file() {
        for family in [
            RetentionFamily::Mailbox,
            RetentionFamily::MailboxDeliveryAttempt,
            RetentionFamily::CompletionEvent,
        ] {
            run_retention_family(
                &store,
                opportunity_epoch,
                owner.clone(),
                batch_limit,
                "pid-mailbox",
                family,
                || MailboxDb::open_historical_without_observation(&mailbox_path),
                |database, request| database.run_retention_batch_without_observation(request),
                &mut summary,
            )?;
        }
        run_mailbox_compaction(
            &store,
            opportunity_epoch,
            owner,
            batch_limit,
            &mailbox_path,
            &mut summary,
        )?;
    }
    Ok(summary)
}

#[allow(clippy::too_many_arguments)]
fn run_retention_family<Database>(
    store: &MaintenanceJobStore,
    opportunity_epoch: i64,
    owner: MaintenanceWorkerIdentity,
    batch_limit: usize,
    partition_root: &str,
    family: RetentionFamily,
    open: impl FnOnce() -> Result<Database, String>,
    run: impl FnOnce(
        &mut Database,
        &RetentionBatchRequest,
    ) -> Result<crate::retention::RetentionBatchOutcome, String>,
    summary: &mut CoordinationRetentionOutcome,
) -> Result<(), String> {
    let key = MaintenanceJobKey::new(
        if partition_root == "state" {
            MaintenanceJobKind::StateRetention
        } else {
            MaintenanceJobKind::MailboxRetention
        },
        format!("{partition_root}/{}", family.as_str()),
    )
    .map_err(|error| error.to_string())?;
    let mut job = match acquire_resumable(store, key, opportunity_epoch, owner)? {
        AcquireMaintenanceJob::DuplicateLive(_) | AcquireMaintenanceJob::AlreadyTerminal(_) => {
            summary.duplicate_jobs += 1;
            return Ok(());
        }
        AcquireMaintenanceJob::Acquired(job) => job,
    };
    if job.is_cancelled().map_err(|error| error.to_string())? {
        job.finish(
            MaintenanceRunState::Cancelled,
            "cancelled",
            "retention cancellation observed",
            now_unix_micros(),
        )
        .map_err(|error| error.to_string())?;
        summary.jobs_completed += 1;
        return Ok(());
    }
    let existing_resume = job
        .checkpoint()
        .cursor
        .as_deref()
        .map(serde_json::from_str::<RetentionResume>)
        .transpose()
        .map_err(|error| format!("invalid durable retention cursor: {error}"))?;
    let resume = existing_resume.unwrap_or_else(|| RetentionResume {
        as_of_unix_micros: now_unix_micros(),
        cursor: None,
    });
    if job.checkpoint().cursor.is_none() {
        job.record_progress(
            "retention_snapshot",
            Some(serde_json::to_string(&resume).map_err(|error| error.to_string())?),
            job.checkpoint().units_completed,
            job.checkpoint().bytes_completed,
            now_unix_micros(),
        )
        .map_err(|error| error.to_string())?;
    }
    let request = RetentionBatchRequest {
        policy: RetentionPolicy::default(),
        family,
        as_of_unix_micros: resume.as_of_unix_micros,
        limit: batch_limit,
        cursor: resume.cursor,
    };
    let mut database = match open() {
        Ok(database) => database,
        Err(error) => {
            job.finish(
                MaintenanceRunState::Failed,
                "open_failed",
                &error,
                now_unix_micros(),
            )
            .map_err(|write_error| write_error.to_string())?;
            summary
                .gaps
                .push(format!("{partition_root}/{}: {error}", family.as_str()));
            return Ok(());
        }
    };
    let outcome = match run(&mut database, &request) {
        Ok(outcome) => outcome,
        Err(error) => {
            drop(database);
            job.finish(
                MaintenanceRunState::Failed,
                "batch_failed",
                &error,
                now_unix_micros(),
            )
            .map_err(|write_error| write_error.to_string())?;
            summary
                .gaps
                .push(format!("{partition_root}/{}: {error}", family.as_str()));
            return Ok(());
        }
    };
    drop(database);
    let cursor = serde_json::to_string(&RetentionResume {
        as_of_unix_micros: request.as_of_unix_micros,
        cursor: outcome.next_cursor.clone(),
    })
    .map_err(|error| error.to_string())?;
    job.record_progress(
        "retention_batch",
        Some(cursor),
        job.checkpoint()
            .units_completed
            .saturating_add(outcome.candidates_examined as u64),
        job.checkpoint().bytes_completed,
        now_unix_micros(),
    )
    .map_err(|error| error.to_string())?;
    match outcome.status {
        RetentionBatchStatus::Complete => {
            job.finish(
                MaintenanceRunState::Succeeded,
                "complete",
                "AGE-372 bounded retention cursor exhausted",
                now_unix_micros(),
            )
            .map_err(|error| error.to_string())?;
            summary.jobs_completed += 1;
        }
        RetentionBatchStatus::MoreWork | RetentionBatchStatus::Busy => {
            let retained_gaps = if outcome.gaps.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&outcome.gaps).map_err(|error| error.to_string())?)
            };
            let reason = retained_gaps.as_deref().map_or_else(
                || format!("AGE-372 batch status {:?}", outcome.status),
                |gaps| {
                    format!(
                        "AGE-372 batch status {:?}; prior gaps: {gaps}",
                        outcome.status
                    )
                },
            );
            job.yield_run("yielded", reason, now_unix_micros())
                .map_err(|error| error.to_string())?;
            summary.jobs_yielded += 1;
            if let Some(gaps) = retained_gaps {
                summary
                    .gaps
                    .push(format!("{partition_root}/{}: {gaps}", family.as_str()));
            }
        }
        RetentionBatchStatus::Partial => {
            let reason = serde_json::to_string(&outcome.gaps).map_err(|error| error.to_string())?;
            job.yield_run("partial", &reason, now_unix_micros())
                .map_err(|error| error.to_string())?;
            summary.jobs_yielded += 1;
            summary
                .gaps
                .push(format!("{partition_root}/{}: {reason}", family.as_str()));
        }
    }
    Ok(())
}

fn run_mailbox_compaction(
    store: &MaintenanceJobStore,
    opportunity_epoch: i64,
    owner: MaintenanceWorkerIdentity,
    batch_limit: usize,
    mailbox_path: &Path,
    summary: &mut CoordinationRetentionOutcome,
) -> Result<(), String> {
    let key = MaintenanceJobKey::new(
        MaintenanceJobKind::HistoricalCompaction,
        "pid-mailbox/delivered-payloads",
    )
    .map_err(|error| error.to_string())?;
    let mut job = match acquire_resumable(store, key, opportunity_epoch, owner)? {
        AcquireMaintenanceJob::DuplicateLive(_) | AcquireMaintenanceJob::AlreadyTerminal(_) => {
            summary.duplicate_jobs += 1;
            return Ok(());
        }
        AcquireMaintenanceJob::Acquired(job) => job,
    };
    if job.is_cancelled().map_err(|error| error.to_string())? {
        job.finish(
            MaintenanceRunState::Cancelled,
            "cancelled",
            "payload compaction cancellation observed",
            now_unix_micros(),
        )
        .map_err(|error| error.to_string())?;
        summary.jobs_completed += 1;
        return Ok(());
    }
    let mailbox = match MailboxDb::open_historical_without_observation(mailbox_path) {
        Ok(mailbox) => mailbox,
        Err(error) => {
            job.finish(
                MaintenanceRunState::Failed,
                "open_failed",
                &error,
                now_unix_micros(),
            )
            .map_err(|write_error| write_error.to_string())?;
            summary
                .gaps
                .push(format!("pid-mailbox/compaction: {error}"));
            return Ok(());
        }
    };
    let report = mailbox.payloads().compact_delivered_payloads(batch_limit);
    drop(mailbox);
    match report {
        Ok(report) => {
            job.record_progress(
                "compaction_batch",
                None,
                job.checkpoint()
                    .units_completed
                    .saturating_add(report.scanned_rows as u64),
                job.checkpoint().bytes_completed,
                now_unix_micros(),
            )
            .map_err(|error| error.to_string())?;
            if report.scanned_rows == batch_limit {
                job.yield_run(
                    "yielded",
                    "bounded compaction batch may have more work",
                    now_unix_micros(),
                )
                .map_err(|error| error.to_string())?;
                summary.jobs_yielded += 1;
            } else {
                job.finish(
                    MaintenanceRunState::Succeeded,
                    "complete",
                    "bounded compaction candidates exhausted",
                    now_unix_micros(),
                )
                .map_err(|error| error.to_string())?;
                summary.jobs_completed += 1;
            }
        }
        Err(error) => {
            job.finish(
                MaintenanceRunState::Failed,
                "compaction_failed",
                &error,
                now_unix_micros(),
            )
            .map_err(|write_error| write_error.to_string())?;
            summary
                .gaps
                .push(format!("pid-mailbox/compaction: {error}"));
        }
    }
    Ok(())
}

fn acquire_resumable(
    store: &MaintenanceJobStore,
    key: MaintenanceJobKey,
    opportunity_epoch: i64,
    owner: MaintenanceWorkerIdentity,
) -> Result<AcquireMaintenanceJob, String> {
    // A daily opportunity is only the launch trigger. An incomplete exact job
    // retains its original epoch, cursor, and cancellation fence across a UTC
    // day boundary. A terminal prior job starts fresh under the new epoch.
    let effective_epoch = store
        .read_status(&key)
        .map_err(|error| error.to_string())?
        .filter(|status| !status.state.terminal())
        .map_or(opportunity_epoch, |status| status.opportunity_epoch);
    store
        .acquire(key, effective_epoch, owner, now_unix_micros())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_store::{
        Digest32, EventCorrelations, EventEnvelopeV1, EventFamily, EventId, EventKind,
        EventWriterConfig, GenerationId, GenerationSource, HeadRecord, NativeProcessIdentity,
        NewEventV1, PayloadNormalizationPolicy, PreparedManifest, ProcessEventWriter,
        ProcessInstanceId, ProducerIdentity, ReadLimits, SupervisorAuthorityId, TraceId,
        WriterInstanceId, close_generation, initialize_generation_schema, mark_generation_writable,
    };
    use crate::retention::RetentionBatchOutcome;
    use rusqlite::Connection;
    use std::process::Command;
    use std::time::{Duration, Instant};

    fn owner(pid: i64) -> MaintenanceWorkerIdentity {
        MaintenanceWorkerIdentity {
            worker_instance_id: Uuid::new_v4(),
            os_pid: pid,
            os_boot_id: "boot".into(),
            os_pid_starttime_ticks: pid * 10,
            schedule_basis: "test".into(),
            launch_id: None,
        }
    }

    fn dead_manifest(writer: u8, generation: u8) -> PreparedManifest {
        let writer = WriterInstanceId::from_bytes([writer; 16]);
        PreparedManifest {
            format_version: crate::event_store::EVENT_STORE_FORMAT_VERSION,
            schema_version: crate::event_store::EVENT_SCHEMA_VERSION,
            writer_instance_id: writer,
            generation_id: GenerationId::from_bytes([generation; 16]),
            predecessor_generation_id: None,
            database_relative_path: crate::event_store::DATABASE_FILE_NAME.to_string(),
            created_at_unix_micros: 1,
            head_epoch: 0,
            source: GenerationSource::NativeProcess,
            producer: ProducerIdentity {
                writer_instance_id: writer,
                process_instance_id: ProcessInstanceId::from_bytes([writer.as_bytes()[0]; 16]),
                process_root_id: ProcessInstanceId::from_bytes([writer.as_bytes()[0]; 16]),
                parent_process_instance_id: None,
                supervisor_authority_id: Some(SupervisorAuthorityId::from_bytes([7; 16])),
                native_process: Some(NativeProcessIdentity {
                    os_pid: i64::from(i32::MAX),
                    os_boot_id_sha256: Digest32::sha256(b"dead-test-boot"),
                    os_pid_starttime_ticks: 1,
                }),
            },
        }
    }

    fn subprocess_producer(writer: u8) -> ProducerIdentity {
        ProducerIdentity {
            writer_instance_id: WriterInstanceId::from_bytes([writer; 16]),
            process_instance_id: ProcessInstanceId::from_bytes([writer + 1; 16]),
            process_root_id: ProcessInstanceId::from_bytes([writer + 1; 16]),
            parent_process_instance_id: None,
            supervisor_authority_id: None,
            native_process: Some(NativeProcessIdentity {
                os_pid: i64::from(std::process::id()),
                os_boot_id_sha256: Digest32::sha256(b"abrupt-writer-fixture-boot"),
                os_pid_starttime_ticks: 1,
            }),
        }
    }

    fn subprocess_trace_event(producer: &ProducerIdentity) -> EventEnvelopeV1 {
        EventEnvelopeV1::normalize(
            NewEventV1 {
                event_id: EventId::from_bytes([95; 16]),
                family: EventFamily::Trace,
                kind: EventKind::registered("trace.abrupt_writer_fixture").unwrap(),
                recorded_at_unix_micros: 10,
                producer_sequence: 1,
                producer: producer.clone(),
                correlations: EventCorrelations {
                    trace_id: Some(TraceId::from_bytes([96; 16])),
                    span_id: Some(crate::event_store::SpanId::from_bytes([97; 16])),
                    ..EventCorrelations::default()
                },
                payload: serde_json::json!({"state": "accepted"}),
                legacy_provenance: None,
                retry_of_generation_id: None,
            },
            &PayloadNormalizationPolicy::registered(&["state"]).unwrap(),
            11,
        )
        .unwrap()
    }

    fn publish_empty_unheaded(event_root: &Path, manifest: &PreparedManifest) -> WriterLayout {
        let layout =
            WriterLayout::create(event_root, *manifest.writer_instance_id.as_bytes()).unwrap();
        let lease = layout
            .acquire_generation_lease(manifest.generation_id)
            .unwrap();
        layout
            .publish_prepared_generation(
                manifest,
                |path| {
                    let mut connection = Connection::open(path)?;
                    initialize_generation_schema(&mut connection, &manifest.generation_metadata()?)
                        .map_err(|error| GenerationError::Validation(error.to_string()))
                },
                |_, _| Ok(()),
            )
            .unwrap();
        drop(lease);
        layout
    }

    fn disposition_path(event_root: &Path, manifest: &PreparedManifest) -> std::path::PathBuf {
        event_root
            .join("retirement/orphan-dispositions")
            .join(id_hex(manifest.writer_instance_id.as_bytes()))
            .join(format!(
                "{}.json",
                id_hex(manifest.generation_id.as_bytes())
            ))
    }

    #[test]
    fn rotation_follow_up_revalidates_a_stale_selected_candidate_after_rotation() {
        let root = tempfile::tempdir().unwrap();
        let event_root = root.path().join(EVENT_STORE_RELATIVE_PATH);
        let old = dead_manifest(40, 41);
        let layout = publish_empty_unheaded(&event_root, &old);
        let old_head = HeadRecord::new(
            0,
            old.writer_instance_id,
            old.generation_id,
            old.sha256().unwrap(),
        )
        .unwrap();
        let old_slot = layout.publish_head(None, &old_head).unwrap();
        crate::event_store::maintenance_discovery::record_generation_phase(
            &event_root,
            old.writer_instance_id,
            old.generation_id,
            DiscoveryPhase::Selected,
        )
        .unwrap();
        let stale_candidate = read_batch(&event_root, &DiscoveryCursor::default(), 64, 1)
            .unwrap()
            .entries
            .into_iter()
            .find(|candidate| candidate.generation == old.generation_id)
            .unwrap();

        let mut successor = dead_manifest(40, 42);
        successor.predecessor_generation_id = Some(old.generation_id);
        successor.head_epoch = 1;
        successor.created_at_unix_micros = 2;
        publish_empty_unheaded(&event_root, &successor);
        let mut old_database = Connection::open(
            layout
                .generation_dir(old.generation_id)
                .join(crate::event_store::DATABASE_FILE_NAME),
        )
        .unwrap();
        mark_generation_writable(&old_database).unwrap();
        close_generation(&mut old_database, 3, successor.generation_id).unwrap();
        drop(old_database);
        record_closed_generation(&event_root, old.writer_instance_id, old.generation_id).unwrap();
        let successor_head = HeadRecord::new(
            1,
            successor.writer_instance_id,
            successor.generation_id,
            successor.sha256().unwrap(),
        )
        .unwrap();
        layout
            .publish_head(Some(old_slot), &successor_head)
            .unwrap();

        let store = MaintenanceJobStore::open(root.path()).unwrap();
        let partition = format!(
            "{}/{}",
            id_hex(old.writer_instance_id.as_bytes()),
            id_hex(old.generation_id.as_bytes())
        );
        run_rotation_follow_up(
            &store,
            &event_root,
            7,
            owner(7),
            &stale_candidate,
            &partition,
        )
        .unwrap();

        let (class, record) = crate::event_store::maintenance_discovery::read_exact_record(
            &event_root,
            old.writer_instance_id,
            old.generation_id,
        )
        .unwrap()
        .unwrap();
        assert_eq!(class, DiscoveryClass::Active);
        assert_eq!(record.phase, DiscoveryPhase::Closed);
        let key = MaintenanceJobKey::new(MaintenanceJobKind::RotationFollowUp, partition).unwrap();
        let status = store.read_status(&key).unwrap().unwrap();
        assert_eq!(status.state, MaintenanceRunState::Succeeded);
        assert_eq!(status.phase, "selected_head_rotated");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn archived_dead_selected_head_remains_reachable_to_public_trace_query() {
        let root = tempfile::tempdir().unwrap();
        let event_root = root.path().join(EVENT_STORE_RELATIVE_PATH);
        let ready = root.path().join("abrupt-writer-ready");
        let executable = std::env::current_exe().unwrap();
        let mut child = Command::new(executable)
            .arg("detached_maintenance::tests::subprocess_abrupt_event_writer_helper")
            .arg("--exact")
            .env("OULIPOLY_AGE374_ABRUPT_EVENT_ROOT", &event_root)
            .env("OULIPOLY_AGE374_ABRUPT_EVENT_READY", &ready)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready.is_file() {
            assert!(
                Instant::now() < deadline,
                "abrupt writer fixture did not publish its accepted event"
            );
            if let Some(status) = child.try_wait().unwrap() {
                panic!("abrupt writer fixture exited before kill: {status}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        child.kill().unwrap();
        let status = child.wait().unwrap();
        assert!(!status.success());

        let writer = WriterInstanceId::from_bytes([94; 16]);
        let candidate = read_batch(&event_root, &DiscoveryCursor::default(), 4096, 8)
            .unwrap()
            .entries
            .into_iter()
            .find(|candidate| candidate.writer == writer)
            .unwrap();
        let partition = format!(
            "{}/{}",
            id_hex(candidate.writer.as_bytes()),
            id_hex(candidate.generation.as_bytes())
        );
        let store = MaintenanceJobStore::open(root.path()).unwrap();
        run_rotation_follow_up(&store, &event_root, 44, owner(44), &candidate, &partition).unwrap();

        let (class, record) = crate::event_store::maintenance_discovery::read_exact_record(
            &event_root,
            candidate.writer,
            candidate.generation,
        )
        .unwrap()
        .unwrap();
        assert_eq!(class, DiscoveryClass::Archive);
        assert_eq!(record.phase, DiscoveryPhase::PreservedDeadHead);

        let report = crate::longitudinal_metrics::query_trace(
            &event_root,
            TraceId::from_bytes([96; 16]),
            &ReadLimits::new(8, 8, 1024 * 1024).unwrap(),
        )
        .unwrap();
        assert!(report.discovery_issues.is_empty());
        assert!(
            report.read.coverage_complete,
            "issues: {:?}",
            report.read.issues
        );
        assert_eq!(report.read.records.len(), 1);
        assert_eq!(
            report.read.records[0].envelope.event_id,
            EventId::from_bytes([95; 16])
        );
    }

    #[test]
    fn subprocess_abrupt_event_writer_helper() {
        let Some(event_root) = std::env::var_os("OULIPOLY_AGE374_ABRUPT_EVENT_ROOT") else {
            return;
        };
        let ready = std::path::PathBuf::from(
            std::env::var_os("OULIPOLY_AGE374_ABRUPT_EVENT_READY").unwrap(),
        );
        let producer = subprocess_producer(94);
        let writer = ProcessEventWriter::start(EventWriterConfig::native(
            std::path::PathBuf::from(event_root),
            producer.clone(),
        ))
        .unwrap();
        writer.append(subprocess_trace_event(&producer)).unwrap();
        fs::write(ready, b"accepted\n").unwrap();
        std::thread::sleep(Duration::from_secs(60));
        drop(writer);
    }

    #[test]
    fn absent_databases_complete_without_creating_authority_databases() {
        let root = tempfile::tempdir().unwrap();
        let outcome = run_coordination_retention_opportunity(root.path(), 1, owner(1), 8).unwrap();
        assert_eq!(outcome.jobs_completed, 0);
        assert!(!root.path().join("state.db").exists());
        assert!(!root.path().join("pid-identity.db").exists());
    }

    #[test]
    fn coordination_retention_uses_maintenance_evidence_without_normal_event_head() {
        let root = tempfile::tempdir().unwrap();
        drop(StateDb::open(&root.path().join("state.db")).unwrap());
        drop(MailboxDb::open(&root.path().join("pid-identity.db")).unwrap());

        run_coordination_retention_opportunity(root.path(), 2, owner(2), 8).unwrap();

        assert!(
            !root
                .path()
                .join("diagnostics/event-store-v1/writers")
                .exists()
        );
        let maintenance_root = root.path().join(crate::maintenance::MAINTENANCE_DIRECTORY);
        assert!(maintenance_root.is_dir());
        assert!(fs::read_dir(maintenance_root).unwrap().next().is_some());
    }

    #[test]
    fn age372_cursor_and_as_of_snapshot_resume_across_bounded_job_slices() {
        let root = tempfile::tempdir().unwrap();
        let store = MaintenanceJobStore::open(root.path()).unwrap();
        let mut first_summary = CoordinationRetentionOutcome {
            jobs_completed: 0,
            jobs_yielded: 0,
            duplicate_jobs: 0,
            gaps: Vec::new(),
        };
        run_retention_family(
            &store,
            3,
            owner(1),
            2,
            "state",
            RetentionFamily::Invocation,
            || Ok(()),
            |_, request| {
                let cutoff = request.validate().unwrap();
                let mut outcome = RetentionBatchOutcome::new(request, cutoff);
                outcome.status = RetentionBatchStatus::MoreWork;
                outcome.candidates_examined = 2;
                outcome.next_cursor = Some(RetentionBatchCursor {
                    policy_version: request.policy.version.clone(),
                    family: request.family,
                    cutoff_unix_micros: cutoff,
                    after_authoritative_at: "2020-01-01T00:00:00.000000Z".into(),
                    after_key: "invocation-a".into(),
                });
                Ok(outcome)
            },
            &mut first_summary,
        )
        .unwrap();
        assert_eq!(first_summary.jobs_yielded, 1);

        let mut resumed_summary = CoordinationRetentionOutcome {
            jobs_completed: 0,
            jobs_yielded: 0,
            duplicate_jobs: 0,
            gaps: Vec::new(),
        };
        run_retention_family(
            &store,
            4,
            owner(2),
            2,
            "state",
            RetentionFamily::Invocation,
            || Ok(()),
            |_, request| {
                assert_eq!(
                    request
                        .cursor
                        .as_ref()
                        .map(|cursor| cursor.after_key.as_str()),
                    Some("invocation-a")
                );
                let cutoff = request.validate().unwrap();
                Ok(RetentionBatchOutcome::new(request, cutoff))
            },
            &mut resumed_summary,
        )
        .unwrap();
        assert_eq!(resumed_summary.jobs_completed, 1);
    }

    #[test]
    fn partial_retention_gap_yields_and_resumes_after_the_failed_candidate() {
        let root = tempfile::tempdir().unwrap();
        let store = MaintenanceJobStore::open(root.path()).unwrap();
        let mut first_summary = CoordinationRetentionOutcome {
            jobs_completed: 0,
            jobs_yielded: 0,
            duplicate_jobs: 0,
            gaps: Vec::new(),
        };
        run_retention_family(
            &store,
            5,
            owner(5),
            1,
            "state",
            RetentionFamily::Invocation,
            || Ok(()),
            |_, request| {
                let cutoff = request.validate().unwrap();
                let mut outcome = RetentionBatchOutcome::new(request, cutoff);
                outcome.status = RetentionBatchStatus::Partial;
                outcome.candidates_examined = 1;
                outcome.next_cursor = Some(RetentionBatchCursor {
                    policy_version: request.policy.version.clone(),
                    family: request.family,
                    cutoff_unix_micros: cutoff,
                    after_authoritative_at: "2020-01-01T00:00:00.000000Z".into(),
                    after_key: "permanent-error".into(),
                });
                outcome.gap("state_delete", "persistent candidate-local failure");
                Ok(outcome)
            },
            &mut first_summary,
        )
        .unwrap();
        assert_eq!(first_summary.jobs_yielded, 1);
        assert_eq!(first_summary.gaps.len(), 1);

        let mut resumed_summary = CoordinationRetentionOutcome {
            jobs_completed: 0,
            jobs_yielded: 0,
            duplicate_jobs: 0,
            gaps: Vec::new(),
        };
        run_retention_family(
            &store,
            6,
            owner(6),
            1,
            "state",
            RetentionFamily::Invocation,
            || Ok(()),
            |_, request| {
                assert_eq!(
                    request
                        .cursor
                        .as_ref()
                        .map(|cursor| cursor.after_key.as_str()),
                    Some("permanent-error")
                );
                let cutoff = request.validate().unwrap();
                Ok(RetentionBatchOutcome::new(request, cutoff))
            },
            &mut resumed_summary,
        )
        .unwrap();
        assert_eq!(resumed_summary.jobs_completed, 1);
    }

    #[test]
    fn cross_day_event_discovery_epoch_drives_exact_candidate_job() {
        let root = tempfile::tempdir().unwrap();
        let store = MaintenanceJobStore::open(root.path()).unwrap();
        let scan_key =
            MaintenanceJobKey::new(MaintenanceJobKind::EventDiscovery, "event-store-v1").unwrap();
        let AcquireMaintenanceJob::Acquired(mut first) =
            acquire_resumable(&store, scan_key.clone(), 8, owner(1)).unwrap()
        else {
            panic!();
        };
        first
            .record_progress(
                "discovery_batch",
                Some(serde_json::to_string(&EventOpportunityCursor::default()).unwrap()),
                1,
                0,
                now_unix_micros(),
            )
            .unwrap();
        first
            .yield_run("yielded", "more discovery", now_unix_micros())
            .unwrap();
        drop(first);
        let AcquireMaintenanceJob::Acquired(resumed) =
            acquire_resumable(&store, scan_key, 9, owner(2)).unwrap()
        else {
            panic!();
        };
        assert_eq!(resumed.checkpoint().opportunity_epoch, 8);
        assert!(resumed.checkpoint().cursor.is_some());
        let effective_epoch = resumed.checkpoint().opportunity_epoch;
        let candidate_key =
            MaintenanceJobKey::new(MaintenanceJobKind::EventRetirement, "aa/bb").unwrap();
        let AcquireMaintenanceJob::Acquired(candidate) =
            acquire_resumable(&store, candidate_key, effective_epoch, owner(3)).unwrap()
        else {
            panic!();
        };
        assert_eq!(candidate.checkpoint().opportunity_epoch, 8);
    }

    #[test]
    fn bounded_discovery_prioritizes_pending_and_advances_active_cursor() {
        let root = tempfile::tempdir().unwrap();
        let event_root = root.path().join("event-store-v1");
        for value in 1..=3 {
            crate::event_store::maintenance_discovery::register_legacy_generation(
                &event_root,
                WriterInstanceId::from_bytes([value; 16]),
                GenerationId::from_bytes([value + 10; 16]),
                DiscoveryPhase::Prepared,
                None,
                None,
            )
            .unwrap();
        }
        move_generation_class(
            &event_root,
            WriterInstanceId::from_bytes([3; 16]),
            GenerationId::from_bytes([13; 16]),
            DiscoveryClass::Active,
            DiscoveryClass::Pending,
            DiscoveryPhase::PendingTrash,
        )
        .unwrap();
        let mut cursor = DiscoveryCursor::default();
        let first = loop {
            let batch = read_batch(&event_root, &cursor, 8, 1).unwrap();
            cursor = batch.cursor.clone();
            if !batch.entries.is_empty() {
                break batch;
            }
        };
        assert_eq!(first.entries[0].class, DiscoveryClass::Pending);
        move_generation_class(
            &event_root,
            first.entries[0].writer,
            first.entries[0].generation,
            DiscoveryClass::Pending,
            DiscoveryClass::Archive,
            DiscoveryPhase::Retired,
        )
        .unwrap();
        let second = loop {
            let batch = read_batch(&event_root, &first.cursor, 64, 1).unwrap();
            if !batch.entries.is_empty() {
                break batch;
            }
            assert!(batch.more);
        };
        assert_eq!(
            second.entries[0].writer,
            WriterInstanceId::from_bytes([1; 16])
        );
        let third = loop {
            let batch = read_batch(&event_root, &second.cursor, 64, 1).unwrap();
            if !batch.entries.is_empty() {
                break batch;
            }
            assert!(batch.more);
        };
        assert_eq!(
            third.entries[0].writer,
            WriterInstanceId::from_bytes([2; 16])
        );
    }

    #[test]
    fn production_opportunity_discharges_prepared_and_incomplete_staging_orphans() {
        for (writer, generation, prepared) in [(31, 41, true), (32, 42, false)] {
            let root = tempfile::tempdir().unwrap();
            let event_root = root.path().join(EVENT_STORE_RELATIVE_PATH);
            let manifest = dead_manifest(writer, generation);
            let layout = if prepared {
                publish_empty_unheaded(&event_root, &manifest)
            } else {
                let layout =
                    WriterLayout::create(&event_root, *manifest.writer_instance_id.as_bytes())
                        .unwrap();
                let lease = layout
                    .acquire_generation_lease(manifest.generation_id)
                    .unwrap();
                let staging_name = format!(
                    "{}.interrupted.tmp",
                    id_hex(manifest.generation_id.as_bytes())
                );
                crate::event_store::maintenance_discovery::publish_generation_intent(
                    &event_root,
                    &manifest,
                    &staging_name,
                )
                .unwrap();
                let staging = layout.writer_dir().join("staging").join(&staging_name);
                fs::create_dir(&staging).unwrap();
                fs::write(
                    staging.join(crate::event_store::DATABASE_FILE_NAME),
                    b"interrupted before SQLite initialization",
                )
                .unwrap();
                drop(lease);
                layout
            };

            let outcome = run_event_store_opportunity(
                root.path(),
                17,
                owner(i64::from(writer)),
                OpportunityWorkLimits::default(),
            )
            .unwrap();
            assert!(matches!(
                outcome,
                OpportunityOutcome::Completed | OpportunityOutcome::Yielded
            ));
            assert!(!layout.generation_dir(manifest.generation_id).exists());
            assert!(disposition_path(&event_root, &manifest).is_file());
            assert!(
                !event_root
                    .join("retirement/orphan-trash")
                    .join(id_hex(manifest.writer_instance_id.as_bytes()))
                    .join(format!(
                        "{}.pending",
                        id_hex(manifest.generation_id.as_bytes())
                    ))
                    .exists()
            );
        }
    }

    #[test]
    fn manifestless_final_generation_is_preserved_even_with_retained_staging_name() {
        let root = tempfile::tempdir().unwrap();
        let event_root = root.path().join(EVENT_STORE_RELATIVE_PATH);
        let manifest = dead_manifest(35, 45);
        let layout = publish_empty_unheaded(&event_root, &manifest);
        let generation = layout.generation_dir(manifest.generation_id);
        fs::remove_file(generation.join(crate::event_store::PREPARED_MANIFEST_FILE_NAME)).unwrap();
        let database_before =
            fs::read(generation.join(crate::event_store::DATABASE_FILE_NAME)).unwrap();

        run_event_store_opportunity(root.path(), 23, owner(35), OpportunityWorkLimits::default())
            .unwrap();

        assert!(generation.is_dir());
        assert_eq!(
            fs::read(generation.join(crate::event_store::DATABASE_FILE_NAME)).unwrap(),
            database_before
        );
        assert!(!disposition_path(&event_root, &manifest).exists());
        let key = MaintenanceJobKey::new(
            MaintenanceJobKind::OrphanClassification,
            format!(
                "{}/{}",
                id_hex(manifest.writer_instance_id.as_bytes()),
                id_hex(manifest.generation_id.as_bytes())
            ),
        )
        .unwrap();
        let status = MaintenanceJobStore::open(root.path())
            .unwrap()
            .read_status(&key)
            .unwrap()
            .unwrap();
        assert_eq!(status.state, MaintenanceRunState::Preserved);
        assert!(
            status
                .terminal_reason
                .as_deref()
                .unwrap()
                .contains("manifest-less final generation")
        );
    }

    #[test]
    fn bad_candidate_is_preserved_and_cursor_advances_to_healthy_candidate() {
        let root = tempfile::tempdir().unwrap();
        let event_root = root.path().join(EVENT_STORE_RELATIVE_PATH);
        let bad = dead_manifest(1, 1);
        let bad_layout = publish_empty_unheaded(&event_root, &bad);
        fs::write(bad_layout.writer_dir().join("HEAD.0"), b"invalid head").unwrap();
        let healthy = dead_manifest(2, 2);
        let healthy_layout = publish_empty_unheaded(&event_root, &healthy);

        let first = run_event_store_opportunity(
            root.path(),
            24,
            owner(24),
            OpportunityWorkLimits {
                max_discovery_nodes: 256,
                max_partitions_per_run: 1,
                ..OpportunityWorkLimits::default()
            },
        )
        .unwrap();
        assert_eq!(first, OpportunityOutcome::Yielded);
        assert!(bad_layout.generation_dir(bad.generation_id).is_dir());
        assert!(
            healthy_layout
                .generation_dir(healthy.generation_id)
                .is_dir()
        );
        let scan_key =
            MaintenanceJobKey::new(MaintenanceJobKind::EventDiscovery, "event-store-v1").unwrap();
        let yielded = MaintenanceJobStore::open(root.path())
            .unwrap()
            .read_status(&scan_key)
            .unwrap()
            .unwrap();
        assert_eq!(yielded.state, MaintenanceRunState::Yielded);
        assert!(yielded.cursor.is_some());
        let mut second = OpportunityOutcome::Yielded;
        for actor in 25..40 {
            second = run_event_store_opportunity(
                root.path(),
                24,
                owner(actor),
                OpportunityWorkLimits {
                    max_discovery_nodes: 256,
                    max_partitions_per_run: 1,
                    ..OpportunityWorkLimits::default()
                },
            )
            .unwrap();
            if second != OpportunityOutcome::Yielded {
                break;
            }
        }
        assert_eq!(second, OpportunityOutcome::Preserved);
        assert!(
            !healthy_layout
                .generation_dir(healthy.generation_id)
                .exists()
        );
        assert!(disposition_path(&event_root, &healthy).is_file());
        let gap_key = MaintenanceJobKey::new(
            MaintenanceJobKind::EventDiscovery,
            format!(
                "event-store-v1/{}/{}",
                id_hex(bad.writer_instance_id.as_bytes()),
                id_hex(bad.generation_id.as_bytes())
            ),
        )
        .unwrap();
        let gap = MaintenanceJobStore::open(root.path())
            .unwrap()
            .read_status(&gap_key)
            .unwrap()
            .unwrap();
        assert_eq!(gap.phase, "candidate_gap");
        assert!(gap.terminal_reason.unwrap().contains("head"));
    }

    #[test]
    fn legacy_error_is_retained_while_later_generation_advances_and_daily_rewalks() {
        let root = tempfile::tempdir().unwrap();
        let event_root = root.path().join(EVENT_STORE_RELATIVE_PATH);
        let manifest = dead_manifest(36, 46);
        let layout = publish_empty_unheaded(&event_root, &manifest);
        fs::remove_dir_all(event_root.join("maintenance-discovery-v1")).unwrap();
        fs::create_dir_all(event_root.join("writers/not-a-writer")).unwrap();

        run_event_store_opportunity(
            root.path(),
            25,
            owner(25),
            OpportunityWorkLimits {
                max_discovery_nodes: 256,
                max_partitions_per_run: 4,
                ..OpportunityWorkLimits::default()
            },
        )
        .unwrap();
        assert!(disposition_path(&event_root, &manifest).is_file());
        assert!(!layout.generation_dir(manifest.generation_id).exists());
        let scan_key =
            MaintenanceJobKey::new(MaintenanceJobKind::EventDiscovery, "event-store-v1").unwrap();
        let first = MaintenanceJobStore::open(root.path())
            .unwrap()
            .read_status(&scan_key)
            .unwrap()
            .unwrap();
        assert!(first.terminal_reason.unwrap().contains("not-a-writer"));

        fs::create_dir_all(event_root.join("writers/another-invalid-writer")).unwrap();
        run_event_store_opportunity(root.path(), 26, owner(26), OpportunityWorkLimits::default())
            .unwrap();
        let second = MaintenanceJobStore::open(root.path())
            .unwrap()
            .read_status(&scan_key)
            .unwrap()
            .unwrap();
        assert_eq!(second.state, MaintenanceRunState::Preserved);
        assert_eq!(second.phase, "discovery_incomplete");
        assert!(
            second
                .terminal_reason
                .unwrap()
                .contains("another-invalid-writer")
        );
    }

    #[cfg(unix)]
    #[test]
    fn legacy_root_enumeration_failure_terminalizes_and_retries_next_epoch() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let event_root = root.path().join(EVENT_STORE_RELATIVE_PATH);
        let writers = event_root.join("writers");
        fs::create_dir_all(&writers).unwrap();
        fs::set_permissions(&writers, fs::Permissions::from_mode(0o000)).unwrap();

        let first = run_event_store_opportunity(
            root.path(),
            27,
            owner(27),
            OpportunityWorkLimits::default(),
        )
        .unwrap();
        fs::set_permissions(&writers, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(first, OpportunityOutcome::Preserved);

        let scan_key =
            MaintenanceJobKey::new(MaintenanceJobKind::EventDiscovery, "event-store-v1").unwrap();
        let failed_opportunity = MaintenanceJobStore::open(root.path())
            .unwrap()
            .read_status(&scan_key)
            .unwrap()
            .unwrap();
        assert_eq!(failed_opportunity.state, MaintenanceRunState::Preserved);
        assert!(
            failed_opportunity
                .terminal_reason
                .unwrap()
                .contains("legacy writers enumeration")
        );

        assert_eq!(
            run_event_store_opportunity(
                root.path(),
                28,
                owner(28),
                OpportunityWorkLimits::default(),
            )
            .unwrap(),
            OpportunityOutcome::Completed
        );
    }

    #[test]
    fn orphan_cancellation_after_disposition_and_unlink_is_restartable() {
        for cancel_stage in ["disposition", "unlink:events.sqlite3"] {
            let root = tempfile::tempdir().unwrap();
            let event_root = root.path().join(EVENT_STORE_RELATIVE_PATH);
            let manifest = dead_manifest(
                37,
                if cancel_stage == "disposition" {
                    47
                } else {
                    48
                },
            );
            let layout = publish_empty_unheaded(&event_root, &manifest);
            let partition = format!(
                "{}/{}",
                id_hex(manifest.writer_instance_id.as_bytes()),
                id_hex(manifest.generation_id.as_bytes())
            );
            let key = MaintenanceJobKey::new(MaintenanceJobKind::OrphanClassification, partition)
                .unwrap();
            let store = MaintenanceJobStore::open(root.path()).unwrap();
            let expected = cancel_stage.to_string();
            with_orphan_test_hook(
                std::sync::Arc::new(move |stage, _| {
                    if stage == expected {
                        store
                            .request_admitted_cancellation(&key, 27, now_unix_micros())
                            .unwrap();
                    }
                }),
                || {
                    run_event_store_opportunity(
                        root.path(),
                        27,
                        owner(27),
                        OpportunityWorkLimits::default(),
                    )
                    .unwrap();
                },
            );
            assert!(disposition_path(&event_root, &manifest).is_file());
            if cancel_stage == "disposition" {
                assert!(layout.generation_dir(manifest.generation_id).is_dir());
            } else {
                assert!(
                    event_root
                        .join("retirement/orphan-trash")
                        .join(id_hex(manifest.writer_instance_id.as_bytes()))
                        .join(format!(
                            "{}.pending",
                            id_hex(manifest.generation_id.as_bytes())
                        ))
                        .is_dir()
                );
            }
            for epoch in 28..31 {
                run_event_store_opportunity(
                    root.path(),
                    epoch,
                    owner(epoch),
                    OpportunityWorkLimits::default(),
                )
                .unwrap();
                if !layout.generation_dir(manifest.generation_id).exists()
                    && !event_root
                        .join("retirement/orphan-trash")
                        .join(id_hex(manifest.writer_instance_id.as_bytes()))
                        .join(format!(
                            "{}.pending",
                            id_hex(manifest.generation_id.as_bytes())
                        ))
                        .exists()
                {
                    break;
                }
            }
            assert!(!layout.generation_dir(manifest.generation_id).exists());
        }
    }

    #[test]
    fn supported_pre_head_interruption_states_have_production_recovery_successors() {
        #[derive(Clone, Copy)]
        enum Boundary {
            InvalidIntentSlot,
            IntentDurableBeforeStagingCreate,
            StagingDirectoryCreated,
            SuccessorCheckpointedSyncedAndClosed,
            PreparedManifestRenamedBeforeStagingSync,
            PublishingPhaseDurable,
            GenerationDirectoryRenamedBeforeParentSync,
        }

        for (index, boundary) in [
            Boundary::InvalidIntentSlot,
            Boundary::IntentDurableBeforeStagingCreate,
            Boundary::StagingDirectoryCreated,
            Boundary::SuccessorCheckpointedSyncedAndClosed,
            Boundary::PreparedManifestRenamedBeforeStagingSync,
            Boundary::PublishingPhaseDurable,
            Boundary::GenerationDirectoryRenamedBeforeParentSync,
        ]
        .into_iter()
        .enumerate()
        {
            let root = tempfile::tempdir().unwrap();
            let event_root = root.path().join(EVENT_STORE_RELATIVE_PATH);
            let manifest = dead_manifest(50 + index as u8, 70 + index as u8);
            let layout =
                WriterLayout::create(&event_root, *manifest.writer_instance_id.as_bytes()).unwrap();
            let lease = layout
                .acquire_generation_lease(manifest.generation_id)
                .unwrap();
            let staging_name =
                format!("{}.boundary.tmp", id_hex(manifest.generation_id.as_bytes()));
            crate::event_store::maintenance_discovery::publish_generation_intent(
                &event_root,
                &manifest,
                &staging_name,
            )
            .unwrap();
            if matches!(boundary, Boundary::InvalidIntentSlot) {
                let mut leaf = event_root.join("maintenance-discovery-v1/active");
                for byte in manifest
                    .writer_instance_id
                    .as_bytes()
                    .iter()
                    .chain(manifest.generation_id.as_bytes())
                {
                    leaf.push(format!("{byte:02x}"));
                }
                fs::write(leaf.join("status.0"), b"torn initial slot").unwrap();
            }
            let staging = layout.writer_dir().join("staging").join(&staging_name);
            if !matches!(
                boundary,
                Boundary::InvalidIntentSlot | Boundary::IntentDurableBeforeStagingCreate
            ) {
                fs::create_dir(&staging).unwrap();
                crate::event_store::maintenance_discovery::record_generation_phase(
                    &event_root,
                    manifest.writer_instance_id,
                    manifest.generation_id,
                    DiscoveryPhase::Staging,
                )
                .unwrap();
            }
            if matches!(
                boundary,
                Boundary::SuccessorCheckpointedSyncedAndClosed
                    | Boundary::PreparedManifestRenamedBeforeStagingSync
                    | Boundary::PublishingPhaseDurable
                    | Boundary::GenerationDirectoryRenamedBeforeParentSync
            ) {
                let mut connection =
                    Connection::open(staging.join(crate::event_store::DATABASE_FILE_NAME)).unwrap();
                initialize_generation_schema(
                    &mut connection,
                    &manifest.generation_metadata().unwrap(),
                )
                .unwrap();
                let checkpoint: (i64, i64, i64) = connection
                    .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                    })
                    .unwrap();
                assert_eq!(checkpoint.0, 0);
                assert_eq!(checkpoint.1, checkpoint.2);
                drop(connection);
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(staging.join(crate::event_store::DATABASE_FILE_NAME))
                    .unwrap()
                    .sync_all()
                    .unwrap();
            }
            if matches!(
                boundary,
                Boundary::PreparedManifestRenamedBeforeStagingSync
                    | Boundary::PublishingPhaseDurable
                    | Boundary::GenerationDirectoryRenamedBeforeParentSync
            ) {
                let mut bytes = serde_json::to_vec(&manifest).unwrap();
                bytes.push(b'\n');
                let prepared = staging.join(crate::event_store::PREPARED_MANIFEST_FILE_NAME);
                let temp = staging.join("prepared.manifest.json.boundary.tmp");
                let mut file = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&temp)
                    .unwrap();
                file.write_all(&bytes).unwrap();
                file.sync_all().unwrap();
                drop(file);
                fs::rename(temp, prepared).unwrap();
            }
            if matches!(
                boundary,
                Boundary::PublishingPhaseDurable
                    | Boundary::GenerationDirectoryRenamedBeforeParentSync
            ) {
                crate::event_store::maintenance_discovery::record_generation_phase(
                    &event_root,
                    manifest.writer_instance_id,
                    manifest.generation_id,
                    DiscoveryPhase::Publishing,
                )
                .unwrap();
            }
            if matches!(
                boundary,
                Boundary::GenerationDirectoryRenamedBeforeParentSync
            ) {
                fs::rename(&staging, layout.generation_dir(manifest.generation_id)).unwrap();
            }
            drop(lease);

            run_event_store_opportunity(
                root.path(),
                19,
                owner(50 + index as i64),
                OpportunityWorkLimits::default(),
            )
            .unwrap();
            if matches!(
                boundary,
                Boundary::InvalidIntentSlot | Boundary::IntentDurableBeforeStagingCreate
            ) {
                let key = MaintenanceJobKey::new(
                    MaintenanceJobKind::OrphanClassification,
                    format!(
                        "{}/{}",
                        id_hex(manifest.writer_instance_id.as_bytes()),
                        id_hex(manifest.generation_id.as_bytes())
                    ),
                )
                .unwrap();
                assert_eq!(
                    MaintenanceJobStore::open(root.path())
                        .unwrap()
                        .read_status(&key)
                        .unwrap()
                        .unwrap()
                        .phase,
                    "absent_intent_discharged"
                );
            } else {
                assert!(disposition_path(&event_root, &manifest).is_file());
                assert!(!staging.exists());
                assert!(!layout.generation_dir(manifest.generation_id).exists());
            }
        }
    }

    #[test]
    fn discovery_intent_without_any_source_is_boundedly_discharged() {
        let root = tempfile::tempdir().unwrap();
        let event_root = root.path().join(EVENT_STORE_RELATIVE_PATH);
        let manifest = dead_manifest(33, 43);
        let layout =
            WriterLayout::create(&event_root, *manifest.writer_instance_id.as_bytes()).unwrap();
        let lease = layout
            .acquire_generation_lease(manifest.generation_id)
            .unwrap();
        crate::event_store::maintenance_discovery::publish_generation_intent(
            &event_root,
            &manifest,
            "never-created.tmp",
        )
        .unwrap();
        drop(lease);

        run_event_store_opportunity(root.path(), 18, owner(33), OpportunityWorkLimits::default())
            .unwrap();
        let key = MaintenanceJobKey::new(
            MaintenanceJobKind::OrphanClassification,
            format!(
                "{}/{}",
                id_hex(manifest.writer_instance_id.as_bytes()),
                id_hex(manifest.generation_id.as_bytes())
            ),
        )
        .unwrap();
        assert_eq!(
            MaintenanceJobStore::open(root.path())
                .unwrap()
                .read_status(&key)
                .unwrap()
                .unwrap()
                .phase,
            "absent_intent_discharged"
        );
    }

    #[test]
    fn legacy_migration_cursor_resumes_across_process_and_day_boundary() {
        let root = tempfile::tempdir().unwrap();
        let event_root = root.path().join(EVENT_STORE_RELATIVE_PATH);
        let manifest = dead_manifest(34, 44);
        let layout = publish_empty_unheaded(&event_root, &manifest);
        fs::remove_dir_all(event_root.join("maintenance-discovery-v1")).unwrap();
        assert!(layout.generation_dir(manifest.generation_id).is_dir());

        for epoch in 20..32 {
            let status = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "detached_maintenance::tests::subprocess_event_opportunity_helper",
                    "--nocapture",
                ])
                .env("AGE377_OPPORTUNITY_ROOT", root.path())
                .env("AGE377_OPPORTUNITY_EPOCH", epoch.to_string())
                .status()
                .unwrap();
            assert!(status.success());
            if disposition_path(&event_root, &manifest).is_file() {
                break;
            }
        }
        assert!(disposition_path(&event_root, &manifest).is_file());
        assert!(!layout.generation_dir(manifest.generation_id).exists());
    }

    #[test]
    fn orphan_restart_consumes_stable_disposition_after_process_death_at_each_boundary() {
        for (index, stage) in [
            "disposition",
            "unlink:events.sqlite3",
            "unlink:prepared.manifest.json",
        ]
        .into_iter()
        .enumerate()
        {
            let root = tempfile::tempdir().unwrap();
            let event_root = root.path().join(EVENT_STORE_RELATIVE_PATH);
            let manifest = dead_manifest(60 + index as u8, 70 + index as u8);
            let layout = publish_empty_unheaded(&event_root, &manifest);
            let ready = root.path().join("orphan-crash-ready");
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "detached_maintenance::tests::subprocess_orphan_crash_helper",
                    "--nocapture",
                ])
                .env("AGE377_ORPHAN_CRASH_ROOT", root.path())
                .env("AGE377_ORPHAN_CRASH_READY", &ready)
                .env("AGE377_ORPHAN_CRASH_STAGE", stage)
                .spawn()
                .unwrap();
            let started = Instant::now();
            while !ready.exists() && started.elapsed() < Duration::from_secs(10) {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(ready.exists(), "orphan crash fixture did not reach {stage}");
            child.kill().unwrap();
            child.wait().unwrap();

            for epoch in 33..38 {
                run_event_store_opportunity(
                    root.path(),
                    epoch,
                    owner(epoch),
                    OpportunityWorkLimits::default(),
                )
                .unwrap();
                let pending = event_root
                    .join("retirement/orphan-trash")
                    .join(id_hex(manifest.writer_instance_id.as_bytes()))
                    .join(format!(
                        "{}.pending",
                        id_hex(manifest.generation_id.as_bytes())
                    ));
                if !layout.generation_dir(manifest.generation_id).exists() && !pending.exists() {
                    break;
                }
            }
            assert!(disposition_path(&event_root, &manifest).is_file());
            assert!(!layout.generation_dir(manifest.generation_id).exists());
        }
    }

    #[test]
    fn subprocess_orphan_crash_helper() {
        let Some(root) = std::env::var_os("AGE377_ORPHAN_CRASH_ROOT") else {
            return;
        };
        let ready =
            std::path::PathBuf::from(std::env::var_os("AGE377_ORPHAN_CRASH_READY").unwrap());
        let expected = std::env::var("AGE377_ORPHAN_CRASH_STAGE").unwrap();
        with_orphan_test_hook(
            std::sync::Arc::new(move |stage, _| {
                if stage == expected {
                    fs::write(&ready, b"ready").unwrap();
                    std::thread::sleep(Duration::from_secs(30));
                }
            }),
            || {
                run_event_store_opportunity(
                    Path::new(&root),
                    32,
                    owner(32),
                    OpportunityWorkLimits::default(),
                )
                .unwrap();
            },
        );
    }

    #[test]
    fn subprocess_event_opportunity_helper() {
        let Ok(root) = std::env::var("AGE377_OPPORTUNITY_ROOT") else {
            return;
        };
        let epoch = std::env::var("AGE377_OPPORTUNITY_EPOCH")
            .unwrap()
            .parse()
            .unwrap();
        run_event_store_opportunity(
            Path::new(&root),
            epoch,
            owner(epoch),
            OpportunityWorkLimits {
                max_discovery_nodes: 8,
                max_partitions_per_run: 1,
                ..OpportunityWorkLimits::default()
            },
        )
        .unwrap();
    }

    #[test]
    fn retention_snapshot_survives_a_post_batch_pre_checkpoint_crash() {
        struct CrashAfterBatch;
        impl Drop for CrashAfterBatch {
            fn drop(&mut self) {
                panic!("simulated crash after retention mutation");
            }
        }
        let root = tempfile::tempdir().unwrap();
        let store = MaintenanceJobStore::open(root.path()).unwrap();
        let mut first_summary = CoordinationRetentionOutcome {
            jobs_completed: 0,
            jobs_yielded: 0,
            duplicate_jobs: 0,
            gaps: Vec::new(),
        };
        let mutation_ran = std::cell::Cell::new(false);
        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = run_retention_family(
                &store,
                61,
                owner(61),
                1,
                "state",
                RetentionFamily::Invocation,
                || Ok(CrashAfterBatch),
                |_, request| {
                    mutation_ran.set(true);
                    let cutoff = request
                        .policy
                        .cutoff_unix_micros(request.family, request.as_of_unix_micros)
                        .unwrap();
                    let mut outcome = RetentionBatchOutcome::new(request, cutoff);
                    outcome.status = RetentionBatchStatus::MoreWork;
                    Ok(outcome)
                },
                &mut first_summary,
            );
        }));
        assert!(crashed.is_err());
        assert!(mutation_ran.get());
        let key =
            MaintenanceJobKey::new(MaintenanceJobKind::StateRetention, "state/invocation").unwrap();
        let checkpoint = store.read_status(&key).unwrap().unwrap();
        assert_eq!(checkpoint.state, MaintenanceRunState::Running);
        assert_eq!(checkpoint.phase, "retention_snapshot");
        let frozen: RetentionResume =
            serde_json::from_str(checkpoint.cursor.as_deref().unwrap()).unwrap();
        let mut resumed_as_of = None;
        let mut second_summary = CoordinationRetentionOutcome {
            jobs_completed: 0,
            jobs_yielded: 0,
            duplicate_jobs: 0,
            gaps: Vec::new(),
        };
        run_retention_family(
            &store,
            62,
            owner(62),
            1,
            "state",
            RetentionFamily::Invocation,
            || Ok(()),
            |_, request| {
                resumed_as_of = Some(request.as_of_unix_micros);
                let cutoff = request
                    .policy
                    .cutoff_unix_micros(request.family, request.as_of_unix_micros)
                    .unwrap();
                Ok(RetentionBatchOutcome::new(request, cutoff))
            },
            &mut second_summary,
        )
        .unwrap();
        assert_eq!(resumed_as_of, Some(frozen.as_of_unix_micros));
        assert_eq!(second_summary.jobs_completed, 1);
    }
}
