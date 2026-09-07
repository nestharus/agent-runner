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
use oulipoly_core::CancellationToken;
use oulipoly_state::mailbox::MailboxDb;
use oulipoly_state::pid_identity::{PidIdentityDb, PidIdentityRow};
use oulipoly_state::{InvocationRecord, InvocationStatus, StateDb};
use std::collections::{HashMap, HashSet, VecDeque};

pub(crate) struct InvocationProjection {
    pub(crate) root_invocation_uuid: Option<String>,
    pub(crate) active_session_id: Option<String>,
    pub(crate) invocation_uuids: HashSet<String>,
    pub(crate) nodes: Vec<MonitorNode>,
    pub(crate) diagnostics: Vec<MonitorDiagnostic>,
    pub(crate) invocation_count: usize,
    pub(crate) live_coverage_incomplete: bool,
}

pub(crate) fn project_invocations(
    state: Option<&StateDb>,
    pid: Option<&PidIdentityDb>,
    mailbox: Option<&MailboxDb>,
    root: &ObservabilityRoot,
    agent_bash_owner_seeds: &HashSet<String>,
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
        project_root_reachable_graph(
            state,
            pid,
            mailbox,
            record,
            active_session_id,
            agent_bash_owner_seeds,
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
        live_coverage_incomplete: false,
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
        live_coverage_incomplete: false,
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

#[derive(Default)]
struct InvocationGraph {
    records: HashMap<String, InvocationRecord>,
    parents: HashMap<String, String>,
    mandatory: HashSet<String>,
}

struct StateSeedPath {
    records: Vec<InvocationRecord>,
}

#[allow(clippy::too_many_arguments)]
fn project_root_reachable_graph(
    state: &StateDb,
    pid: Option<&PidIdentityDb>,
    mailbox: Option<&MailboxDb>,
    root_record: InvocationRecord,
    active_session_id: Option<String>,
    agent_bash_owner_seeds: &HashSet<String>,
    limits: SnapshotLimits,
    cancellation: &CancellationToken,
    projection: &mut InvocationProjection,
) {
    let mut seeds = invocation_live_seeds(
        state,
        pid,
        mailbox,
        active_session_id.as_deref(),
        agent_bash_owner_seeds,
        cancellation,
        &mut projection.diagnostics,
    );
    seeds.insert(root_record.invocation_uuid.clone());
    let mut paths = seed_paths(state, seeds, cancellation, &mut projection.diagnostics);
    add_delivery_owner_paths(
        state,
        mailbox,
        cancellation,
        &mut paths,
        &mut projection.diagnostics,
    );
    let mut graph = root_reachable_state_graph(&root_record, &paths);
    attach_root_reachable_delivery_paths(
        mailbox,
        &paths,
        cancellation,
        &mut graph,
        &mut projection.diagnostics,
    );
    if limits.include_terminal {
        append_terminal_history(
            state,
            mailbox,
            limits.max_invocation_nodes,
            cancellation,
            &mut graph,
            &mut projection.diagnostics,
        );
    }
    project_invocation_graph(
        pid,
        &root_record.invocation_uuid,
        active_session_id.as_deref(),
        limits,
        graph,
        projection,
    );
}

fn invocation_live_seeds(
    state: &StateDb,
    pid: Option<&PidIdentityDb>,
    mailbox: Option<&MailboxDb>,
    session_id: Option<&str>,
    agent_bash_owner_seeds: &HashSet<String>,
    cancellation: &CancellationToken,
    diagnostics: &mut Vec<MonitorDiagnostic>,
) -> HashSet<String> {
    let mut seeds = agent_bash_owner_seeds.clone();
    match state.list_running_invocations_with_cancel(cancellation) {
        Ok(records) => seeds.extend(records.into_iter().map(|record| record.invocation_uuid)),
        Err(error) => diagnostics.push(storage_diagnostic("invocation:running-seeds-read", error)),
    }
    if let Some(pid) = pid {
        match pid.list_identities() {
            Ok(rows) => seeds.extend(rows.into_iter().filter_map(|row| {
                (pid_row_liveness(&row) == LivenessStatus::VerifiedLive)
                    .then_some(row.invocation_uuid)
            })),
            Err(error) => {
                diagnostics.push(storage_diagnostic("pid-identity:live-seeds-read", error))
            }
        }
    }
    if let (Some(mailbox), Some(session_id)) = (mailbox, session_id) {
        match mailbox.list_pending(session_id) {
            Ok(rows) => seeds.extend(rows.into_iter().filter_map(|row| row.owner_invocation_uuid)),
            Err(error) => diagnostics.push(storage_diagnostic(
                "mailbox:pending-owner-seeds-read",
                error,
            )),
        }
        match mailbox.wake_session_reader().wake_claim(session_id) {
            Ok(Some(claim)) => seeds.extend(claim.wake_invocation_uuid),
            Ok(None) => {}
            Err(error) => diagnostics.push(storage_diagnostic(
                "mailbox:wake-invocation-seed-read",
                error,
            )),
        }
    }
    seeds
}

fn seed_paths(
    state: &StateDb,
    seeds: HashSet<String>,
    cancellation: &CancellationToken,
    diagnostics: &mut Vec<MonitorDiagnostic>,
) -> Vec<StateSeedPath> {
    let mut paths = Vec::new();
    for seed in seeds {
        if cancellation.is_cancelled() {
            break;
        }
        if let Some(path) = state_seed_path(state, &seed, cancellation, diagnostics) {
            paths.push(path);
        }
    }
    paths
}

fn add_delivery_owner_paths(
    state: &StateDb,
    mailbox: Option<&MailboxDb>,
    cancellation: &CancellationToken,
    paths: &mut Vec<StateSeedPath>,
    diagnostics: &mut Vec<MonitorDiagnostic>,
) {
    let Some(mailbox) = mailbox else {
        return;
    };
    let edges = match mailbox.list_delivery_invocation_edges() {
        Ok(edges) => edges,
        Err(error) => {
            diagnostics.push(storage_diagnostic("invocation:delivery-edges-read", error));
            return;
        }
    };
    let mut known_seeds = paths
        .iter()
        .filter_map(|path| path.records.first())
        .map(|record| record.invocation_uuid.clone())
        .collect::<HashSet<_>>();
    loop {
        if cancellation.is_cancelled() {
            return;
        }
        let membership = path_membership(paths);
        let owners = edges
            .iter()
            .filter(|edge| membership.contains_key(&edge.delivered_by_invocation_uuid))
            .map(|edge| edge.owner_invocation_uuid.clone())
            .filter(|owner| known_seeds.insert(owner.clone()))
            .collect::<Vec<_>>();
        if owners.is_empty() {
            return;
        }
        for owner in owners {
            if let Some(path) = state_seed_path(state, &owner, cancellation, diagnostics) {
                paths.push(path);
            }
        }
    }
}

fn state_seed_path(
    state: &StateDb,
    seed: &str,
    cancellation: &CancellationToken,
    diagnostics: &mut Vec<MonitorDiagnostic>,
) -> Option<StateSeedPath> {
    let mut current = match state.get_invocation_by_uuid(seed) {
        Ok(record) => record?,
        Err(error) => {
            diagnostics.push(storage_diagnostic("invocation:seed-read", error));
            return None;
        }
    };
    let mut records = Vec::new();
    let mut visited = HashSet::new();
    loop {
        if cancellation.is_cancelled() {
            return None;
        }
        if !visited.insert(current.id) {
            diagnostics.push(invocation_cycle_diagnostic(&current.invocation_uuid));
            break;
        }
        let parent_id = current.parent_invocation_id;
        records.push(current);
        let Some(parent_id) = parent_id else {
            break;
        };
        current = match state.get_invocation_by_id(parent_id) {
            Ok(Some(parent)) => parent,
            Ok(None) => {
                diagnostics.push(invocation_missing_parent_diagnostic(
                    &records.last().unwrap().invocation_uuid,
                    parent_id,
                ));
                break;
            }
            Err(error) => {
                diagnostics.push(storage_diagnostic("invocation:ancestor-read", error));
                break;
            }
        };
    }
    Some(StateSeedPath { records })
}

fn root_reachable_state_graph(root: &InvocationRecord, paths: &[StateSeedPath]) -> InvocationGraph {
    let mut graph = InvocationGraph::default();
    graph
        .records
        .insert(root.invocation_uuid.clone(), root.clone());
    graph.mandatory.insert(root.invocation_uuid.clone());
    for path in paths {
        let Some(root_index) = path.records.iter().position(|record| record.id == root.id) else {
            continue;
        };
        include_state_path_segment(&mut graph, path, root_index);
    }
    graph
}

fn include_state_path_segment(graph: &mut InvocationGraph, path: &StateSeedPath, top: usize) {
    for index in 0..=top {
        let record = &path.records[index];
        graph
            .records
            .insert(record.invocation_uuid.clone(), record.clone());
        graph.mandatory.insert(record.invocation_uuid.clone());
        if index < top {
            graph.parents.insert(
                record.invocation_uuid.clone(),
                path.records[index + 1].invocation_uuid.clone(),
            );
        }
    }
}

fn path_membership(paths: &[StateSeedPath]) -> HashMap<String, Vec<(usize, usize)>> {
    let mut membership: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
    for (path_index, path) in paths.iter().enumerate() {
        for (record_index, record) in path.records.iter().enumerate() {
            membership
                .entry(record.invocation_uuid.clone())
                .or_default()
                .push((path_index, record_index));
        }
    }
    membership
}

#[allow(clippy::too_many_arguments)]
fn attach_root_reachable_delivery_paths(
    mailbox: Option<&MailboxDb>,
    paths: &[StateSeedPath],
    cancellation: &CancellationToken,
    graph: &mut InvocationGraph,
    diagnostics: &mut Vec<MonitorDiagnostic>,
) {
    let Some(mailbox) = mailbox else {
        return;
    };
    let membership = path_membership(paths);
    let mut pending = graph.records.keys().cloned().collect::<VecDeque<_>>();
    let mut visited = HashSet::new();
    while let Some(owner_uuid) = pending.pop_front() {
        if cancellation.is_cancelled() || !visited.insert(owner_uuid.clone()) {
            continue;
        }
        let child_uuids = match mailbox.list_delivery_invocation_children(&owner_uuid, usize::MAX) {
            Ok(children) => children,
            Err(error) => {
                diagnostics.push(storage_diagnostic(
                    "invocation:delivery-children-read",
                    error,
                ));
                continue;
            }
        };
        for child_uuid in child_uuids {
            if graph.records.contains_key(&child_uuid) {
                continue;
            }
            let Some(occurrences) = membership.get(&child_uuid) else {
                continue;
            };
            let before = graph.records.len();
            for (path_index, child_index) in occurrences {
                include_state_path_segment(graph, &paths[*path_index], *child_index);
            }
            graph.parents.insert(child_uuid.clone(), owner_uuid.clone());
            if graph.records.len() > before {
                for uuid in graph.records.keys() {
                    if !visited.contains(uuid) {
                        pending.push_back(uuid.clone());
                    }
                }
            }
        }
    }
}

fn append_terminal_history(
    state: &StateDb,
    mailbox: Option<&MailboxDb>,
    cap: usize,
    cancellation: &CancellationToken,
    graph: &mut InvocationGraph,
    diagnostics: &mut Vec<MonitorDiagnostic>,
) {
    if graph.records.len() >= cap {
        return;
    }
    let mut parents = graph.records.keys().cloned().collect::<VecDeque<_>>();
    let mut expanded = HashSet::new();
    while let Some(parent_uuid) = parents.pop_front() {
        if cancellation.is_cancelled() || graph.records.len() >= cap {
            return;
        }
        if !expanded.insert(parent_uuid.clone()) {
            continue;
        }
        let Some(parent) = graph.records.get(&parent_uuid).cloned() else {
            continue;
        };
        let remaining = cap.saturating_sub(graph.records.len());
        let lookahead = remaining
            .saturating_add(graph.records.len())
            .saturating_add(1);
        let mut children = match state.list_invocation_children_bounded(parent.id, lookahead, false)
        {
            Ok(children) => children,
            Err(error) => {
                diagnostics.push(storage_diagnostic("invocation:children-read", error));
                Vec::new()
            }
        };
        if let Some(mailbox) = mailbox {
            match mailbox.list_delivery_invocation_children(&parent_uuid, usize::MAX) {
                Ok(uuids) => {
                    for uuid in uuids {
                        if let Ok(Some(record)) = state.get_invocation_by_uuid(&uuid) {
                            children.push(record);
                        }
                    }
                }
                Err(error) => diagnostics.push(storage_diagnostic(
                    "invocation:delivery-children-read",
                    error,
                )),
            }
        }
        children.retain(|record| !graph.records.contains_key(&record.invocation_uuid));
        children.sort_by_key(|record| (record.created_at, record.id));
        children.dedup_by_key(|record| record.id);
        if children.len() > remaining {
            diagnostics.push(invocation_truncated_diagnostic());
        }
        for child in children.into_iter().take(remaining) {
            let child_uuid = child.invocation_uuid.clone();
            if graph.records.contains_key(&child_uuid) {
                continue;
            }
            graph
                .parents
                .insert(child_uuid.clone(), parent_uuid.clone());
            graph.records.insert(child_uuid.clone(), child);
            parents.push_back(child_uuid);
        }
    }
}

fn project_invocation_graph(
    pid: Option<&PidIdentityDb>,
    root_uuid: &str,
    active_session_id: Option<&str>,
    limits: SnapshotLimits,
    graph: InvocationGraph,
    projection: &mut InvocationProjection,
) {
    let children = graph_children(&graph.parents);
    let mut pending = VecDeque::from([(root_uuid.to_string(), 0_usize)]);
    let mut visited = HashSet::new();
    while let Some((uuid, depth)) = pending.pop_front() {
        let mandatory = graph.mandatory.contains(&uuid);
        if depth > limits.invocation_subtree_depth
            || projection.invocation_count >= limits.max_invocation_nodes
        {
            if mandatory {
                projection.live_coverage_incomplete = true;
            }
            continue;
        }
        if !visited.insert(uuid.clone()) {
            continue;
        }
        let Some(record) = graph.records.get(&uuid) else {
            continue;
        };
        projection.invocation_count += 1;
        projection.invocation_uuids.insert(uuid.clone());
        let selected_pid = selected_pid_row(pid, &uuid, &mut projection.diagnostics);
        projection.nodes.push(invocation_node(
            record,
            graph.parents.get(&uuid).map(String::as_str),
            active_session_id,
            &selected_pid,
        ));
        if let Some((row, liveness)) = selected_pid {
            projection.nodes.push(process_node(record, &row, liveness));
        }
        if let Some(child_uuids) = children.get(&uuid) {
            for child_uuid in child_uuids {
                pending.push_back((child_uuid.clone(), depth + 1));
            }
        }
    }
    if graph.mandatory.len() > projection.invocation_count
        || graph.mandatory.len() > limits.max_invocation_nodes
    {
        projection.live_coverage_incomplete = true;
    }
    if projection.live_coverage_incomplete {
        projection
            .diagnostics
            .push(invocation_live_coverage_diagnostic());
    }
}

fn graph_children(parents: &HashMap<String, String>) -> HashMap<String, Vec<String>> {
    let mut children: HashMap<String, Vec<String>> = HashMap::new();
    for (child, parent) in parents {
        children
            .entry(parent.clone())
            .or_default()
            .push(child.clone());
    }
    for child_uuids in children.values_mut() {
        child_uuids.sort();
    }
    children
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

fn invocation_live_coverage_diagnostic() -> MonitorDiagnostic {
    MonitorDiagnostic {
        code: "truncated:invocation-live-coverage".to_string(),
        severity: MonitorDiagnosticSeverity::Error,
        message: "root-reachable live closure exceeded the hard node/depth cap; live coverage and running totals are incomplete and require pagination".to_string(),
        node_id: None,
    }
}

fn invocation_missing_parent_diagnostic(child_uuid: &str, parent_id: i64) -> MonitorDiagnostic {
    MonitorDiagnostic {
        code: "invocation:missing-parent".to_string(),
        severity: MonitorDiagnosticSeverity::Warning,
        message: format!(
            "invocation {child_uuid} references missing durable parent row {parent_id}; it was not promoted to a root"
        ),
        node_id: Some(invocation_node_id(child_uuid)),
    }
}

fn invocation_cycle_diagnostic(uuid: &str) -> MonitorDiagnostic {
    MonitorDiagnostic {
        code: "invocation:ancestor-cycle".to_string(),
        severity: MonitorDiagnosticSeverity::Error,
        message: format!("durable invocation ancestry contains a cycle at {uuid}"),
        node_id: Some(invocation_node_id(uuid)),
    }
}
