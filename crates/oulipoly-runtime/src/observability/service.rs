//! ## Declared roles
//!
//! `accessor`, `mapper`, `orchestration`, `predicate`
//!
//! Public snapshot port and production read-only snapshot assembly.

use crate::observability::agent_bash::{
    AgentBashMetaCache, AgentBashProjectInput, default_agent_bash_root, project_agent_bash,
};
use crate::observability::dto::{
    InspectRef, MonitorDiagnostic, MonitorDiagnosticSeverity, MonitorNode, MonitorNodeKind,
    MonitorSnapshot, MonitorStatus, MonitorSummary,
};
use crate::observability::invocation::{invocation_node_id, project_invocations, session_node_id};
use crate::observability::limits::SnapshotLimits;
use crate::observability::mailbox::project_mailbox;
use crate::observability::state_access::SnapshotStores;
use crate::observability::transcript_source::SessionTranscriptResolver;
use oulipoly_config::SessionStorage;
use oulipoly_state::mailbox::MailboxRow;
use serde::Serialize;
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ObservabilityRoot {
    pub invocation_uuid: Option<String>,
    pub session_id: Option<String>,
    pub provider_name: Option<String>,
    pub model_name: Option<String>,
}

pub trait ObservabilitySnapshotPort {
    fn snapshot(&self, root: &ObservabilityRoot, limits: SnapshotLimits) -> MonitorSnapshot;
}

pub struct ProductionObservabilitySnapshotService {
    agent_bash_root: Option<PathBuf>,
    agent_bash_cache: AgentBashMetaCache,
    transcript: SessionTranscriptResolver,
}

impl ProductionObservabilitySnapshotService {
    pub fn new(agent_bash_root: Option<PathBuf>) -> Self {
        Self::with_session_storage(agent_bash_root, None)
    }

    /// Construct a service that can surface the active session's live transcript
    /// in the inspect pane by resolving it through `session_storage`.
    pub fn with_session_storage(
        agent_bash_root: Option<PathBuf>,
        session_storage: Option<SessionStorage>,
    ) -> Self {
        Self {
            agent_bash_root,
            agent_bash_cache: AgentBashMetaCache::default(),
            transcript: SessionTranscriptResolver::new(session_storage),
        }
    }

    /// Default agent-bash root plus the active session's transcript storage, for
    /// the interactive monitor.
    pub fn for_session(session_storage: Option<SessionStorage>) -> Self {
        Self::with_session_storage(default_agent_bash_root(), session_storage)
    }
}

impl Default for ProductionObservabilitySnapshotService {
    fn default() -> Self {
        Self::new(default_agent_bash_root())
    }
}

impl ObservabilitySnapshotPort for ProductionObservabilitySnapshotService {
    fn snapshot(&self, root: &ObservabilityRoot, limits: SnapshotLimits) -> MonitorSnapshot {
        let generated_at = SystemTime::now();
        let stores = SnapshotStores::open_default_read_only();
        let invocation =
            project_invocations(stores.state.as_ref(), stores.pid.as_ref(), root, limits);
        let active_session_id =
            active_snapshot_session_id(invocation.active_session_id.clone(), root);
        let mailbox = project_mailbox(
            stores.mailbox.as_ref(),
            stores.pid.as_ref(),
            root,
            active_session_id.as_deref(),
            invocation.root_invocation_uuid.as_deref(),
            &invocation.invocation_uuids,
            limits,
        );
        let agent_bash = project_agent_bash(self.agent_bash_input(
            &stores,
            active_session_id.as_deref(),
            &invocation.invocation_uuids,
            &mailbox.rows,
            limits,
        ));
        let invocation_count = invocation.invocation_count;
        let pending_mailbox_count = mailbox.pending_count;
        let running_agent_bash_count = agent_bash.running_count;
        let root_invocation_uuid = invocation.root_invocation_uuid.clone();
        let diagnostics = snapshot_diagnostics(
            stores.diagnostics,
            invocation.diagnostics,
            mailbox.diagnostics,
            agent_bash.diagnostics,
        );
        let mut nodes = snapshot_nodes(mailbox.nodes, invocation.nodes, agent_bash.nodes);
        self.attach_session_transcript(
            &mut nodes,
            root,
            active_session_id.as_deref(),
            root_invocation_uuid.as_deref(),
            limits,
        );
        let summary = summary(
            &nodes,
            &diagnostics,
            invocation_count,
            pending_mailbox_count,
            running_agent_bash_count,
        );
        monitor_snapshot(
            generated_at,
            invocation.root_invocation_uuid,
            active_session_id,
            summary,
            nodes,
            diagnostics,
        )
    }
}

