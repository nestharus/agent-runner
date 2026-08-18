//! ## Declared roles
//!
//! `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `predicate`
//!
//! Invocation subtree projection for monitor snapshots.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/observability/invocation.rs
//!     role: adapter
//!     Translates:
//!       - observability monitor-node projection contract
//!       - StateDb invocation-record read contract
//!       - PidIdentity exact-process-identity contract
//!       - Rust standard collection contract
//! ```
//!
//! This module adapts durable invocation and process-identity records into the
//! monitor-node projection. Candidate ordering never establishes liveness;
//! exact process identity remains authoritative.

use crate::observability::dto::{
    CancelRef, InspectRef, LivenessStatus, MonitorDiagnostic, MonitorDiagnosticSeverity,
    MonitorNode, MonitorNodeKind, MonitorStatus,
};
use crate::observability::limits::SnapshotLimits;
use crate::observability::liveness::pid_row_liveness;
use crate::observability::service::ObservabilityRoot;
use crate::observability::state_access::{process_identity_ref, storage_diagnostic};
use oulipoly_provider::client::CancellationToken;
use oulipoly_state::mailbox::MailboxDb;
use oulipoly_state::pid_identity::{PidIdentityDb, PidIdentityRow};
use oulipoly_state::{InvocationRecord, InvocationStatus, StateDb};
use std::collections::HashSet;

pub(crate) struct InvocationProjection {
    pub(crate) root_invocation_uuid: Option<String>,
    pub(crate) active_session_id: Option<String>,
    pub(crate) invocation_uuids: HashSet<String>,
    pub(crate) nodes: Vec<MonitorNode>,
    pub(crate) diagnostics: Vec<MonitorDiagnostic>,
    pub(crate) invocation_count: usize,
}

pub(crate) fn project_invocations(
    state: Option<&StateDb>,
    pid: Option<&PidIdentityDb>,
    mailbox: Option<&MailboxDb>,
    root: &ObservabilityRoot,
    limits: SnapshotLimits,
    cancellation: &CancellationToken,
) -> InvocationProjection {
    let mut diagnostics = Vec::new();
    let Some(state) = state else {
        return empty_projection(root);
    };
    if cancellation.is_cancelled() {
        return empty_projection(root);
    }
    let root_record = match root_invocation_record(state, root) {
        Ok(record) => record,
        Err(err) => {
            diagnostics.push(storage_diagnostic("invocation:root-read", err));
            None
        }
    };
    let active_session_id = active_session_id(root, root_record.as_ref());
    let root_invocation_uuid = projection_root_invocation_uuid(root, root_record.as_ref());
    let mut projection =
        invocation_projection(root_invocation_uuid, active_session_id.clone(), diagnostics);
    if let Some(record) = root_record {
        build_invocation_tree(
            state,
            pid,
            mailbox,
            record,
            None,
            active_session_id,
            limits,
            cancellation,
            &mut projection,
        );
    }
    projection
}

fn projection_root_invocation_uuid(
    root: &ObservabilityRoot,
    record: Option<&InvocationRecord>,
) -> Option<String> {
    record
        .map(|record| record.invocation_uuid.clone())
        .or_else(|| root.invocation_uuid.clone())
}

fn invocation_projection(
    root_invocation_uuid: Option<String>,
    active_session_id: Option<String>,
    diagnostics: Vec<MonitorDiagnostic>,
) -> InvocationProjection {
    InvocationProjection {
        root_invocation_uuid,
        active_session_id,
        invocation_uuids: HashSet::new(),
        nodes: Vec::new(),
        diagnostics,
        invocation_count: 0,
    }
}

fn empty_projection(root: &ObservabilityRoot) -> InvocationProjection {
    InvocationProjection {
        root_invocation_uuid: root.invocation_uuid.clone(),
        active_session_id: root.session_id.clone(),
        invocation_uuids: HashSet::new(),
        nodes: Vec::new(),
        diagnostics: Vec::new(),
        invocation_count: 0,
    }
}

