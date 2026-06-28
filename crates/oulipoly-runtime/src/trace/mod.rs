//! Invocation trace tree construction and rendering.
//!
//! PR-B owns the `trace <uuid>` CLI surface. The implementation builds an
//! invocation tree from SQLite parent edges, renders deterministic human output,
//! and exposes a structured JSON shape for machine consumers.

use crate::session_export::ContentChunk;
use crate::session_metadata::TranscriptState;
use crate::sessions::locate_transcript;
use chrono::{DateTime, SecondsFormat, Utc};
use oulipoly_agent_messenger::ReturnedArtifactRef;
use oulipoly_config::SessionsConfig;
use oulipoly_state::{InvocationRecord, StateDb};
use serde::Serialize;
use std::collections::HashSet;
use uuid::Uuid;

#[derive(Debug, Clone, Copy)]
pub struct TraceOptions {
    pub max_depth: usize,
    pub json: bool,
    pub inline_transcript: bool,
    pub transcript: bool,
}

pub const STALE_RUNNING_THRESHOLD_SECONDS: u64 = 1800;

#[derive(Debug, Clone, Serialize)]
pub struct TraceReport {
    pub requested_id: String,
    pub generated_at: DateTime<Utc>,
    pub root: TraceNode,
    #[serde(skip)]
    show_transcript_footer: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TraceNode {
    pub invocation: TraceInvocation,
    pub session: TraceSession,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript: Option<Vec<TraceTranscriptTurn>>,
    pub warnings: Vec<String>,
    pub children: Vec<TraceNode>,
    #[serde(skip)]
    ascii_leaves: Vec<AsciiLeaf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TraceInvocation {
    pub row_id: i64,
    pub id: String,
    pub agent_runner_invocation_id: String,
    pub source: Option<String>,
    pub model_name: String,
    pub parent_id: Option<String>,
    pub status: String,
    pub success: Option<bool>,
    pub exit_code: Option<i32>,
    pub error_category: Option<String>,
    pub terminal_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stale_running: Option<StaleRunning>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub returned_artifacts: Vec<ReturnedArtifactRef>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StaleRunning {
    pub age_seconds: u64,
    pub threshold_seconds: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TraceSession {
    pub id: Option<String>,
    pub chain_id: Option<String>,
    pub provider_session_id: Option<String>,
    pub agent_runner_chain_id: Option<String>,
    pub resume_input_id: Option<String>,
    pub capture_method: Option<String>,
    pub transcript_path: Option<String>,
    pub transcript_state: TranscriptState,
    pub turn_count: Option<u64>,
    pub assistant_turn_count: Option<u64>,
    pub sidechain_turn_count: Option<u64>,
    pub resume_acceptance: Option<String>,
    pub resume_acceptance_evidence: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TraceTranscriptTurn {
    pub turn_id: String,
    pub role: String,
    pub timestamp: String,
    pub body_state: TraceBodyState,
    pub content: Option<Vec<ContentChunk>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceBodyState {
    Stored,
    Missing,
    Invalid,
}

#[derive(Debug, Clone)]
enum AsciiLeaf {
    Cycle(String),
    DepthLimit,
}

#[derive(Debug, Clone, Copy)]
struct TraceBuildContext<'a> {
    options: TraceOptions,
    sessions_cfg: Option<&'a SessionsConfig>,
    generated_at: DateTime<Utc>,
}

pub fn trace_invocation(
    db: &StateDb,
    root_uuid: &str,
    options: TraceOptions,
) -> Result<TraceReport, String> {
    trace_invocation_with_sessions(db, root_uuid, options, None)
}

pub fn trace_invocation_with_sessions(
    db: &StateDb,
    root_uuid: &str,
    options: TraceOptions,
    sessions_cfg: Option<&SessionsConfig>,
) -> Result<TraceReport, String> {
    Uuid::parse_str(root_uuid)
        .map_err(|e| format!("Invalid invocation UUID '{root_uuid}': {e}"))?;

    let root_record = db
        .get_invocation_by_uuid(root_uuid)?
        .ok_or_else(|| format!("Invocation not found: {root_uuid}"))?;

    let mut visited = HashSet::from([root_record.id]);
    let generated_at = Utc::now();
    let context = TraceBuildContext {
        options,
        sessions_cfg,
        generated_at,
    };
    let root = build_trace_node(db, root_record, None, 0, context, &mut visited)?;

    Ok(TraceReport {
        requested_id: root_uuid.to_string(),
        generated_at,
        root,
        show_transcript_footer: options.transcript && !options.json,
    })
}

pub fn render_ascii_trace(report: &TraceReport) -> String {
    let mut output = String::new();
    render_ascii_node(&report.root, 0, &mut output);

    if report.show_transcript_footer {
        output.push_str(&format!(
            "=== Transcript: {} ===\n(no transcript: session_id unresolved — see PR-C)\n",
            report.root.invocation.id
        ));
    }

    output
}

fn build_trace_node(
    db: &StateDb,
    record: InvocationRecord,
    parent_uuid: Option<String>,
    depth: usize,
    context: TraceBuildContext<'_>,
    visited: &mut HashSet<i64>,
) -> Result<TraceNode, String> {
    let (session, mut node_warnings) = build_trace_session(db, &record, context.sessions_cfg)?;
    let transcript = if context.options.inline_transcript {
        match read_inline_transcript(db, record.provider_name.as_deref(), session.id.as_deref()) {
            Ok(turns) => Some(turns),
            Err(err) => {
                node_warnings.push(err);
                Some(Vec::new())
            }
        }
    } else {
        None
    };
    let stored_status = record.status.as_str().to_string();
    let age_seconds = context
        .generated_at
        .signed_duration_since(record.created_at)
        .num_seconds()
        .max(0) as u64;
    let stale_running = record.status == oulipoly_state::InvocationStatus::Running
        && record.finished_at.is_none()
        && parent_uuid.is_none()
        && age_seconds >= STALE_RUNNING_THRESHOLD_SECONDS;
    let stale_running = stale_running.then_some(StaleRunning {
        age_seconds,
        threshold_seconds: STALE_RUNNING_THRESHOLD_SECONDS,
    });
    let (status, terminal_reason) = if stale_running.is_some() && context.options.json {
        (
            oulipoly_state::InvocationStatus::Failed
                .as_str()
                .to_string(),
            Some("tracing_timeout".to_string()),
        )
    } else {
        (stored_status, record.terminal_reason)
    };
    if let Some(stale) = &stale_running {
        node_warnings.push(format!(
            "stale_running: row exceeded 30m running threshold (age {}s); status lifted to failed in JSON output only",
            stale.age_seconds
        ));
    }
    let returned_artifacts = db.list_returned_artifacts(record.id)?;

    let mut node = TraceNode {
        invocation: TraceInvocation {
            row_id: record.id,
            id: record.invocation_uuid.clone(),
            agent_runner_invocation_id: record.invocation_uuid.clone(),
            source: record.provider_name.clone(),
            model_name: record.model_name,
            parent_id: parent_uuid,
            status,
            success: record.success,
            exit_code: record.exit_code,
            error_category: record.error_category,
            terminal_reason,
            stale_running,
            returned_artifacts,
            started_at: record.created_at,
            finished_at: record.finished_at,
        },
        session,
        transcript,
        warnings: std::mem::take(&mut node_warnings),
        children: Vec::new(),
        ascii_leaves: Vec::new(),
    };

    let children = db.list_invocation_children(node.invocation.row_id)?;
    if depth >= context.options.max_depth {
        if !children.is_empty() {
            node.warnings.push(format!(
                "depth limit reached at child of {}",
                node.invocation.id
            ));
            node.ascii_leaves.push(AsciiLeaf::DepthLimit);
        }
        return Ok(node);
    }

    for child in children {
        let child_id = child.id;
        let child_uuid = child.invocation_uuid.clone();
        if !visited.insert(child_id) {
            node.warnings
                .push(format!("cycle detected pointing to {child_uuid}"));
            node.ascii_leaves.push(AsciiLeaf::Cycle(child_uuid));
            continue;
        }

        let child_node = build_trace_node(
            db,
            child,
            Some(node.invocation.id.clone()),
            depth + 1,
            context,
            visited,
        )?;
        node.warnings.extend(child_node.warnings.iter().cloned());
        node.children.push(child_node);
        visited.remove(&child_id);
    }

    Ok(node)
}

fn read_inline_transcript(
    db: &StateDb,
    provider_name: Option<&str>,
    session_id: Option<&str>,
) -> Result<Vec<TraceTranscriptTurn>, String> {
    let (Some(provider_name), Some(session_id)) = (provider_name, session_id) else {
        return Ok(Vec::new());
    };
    let mut stmt = db
        .connection()
        .prepare(
            "SELECT turn_id, timestamp, role, body
             FROM session_turns
             WHERE provider_name = ?1 AND session_id = ?2
             ORDER BY timestamp, id",
        )
        .map_err(|e| format!("failed to prepare inline transcript query: {e}"))?;
    let rows = stmt
        .query_map(rusqlite::params![provider_name, session_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .map_err(|e| format!("failed to query inline transcript rows: {e}"))?;

    let mut turns = Vec::new();
    for row in rows {
        let (turn_id, timestamp, role, body) =
            row.map_err(|e| format!("failed to read inline transcript row: {e}"))?;
        let (body_state, content) = match body {
            Some(body) => match serde_json::from_str::<Vec<ContentChunk>>(&body) {
                Ok(content) => (TraceBodyState::Stored, Some(content)),
                Err(_) => (TraceBodyState::Invalid, None),
            },
            None => (TraceBodyState::Missing, None),
        };
        turns.push(TraceTranscriptTurn {
            turn_id,
            role,
            timestamp,
            body_state,
            content,
        });
    }
    Ok(turns)
}

fn build_trace_session(
    db: &StateDb,
    record: &InvocationRecord,
    sessions_cfg: Option<&SessionsConfig>,
) -> Result<(TraceSession, Vec<String>), String> {
    let mut warnings = Vec::new();
    if record.session_capture_method.as_deref() == Some("failed") {
        warnings.push(
            "session capture failed during execution; reason was logged to stderr at execution time"
                .to_string(),
        );
    } else if record.session_capture_method.as_deref() == Some("resumed")
        && record.resume_acceptance_status.as_deref() != Some("accepted")
    {
        warnings.push(
            "session capture method 'resumed' marks an attempted resume target; child acceptance is not accepted, so inspect resume_acceptance and exit_code"
                .to_string(),
        );
    }
    let provider_session_id = record
        .provider_session_id
        .clone()
        .or_else(|| record.session_id.clone());
    let Some(session_id) = provider_session_id.clone() else {
        return Ok((
            TraceSession {
                id: None,
                chain_id: None,
                provider_session_id: None,
                agent_runner_chain_id: None,
                resume_input_id: record.resume_input_id.clone(),
                capture_method: record.session_capture_method.clone(),
                transcript_path: None,
                transcript_state: TranscriptState::Unresolved,
                turn_count: None,
                assistant_turn_count: None,
                sidechain_turn_count: None,
                resume_acceptance: record.resume_acceptance_status.clone(),
                resume_acceptance_evidence: record.resume_acceptance_evidence.clone(),
            },
            warnings,
        ));
    };
    let explicit_resume_input_id = record.resume_input_id.clone();
    let resume_input_id = explicit_resume_input_id.clone().or_else(|| {
        (record.session_capture_method.as_deref() == Some("resumed")).then(|| session_id.clone())
    });

    let Some(provider_name) = record.provider_name.as_deref() else {
        warnings.push("session_id is present but provider_name is missing".to_string());
        return Ok((
            TraceSession {
                id: Some(session_id),
                chain_id: None,
                provider_session_id,
                agent_runner_chain_id: None,
                resume_input_id,
                capture_method: record.session_capture_method.clone(),
                transcript_path: None,
                transcript_state: TranscriptState::Unresolved,
                turn_count: None,
                assistant_turn_count: None,
                sidechain_turn_count: None,
                resume_acceptance: record.resume_acceptance_status.clone(),
                resume_acceptance_evidence: record.resume_acceptance_evidence.clone(),
            },
            warnings,
        ));
    };

    // Per V10 (failures observable, never silent): a DB error counting
    // turns shouldn't abort the entire trace — push a warning, fall back
    // to None counts, and let the caller render the rest of the tree.
    // This mirrors how locate_transcript failures are handled below.
    let counts = match db.count_session_turns(provider_name, &session_id) {
        Ok(c) => Some(c),
        Err(e) => {
            warnings.push(format!(
                "failed to count session turns for {provider_name}/{session_id}: {e}"
            ));
            None
        }
    };
    let (turn_count, assistant_turn_count, sidechain_turn_count) = counts
        .as_ref()
        .map(|c| (Some(c.total), Some(c.assistant), Some(c.sidechain)))
        .unwrap_or((None, None, None));
    let chain_id = db
        .chain_id_for_segment(provider_name, &session_id)
        .unwrap_or(None);
    let agent_runner_chain_id = if record.provider_session_capture_method.as_deref()
        == Some("resumed")
        && explicit_resume_input_id.as_deref() == provider_session_id.as_deref()
    {
        None
    } else {
        chain_id.clone()
    };

    let Some(sessions_cfg) = sessions_cfg else {
        return Ok((
            TraceSession {
                id: Some(session_id),
                chain_id: chain_id.clone(),
                provider_session_id: provider_session_id.clone(),
                agent_runner_chain_id: agent_runner_chain_id.clone(),
                resume_input_id: resume_input_id.clone(),
                capture_method: record.session_capture_method.clone(),
                transcript_path: None,
                transcript_state: TranscriptState::NoLocator,
                turn_count,
                assistant_turn_count,
                sidechain_turn_count,
                resume_acceptance: record.resume_acceptance_status.clone(),
                resume_acceptance_evidence: record.resume_acceptance_evidence.clone(),
            },
            warnings,
        ));
    };

    match locate_transcript(sessions_cfg, provider_name, &session_id) {
        Ok(None) => Ok((
            TraceSession {
                id: Some(session_id),
                chain_id: chain_id.clone(),
                provider_session_id: provider_session_id.clone(),
                agent_runner_chain_id: agent_runner_chain_id.clone(),
                resume_input_id: resume_input_id.clone(),
                capture_method: record.session_capture_method.clone(),
                transcript_path: None,
                transcript_state: TranscriptState::NoLocator,
                turn_count,
                assistant_turn_count,
                sidechain_turn_count,
                resume_acceptance: record.resume_acceptance_status.clone(),
                resume_acceptance_evidence: record.resume_acceptance_evidence.clone(),
            },
            warnings,
        )),
        Ok(Some(path)) if path.exists() => Ok((
            TraceSession {
                id: Some(session_id),
                chain_id: chain_id.clone(),
                provider_session_id: provider_session_id.clone(),
                agent_runner_chain_id: agent_runner_chain_id.clone(),
                resume_input_id: resume_input_id.clone(),
                capture_method: record.session_capture_method.clone(),
                transcript_path: Some(path.display().to_string()),
                transcript_state: TranscriptState::Available,
                turn_count,
                assistant_turn_count,
                sidechain_turn_count,
                resume_acceptance: record.resume_acceptance_status.clone(),
                resume_acceptance_evidence: record.resume_acceptance_evidence.clone(),
            },
            warnings,
        )),
        Ok(Some(_path)) => Ok((
            TraceSession {
                id: Some(session_id),
                chain_id: chain_id.clone(),
                provider_session_id: provider_session_id.clone(),
                agent_runner_chain_id: agent_runner_chain_id.clone(),
                resume_input_id: resume_input_id.clone(),
                capture_method: record.session_capture_method.clone(),
                transcript_path: None,
                transcript_state: TranscriptState::Missing,
                turn_count,
                assistant_turn_count,
                sidechain_turn_count,
                resume_acceptance: record.resume_acceptance_status.clone(),
                resume_acceptance_evidence: record.resume_acceptance_evidence.clone(),
            },
            warnings,
        )),
        Err(err) => {
            warnings.push(format!("transcript locator failed: {err}"));
            Ok((
                TraceSession {
                    id: Some(session_id),
                    chain_id: chain_id.clone(),
                    provider_session_id,
                    agent_runner_chain_id,
                    resume_input_id,
                    capture_method: record.session_capture_method.clone(),
                    transcript_path: None,
                    transcript_state: TranscriptState::Missing,
                    turn_count,
                    assistant_turn_count,
                    sidechain_turn_count,
                    resume_acceptance: record.resume_acceptance_status.clone(),
                    resume_acceptance_evidence: record.resume_acceptance_evidence.clone(),
                },
                warnings,
            ))
        }
    }
}

fn render_ascii_node(node: &TraceNode, depth: usize, output: &mut String) {
    let indent = "  ".repeat(depth);
    if depth > 0 {
        output.push_str(&indent);
        output.push_str("└── ");
    }
    output.push_str(&format_ascii_node(node));
    output.push('\n');

    for warning in &node.warnings {
        output.push_str(&"  ".repeat(depth + 1));
        output.push_str("! ");
        output.push_str(warning);
        output.push('\n');
    }

    for child in &node.children {
        render_ascii_node(child, depth + 1, output);
    }

    for leaf in &node.ascii_leaves {
        output.push_str(&"  ".repeat(depth + 1));
        match leaf {
            AsciiLeaf::Cycle(uuid) => {
                output.push_str("! cycle -> ");
                output.push_str(uuid);
            }
            AsciiLeaf::DepthLimit => output.push_str("! depth limit reached"),
        }
        output.push('\n');
    }
}

fn format_ascii_node(node: &TraceNode) -> String {
    let has_dual_id_context = node.session.provider_session_id.is_some()
        || node.session.agent_runner_chain_id.is_some()
        || node.session.resume_input_id.is_some();
    let mut role_fields = Vec::new();
    if has_dual_id_context {
        role_fields.push(format!(
            "agent_runner_invocation={}",
            node.invocation.agent_runner_invocation_id
        ));
    }
    if let Some(provider_session_id) = node.session.provider_session_id.as_deref() {
        role_fields.push(format!("provider_session={provider_session_id}"));
    }
    if let Some(chain_id) = node.session.agent_runner_chain_id.as_deref() {
        role_fields.push(format!("chain={chain_id}"));
    }
    if let Some(resume_input_id) = node.session.resume_input_id.as_deref() {
        role_fields.push(format!("resume_input={resume_input_id}"));
    }
    role_fields.push(format!(
        "session={}",
        node.session.id.as_deref().unwrap_or("—")
    ));
    let session_field = role_fields.join(" ");
    let resume_acceptance = node
        .session
        .resume_acceptance
        .as_deref()
        .map(|status| format!(" resume={status}"))
        .unwrap_or_default();
    format!(
        "{}  {}  {}  {}  {}  {}{}  {}",
        node.invocation.id,
        node.invocation.source.as_deref().unwrap_or("—"),
        node.invocation.model_name,
        node.invocation.status,
        node.invocation
            .started_at
            .to_rfc3339_opts(SecondsFormat::Secs, true),
        session_field,
        resume_acceptance,
        node.session.transcript_state.as_str(),
    )
}

// Tests use `set_permissions` with Unix mode bits to make fixture
// scripts executable. Gate the whole module on Unix; the trace logic
// itself is platform-agnostic, but the test fixtures are not.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::test_support::lock_env;
    use chrono::{DateTime, Duration, Utc};
    use oulipoly_state::{SessionTurnIngest, StateDb};
    use rusqlite::{Connection, params};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    const ROOT_UUID: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    const CHILD_ALPHA_UUID: &str = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
    const CHILD_BETA_UUID: &str = "cccccccc-cccc-cccc-cccc-cccccccccccc";
    const GRANDCHILD_UUID: &str = "dddddddd-dddd-dddd-dddd-dddddddddddd";
    const LEGACY_UUID: &str = "eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee";

    struct FixtureRow<'a> {
        row_id: i64,
        invocation_uuid: &'a str,
        model_name: &'a str,
        provider_name: Option<&'a str>,
        provider_index: i64,
        parent_invocation_id: Option<i64>,
        status: &'a str,
        success: Option<bool>,
        exit_code: Option<i32>,
        error_category: Option<&'a str>,
        terminal_reason: Option<&'a str>,
        created_at: &'a str,
        finished_at: Option<&'a str>,
    }

    struct TraceFixture {
        _dir: tempfile::TempDir,
        db_path: PathBuf,
    }

    impl TraceFixture {
        fn new(rows: &[FixtureRow<'_>]) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let db_path = dir.path().join("state.db");
            let _ = StateDb::open(&db_path).unwrap();
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
            for row in rows {
                conn.execute(
                    "INSERT INTO invocations (
                        id,
                        invocation_uuid,
                        model_name,
                        provider_name,
                        provider_index,
                        parent_invocation_id,
                        status,
                        success,
                        exit_code,
                        error_category,
                        terminal_reason,
                        created_at,
                        finished_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                    params![
                        row.row_id,
                        row.invocation_uuid,
                        row.model_name,
                        row.provider_name,
                        row.provider_index,
                        row.parent_invocation_id,
                        row.status,
                        row.success.map(|value| if value { 1i64 } else { 0i64 }),
                        row.exit_code,
                        row.error_category,
                        row.terminal_reason,
                        row.created_at,
                        row.finished_at,
                    ],
                )
                .unwrap();
            }

            Self { _dir: dir, db_path }
        }

        fn db(&self) -> StateDb {
            StateDb::open(&self.db_path).unwrap()
        }

        fn ingest_session_turns(&self, provider_name: &str, turns: &[SessionTurnIngest]) {
            let db = self.db();
            db.ingest_session_turns_batch(provider_name, turns).unwrap();
        }

        fn set_session_capture(
            &self,
            row_id: i64,
            session_id: Option<&str>,
            capture_method: Option<&str>,
        ) {
            let conn = Connection::open(&self.db_path).unwrap();
            conn.execute(
                "UPDATE invocations
                 SET session_id = ?1, session_capture_method = ?2
                 WHERE id = ?3",
                params![session_id, capture_method, row_id],
            )
            .unwrap();
        }

        fn set_resume_acceptance(&self, row_id: i64, status: &str, evidence: Option<&str>) {
            let db = self.db();
            db.update_resume_acceptance(row_id, status, evidence)
                .unwrap();
        }

        fn seed_chain_segment(&self, chain_id: &str, provider_name: &str, session_id: &str) {
            let conn = Connection::open(&self.db_path).unwrap();
            conn.execute(
                "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
                 VALUES (?1, '2026-04-17T08:00:00Z', '2026-04-17T08:00:00Z', 'fixture')",
                params![chain_id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO session_chain_segments
                    (chain_id, provider_name, session_id, started_at, transition_reason)
                 VALUES (?1, ?2, ?3, '2026-04-17T08:00:00Z', 'initial')",
                params![chain_id, provider_name, session_id],
            )
            .unwrap();
        }

        fn set_exit_status(&self, row_id: i64, status: &str, success: bool, exit_code: i32) {
            let conn = Connection::open(&self.db_path).unwrap();
            conn.execute(
                "UPDATE invocations
                 SET status = ?1, success = ?2, exit_code = ?3
                 WHERE id = ?4",
                params![status, success, exit_code, row_id],
            )
            .unwrap();
        }
    }

    fn write_sessions_config(config_home: &std::path::Path, body: &str) {
        let app_dir = config_home.join("oulipoly-agent-runner");
        fs::create_dir_all(&app_dir).unwrap();
        fs::write(app_dir.join("sessions.toml"), body).unwrap();
    }

    fn fixture_script(dir: &tempfile::TempDir, name: &str, body: &str) -> PathBuf {
        let path = dir.path().join(name);
        fs::write(
            &path,
            format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n"),
        )
        .unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
        path
    }

    struct IsolatedDataDir {
        _lock: std::sync::MutexGuard<'static, ()>,
        old_data_dir: Option<std::ffi::OsString>,
    }

    impl IsolatedDataDir {
        fn new(data_home: &std::path::Path) -> Self {
            let data_root = data_home.join(oulipoly_state::paths::APP_DATA_DIR_NAME);
            let lock = lock_env();
            let old_data_dir = std::env::var_os(oulipoly_state::paths::DATA_DIR_ENV);
            unsafe {
                std::env::set_var(oulipoly_state::paths::DATA_DIR_ENV, data_root);
            }
            Self {
                _lock: lock,
                old_data_dir,
            }
        }
    }

    impl Drop for IsolatedDataDir {
        fn drop(&mut self) {
            unsafe {
                match self.old_data_dir.take() {
                    Some(value) => std::env::set_var(oulipoly_state::paths::DATA_DIR_ENV, value),
                    None => std::env::remove_var(oulipoly_state::paths::DATA_DIR_ENV),
                }
            }
        }
    }

    fn trace_options(max_depth: usize) -> TraceOptions {
        TraceOptions {
            max_depth,
            json: false,
            inline_transcript: false,
            transcript: false,
        }
    }

    fn build_resumed_trace_report(options: TraceOptions) -> TraceReport {
        build_resumed_trace_report_with_exit(options, None)
    }

    fn build_resumed_trace_report_with_exit(
        options: TraceOptions,
        exit_override: Option<(&str, bool, i32)>,
    ) -> TraceReport {
        let fixture = TraceFixture::new(&base_rows());
        fixture.set_session_capture(
            1,
            Some("5169694d-de0f-40d1-890c-6e28e55bab27"),
            Some("resumed"),
        );
        if let Some((status, success, exit_code)) = exit_override {
            fixture.set_exit_status(1, status, success, exit_code);
        }
        fixture.ingest_session_turns(
            "fixture-provider",
            &[
                SessionTurnIngest {
                    session_id: "5169694d-de0f-40d1-890c-6e28e55bab27".to_string(),
                    turn_id: "root-turn".to_string(),
                    timestamp: DateTime::parse_from_rfc3339("2026-04-17T08:00:00Z")
                        .unwrap()
                        .with_timezone(&Utc),
                    role: "user".to_string(),
                    parent_turn_id: None,
                    is_sidechain: false,
                    is_compaction_boundary: false,
                    body: None,
                },
                SessionTurnIngest {
                    session_id: "5169694d-de0f-40d1-890c-6e28e55bab27".to_string(),
                    turn_id: "assistant-main".to_string(),
                    timestamp: DateTime::parse_from_rfc3339("2026-04-17T08:00:01Z")
                        .unwrap()
                        .with_timezone(&Utc),
                    role: "assistant".to_string(),
                    parent_turn_id: Some("root-turn".to_string()),
                    is_sidechain: false,
                    is_compaction_boundary: false,
                    body: None,
                },
                SessionTurnIngest {
                    session_id: "5169694d-de0f-40d1-890c-6e28e55bab27".to_string(),
                    turn_id: "assistant-side".to_string(),
                    timestamp: DateTime::parse_from_rfc3339("2026-04-17T08:00:02Z")
                        .unwrap()
                        .with_timezone(&Utc),
                    role: "assistant".to_string(),
                    parent_turn_id: Some("assistant-main".to_string()),
                    is_sidechain: true,
                    is_compaction_boundary: false,
                    body: None,
                },
            ],
        );

        let env_dir = tempfile::tempdir().unwrap();
        let transcript = env_dir.path().join("resume-session.jsonl");
        fs::write(&transcript, "{\"type\":\"system\"}\n").unwrap();
        let locator = fixture_script(
            &env_dir,
            "resume-locator.sh",
            &format!(r#"printf '%s\n' "{}""#, transcript.display()),
        );
        write_sessions_config(
            env_dir.path(),
            &format!(
                r#"[fixture-provider]
turn_script = "ignored"
transcript_locator = "{}"
"#,
                locator.display()
            ),
        );

        let sessions_cfg = SessionsConfig::load(
            &env_dir
                .path()
                .join("oulipoly-agent-runner")
                .join("sessions.toml"),
        )
        .unwrap();
        let db = fixture.db();

        trace_invocation_with_sessions(&db, ROOT_UUID, options, Some(&sessions_cfg)).unwrap()
    }

    fn base_rows() -> Vec<FixtureRow<'static>> {
        vec![
            FixtureRow {
                row_id: 1,
                invocation_uuid: ROOT_UUID,
                model_name: "fixture-root",
                provider_name: Some("fixture-provider"),
                provider_index: 0,
                parent_invocation_id: None,
                status: "succeeded",
                success: Some(true),
                exit_code: Some(0),
                error_category: None,
                terminal_reason: None,
                created_at: "2026-04-17T08:00:00Z",
                finished_at: Some("2026-04-17T08:00:05Z"),
            },
            FixtureRow {
                row_id: 2,
                invocation_uuid: CHILD_BETA_UUID,
                model_name: "fixture-child-beta",
                provider_name: Some("fixture-provider"),
                provider_index: 0,
                parent_invocation_id: Some(1),
                status: "failed",
                success: Some(false),
                exit_code: Some(7),
                error_category: Some("fixture_error"),
                terminal_reason: Some("exit_nonzero"),
                created_at: "2026-04-17T08:00:02Z",
                finished_at: Some("2026-04-17T08:00:06Z"),
            },
            FixtureRow {
                row_id: 3,
                invocation_uuid: CHILD_ALPHA_UUID,
                model_name: "fixture-child-alpha",
                provider_name: Some("fixture-provider"),
                provider_index: 0,
                parent_invocation_id: Some(1),
                status: "running",
                success: None,
                exit_code: None,
                error_category: None,
                terminal_reason: None,
                created_at: "2026-04-17T08:00:01Z",
                finished_at: None,
            },
            FixtureRow {
                row_id: 4,
                invocation_uuid: GRANDCHILD_UUID,
                model_name: "fixture-grandchild",
                provider_name: Some("fixture-provider"),
                provider_index: 0,
                parent_invocation_id: Some(3),
                status: "succeeded",
                success: Some(true),
                exit_code: Some(0),
                error_category: None,
                terminal_reason: None,
                created_at: "2026-04-17T08:00:03Z",
                finished_at: Some("2026-04-17T08:00:04Z"),
            },
        ]
    }

    fn legacy_root_rows() -> Vec<FixtureRow<'static>> {
        vec![FixtureRow {
            row_id: 10,
            invocation_uuid: LEGACY_UUID,
            model_name: "legacy-model",
            provider_name: None,
            provider_index: 0,
            parent_invocation_id: None,
            status: "legacy",
            success: Some(false),
            exit_code: Some(1),
            error_category: Some("legacy"),
            terminal_reason: None,
            created_at: "2026-04-17T09:00:00Z",
            finished_at: Some("2026-04-17T09:00:01Z"),
        }]
    }

    fn cycle_rows() -> Vec<FixtureRow<'static>> {
        vec![
            FixtureRow {
                row_id: 1,
                invocation_uuid: ROOT_UUID,
                model_name: "fixture-root",
                provider_name: Some("fixture-provider"),
                provider_index: 0,
                parent_invocation_id: Some(4),
                status: "succeeded",
                success: Some(true),
                exit_code: Some(0),
                error_category: None,
                terminal_reason: None,
                created_at: "2026-04-17T08:00:00Z",
                finished_at: Some("2026-04-17T08:00:05Z"),
            },
            FixtureRow {
                row_id: 2,
                invocation_uuid: CHILD_ALPHA_UUID,
                model_name: "fixture-child-alpha",
                provider_name: Some("fixture-provider"),
                provider_index: 0,
                parent_invocation_id: Some(1),
                status: "running",
                success: None,
                exit_code: None,
                error_category: None,
                terminal_reason: None,
                created_at: "2026-04-17T08:00:01Z",
                finished_at: None,
            },
            FixtureRow {
                row_id: 4,
                invocation_uuid: GRANDCHILD_UUID,
                model_name: "fixture-grandchild",
                provider_name: Some("fixture-provider"),
                provider_index: 0,
                parent_invocation_id: Some(2),
                status: "succeeded",
                success: Some(true),
                exit_code: Some(0),
                error_category: None,
                terminal_reason: None,
                created_at: "2026-04-17T08:00:02Z",
                finished_at: Some("2026-04-17T08:00:03Z"),
            },
        ]
    }

    #[test]
    fn single_root_with_no_children_emits_one_node() {
        let fixture = TraceFixture::new(&legacy_root_rows());
        let db = fixture.db();

        let report = trace_invocation(&db, LEGACY_UUID, trace_options(64)).unwrap();

        assert_eq!(report.root.invocation.id, LEGACY_UUID);
        assert!(report.root.children.is_empty());
    }

    #[test]
    fn root_with_children_is_sorted_by_created_at_then_row_id() {
        let fixture = TraceFixture::new(&base_rows());
        let db = fixture.db();

        let report = trace_invocation(&db, ROOT_UUID, trace_options(64)).unwrap();

        let child_ids: Vec<&str> = report
            .root
            .children
            .iter()
            .map(|child| child.invocation.id.as_str())
            .collect();
        assert_eq!(child_ids, vec![CHILD_ALPHA_UUID, CHILD_BETA_UUID]);
    }

    #[test]
    fn three_level_tree_walk_nests_children_under_their_parent() {
        let fixture = TraceFixture::new(&base_rows());
        let db = fixture.db();

        let report = trace_invocation(&db, ROOT_UUID, trace_options(64)).unwrap();

        assert_eq!(report.root.children.len(), 2);
        assert_eq!(report.root.children[0].invocation.id, CHILD_ALPHA_UUID);
        assert_eq!(report.root.children[0].children.len(), 1);
        assert_eq!(
            report.root.children[0].children[0].invocation.id,
            GRANDCHILD_UUID
        );
    }

    #[test]
    fn cycle_leaf_is_emitted_without_descending_forever() {
        let fixture = TraceFixture::new(&cycle_rows());
        let db = fixture.db();

        let report = trace_invocation(&db, ROOT_UUID, trace_options(64)).unwrap();
        let ascii = render_ascii_trace(&report);

        assert!(ascii.contains("! cycle ->"));
        assert!(ascii.contains(ROOT_UUID));
        assert!(
            report
                .root
                .warnings
                .iter()
                .any(|warning| warning.contains("cycle"))
        );
    }

    #[test]
    fn depth_limit_leaf_is_emitted_when_requested() {
        let fixture = TraceFixture::new(&base_rows());
        let db = fixture.db();

        let report = trace_invocation(&db, ROOT_UUID, trace_options(1)).unwrap();
        let ascii = render_ascii_trace(&report);

        assert!(ascii.contains("! depth limit reached"), "{ascii}");
        assert!(
            report
                .root
                .warnings
                .iter()
                .any(|warning| warning.contains("depth limit"))
        );
    }

    #[test]
    fn ascii_format_matches_single_node_contract() {
        let fixture = TraceFixture::new(&legacy_root_rows());
        let db = fixture.db();

        let report = trace_invocation(&db, LEGACY_UUID, trace_options(64)).unwrap();
        let ascii = render_ascii_trace(&report);

        assert_eq!(
            ascii.trim_end(),
            "eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee  —  legacy-model  legacy  2026-04-17T09:00:00Z  session=—  unresolved"
        );
        assert!(!ascii.contains(" resume="), "{ascii}");
    }

    #[test]
    fn ascii_indents_nested_children() {
        let fixture = TraceFixture::new(&base_rows());
        let db = fixture.db();

        let report = trace_invocation(&db, ROOT_UUID, trace_options(64)).unwrap();
        let ascii = render_ascii_trace(&report);
        let lines: Vec<&str> = ascii.lines().collect();

        assert_eq!(
            lines[0],
            "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa  fixture-provider  fixture-root  succeeded  2026-04-17T08:00:00Z  session=—  unresolved"
        );
        assert!(lines[1].starts_with("  "), "{ascii}");
        assert!(lines[1].contains(CHILD_ALPHA_UUID), "{ascii}");
        assert!(lines[2].starts_with("    "), "{ascii}");
        assert!(lines[2].contains(GRANDCHILD_UUID), "{ascii}");
        assert!(!ascii.contains(" resume="), "{ascii}");
    }

    #[test]
    fn legacy_rows_render_provider_dash() {
        let fixture = TraceFixture::new(&legacy_root_rows());
        let db = fixture.db();

        let report = trace_invocation(&db, LEGACY_UUID, trace_options(64)).unwrap();
        let ascii = render_ascii_trace(&report);

        assert!(ascii.contains("  —  legacy-model  legacy  "), "{ascii}");
    }

    #[test]
    fn json_output_has_top_level_shape_and_nested_children() {
        let fixture = TraceFixture::new(&base_rows());
        let db = fixture.db();

        let report = trace_invocation(
            &db,
            ROOT_UUID,
            TraceOptions {
                max_depth: 64,
                json: true,
                inline_transcript: false,
                transcript: false,
            },
        )
        .unwrap();
        let json = serde_json::to_value(&report).unwrap();

        assert_eq!(json["requested_id"], ROOT_UUID);
        assert!(DateTime::parse_from_rfc3339(json["generated_at"].as_str().unwrap()).is_ok());
        assert_eq!(json["root"]["invocation"]["id"], ROOT_UUID);
        assert_eq!(
            json["root"]["children"][0]["invocation"]["id"],
            CHILD_ALPHA_UUID
        );
        assert_eq!(
            json["root"]["children"][0]["children"][0]["invocation"]["id"],
            GRANDCHILD_UUID
        );
    }

    // RISK: trace JSON could drop existing fields while adding terminal_reason/stale_running (proposal §test-intent "JSON contract test", assumptions A6/A7)
    // LEVEL: unit
    // SOURCE: contracts/nes-250-contract.md § Test catalog § Trace JSON contract (T-TRACE-JSON-ADDITIVE)
    #[test]
    fn t_trace_json_additive_keeps_existing_keys_and_adds_terminal_reason() {
        let fixture = TraceFixture::new(&base_rows());
        let db = fixture.db();

        let report = trace_invocation(
            &db,
            ROOT_UUID,
            TraceOptions {
                max_depth: 64,
                json: true,
                inline_transcript: false,
                transcript: false,
            },
        )
        .unwrap();
        let json = serde_json::to_value(&report).unwrap();
        let root = json["root"]["invocation"].as_object().unwrap();
        for key in [
            "row_id",
            "id",
            "source",
            "model_name",
            "parent_id",
            "status",
            "success",
            "exit_code",
            "error_category",
            "started_at",
            "finished_at",
        ] {
            assert!(
                root.contains_key(key),
                "missing existing key {key}: {root:?}"
            );
        }
        assert!(root.contains_key("terminal_reason"));
        assert!(root["terminal_reason"].is_null());

        let failed_child = &json["root"]["children"][1]["invocation"];
        assert_eq!(failed_child["status"], "failed");
        assert_eq!(failed_child["error_category"], "fixture_error");
        assert_eq!(failed_child["terminal_reason"], "exit_nonzero");
    }

    // RISK: non-stale running rows could lose the explicit null-terminal JSON contract (proposal §test-intent "terminal-reason absence characterization", assumption A5)
    // LEVEL: unit
    // SOURCE: contracts/nes-250-contract.md § Test catalog § Trace JSON contract (T-TRACE-JSON-RUNNING-NON-STALE-NULL)
    #[test]
    fn json_running_row_uses_null_terminal_fields() {
        let fixture = TraceFixture::new(&base_rows());
        let db = fixture.db();

        let report = trace_invocation(
            &db,
            ROOT_UUID,
            TraceOptions {
                max_depth: 64,
                json: true,
                inline_transcript: false,
                transcript: false,
            },
        )
        .unwrap();
        let json = serde_json::to_value(&report).unwrap();
        let running = &json["root"]["children"][0]["invocation"];

        assert_eq!(running["status"], "running");
        assert!(running["success"].is_null());
        assert!(running["exit_code"].is_null());
        assert!(running["terminal_reason"].is_null());
        assert!(running["finished_at"].is_null());
        assert!(
            !running.as_object().unwrap().contains_key("stale_running"),
            "T-TRACE-JSON-RUNNING-NON-STALE-NULL omits stale_running for fresh running rows"
        );
    }

    // RISK: stale-running JSON lift could leave audit JSON in stored running/null shape (proposal §test-intent "Replace stale-running trace characterization", assumptions A5/A7)
    // LEVEL: unit
    // SOURCE: contracts/nes-250-contract.md § Test catalog § Stale-running JSON lift (T-STALE-LIFT-JSON)
    #[test]
    fn t_stale_lift_json_projects_old_running_row_to_failed_with_reason() {
        let created_at = (Utc::now() - Duration::minutes(31)).to_rfc3339();
        let rows = [FixtureRow {
            row_id: 1,
            invocation_uuid: ROOT_UUID,
            model_name: "fixture-root",
            provider_name: Some("fixture-provider"),
            provider_index: 0,
            parent_invocation_id: None,
            status: "running",
            success: None,
            exit_code: None,
            error_category: None,
            terminal_reason: None,
            created_at: &created_at,
            finished_at: None,
        }];
        let fixture = TraceFixture::new(&rows);
        let db = fixture.db();

        let report = trace_invocation(
            &db,
            ROOT_UUID,
            TraceOptions {
                max_depth: 64,
                json: true,
                inline_transcript: false,
                transcript: false,
            },
        )
        .unwrap();
        let json = serde_json::to_value(&report).unwrap();
        let invocation = &json["root"]["invocation"];

        assert_eq!(invocation["status"], "failed");
        assert!(invocation["success"].is_null());
        assert!(invocation["exit_code"].is_null());
        assert!(invocation["finished_at"].is_null());
        assert_eq!(invocation["terminal_reason"], "tracing_timeout");
        assert!(
            invocation["stale_running"]["age_seconds"].as_u64().unwrap()
                >= STALE_RUNNING_THRESHOLD_SECONDS
        );
        assert_eq!(
            invocation["stale_running"]["threshold_seconds"],
            STALE_RUNNING_THRESHOLD_SECONDS
        );
        assert!(
            json["root"]["warnings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|warning| warning.as_str().unwrap().contains("stale_running")),
            "T-STALE-LIFT-JSON emits stable stale_running warning"
        );
    }

    // RISK: stale-running JSON lift could mutate durable invocation state (proposal §test-intent "JSON-only stale-lift isolation test", assumption A5)
    // LEVEL: unit
    // SOURCE: contracts/nes-250-contract.md § Test catalog § Stale-running JSON lift (T-STALE-LIFT-DB-UNCHANGED)
    #[test]
    fn t_stale_lift_db_unchanged_after_json_trace() {
        let created_at = (Utc::now() - Duration::minutes(31)).to_rfc3339();
        let rows = [FixtureRow {
            row_id: 1,
            invocation_uuid: ROOT_UUID,
            model_name: "fixture-root",
            provider_name: Some("fixture-provider"),
            provider_index: 0,
            parent_invocation_id: None,
            status: "running",
            success: None,
            exit_code: None,
            error_category: None,
            terminal_reason: None,
            created_at: &created_at,
            finished_at: None,
        }];
        let fixture = TraceFixture::new(&rows);
        let db = fixture.db();

        let _ = trace_invocation(
            &db,
            ROOT_UUID,
            TraceOptions {
                max_depth: 64,
                json: true,
                inline_transcript: false,
                transcript: false,
            },
        )
        .unwrap();

        let row = db.get_invocation_by_uuid(ROOT_UUID).unwrap().unwrap();
        assert_eq!(row.status, oulipoly_state::InvocationStatus::Running);
        assert_eq!(row.success, None);
        assert_eq!(row.exit_code, None);
        assert_eq!(row.terminal_reason, None);
        assert_eq!(row.finished_at, None);
    }

    // RISK: stale-running lift could leak JSON failed projection into human trace output (proposal §test-intent "JSON-only stale-lift isolation test", assumptions A5/A7)
    // LEVEL: unit
    // SOURCE: contracts/nes-250-contract.md § Test catalog § Stale-running JSON lift (T-STALE-LIFT-ASCII)
    #[test]
    fn t_stale_lift_ascii_preserves_running_status_and_warns() {
        let created_at = (Utc::now() - Duration::minutes(31)).to_rfc3339();
        let rows = [FixtureRow {
            row_id: 1,
            invocation_uuid: ROOT_UUID,
            model_name: "fixture-root",
            provider_name: Some("fixture-provider"),
            provider_index: 0,
            parent_invocation_id: None,
            status: "running",
            success: None,
            exit_code: None,
            error_category: None,
            terminal_reason: None,
            created_at: &created_at,
            finished_at: None,
        }];
        let fixture = TraceFixture::new(&rows);
        let db = fixture.db();

        let report = trace_invocation(&db, ROOT_UUID, trace_options(64)).unwrap();
        let ascii = render_ascii_trace(&report);

        assert!(ascii.contains("  running  "), "{ascii}");
        assert!(!ascii.contains("  failed  "), "{ascii}");
        assert!(ascii.contains("stale_running"), "{ascii}");
        assert!(
            report
                .root
                .warnings
                .iter()
                .any(|warning| warning.contains("stale_running"))
        );
    }

    // RISK: fixed stale threshold could classify fresh running rows as failed (proposal §test-intent "Replace stale-running trace characterization", assumption A7)
    // LEVEL: unit
    // SOURCE: contracts/nes-250-contract.md § Test catalog § Stale-running JSON lift (T-STALE-NOT-STALE)
    #[test]
    fn t_stale_not_stale_running_row_stays_running_without_stale_object() {
        let created_at = (Utc::now() - Duration::minutes(5)).to_rfc3339();
        let rows = [FixtureRow {
            row_id: 1,
            invocation_uuid: ROOT_UUID,
            model_name: "fixture-root",
            provider_name: Some("fixture-provider"),
            provider_index: 0,
            parent_invocation_id: None,
            status: "running",
            success: None,
            exit_code: None,
            error_category: None,
            terminal_reason: None,
            created_at: &created_at,
            finished_at: None,
        }];
        let fixture = TraceFixture::new(&rows);
        let db = fixture.db();

        let report = trace_invocation(
            &db,
            ROOT_UUID,
            TraceOptions {
                max_depth: 64,
                json: true,
                inline_transcript: false,
                transcript: false,
            },
        )
        .unwrap();
        let json = serde_json::to_value(&report).unwrap();
        let invocation = json["root"]["invocation"].as_object().unwrap();

        assert_eq!(invocation["status"], "running");
        assert!(invocation["terminal_reason"].is_null());
        assert!(!invocation.contains_key("stale_running"));
        assert!(json["root"]["warnings"].as_array().unwrap().is_empty());
    }

    #[test]
    fn json_session_fields_are_null_or_unresolved_in_pr_b() {
        let fixture = TraceFixture::new(&base_rows());
        let db = fixture.db();

        let report = trace_invocation(
            &db,
            ROOT_UUID,
            TraceOptions {
                max_depth: 64,
                json: true,
                inline_transcript: false,
                transcript: false,
            },
        )
        .unwrap();
        let json = serde_json::to_value(&report).unwrap();
        let session = &json["root"]["session"];

        assert!(
            json["root"]["invocation"]
                .get("agent_runner_invocation_id")
                .is_some()
        );
        assert!(session["id"].is_null());
        assert!(session.get("provider_session_id").is_some());
        assert!(session["provider_session_id"].is_null());
        assert!(session.get("resume_input_id").is_some());
        assert!(session["resume_input_id"].is_null());
        assert!(session.get("agent_runner_chain_id").is_some());
        assert!(session["agent_runner_chain_id"].is_null());
        assert!(session["capture_method"].is_null());
        assert!(session["transcript_path"].is_null());
        assert_eq!(session["transcript_state"], "unresolved");
        assert!(session["turn_count"].is_null());
        assert!(session["assistant_turn_count"].is_null());
        assert!(session["sidechain_turn_count"].is_null());
    }

    #[test]
    fn human_mode_transcript_footer_uses_unresolved_placeholder() {
        let fixture = TraceFixture::new(&base_rows());
        let db = fixture.db();

        let report = trace_invocation(
            &db,
            ROOT_UUID,
            TraceOptions {
                max_depth: 64,
                json: false,
                inline_transcript: false,
                transcript: true,
            },
        )
        .unwrap();
        let ascii = render_ascii_trace(&report);

        assert!(ascii.contains("=== Transcript: aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa ==="));
        assert!(
            ascii.contains("(no transcript: session_id unresolved"),
            "{ascii}"
        );
    }

    #[test]
    fn inline_transcript_reports_mixed_body_states_and_empty_arrays() {
        // risk: mixed-state regression; level: component; source: contract §4 T10 / proposal A6.
        let fixture = TraceFixture::new(&base_rows());
        fixture.set_session_capture(
            1,
            Some("5169694d-de0f-40d1-890c-6e28e55bab27"),
            Some("forced_flag_verified"),
        );
        fixture.ingest_session_turns(
            "fixture-provider",
            &[
                SessionTurnIngest {
                    session_id: "5169694d-de0f-40d1-890c-6e28e55bab27".to_string(),
                    turn_id: "legacy-turn".to_string(),
                    timestamp: DateTime::parse_from_rfc3339("2026-04-17T08:00:00Z")
                        .unwrap()
                        .with_timezone(&Utc),
                    role: "user".to_string(),
                    parent_turn_id: None,
                    is_sidechain: false,
                    is_compaction_boundary: false,
                    body: None,
                },
                SessionTurnIngest {
                    session_id: "5169694d-de0f-40d1-890c-6e28e55bab27".to_string(),
                    turn_id: "body-turn".to_string(),
                    timestamp: DateTime::parse_from_rfc3339("2026-04-17T08:00:01Z")
                        .unwrap()
                        .with_timezone(&Utc),
                    role: "assistant".to_string(),
                    parent_turn_id: Some("legacy-turn".to_string()),
                    is_sidechain: false,
                    is_compaction_boundary: false,
                    body: Some(
                        r#"[{"type":"text","text":"db stored assistant body"}]"#.to_string(),
                    ),
                },
            ],
        );
        let db = fixture.db();

        let report = trace_invocation(
            &db,
            ROOT_UUID,
            TraceOptions {
                max_depth: 64,
                json: true,
                inline_transcript: true,
                transcript: false,
            },
        )
        .unwrap();
        let json = serde_json::to_value(&report).unwrap();

        let transcript = json["root"]["transcript"].as_array().unwrap();
        assert!(transcript.iter().any(|turn| {
            turn["turn_id"] == "legacy-turn"
                && turn["role"] == "user"
                && turn["body_state"] == "missing"
                && turn["content"].is_null()
        }));
        assert!(transcript.iter().any(|turn| {
            turn["turn_id"] == "body-turn"
                && turn["role"] == "assistant"
                && turn["body_state"] == "stored"
                && turn["content"]
                    == serde_json::json!([{"type":"text","text":"db stored assistant body"}])
        }));
        assert_eq!(
            json["root"]["children"][0]["transcript"]
                .as_array()
                .unwrap()
                .len(),
            0,
            "inline transcript nodes with zero session_turns rows use an empty array"
        );
    }

    #[test]
    fn json_output_reports_available_transcript_when_locator_finds_existing_file() {
        let fixture = TraceFixture::new(&base_rows());
        fixture.set_session_capture(
            1,
            Some("5169694d-de0f-40d1-890c-6e28e55bab27"),
            Some("forced_flag_verified"),
        );
        let db = fixture.db();

        let env_dir = tempfile::tempdir().unwrap();
        let transcript = env_dir.path().join("trace-session.jsonl");
        fs::write(&transcript, "{\"type\":\"system\"}\n").unwrap();
        let _data_dir = IsolatedDataDir::new(env_dir.path());
        let locator = fixture_script(
            &env_dir,
            "locator.sh",
            &format!(r#"printf '%s\n' "{}""#, transcript.display()),
        );
        write_sessions_config(
            env_dir.path(),
            &format!(
                r#"[fixture-provider]
turn_script = "ignored"
transcript_locator = "{}"
"#,
                locator.display()
            ),
        );

        let sessions_cfg = SessionsConfig::load(
            &env_dir
                .path()
                .join("oulipoly-agent-runner")
                .join("sessions.toml"),
        )
        .unwrap();
        let report = trace_invocation_with_sessions(
            &db,
            ROOT_UUID,
            TraceOptions {
                max_depth: 64,
                json: true,
                inline_transcript: false,
                transcript: false,
            },
            Some(&sessions_cfg),
        )
        .unwrap();
        let json = serde_json::to_value(&report).unwrap();

        assert_eq!(
            json["root"]["session"]["id"],
            "5169694d-de0f-40d1-890c-6e28e55bab27"
        );
        assert_eq!(
            json["root"]["session"]["capture_method"],
            "forced_flag_verified"
        );
        assert_eq!(json["root"]["session"]["transcript_state"], "available");
        assert_eq!(
            json["root"]["session"]["transcript_path"],
            transcript.display().to_string()
        );
    }

    #[test]
    fn json_output_reports_no_locator_when_session_id_is_present_but_unconfigured() {
        let fixture = TraceFixture::new(&base_rows());
        fixture.set_session_capture(
            1,
            Some("019d9d09-2de9-7902-b148-f9f3bed4fa41"),
            Some("stdout_json_event"),
        );
        let db = fixture.db();

        let report = trace_invocation(
            &db,
            ROOT_UUID,
            TraceOptions {
                max_depth: 64,
                json: true,
                inline_transcript: false,
                transcript: false,
            },
        )
        .unwrap();
        let json = serde_json::to_value(&report).unwrap();

        assert_eq!(
            json["root"]["session"]["id"],
            "019d9d09-2de9-7902-b148-f9f3bed4fa41"
        );
        assert_eq!(
            json["root"]["session"]["capture_method"],
            "stdout_json_event"
        );
        assert_eq!(json["root"]["session"]["transcript_state"], "no_locator");
        assert!(json["root"]["session"]["transcript_path"].is_null());
    }

    /// `transcript_state = "missing"` — locator returns a path but the
    /// file doesn't exist on disk. V10 says degraded states must be
    /// observable; this exercise confirms the runtime distinguishes
    /// "missing" from "available" rather than collapsing them.
    #[test]
    fn json_output_reports_missing_when_locator_path_does_not_exist() {
        let fixture = TraceFixture::new(&base_rows());
        fixture.set_session_capture(
            1,
            Some("5169694d-de0f-40d1-890c-6e28e55bab27"),
            Some("forced_flag_verified"),
        );
        let db = fixture.db();

        let env_dir = tempfile::tempdir().unwrap();
        let _data_dir = IsolatedDataDir::new(env_dir.path());
        // Locator points at a path that does NOT exist on disk.
        let nonexistent = env_dir.path().join("does-not-exist.jsonl");
        let locator = fixture_script(
            &env_dir,
            "missing-locator.sh",
            &format!(r#"printf '%s\n' "{}""#, nonexistent.display()),
        );
        write_sessions_config(
            env_dir.path(),
            &format!(
                r#"[fixture-provider]
turn_script = "ignored"
transcript_locator = "{}"
"#,
                locator.display()
            ),
        );

        let sessions_cfg = SessionsConfig::load(
            &env_dir
                .path()
                .join("oulipoly-agent-runner")
                .join("sessions.toml"),
        )
        .unwrap();
        let report = trace_invocation_with_sessions(
            &db,
            ROOT_UUID,
            TraceOptions {
                max_depth: 64,
                json: true,
                inline_transcript: false,
                transcript: false,
            },
            Some(&sessions_cfg),
        )
        .unwrap();
        let json = serde_json::to_value(&report).unwrap();

        assert_eq!(
            json["root"]["session"]["id"],
            "5169694d-de0f-40d1-890c-6e28e55bab27"
        );
        assert_eq!(json["root"]["session"]["transcript_state"], "missing");
        // transcript_path may be the unresolved path (as a hint) or
        // null — both are acceptable per the contract; assert it isn't
        // misleadingly reported as available.
        assert_ne!(json["root"]["session"]["transcript_state"], "available");
    }

    #[test]
    fn json_output_populates_sidechain_turn_count_from_session_turns() {
        let fixture = TraceFixture::new(&base_rows());
        fixture.set_session_capture(
            1,
            Some("5169694d-de0f-40d1-890c-6e28e55bab27"),
            Some("forced_flag_verified"),
        );
        fixture.ingest_session_turns(
            "fixture-provider",
            &[
                SessionTurnIngest {
                    session_id: "5169694d-de0f-40d1-890c-6e28e55bab27".to_string(),
                    turn_id: "root-turn".to_string(),
                    timestamp: DateTime::parse_from_rfc3339("2026-04-17T08:00:00Z")
                        .unwrap()
                        .with_timezone(&Utc),
                    role: "user".to_string(),
                    parent_turn_id: None,
                    is_sidechain: false,
                    is_compaction_boundary: false,
                    body: None,
                },
                SessionTurnIngest {
                    session_id: "5169694d-de0f-40d1-890c-6e28e55bab27".to_string(),
                    turn_id: "assistant-main".to_string(),
                    timestamp: DateTime::parse_from_rfc3339("2026-04-17T08:00:01Z")
                        .unwrap()
                        .with_timezone(&Utc),
                    role: "assistant".to_string(),
                    parent_turn_id: Some("root-turn".to_string()),
                    is_sidechain: false,
                    is_compaction_boundary: false,
                    body: None,
                },
                SessionTurnIngest {
                    session_id: "5169694d-de0f-40d1-890c-6e28e55bab27".to_string(),
                    turn_id: "assistant-side".to_string(),
                    timestamp: DateTime::parse_from_rfc3339("2026-04-17T08:00:02Z")
                        .unwrap()
                        .with_timezone(&Utc),
                    role: "assistant".to_string(),
                    parent_turn_id: Some("assistant-main".to_string()),
                    is_sidechain: true,
                    is_compaction_boundary: false,
                    body: None,
                },
            ],
        );
        let db = fixture.db();

        let report = trace_invocation(
            &db,
            ROOT_UUID,
            TraceOptions {
                max_depth: 64,
                json: true,
                inline_transcript: false,
                transcript: false,
            },
        )
        .unwrap();
        let json = serde_json::to_value(&report).unwrap();

        assert_eq!(json["root"]["session"]["turn_count"], 3);
        assert_eq!(json["root"]["session"]["assistant_turn_count"], 2);
        assert_eq!(json["root"]["session"]["sidechain_turn_count"], 1);
    }

    #[test]
    fn resumed_session_pushes_attempted_resume_warning() {
        let report = build_resumed_trace_report(trace_options(64));

        assert!(
            report
                .root
                .warnings
                .iter()
                .any(|warning| warning.contains("attempted resume target")),
            "{:?}",
            report.root.warnings
        );
    }

    #[test]
    fn ascii_output_renders_non_stale_session_warnings() {
        let report = build_resumed_trace_report(trace_options(64));
        let ascii = render_ascii_trace(&report);

        assert!(
            ascii.contains("attempted resume target"),
            "ASCII trace should surface non-stale session warnings: {ascii}"
        );
    }

    #[test]
    fn resumed_session_still_resolves_transcript_state() {
        let report = build_resumed_trace_report(trace_options(64));

        assert!(matches!(
            report.root.session.transcript_state,
            TranscriptState::Available
        ));
        assert!(report.root.session.transcript_path.is_some());
    }

    #[test]
    fn resumed_session_still_counts_turns() {
        let report = build_resumed_trace_report(trace_options(64));

        assert_eq!(report.root.session.turn_count, Some(3));
        assert_eq!(report.root.session.assistant_turn_count, Some(2));
        assert_eq!(report.root.session.sidechain_turn_count, Some(1));
    }

    #[test]
    fn ascii_output_uses_resume_target_label_for_resumed_session() {
        let fixture = TraceFixture::new(&base_rows());
        fixture.set_session_capture(
            1,
            Some("5169694d-de0f-40d1-890c-6e28e55bab27"),
            Some("resumed"),
        );
        fixture.set_resume_acceptance(1, "accepted", Some("matched session id"));
        let report = trace_invocation(&fixture.db(), ROOT_UUID, trace_options(64)).unwrap();
        let ascii = render_ascii_trace(&report);

        assert!(
            ascii.contains("provider_session=5169694d-de0f-40d1-890c-6e28e55bab27"),
            "{ascii}"
        );
        assert!(
            ascii.contains("resume_input=5169694d-de0f-40d1-890c-6e28e55bab27"),
            "{ascii}"
        );
        assert!(ascii.contains("agent_runner_invocation="), "{ascii}");
        assert!(ascii.contains(" resume=accepted"), "{ascii}");
    }

    #[test]
    fn resumed_session_warning_persists_when_invocation_exited_nonzero() {
        // Per PR-F contract §test-contract item 9: trace must surface the
        // attempted-resume warning specifically when the invocation row
        // shows the child failed to attach (non-zero exit). Verifies
        // build_trace_session does NOT short-circuit the resume warning
        // based on success/exit_code.
        let fixture = TraceFixture::new(&base_rows());
        fixture.set_session_capture(
            1,
            Some("5169694d-de0f-40d1-890c-6e28e55bab27"),
            Some("resumed"),
        );
        fixture.set_exit_status(1, "failed", false, 7);

        let report = trace_invocation(&fixture.db(), ROOT_UUID, trace_options(64)).unwrap();

        assert!(
            report
                .root
                .warnings
                .iter()
                .any(|warning| warning.contains("attempted resume target")),
            "{:?}",
            report.root.warnings
        );
    }

    #[test]
    fn resumed_session_with_nonzero_exit_carries_full_trace_bundle() {
        // Per PR-F contract §test-contract item 9: a single end-to-end
        // scenario that combines resumed provenance, a non-zero child
        // exit, and a transcript fixture. Asserts the entire bundle the
        // contract names — warning text, transcript still resolved,
        // turn counts populated, ASCII label switched to Resume target,
        // JSON capture_method preserved — in one place so a future
        // regression cannot pass by satisfying only some elements.
        let report = build_resumed_trace_report_with_exit(
            TraceOptions {
                max_depth: 64,
                json: true,
                inline_transcript: false,
                transcript: false,
            },
            Some(("failed", false, 7)),
        );

        assert!(
            report
                .root
                .warnings
                .iter()
                .any(|warning| warning.contains("attempted resume target")),
            "{:?}",
            report.root.warnings
        );
        assert!(matches!(
            report.root.session.transcript_state,
            TranscriptState::Available
        ));
        assert!(report.root.session.transcript_path.is_some());
        assert_eq!(report.root.session.turn_count, Some(3));
        assert_eq!(report.root.session.assistant_turn_count, Some(2));
        assert_eq!(report.root.session.sidechain_turn_count, Some(1));

        let ascii = render_ascii_trace(&report);
        assert!(
            ascii.contains("provider_session=5169694d-de0f-40d1-890c-6e28e55bab27"),
            "{ascii}"
        );
        assert!(
            ascii.contains("resume_input=5169694d-de0f-40d1-890c-6e28e55bab27"),
            "{ascii}"
        );

        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["root"]["session"]["capture_method"], "resumed");
        assert_eq!(
            json["root"]["session"]["provider_session_id"],
            "5169694d-de0f-40d1-890c-6e28e55bab27"
        );
        assert_eq!(
            json["root"]["session"]["resume_input_id"],
            "5169694d-de0f-40d1-890c-6e28e55bab27"
        );
    }

    #[test]
    fn json_output_preserves_resumed_capture_method() {
        let report = build_resumed_trace_report(TraceOptions {
            max_depth: 64,
            json: true,
            inline_transcript: false,
            transcript: false,
        });
        let json = serde_json::to_value(&report).unwrap();

        assert_eq!(json["root"]["session"]["capture_method"], "resumed");
    }

    #[test]
    fn json_output_includes_resume_acceptance_status() {
        let fixture = TraceFixture::new(&base_rows());
        fixture.set_session_capture(
            1,
            Some("5169694d-de0f-40d1-890c-6e28e55bab27"),
            Some("resumed"),
        );
        fixture.set_resume_acceptance(1, "accepted", Some("matched session id"));

        let report = trace_invocation(
            &fixture.db(),
            ROOT_UUID,
            TraceOptions {
                max_depth: 64,
                json: true,
                inline_transcript: false,
                transcript: false,
            },
        )
        .unwrap();
        let json = serde_json::to_value(&report).unwrap();

        assert_eq!(json["root"]["session"]["resume_acceptance"], "accepted");
        assert_eq!(
            json["root"]["session"]["resume_acceptance_evidence"],
            "matched session id"
        );
    }

    // risk: Trace integration; level: particular-integration; source: proposal §11.1 Trace integration.
    #[test]
    fn trace_json_includes_chain_id() {
        let chain_id = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
        let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
        let fixture = TraceFixture::new(&base_rows());
        fixture.set_session_capture(1, Some(session_id), Some("resumed"));
        fixture.seed_chain_segment(chain_id, "fixture-provider", session_id);

        let report = trace_invocation(
            &fixture.db(),
            ROOT_UUID,
            TraceOptions {
                max_depth: 64,
                json: true,
                inline_transcript: false,
                transcript: false,
            },
        )
        .unwrap();
        let json = serde_json::to_value(&report).unwrap();

        assert_eq!(json["root"]["session"]["id"], session_id);
        assert_eq!(json["root"]["session"]["chain_id"], chain_id);
        assert_eq!(
            json["root"]["invocation"]["agent_runner_invocation_id"],
            json["root"]["invocation"]["id"]
        );
        assert_eq!(json["root"]["session"]["provider_session_id"], session_id);
        assert_eq!(json["root"]["session"]["agent_runner_chain_id"], chain_id);
        assert!(json["root"]["session"].get("transcript_state").is_some());
    }

    #[test]
    fn trace_json_dual_id_fields() {
        let chain_id = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
        let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
        let fixture = TraceFixture::new(&base_rows());
        fixture.set_session_capture(1, Some(session_id), Some("resumed"));
        fixture.seed_chain_segment(chain_id, "fixture-provider", session_id);

        let report = trace_invocation(
            &fixture.db(),
            ROOT_UUID,
            TraceOptions {
                max_depth: 64,
                json: true,
                inline_transcript: false,
                transcript: false,
            },
        )
        .unwrap();
        let json = serde_json::to_value(&report).unwrap();

        assert_eq!(json["root"]["invocation"]["id"], ROOT_UUID);
        assert_eq!(
            json["root"]["invocation"]["agent_runner_invocation_id"],
            ROOT_UUID
        );
        assert_eq!(json["root"]["session"]["id"], session_id);
        assert_eq!(json["root"]["session"]["provider_session_id"], session_id);
        assert_eq!(json["root"]["session"]["resume_input_id"], session_id);
        assert_eq!(json["root"]["session"]["chain_id"], chain_id);
        assert_eq!(json["root"]["session"]["agent_runner_chain_id"], chain_id);
    }

    #[test]
    fn trace_ascii_role_labels() {
        let chain_id = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
        let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
        let fixture = TraceFixture::new(&base_rows());
        fixture.set_session_capture(1, Some(session_id), Some("resumed"));
        fixture.seed_chain_segment(chain_id, "fixture-provider", session_id);

        let report = trace_invocation(&fixture.db(), ROOT_UUID, trace_options(64)).unwrap();
        let ascii = render_ascii_trace(&report);

        assert!(
            ascii.contains(&format!("agent_runner_invocation={ROOT_UUID}")),
            "{ascii}"
        );
        assert!(
            ascii.contains(&format!("provider_session={session_id}")),
            "{ascii}"
        );
        assert!(
            ascii.contains(&format!("resume_input={session_id}")),
            "{ascii}"
        );
        assert!(ascii.contains(&format!("chain={chain_id}")), "{ascii}");
    }
}