impl ProductionObservabilitySnapshotService {
    /// Build the agent-bash projection input from the open stores and the active
    /// session's invocation/mailbox context.
    fn agent_bash_input<'a>(
        &'a self,
        stores: &'a SnapshotStores,
        active_session_id: Option<&'a str>,
        invocation_uuids: &'a HashSet<String>,
        mailbox_rows: &'a [MailboxRow],
        limits: SnapshotLimits,
    ) -> AgentBashProjectInput<'a> {
        AgentBashProjectInput {
            root_dir: self.agent_bash_root.as_deref(),
            cache: &self.agent_bash_cache,
            state: stores.state.as_ref(),
            pid: stores.pid.as_ref(),
            session_id: active_session_id,
            invocation_uuids,
            mailbox_rows,
            limits,
        }
    }

    /// Point the session, root-invocation, and root-process nodes at the active
    /// session's live transcript so the inspect pane can stream it. No-op when the
    /// transcript cannot be resolved (unknown provider/session or missing file).
    fn attach_session_transcript(
        &self,
        nodes: &mut [MonitorNode],
        root: &ObservabilityRoot,
        active_session_id: Option<&str>,
        root_invocation_uuid: Option<&str>,
        limits: SnapshotLimits,
    ) {
        let Some(path) = self
            .transcript
            .resolve(root.provider_name.as_deref(), active_session_id)
        else {
            return;
        };
        attach_transcript_inspect_ref(
            nodes,
            active_session_id,
            root_invocation_uuid,
            path,
            limits.transcript_tail_bytes,
        );
    }
}

fn attach_transcript_inspect_ref(
    nodes: &mut [MonitorNode],
    active_session_id: Option<&str>,
    root_invocation_uuid: Option<&str>,
    path: String,
    max_tail_bytes: usize,
) {
    let session_node = active_session_id.map(session_node_id);
    let root_invocation_node = root_invocation_uuid.map(invocation_node_id);
    let target_indices = transcript_inspect_target_indices(
        nodes,
        session_node.as_deref(),
        root_invocation_node.as_deref(),
    );
    for index in target_indices {
        nodes[index].inspect_ref = Some(session_transcript_inspect_ref(&path, max_tail_bytes));
    }
}

fn transcript_inspect_target_indices(
    nodes: &[MonitorNode],
    session_node_id: Option<&str>,
    root_invocation_node_id: Option<&str>,
) -> Vec<usize> {
    nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| {
            node_streams_session_transcript(node, session_node_id, root_invocation_node_id)
                .then_some(index)
        })
        .collect()
}

fn session_transcript_inspect_ref(path: &str, max_tail_bytes: usize) -> InspectRef {
    InspectRef::SessionTranscript {
        path: path.to_string(),
        max_tail_bytes,
    }
}

fn node_streams_session_transcript(
    node: &MonitorNode,
    session_node_id: Option<&str>,
    root_invocation_node_id: Option<&str>,
) -> bool {
    node_id_matches(node, session_node_id)
        || node_id_matches(node, root_invocation_node_id)
        || node_parent_matches(node, root_invocation_node_id)
}