fn root_invocation_record(
    state: &StateDb,
    root: &ObservabilityRoot,
) -> Result<Option<InvocationRecord>, String> {
    if let Some(uuid) = root.invocation_uuid.as_deref() {
        return state.get_invocation_by_uuid(uuid);
    }
    Ok(None)
}

fn active_session_id(
    root: &ObservabilityRoot,
    record: Option<&InvocationRecord>,
) -> Option<String> {
    root.session_id
        .clone()
        .or_else(|| record.and_then(resolved_invocation_session_id))
}

pub(crate) fn resolved_invocation_session_id(record: &InvocationRecord) -> Option<String> {
    record
        .provider_session_id
        .clone()
        .or_else(|| record.session_id.clone())
}

#[allow(clippy::too_many_arguments)]
fn build_invocation_tree(
    state: &StateDb,
    pid: Option<&PidIdentityDb>,
    mailbox: Option<&MailboxDb>,
    record: InvocationRecord,
    parent_uuid: Option<String>,
    active_session_id: Option<String>,
    limits: SnapshotLimits,
    cancellation: &CancellationToken,
    projection: &mut InvocationProjection,
) {
    let mut visited = HashSet::from([record.id]);
    build_invocation_node(
        state,
        pid,
        mailbox,
        record,
        parent_uuid,
        active_session_id,
        0,
        limits,
        cancellation,
        &mut visited,
        projection,
    );
}

#[allow(clippy::too_many_arguments)]
fn build_invocation_node(
    state: &StateDb,
    pid: Option<&PidIdentityDb>,
    mailbox: Option<&MailboxDb>,
    record: InvocationRecord,
    parent_uuid: Option<String>,
    active_session_id: Option<String>,
    depth: usize,
    limits: SnapshotLimits,
    cancellation: &CancellationToken,
    visited: &mut HashSet<i64>,
    projection: &mut InvocationProjection,
) {
    if cancellation.is_cancelled() {
        return;
    }
    if invocation_node_limit_reached(projection, limits) {
        push_invocation_truncated_diagnostic(projection);
        return;
    }
    record_invocation_entry(projection, &record);
    let selected_pid = selected_pid_row(pid, &record.invocation_uuid, &mut projection.diagnostics);
    push_invocation_record_node(
        projection,
        &record,
        parent_uuid.as_deref(),
        active_session_id.as_deref(),
        &selected_pid,
    );
    push_selected_process_node(projection, &record, selected_pid);
    if !should_read_invocation_children(depth, limits) {
        return;
    }
    let Some(children) = invocation_children(
        state,
        mailbox,
        &record,
        limits.include_terminal,
        limits
            .max_invocation_nodes
            .saturating_sub(projection.invocation_count),
        cancellation,
        &mut projection.diagnostics,
    ) else {
        return;
    };
    visit_invocation_children(
        state,
        pid,
        mailbox,
        record.invocation_uuid.clone(),
        active_session_id,
        depth,
        limits,
        cancellation,
        visited,
        projection,
        children,
    );
}

fn invocation_node_limit_reached(
    projection: &InvocationProjection,
    limits: SnapshotLimits,
) -> bool {
    projection.invocation_count >= limits.max_invocation_nodes
}

fn push_invocation_truncated_diagnostic(projection: &mut InvocationProjection) {
    projection
        .diagnostics
        .push(invocation_truncated_diagnostic());
}

fn record_invocation_entry(projection: &mut InvocationProjection, record: &InvocationRecord) {
    projection.invocation_count += 1;
    projection
        .invocation_uuids
        .insert(record.invocation_uuid.clone());
}

fn push_invocation_record_node(
    projection: &mut InvocationProjection,
    record: &InvocationRecord,
    parent_uuid: Option<&str>,
    active_session_id: Option<&str>,
    selected_pid: &Option<(PidIdentityRow, LivenessStatus)>,
) {
    projection.nodes.push(invocation_node(
        record,
        parent_uuid,
        active_session_id,
        selected_pid,
    ));
}

