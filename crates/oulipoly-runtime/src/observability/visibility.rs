//! ## Declared roles
//!
//! `filter`, `predicate`, `mapper`
//!
//! Snapshot node visibility rules for monitor views.

use crate::observability::dto::{MonitorNode, MonitorNodeId, MonitorStatus};
use std::collections::{HashMap, HashSet};

pub(super) fn retain_live_subtrees(nodes: &mut Vec<MonitorNode>) {
    let retained = live_or_live_ancestor_ids(nodes);
    nodes.retain(|node| retained.contains(&node.id));
}

fn live_or_live_ancestor_ids(nodes: &[MonitorNode]) -> HashSet<MonitorNodeId> {
    let parents = parent_index(nodes);
    let mut retained = HashSet::new();
    for node in nodes.iter().filter(|node| !is_terminal_status(node.status)) {
        retain_node_and_ancestors(node, &parents, &mut retained);
    }
    retained
}

fn parent_index(nodes: &[MonitorNode]) -> HashMap<MonitorNodeId, Option<MonitorNodeId>> {
    nodes
        .iter()
        .map(|node| (node.id.clone(), node.parent_id.clone()))
        .collect()
}

fn retain_node_and_ancestors(
    node: &MonitorNode,
    parents: &HashMap<MonitorNodeId, Option<MonitorNodeId>>,
    retained: &mut HashSet<MonitorNodeId>,
) {
    let mut current = Some(node.id.clone());
    while let Some(id) = current {
        if !retained.insert(id.clone()) {
            break;
        }
        current = parents.get(&id).and_then(Clone::clone);
    }
}

fn is_terminal_status(status: MonitorStatus) -> bool {
    matches!(
        status,
        MonitorStatus::Succeeded
            | MonitorStatus::Failed
            | MonitorStatus::Error
            | MonitorStatus::Delivered
            | MonitorStatus::Cancelled
            | MonitorStatus::Stale
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observability::dto::{LivenessStatus, MonitorNodeKind};

    #[test]
    fn fully_terminal_subtree_is_hidden() {
        let mut nodes = vec![
            node("root", None, MonitorStatus::Succeeded),
            node("child", Some("root"), MonitorStatus::Stale),
        ];

        retain_live_subtrees(&mut nodes);

        assert!(nodes.is_empty());
    }

    #[test]
    fn live_node_retains_terminal_ancestors() {
        let mut nodes = vec![
            node("root", None, MonitorStatus::Succeeded),
            node("middle", Some("root"), MonitorStatus::Failed),
            node("live", Some("middle"), MonitorStatus::Running),
        ];

        retain_live_subtrees(&mut nodes);

        assert_eq!(node_ids(&nodes), vec!["root", "middle", "live"]);
    }

    #[test]
    fn terminal_siblings_of_live_node_are_dropped() {
        let mut nodes = vec![
            node("root", None, MonitorStatus::Succeeded),
            node("done", Some("root"), MonitorStatus::Delivered),
            node("live", Some("root"), MonitorStatus::Idle),
        ];

        retain_live_subtrees(&mut nodes);

        assert_eq!(node_ids(&nodes), vec!["root", "live"]);
    }

    #[test]
    fn unknown_status_is_live() {
        let mut nodes = vec![node("unknown", None, MonitorStatus::Unknown)];

        retain_live_subtrees(&mut nodes);

        assert_eq!(node_ids(&nodes), vec!["unknown"]);
    }

    fn node(id: &str, parent_id: Option<&str>, status: MonitorStatus) -> MonitorNode {
        MonitorNode {
            id: id.to_string(),
            parent_id: parent_id.map(str::to_string),
            kind: MonitorNodeKind::Invocation,
            label: id.to_string(),
            status,
            pid: None,
            pgid: None,
            liveness: LivenessStatus::NotApplicable,
            started_at: None,
            updated_at: None,
            completed_at: None,
            last_output_excerpt: None,
            inspect_ref: None,
            cancel_ref: None,
            wake: None,
            mailbox: None,
        }
    }

    fn node_ids(nodes: &[MonitorNode]) -> Vec<&str> {
        nodes.iter().map(|node| node.id.as_str()).collect()
    }
}