fn node_id_matches(node: &MonitorNode, id: Option<&str>) -> bool {
    id.is_some_and(|id| node.id == id)
}

fn node_parent_matches(node: &MonitorNode, parent_id: Option<&str>) -> bool {
    matches!((node.parent_id.as_deref(), parent_id), (Some(parent), Some(id)) if parent == id)
}

fn active_snapshot_session_id(
    invocation_session_id: Option<String>,
    root: &ObservabilityRoot,
) -> Option<String> {
    invocation_session_id.or_else(|| root.session_id.clone())
}

fn snapshot_diagnostics(
    stores: Vec<MonitorDiagnostic>,
    invocation: Vec<MonitorDiagnostic>,
    mailbox: Vec<MonitorDiagnostic>,
    agent_bash: Vec<MonitorDiagnostic>,
) -> Vec<MonitorDiagnostic> {
    let mut diagnostics = stores;
    diagnostics.extend(invocation);
    diagnostics.extend(mailbox);
    diagnostics.extend(agent_bash);
    diagnostics
}

fn snapshot_nodes(
    mailbox: Vec<MonitorNode>,
    invocation: Vec<MonitorNode>,
    agent_bash: Vec<MonitorNode>,
) -> Vec<MonitorNode> {
    let mut nodes = Vec::new();
    nodes.extend(mailbox);
    nodes.extend(invocation);
    nodes.extend(agent_bash);
    nodes
}

fn monitor_snapshot(
    generated_at: SystemTime,
    root_invocation_uuid: Option<String>,
    active_session_id: Option<String>,
    summary: MonitorSummary,
    nodes: Vec<MonitorNode>,
    diagnostics: Vec<MonitorDiagnostic>,
) -> MonitorSnapshot {
    MonitorSnapshot {
        generated_at,
        root_invocation_uuid,
        active_session_id,
        summary,
        nodes,
        diagnostics,
    }
}

fn summary(
    nodes: &[MonitorNode],
    diagnostics: &[MonitorDiagnostic],
    invocation_count: usize,
    pending_mailbox_count: usize,
    running_agent_bash_count: usize,
) -> MonitorSummary {
    MonitorSummary {
        status: summary_status(nodes, diagnostics),
        total_nodes: nodes.len(),
        invocation_nodes: invocation_count,
        running_nodes: running_nodes(nodes),
        pending_mailbox_count,
        running_agent_bash_count,
        diagnostics_count: diagnostics.len(),
    }
}

fn running_nodes(nodes: &[MonitorNode]) -> usize {
    nodes
        .iter()
        .filter(|node| node.status == MonitorStatus::Running)
        .count()
}

fn summary_status(nodes: &[MonitorNode], diagnostics: &[MonitorDiagnostic]) -> MonitorStatus {
    if has_error_diagnostic(diagnostics) {
        return MonitorStatus::Error;
    }
    if has_stale_node(nodes) {
        return MonitorStatus::Stale;
    }
    if has_running_node(nodes) {
        return MonitorStatus::Running;
    }
    if has_pending_mailbox_node(nodes) {
        return MonitorStatus::Pending;
    }
    MonitorStatus::Idle
}

fn has_error_diagnostic(diagnostics: &[MonitorDiagnostic]) -> bool {
    diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == MonitorDiagnosticSeverity::Error)
}

fn has_stale_node(nodes: &[MonitorNode]) -> bool {
    nodes.iter().any(|node| node.status == MonitorStatus::Stale)
}

fn has_running_node(nodes: &[MonitorNode]) -> bool {
    nodes
        .iter()
        .any(|node| node.status == MonitorStatus::Running)
}

fn has_pending_mailbox_node(nodes: &[MonitorNode]) -> bool {
    nodes.iter().any(pending_mailbox_node)
}

fn pending_mailbox_node(node: &MonitorNode) -> bool {
    node.kind == MonitorNodeKind::MailboxNotification && node.status == MonitorStatus::Pending
}