fn push_selected_process_node(
    projection: &mut InvocationProjection,
    record: &InvocationRecord,
    selected_pid: Option<(PidIdentityRow, LivenessStatus)>,
) {
    if let Some((row, liveness)) = selected_pid {
        projection.nodes.push(process_node(record, &row, liveness));
    }
}

fn should_read_invocation_children(depth: usize, limits: SnapshotLimits) -> bool {
    depth < limits.invocation_subtree_depth
}

fn invocation_children(
    state: &StateDb,
    mailbox: Option<&MailboxDb>,
    record: &InvocationRecord,
    include_terminal: bool,
    remaining_nodes: usize,
    cancellation: &CancellationToken,
    diagnostics: &mut Vec<MonitorDiagnostic>,
) -> Option<Vec<InvocationRecord>> {
    match read_invocation_children(
        state,
        record.id,
        remaining_nodes.saturating_add(1),
        !include_terminal,
    ) {
        Ok(mut children) => {
            if cancellation.is_cancelled() {
                return None;
            }
            append_delivery_invocation_children(
                state,
                mailbox,
                record,
                remaining_nodes,
                cancellation,
                &mut children,
                diagnostics,
            );
            order_invocation_children(&mut children, include_terminal);
            Some(children)
        }
        Err(err) => {
            push_invocation_children_read_diagnostic(diagnostics, err);
            None
        }
    }
}

fn append_delivery_invocation_children(
    state: &StateDb,
    mailbox: Option<&MailboxDb>,
    record: &InvocationRecord,
    limit: usize,
    cancellation: &CancellationToken,
    children: &mut Vec<InvocationRecord>,
    diagnostics: &mut Vec<MonitorDiagnostic>,
) {
    let Some(mailbox) = mailbox else {
        return;
    };
    let child_uuids =
        match mailbox.list_delivery_invocation_children(&record.invocation_uuid, limit) {
            Ok(uuids) => uuids,
            Err(err) => {
                diagnostics.push(storage_diagnostic("invocation:delivery-children-read", err));
                return;
            }
        };
    let mut child_ids = children
        .iter()
        .map(|child| child.id)
        .collect::<HashSet<_>>();
    for uuid in child_uuids {
        if cancellation.is_cancelled() {
            return;
        }
        match state.get_invocation_by_uuid(&uuid) {
            Ok(Some(child)) if child.id != record.id && child_ids.insert(child.id) => {
                children.push(child);
            }
            Ok(_) => {}
            Err(err) => {
                diagnostics.push(storage_diagnostic("invocation:delivery-child-read", err));
            }
        }
    }
}

fn read_invocation_children(
    state: &StateDb,
    record_id: i64,
    limit: usize,
    prioritize_running: bool,
) -> Result<Vec<InvocationRecord>, String> {
    state.list_invocation_children_bounded(record_id, limit, prioritize_running)
}

fn order_invocation_children(children: &mut [InvocationRecord], include_terminal: bool) {
    if include_terminal {
        return;
    }
    prioritize_running_candidates(children);
}

fn prioritize_running_candidates(children: &mut [InvocationRecord]) {
    children.sort_by_key(|record| record.status != InvocationStatus::Running);
}

fn push_invocation_children_read_diagnostic(
    diagnostics: &mut Vec<MonitorDiagnostic>,
    message: String,
) {
    diagnostics.push(storage_diagnostic("invocation:children-read", message));
}

