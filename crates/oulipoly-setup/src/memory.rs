use oulipoly_state::StateDb;
use rusqlite::params;
use serde::Serialize;
use std::path::Path;

pub struct MemoryGraph {
    connection: rusqlite::Connection,
    _state_authority: StateDb,
    _temporary_directory: Option<tempfile::TempDir>,
}

#[derive(Clone, Serialize)]
pub struct MemoryNode {
    pub id: String,
    pub node_type: String,
    pub label: String,
    pub data: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Serialize)]
pub struct MemoryEdge {
    pub source_id: String,
    pub target_id: String,
    pub edge_type: String,
    pub data: Option<String>,
    pub created_at: String,
}

#[derive(Clone, Serialize)]
pub struct MemorySnapshot {
    pub nodes: Vec<MemoryNode>,
    pub edges: Vec<MemoryEdge>,
}

impl MemoryGraph {
    pub fn open(path: &Path) -> Result<Self, String> {
        let temporary_directory = (path == Path::new(":memory:"))
            .then(tempfile::tempdir)
            .transpose()
            .map_err(|error| format!("Failed to create temporary memory database: {error}"))?;
        let database_path = temporary_directory
            .as_ref()
            .map(|directory| directory.path().join("state.db"))
            .unwrap_or_else(|| path.to_path_buf());
        let state_authority = StateDb::open_for_memory(&database_path)?;
        let connection = rusqlite::Connection::open(state_authority.path())
            .map_err(|error| format!("Failed to open setup memory database: {error}"))?;
        connection
            .pragma_update(None, "foreign_keys", true)
            .map_err(|error| format!("Failed to configure setup memory database: {error}"))?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|error| format!("Failed to configure setup memory timeout: {error}"))?;
        Ok(MemoryGraph {
            connection,
            _state_authority: state_authority,
            _temporary_directory: temporary_directory,
        })
    }

    pub fn upsert_node(
        &self,
        id: &str,
        node_type: &str,
        label: &str,
        data: &str,
    ) -> Result<(), String> {
        let now = chrono::Utc::now().to_rfc3339();
        self.connection
            .execute(
                "INSERT INTO memory_nodes (id, node_type, label, data, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)
             ON CONFLICT (id) DO UPDATE SET
                node_type = ?2, label = ?3, data = ?4, updated_at = ?5",
                params![id, node_type, label, data, &now],
            )
            .map_err(|e| format!("Failed to upsert node: {e}"))?;
        Ok(())
    }

    pub fn add_edge(
        &self,
        source_id: &str,
        target_id: &str,
        edge_type: &str,
    ) -> Result<(), String> {
        let now = chrono::Utc::now().to_rfc3339();
        self.connection
            .execute(
                "INSERT OR IGNORE INTO memory_edges (source_id, target_id, edge_type, created_at)
             VALUES (?1, ?2, ?3, ?4)",
                params![source_id, target_id, edge_type, &now],
            )
            .map_err(|e| format!("Failed to add edge: {e}"))?;
        Ok(())
    }

    pub fn get_node(&self, id: &str) -> Result<Option<MemoryNode>, String> {
        let mut stmt = self.connection.prepare(
            "SELECT id, node_type, label, data, created_at, updated_at FROM memory_nodes WHERE id = ?1"
        ).map_err(|e| format!("Query error: {e}"))?;

        let result = stmt.query_row(params![id], |row| {
            Ok(MemoryNode {
                id: row.get(0)?,
                node_type: row.get(1)?,
                label: row.get(2)?,
                data: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        });

        match result {
            Ok(node) => Ok(Some(node)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(format!("Failed to get node: {e}")),
        }
    }

    pub fn get_neighbors(&self, node_id: &str) -> Result<Vec<(MemoryEdge, MemoryNode)>, String> {
        let mut stmt = self
            .connection
            .prepare(
                "SELECT e.source_id, e.target_id, e.edge_type, e.data, e.created_at,
                    n.id, n.node_type, n.label, n.data, n.created_at, n.updated_at
             FROM memory_edges e
             JOIN memory_nodes n ON n.id = e.target_id
             WHERE e.source_id = ?1",
            )
            .map_err(|e| format!("Query error: {e}"))?;

        let results = stmt
            .query_map(params![node_id], |row| {
                Ok((
                    MemoryEdge {
                        source_id: row.get(0)?,
                        target_id: row.get(1)?,
                        edge_type: row.get(2)?,
                        data: row.get(3)?,
                        created_at: row.get(4)?,
                    },
                    MemoryNode {
                        id: row.get(5)?,
                        node_type: row.get(6)?,
                        label: row.get(7)?,
                        data: row.get(8)?,
                        created_at: row.get(9)?,
                        updated_at: row.get(10)?,
                    },
                ))
            })
            .map_err(|e| format!("Query error: {e}"))?;

        results
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect neighbors: {e}"))
    }

    pub fn subgraph_for_context(&self, node_types: &[&str]) -> Result<MemorySnapshot, String> {
        let placeholders: Vec<String> = node_types
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 1))
            .collect();
        let ph = placeholders.join(", ");

        let query = format!(
            "SELECT id, node_type, label, data, created_at, updated_at FROM memory_nodes WHERE node_type IN ({ph})"
        );

        let mut stmt = self
            .connection
            .prepare(&query)
            .map_err(|e| format!("Query error: {e}"))?;

        let params: Vec<&dyn rusqlite::ToSql> = node_types
            .iter()
            .map(|s| s as &dyn rusqlite::ToSql)
            .collect();

        let nodes: Vec<MemoryNode> = stmt
            .query_map(params.as_slice(), |row| {
                Ok(MemoryNode {
                    id: row.get(0)?,
                    node_type: row.get(1)?,
                    label: row.get(2)?,
                    data: row.get(3)?,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect nodes: {e}"))?;

        let node_ids: Vec<String> = nodes.iter().map(|n| n.id.clone()).collect();
        let edges = self.get_edges_between(&node_ids)?;

        Ok(MemorySnapshot { nodes, edges })
    }

    fn get_edges_between(&self, node_ids: &[String]) -> Result<Vec<MemoryEdge>, String> {
        if node_ids.is_empty() {
            return Ok(vec![]);
        }

        let placeholders: Vec<String> = node_ids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 1))
            .collect();
        let ph = placeholders.join(", ");

        let offset = node_ids.len();
        let placeholders2: Vec<String> = node_ids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", offset + i + 1))
            .collect();
        let ph2 = placeholders2.join(", ");

        let query = format!(
            "SELECT source_id, target_id, edge_type, data, created_at
             FROM memory_edges
             WHERE source_id IN ({ph}) AND target_id IN ({ph2})"
        );

        let mut stmt = self
            .connection
            .prepare(&query)
            .map_err(|e| format!("Query error: {e}"))?;

        // Double the params (for both IN clauses)
        let mut params: Vec<&dyn rusqlite::ToSql> = Vec::new();
        for id in node_ids {
            params.push(id as &dyn rusqlite::ToSql);
        }
        for id in node_ids {
            params.push(id as &dyn rusqlite::ToSql);
        }

        stmt.query_map(params.as_slice(), |row| {
            Ok(MemoryEdge {
                source_id: row.get(0)?,
                target_id: row.get(1)?,
                edge_type: row.get(2)?,
                data: row.get(3)?,
                created_at: row.get(4)?,
            })
        })
        .map_err(|e| format!("Query error: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to collect edges: {e}"))
    }

    pub fn snapshot(&self) -> Result<MemorySnapshot, String> {
        let mut stmt = self
            .connection
            .prepare("SELECT id, node_type, label, data, created_at, updated_at FROM memory_nodes")
            .map_err(|e| format!("Query error: {e}"))?;

        let nodes: Vec<MemoryNode> = stmt
            .query_map([], |row| {
                Ok(MemoryNode {
                    id: row.get(0)?,
                    node_type: row.get(1)?,
                    label: row.get(2)?,
                    data: row.get(3)?,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect: {e}"))?;

        let mut stmt = self
            .connection
            .prepare("SELECT source_id, target_id, edge_type, data, created_at FROM memory_edges")
            .map_err(|e| format!("Query error: {e}"))?;

        let edges: Vec<MemoryEdge> = stmt
            .query_map([], |row| {
                Ok(MemoryEdge {
                    source_id: row.get(0)?,
                    target_id: row.get(1)?,
                    edge_type: row.get(2)?,
                    data: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect: {e}"))?;

        Ok(MemorySnapshot { nodes, edges })
    }

    // Session tracking
    pub fn create_session(&self, session_id: &str) -> Result<(), String> {
        let now = chrono::Utc::now().to_rfc3339();
        self.connection
            .execute(
                "INSERT INTO setup_sessions (id, started_at) VALUES (?1, ?2)",
                params![session_id, &now],
            )
            .map_err(|e| format!("Failed to create session: {e}"))?;
        Ok(())
    }

    pub fn record_turn(
        &self,
        session_id: &str,
        turn_number: i32,
        prompt: &str,
        response: &str,
        events: &str,
    ) -> Result<(), String> {
        let now = chrono::Utc::now().to_rfc3339();
        self.connection.execute(
            "INSERT INTO setup_turns (session_id, turn_number, agent_prompt, agent_response, events_emitted, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![session_id, turn_number, prompt, response, events, &now],
        ).map_err(|e| format!("Failed to record turn: {e}"))?;

        self.connection
            .execute(
                "UPDATE setup_sessions SET turn_count = ?1 WHERE id = ?2",
                params![turn_number, session_id],
            )
            .map_err(|e| format!("Failed to update session: {e}"))?;

        Ok(())
    }

    pub fn end_session(&self, session_id: &str, outcome: &str) -> Result<(), String> {
        let now = chrono::Utc::now().to_rfc3339();
        self.connection
            .execute(
                "UPDATE setup_sessions SET ended_at = ?1, outcome = ?2 WHERE id = ?3",
                params![&now, outcome, session_id],
            )
            .map_err(|e| format!("Failed to end session: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_graph() -> MemoryGraph {
        MemoryGraph::open(Path::new(":memory:")).unwrap()
    }

    #[test]
    fn upsert_and_get_node() {
        let g = test_graph();
        g.upsert_node("cli:claude", "cli", "claude", r#"{"version":"1.0"}"#)
            .unwrap();
        let node = g.get_node("cli:claude").unwrap().unwrap();
        assert_eq!(node.label, "claude");
        assert_eq!(node.node_type, "cli");
    }

    #[test]
    fn add_edge_and_neighbors() {
        let g = test_graph();
        g.upsert_node("cli:claude", "cli", "claude", "{}").unwrap();
        g.upsert_node("model:opus", "model", "opus", "{}").unwrap();
        g.add_edge("cli:claude", "model:opus", "uses_model")
            .unwrap();

        let neighbors = g.get_neighbors("cli:claude").unwrap();
        assert_eq!(neighbors.len(), 1);
        assert_eq!(neighbors[0].1.label, "opus");
        assert_eq!(neighbors[0].0.edge_type, "uses_model");
    }

    #[test]
    fn subgraph_filters_by_type() {
        let g = test_graph();
        g.upsert_node("cli:claude", "cli", "claude", "{}").unwrap();
        g.upsert_node("model:opus", "model", "opus", "{}").unwrap();
        g.upsert_node("pref:theme", "preference", "theme", "{}")
            .unwrap();

        let snap = g.subgraph_for_context(&["cli", "model"]).unwrap();
        assert_eq!(snap.nodes.len(), 2);
    }

    #[test]
    fn session_tracking() {
        let g = test_graph();
        g.create_session("sess-1").unwrap();
        g.record_turn("sess-1", 1, "prompt", "response", "[]")
            .unwrap();
        g.end_session("sess-1", "success").unwrap();
    }

    #[test]
    fn snapshot_returns_all() {
        let g = test_graph();
        g.upsert_node("a", "cli", "a", "{}").unwrap();
        g.upsert_node("b", "cli", "b", "{}").unwrap();
        let snap = g.snapshot().unwrap();
        assert_eq!(snap.nodes.len(), 2);
    }

    #[test]
    fn missing_node_returns_none() {
        let g = test_graph();
        assert!(g.get_node("nonexistent").unwrap().is_none());
    }

    #[test]
    fn file_backed_graph_and_rebuild_authority_exclude_each_other() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        let graph = MemoryGraph::open(&state_path).unwrap();
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let rebuild_path = state_path.clone();
        let rebuild = std::thread::spawn(move || {
            sender
                .send(StateDb::acquire_rebuild_authority(&rebuild_path))
                .unwrap()
        });
        assert!(
            receiver
                .recv_timeout(std::time::Duration::from_millis(100))
                .is_err(),
            "rebuild escaped a live file-backed memory graph"
        );
        drop(graph);
        drop(receiver.recv().unwrap().unwrap());
        rebuild.join().unwrap();

        let authority = StateDb::acquire_rebuild_authority(&state_path).unwrap();
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let graph_path = state_path.clone();
        let opener =
            std::thread::spawn(move || sender.send(MemoryGraph::open(&graph_path)).unwrap());
        assert!(
            receiver
                .recv_timeout(std::time::Duration::from_millis(100))
                .is_err(),
            "file-backed memory graph escaped rebuild authority"
        );
        drop(authority);
        drop(receiver.recv().unwrap().unwrap());
        opener.join().unwrap();
    }
}
