//! Invocation trace tree construction and rendering.
//!
//! PR-B owns the `trace <uuid>` CLI surface. The implementation builds an
//! invocation tree from SQLite parent edges, renders deterministic human output,
//! and exposes a structured JSON shape for machine consumers.

use crate::config::SessionsConfig;
use crate::sessions::locate_transcript;
use crate::state::{InvocationRecord, StateDb};
use chrono::{DateTime, SecondsFormat, Utc};
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
    pub transcript: Option<()>,
    pub warnings: Vec<String>,
    pub children: Vec<TraceNode>,
    #[serde(skip)]
    ascii_leaves: Vec<AsciiLeaf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TraceInvocation {
    pub row_id: i64,
    pub id: String,
    pub source: Option<String>,
    pub model_name: String,
    pub parent_id: Option<String>,
    pub status: String,
    pub success: Option<bool>,
    pub exit_code: Option<i32>,
    pub error_category: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TraceSession {
    pub id: Option<String>,
    pub capture_method: Option<String>,
    pub transcript_path: Option<String>,
    pub transcript_state: TranscriptState,
    pub turn_count: Option<u64>,
    pub assistant_turn_count: Option<u64>,
    pub sidechain_turn_count: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptState {
    Unresolved,
    NoLocator,
    Missing,
    Available,
}

impl TranscriptState {
    fn as_str(self) -> &'static str {
        match self {
            TranscriptState::Unresolved => "unresolved",
            TranscriptState::NoLocator => "no_locator",
            TranscriptState::Missing => "missing",
            TranscriptState::Available => "available",
        }
    }
}

#[derive(Debug, Clone)]
enum AsciiLeaf {
    Cycle(String),
    DepthLimit,
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
    let root = build_trace_node(
        db,
        root_record,
        None,
        0,
        options,
        sessions_cfg,
        &mut visited,
    )?;

    Ok(TraceReport {
        requested_id: root_uuid.to_string(),
        generated_at: Utc::now(),
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
    options: TraceOptions,
    sessions_cfg: Option<&SessionsConfig>,
    visited: &mut HashSet<i64>,
) -> Result<TraceNode, String> {
    let (session, mut node_warnings) = build_trace_session(db, &record, sessions_cfg)?;
    let mut node = TraceNode {
        invocation: TraceInvocation {
            row_id: record.id,
            id: record.invocation_uuid.clone(),
            source: record.provider_name.clone(),
            model_name: record.model_name,
            parent_id: parent_uuid,
            status: record.status.as_str().to_string(),
            success: record.success,
            exit_code: record.exit_code,
            error_category: record.error_category,
            started_at: record.created_at,
            finished_at: record.finished_at,
        },
        session,
        transcript: options.inline_transcript.then_some(()),
        warnings: std::mem::take(&mut node_warnings),
        children: Vec::new(),
        ascii_leaves: Vec::new(),
    };

    let children = db.list_invocation_children(node.invocation.row_id)?;
    if depth >= options.max_depth {
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
            options,
            sessions_cfg,
            visited,
        )?;
        node.warnings.extend(child_node.warnings.iter().cloned());
        node.children.push(child_node);
        visited.remove(&child_id);
    }

    Ok(node)
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
    }

    let Some(session_id) = record.session_id.clone() else {
        return Ok((
            TraceSession {
                id: None,
                capture_method: record.session_capture_method.clone(),
                transcript_path: None,
                transcript_state: TranscriptState::Unresolved,
                turn_count: None,
                assistant_turn_count: None,
                sidechain_turn_count: None,
            },
            warnings,
        ));
    };

    let Some(provider_name) = record.provider_name.as_deref() else {
        warnings.push("session_id is present but provider_name is missing".to_string());
        return Ok((
            TraceSession {
                id: Some(session_id),
                capture_method: record.session_capture_method.clone(),
                transcript_path: None,
                transcript_state: TranscriptState::Unresolved,
                turn_count: None,
                assistant_turn_count: None,
                sidechain_turn_count: None,
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

    let Some(sessions_cfg) = sessions_cfg else {
        return Ok((
            TraceSession {
                id: Some(session_id),
                capture_method: record.session_capture_method.clone(),
                transcript_path: None,
                transcript_state: TranscriptState::NoLocator,
                turn_count,
                assistant_turn_count,
                sidechain_turn_count,
            },
            warnings,
        ));
    };

    match locate_transcript(sessions_cfg, provider_name, &session_id) {
        Ok(None) => Ok((
            TraceSession {
                id: Some(session_id),
                capture_method: record.session_capture_method.clone(),
                transcript_path: None,
                transcript_state: TranscriptState::NoLocator,
                turn_count,
                assistant_turn_count,
                sidechain_turn_count,
            },
            warnings,
        )),
        Ok(Some(path)) if path.exists() => Ok((
            TraceSession {
                id: Some(session_id),
                capture_method: record.session_capture_method.clone(),
                transcript_path: Some(path.display().to_string()),
                transcript_state: TranscriptState::Available,
                turn_count,
                assistant_turn_count,
                sidechain_turn_count,
            },
            warnings,
        )),
        Ok(Some(_path)) => Ok((
            TraceSession {
                id: Some(session_id),
                capture_method: record.session_capture_method.clone(),
                transcript_path: None,
                transcript_state: TranscriptState::Missing,
                turn_count,
                assistant_turn_count,
                sidechain_turn_count,
            },
            warnings,
        )),
        Err(err) => {
            warnings.push(format!("transcript locator failed: {err}"));
            Ok((
                TraceSession {
                    id: Some(session_id),
                    capture_method: record.session_capture_method.clone(),
                    transcript_path: None,
                    transcript_state: TranscriptState::Missing,
                    turn_count,
                    assistant_turn_count,
                    sidechain_turn_count,
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
    format!(
        "{}  {}  {}  {}  {}  session={}  {}",
        node.invocation.id,
        node.invocation.source.as_deref().unwrap_or("—"),
        node.invocation.model_name,
        node.invocation.status,
        node.invocation
            .started_at
            .to_rfc3339_opts(SecondsFormat::Secs, true),
        node.session.id.as_deref().unwrap_or("—"),
        node.session.transcript_state.as_str(),
    )
}

// Tests use `set_permissions` with Unix mode bits to make fixture
// scripts executable. Gate the whole module on Unix; the trace logic
// itself is platform-agnostic, but the test fixtures are not.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::state::{SessionTurnIngest, StateDb};
    use chrono::{DateTime, Utc};
    use rusqlite::{Connection, params};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::{Mutex, OnceLock};

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
                        created_at,
                        finished_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
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
    }

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
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

    fn trace_options(max_depth: usize) -> TraceOptions {
        TraceOptions {
            max_depth,
            json: false,
            inline_transcript: false,
            transcript: false,
        }
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
        assert!(running["finished_at"].is_null());
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

        assert!(session["id"].is_null());
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
    fn inline_transcript_adds_null_field_to_each_json_node() {
        let fixture = TraceFixture::new(&base_rows());
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

        assert!(json["root"]["transcript"].is_null());
        assert!(json["root"]["children"][0]["transcript"].is_null());
        assert!(json["root"]["children"][0]["children"][0]["transcript"].is_null());
    }

    #[test]
    fn json_output_reports_available_transcript_when_locator_finds_existing_file() {
        let _guard = env_lock().lock().unwrap();
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
        let _guard = env_lock().lock().unwrap();
        let fixture = TraceFixture::new(&base_rows());
        fixture.set_session_capture(
            1,
            Some("5169694d-de0f-40d1-890c-6e28e55bab27"),
            Some("forced_flag_verified"),
        );
        let db = fixture.db();

        let env_dir = tempfile::tempdir().unwrap();
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
}