#[allow(clippy::too_many_arguments)]
fn visit_invocation_children(
    state: &StateDb,
    pid: Option<&PidIdentityDb>,
    mailbox: Option<&MailboxDb>,
    parent_invocation_uuid: String,
    active_session_id: Option<String>,
    depth: usize,
    limits: SnapshotLimits,
    cancellation: &CancellationToken,
    visited: &mut HashSet<i64>,
    projection: &mut InvocationProjection,
    children: Vec<InvocationRecord>,
) {
    for child in children {
        if cancellation.is_cancelled() {
            return;
        }
        if child_was_visited(visited, child.id) {
            continue;
        }
        let child_id = child.id;
        remember_visited_child(visited, child_id);
        build_invocation_node(
            state,
            pid,
            mailbox,
            child,
            Some(parent_invocation_uuid.clone()),
            active_session_id.clone(),
            depth + 1,
            limits,
            cancellation,
            visited,
            projection,
        );
        forget_visited_child(visited, child_id);
    }
}

fn child_was_visited(visited: &HashSet<i64>, child_id: i64) -> bool {
    visited.contains(&child_id)
}

fn remember_visited_child(visited: &mut HashSet<i64>, child_id: i64) {
    visited.insert(child_id);
}

fn forget_visited_child(visited: &mut HashSet<i64>, child_id: i64) {
    visited.remove(&child_id);
}

fn selected_pid_row(
    pid: Option<&PidIdentityDb>,
    invocation_uuid: &str,
    diagnostics: &mut Vec<MonitorDiagnostic>,
) -> Option<(PidIdentityRow, LivenessStatus)> {
    let pid = pid?;
    let rows = match read_pid_rows_for_invocation(pid, invocation_uuid) {
        Ok(rows) => rows,
        Err(err) => {
            diagnostics.push(pid_invocation_lookup_diagnostic(err));
            return None;
        }
    };
    select_pid_row(&rows)
}

fn read_pid_rows_for_invocation(
    pid: &PidIdentityDb,
    invocation_uuid: &str,
) -> Result<Vec<PidIdentityRow>, String> {
    pid.lookup_by_invocation_uuid(invocation_uuid)
}

fn pid_invocation_lookup_diagnostic(message: String) -> MonitorDiagnostic {
    storage_diagnostic("pid-identity:invocation-lookup", message)
}

fn select_pid_row(rows: &[PidIdentityRow]) -> Option<(PidIdentityRow, LivenessStatus)> {
    live_selected_pid_row(rows).or_else(|| fallback_selected_pid_row(rows))
}

fn live_selected_pid_row(rows: &[PidIdentityRow]) -> Option<(PidIdentityRow, LivenessStatus)> {
    rows.iter().find_map(live_selected_row)
}

fn fallback_selected_pid_row(rows: &[PidIdentityRow]) -> Option<(PidIdentityRow, LivenessStatus)> {
    rows.first().map(fallback_selected_row)
}

fn live_selected_row(row: &PidIdentityRow) -> Option<(PidIdentityRow, LivenessStatus)> {
    let liveness = pid_row_liveness(row);
    if !liveness_is_verified_live(liveness) {
        return None;
    }
    Some(selected_row_tuple(row, liveness))
}

fn liveness_is_verified_live(liveness: LivenessStatus) -> bool {
    liveness == LivenessStatus::VerifiedLive
}

fn selected_row_tuple(
    row: &PidIdentityRow,
    liveness: LivenessStatus,
) -> (PidIdentityRow, LivenessStatus) {
    (row.clone(), liveness)
}

fn fallback_selected_row(row: &PidIdentityRow) -> (PidIdentityRow, LivenessStatus) {
    selected_row_tuple(row, pid_row_liveness(row))
}

fn invocation_node(
    record: &InvocationRecord,
    parent_uuid: Option<&str>,
    active_session_id: Option<&str>,
    selected_pid: &Option<(PidIdentityRow, LivenessStatus)>,
) -> MonitorNode {
    let parent_id = parent_uuid
        .map(invocation_node_id)
        .or_else(|| active_session_id.map(session_node_id));
    let node_liveness = selected_pid
        .as_ref()
        .map(|(_, liveness)| *liveness)
        .unwrap_or(LivenessStatus::Unknown);
    MonitorNode {
        id: invocation_node_id(&record.invocation_uuid),
        parent_id,
        kind: MonitorNodeKind::Invocation,
        label: invocation_label(record),
        status: invocation_status(record.status, node_liveness),
        pid: selected_pid.as_ref().map(|(row, _)| row.os_pid),
        pgid: selected_pid.as_ref().and_then(|(row, _)| row.os_pgid),
        liveness: node_liveness,
        started_at: Some(record.created_at.to_rfc3339()),
        updated_at: None,
        completed_at: record.finished_at.map(|value| value.to_rfc3339()),
        last_output_excerpt: None,
        inspect_ref: Some(InspectRef::InvocationStatus {
            uuid: record.invocation_uuid.clone(),
        }),
        cancel_ref: None,
        wake: None,
        mailbox: None,
    }
}

fn process_node(
    record: &InvocationRecord,
    row: &PidIdentityRow,
    liveness: LivenessStatus,
) -> MonitorNode {
    MonitorNode {
        id: process_node_id(&record.invocation_uuid, row.os_pid),
        parent_id: Some(invocation_node_id(&record.invocation_uuid)),
        kind: MonitorNodeKind::ProviderProcess,
        label: process_label(row.os_pid),
        status: process_status(liveness),
        pid: Some(row.os_pid),
        pgid: row.os_pgid,
        liveness,
        started_at: Some(row.recorded_at.clone()),
        updated_at: None,
        completed_at: None,
        last_output_excerpt: None,
        inspect_ref: None,
        cancel_ref: row.os_pgid.map(|pgid| CancelRef::ProcessGroup {
            pgid,
            identity: Some(process_identity_ref(&row.identity())),
        }),
        wake: None,
        mailbox: None,
    }
}

fn process_label(pid: i64) -> String {
    format!("provider process pid {pid}")
}

pub(crate) fn session_node_id(session_id: &str) -> String {
    format!("session:{session_id}")
}

pub(crate) fn invocation_node_id(uuid: &str) -> String {
    format!("invocation:{uuid}")
}

fn process_node_id(invocation_uuid: &str, pid: i64) -> String {
    format!("process:{invocation_uuid}:{pid}")
}

fn invocation_label(record: &InvocationRecord) -> String {
    match record.provider_name.as_deref() {
        Some(provider) => format!("{} via {provider}", record.model_name),
        None => record.model_name.clone(),
    }
}

/// Reconcile the recorded invocation status against process liveness: a
/// `Running` row whose process is not verifiably alive is stale, not running
/// (the invocation died without finalizing its status).
fn invocation_status(status: InvocationStatus, liveness: LivenessStatus) -> MonitorStatus {
    match status {
        InvocationStatus::Running => match liveness {
            LivenessStatus::VerifiedLive | LivenessStatus::UnverifiedLive => MonitorStatus::Running,
            _ => MonitorStatus::Stale,
        },
        InvocationStatus::Succeeded => MonitorStatus::Succeeded,
        InvocationStatus::Failed => MonitorStatus::Failed,
        InvocationStatus::Legacy => MonitorStatus::Unknown,
    }
}

fn process_status(liveness: LivenessStatus) -> MonitorStatus {
    match liveness {
        LivenessStatus::VerifiedLive | LivenessStatus::UnverifiedLive => MonitorStatus::Running,
        LivenessStatus::Dead | LivenessStatus::PidReused => MonitorStatus::Stale,
        LivenessStatus::Unknown | LivenessStatus::NotApplicable => MonitorStatus::Unknown,
    }
}

fn invocation_truncated_diagnostic() -> MonitorDiagnostic {
    MonitorDiagnostic {
        code: "truncated:invocation-nodes".to_string(),
        severity: MonitorDiagnosticSeverity::Warning,
        message: "invocation subtree exceeded the snapshot node cap".to_string(),
        node_id: None,
    }
}
