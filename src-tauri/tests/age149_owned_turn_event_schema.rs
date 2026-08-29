#![cfg(unix)]

//! ## Declared roles
//! orchestration, accessor, mapper, parser, filter, predicate, validator, formatter

use chrono::{DateTime, Utc};
use oulipoly_state::{SessionTurnIngest, StateDb};
use rusqlite::{Connection, params};
use serde::Deserialize;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[allow(dead_code)]
#[path = "../src/main/owned_turn_event_ingest.rs"]
mod owned_turn_event_ingest;

use owned_turn_event_ingest::ingest_owned_turn_event_rows;

const PROVIDER_NAME: &str = "fixture-provider";
const CHAIN_ID: &str = "idx-main-06-chain";

#[derive(Debug, Deserialize)]
struct OwnedTurnFixture {
    session_id: String,
    turn_id: String,
    timestamp: String,
    role: String,
    #[serde(default)]
    #[allow(dead_code)]
    is_compaction_boundary: bool,
}

struct CliFixture {
    dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    app_config_dir: PathBuf,
}

impl CliFixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config_home = dir.path().join("config");
        let data_home = dir.path().join("data");
        let app_config_dir = config_home.join("oulipoly-agent-runner");
        let models_dir = app_config_dir.join("models");
        fs::create_dir_all(&models_dir).unwrap();
        Self {
            dir,
            config_home,
            data_home,
            app_config_dir,
        }
    }

    fn db_path(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("state.db")
    }

    fn sessions_path(&self) -> PathBuf {
        self.app_config_dir.join("sessions.toml")
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env(
            "OULIPOLY_DATA_DIR",
            self.data_home.join("oulipoly-agent-runner"),
        );
        cmd.env("HOME", &self.data_home);
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd
    }
}

// risk: PP-002 owned turn/event schema; level: characterization; source: AGE-149 IDX-MAIN-06.
#[test]
fn idx_main_06_compaction_boundary_proxy_preserves_migrate_db_reports() {
    let with_compaction = run_owned_boundary_migrate_case("with_compaction.jsonl");
    let without_compaction = run_owned_boundary_migrate_case("without_compaction.jsonl");

    assert_eq!(with_compaction.status_code, Some(0));
    assert_eq!(with_compaction.stderr, "");
    assert!(
        with_compaction.stdout.contains(
            "compaction backfill session: provider=fixture-provider session_id=11111111-1111-4111-8111-111111111111 flagged=1"
        ),
        "{}",
        with_compaction.stdout
    );
    assert!(
        with_compaction
            .stdout
            .contains("compaction backfill: 1 turns flagged across 1 sessions"),
        "{}",
        with_compaction.stdout
    );
    assert_eq!(
        with_compaction.turn_flags,
        vec![("boundary-1".to_string(), 1)]
    );
    assert_eq!(without_compaction.status_code, Some(0));
    assert_eq!(without_compaction.stderr, "");
    assert!(
        without_compaction.stdout.contains(
            "compaction backfill session: provider=fixture-provider session_id=11111111-1111-4111-8111-111111111111 flagged=0"
        ),
        "{}",
        without_compaction.stdout
    );
    assert!(
        without_compaction
            .stdout
            .contains("compaction backfill: 0 turns flagged across 1 sessions"),
        "{}",
        without_compaction.stdout
    );
    assert_eq!(
        without_compaction.turn_flags,
        vec![("plain-1".to_string(), 0)]
    );
}

// risk: PP-002 owned turn/event schema; level: characterization; source: AGE-149 IDX-MAIN-06.
#[test]
fn idx_main_06_owned_turn_event_boundary_with_compaction_exposes_boundary_and_summary_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let state_path = temp.path().join("state.db");
    let state = StateDb::open(&state_path).unwrap();
    let rows = read_owned_turn_fixture("with_compaction.jsonl");
    let session_id = common_session_id(&rows);

    let inserted = ingest_owned_turn_event_rows(
        &state,
        PROVIDER_NAME,
        &session_id,
        &fixture_jsonl_path("with_compaction.jsonl"),
    )
    .unwrap();
    assert_eq!(inserted, 1);

    let owned_rows = state
        .owned_turn_event_rows_for_session(&session_id)
        .unwrap();
    let compaction_rows: Vec<_> = owned_rows
        .iter()
        .filter(|row| row.is_compaction_boundary)
        .collect();
    assert!(
        !compaction_rows.is_empty(),
        "expected >= 1 compaction-boundary row for with_compaction.jsonl"
    );
    let boundary = compaction_rows[0];
    assert_eq!(boundary.session_id, session_id);
    assert_eq!(boundary.turn_uuid, "boundary-1");
    let summary = boundary
        .summary_metadata_json
        .as_deref()
        .expect("expected summary metadata for compaction-boundary row");
    let summary: Value = serde_json::from_str(summary).unwrap();
    assert_eq!(summary["turn_id"], "boundary-1");
    assert_eq!(summary["timestamp"], "2026-04-17T08:00:00Z");
    assert_eq!(summary["role"], "assistant");
    assert_eq!(summary["is_compaction_boundary"], Value::Bool(true));
}

// risk: PP-002 owned turn/event schema; level: characterization; source: AGE-149 IDX-MAIN-06.
#[test]
fn idx_main_06_owned_turn_event_boundary_without_compaction_has_no_is_compaction_boundary() {
    let temp = tempfile::tempdir().unwrap();
    let state_path = temp.path().join("state.db");
    let state = StateDb::open(&state_path).unwrap();
    let rows = read_owned_turn_fixture("without_compaction.jsonl");
    let session_id = common_session_id(&rows);

    let inserted = ingest_owned_turn_event_rows(
        &state,
        PROVIDER_NAME,
        &session_id,
        &fixture_jsonl_path("without_compaction.jsonl"),
    )
    .unwrap();
    assert_eq!(inserted, 1);

    let owned_rows = state
        .owned_turn_event_rows_for_session(&session_id)
        .unwrap();
    assert_eq!(owned_rows.len(), 1);
    assert_eq!(owned_rows[0].session_id, session_id);
    assert_eq!(owned_rows[0].turn_uuid, "plain-1");
    assert!(
        owned_rows.iter().all(|row| !row.is_compaction_boundary),
        "expected no compaction-boundary rows for without_compaction.jsonl: {owned_rows:?}"
    );
}

// risk: PP-002 owned turn/event schema; level: characterization; source: AGE-149 IDX-MAIN-06.
#[test]
fn idx_main_06_migrate_db_consumes_owned_evidence_without_provider_private_transcript() {
    let with_compaction = run_owned_boundary_migrate_case("with_compaction.jsonl");
    let without_compaction = run_owned_boundary_migrate_case("without_compaction.jsonl");

    assert_eq!(with_compaction.status_code, Some(0));
    assert_eq!(with_compaction.stderr, "");
    assert_eq!(
        with_compaction.stdout,
        "session chain backfill: chains=0 segments=0 skipped_existing=true\n\
         compaction backfill session: provider=fixture-provider session_id=11111111-1111-4111-8111-111111111111 flagged=1\n\
         compaction backfill: 1 turns flagged across 1 sessions\n"
    );
    assert_eq!(
        with_compaction.turn_flags,
        vec![("boundary-1".to_string(), 1)]
    );

    assert_eq!(without_compaction.status_code, Some(0));
    assert_eq!(without_compaction.stderr, "");
    assert_eq!(
        without_compaction.stdout,
        "session chain backfill: chains=0 segments=0 skipped_existing=true\n\
         compaction backfill session: provider=fixture-provider session_id=11111111-1111-4111-8111-111111111111 flagged=0\n\
         compaction backfill: 0 turns flagged across 1 sessions\n"
    );
    assert_eq!(
        without_compaction.turn_flags,
        vec![("plain-1".to_string(), 0)]
    );
}

#[derive(Debug)]
struct CaseObservation {
    status_code: Option<i32>,
    stdout: String,
    stderr: String,
    turn_flags: Vec<(String, i64)>,
}

fn run_owned_boundary_migrate_case(name: &str) -> CaseObservation {
    let fixture = CliFixture::new();
    let rows = read_owned_turn_fixture(name);
    let session_id = common_session_id(&rows);
    let transcript = fixture.dir.path().join(format!("owned-boundary-{name}"));
    write_provider_private_free_jsonl(&transcript);
    assert_provider_private_free_transcript(&transcript);
    write_sessions_config(&fixture, &transcript);
    seed_state_without_compaction_evidence(&fixture.db_path(), &rows, &session_id);

    let state = StateDb::open(&fixture.db_path()).unwrap();
    ingest_owned_turn_event_rows(
        &state,
        PROVIDER_NAME,
        &session_id,
        &fixture_jsonl_path(name),
    )
    .unwrap();
    let owned_rows = state
        .owned_turn_event_rows_for_session(&session_id)
        .unwrap();
    assert_one_owned_row(owned_rows.len());
    drop(state);

    let output = fixture.command().arg("migrate-db").output().unwrap();
    let conn = Connection::open(fixture.db_path()).unwrap();

    case_observation(&output, &conn)
}

fn case_observation(output: &Output, conn: &Connection) -> CaseObservation {
    CaseObservation {
        status_code: output.status.code(),
        stdout: stdout(output),
        stderr: stderr(output),
        turn_flags: turn_flags(conn),
    }
}

fn read_owned_turn_fixture(name: &str) -> Vec<OwnedTurnFixture> {
    parse_owned_turn_fixture(&read_fixture_jsonl(name))
}

fn read_fixture_jsonl(name: &str) -> String {
    fs::read_to_string(fixture_jsonl_path(name)).unwrap()
}

fn parse_owned_turn_fixture(jsonl: &str) -> Vec<OwnedTurnFixture> {
    jsonl.lines().map(parse_owned_turn_line).collect()
}

fn parse_owned_turn_line(line: &str) -> OwnedTurnFixture {
    serde_json::from_str(line).unwrap()
}

fn fixture_jsonl_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/jsonl/adapter")
        .join(name)
}

fn common_session_id(rows: &[OwnedTurnFixture]) -> String {
    let session_id = first_session_id(rows);
    assert_single_fixture_session(rows, &session_id);
    session_id
}

fn first_session_id(rows: &[OwnedTurnFixture]) -> String {
    rows.first().unwrap().session_id.clone()
}

fn assert_single_fixture_session(rows: &[OwnedTurnFixture], session_id: &str) {
    assert!(
        rows.iter().all(|row| row.session_id == session_id),
        "IDX-MAIN-06 fixtures must each describe one session"
    );
}

fn write_provider_private_free_jsonl(path: &Path) {
    fs::write(path, "{}\n").unwrap();
}

fn assert_provider_private_free_transcript(path: &Path) {
    assert_provider_private_free_jsonl(&read_provider_private_free_jsonl(path));
}

fn read_provider_private_free_jsonl(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

fn write_sessions_config(fixture: &CliFixture, transcript: &Path) {
    fs::write(
        fixture.sessions_path(),
        sessions_config(fixture, transcript),
    )
    .unwrap();
}

fn seed_state_without_compaction_evidence(
    path: &Path,
    rows: &[OwnedTurnFixture],
    session_id: &str,
) {
    let db = StateDb::open(path).unwrap();
    db.ingest_session_turns_batch(PROVIDER_NAME, &session_turns_without_compaction(rows))
        .unwrap();
    drop(db);

    insert_fixture_chain_segment(path, session_id);
}

fn assert_one_owned_row(row_count: usize) {
    assert_eq!(row_count, 1);
}

fn assert_provider_private_free_jsonl(transcript: &str) {
    assert!(!transcript.contains("isCompactSummary"));
    assert!(!transcript.contains("uuid"));
}

fn sessions_config(fixture: &CliFixture, transcript: &Path) -> String {
    format!(
        r#"[{PROVIDER_NAME}]
turn_script = "true"
transcript_locator = "printf '%s\n' {}"
state_dir = "{}"
"#,
        shell_quote(&transcript.to_string_lossy()),
        fixture.dir.path().join("locator-state").display()
    )
}

fn session_turns_without_compaction(rows: &[OwnedTurnFixture]) -> Vec<SessionTurnIngest> {
    rows.iter().map(session_turn_without_compaction).collect()
}

fn session_turn_without_compaction(row: &OwnedTurnFixture) -> SessionTurnIngest {
    session_turn(
        row,
        parsed_fixture_timestamp(row),
        serialized_fixture_body(row_to_json_without_compaction(row)),
    )
}

fn session_turn(
    row: &OwnedTurnFixture,
    timestamp: DateTime<Utc>,
    body: String,
) -> SessionTurnIngest {
    SessionTurnIngest {
        session_id: row.session_id.clone(),
        turn_id: row.turn_id.clone(),
        timestamp,
        role: row.role.clone(),
        parent_turn_id: None,
        is_sidechain: false,
        is_compaction_boundary: false,
        body: Some(body),
    }
}

fn insert_fixture_chain_segment(path: &Path, session_id: &str) {
    let conn = Connection::open(path).unwrap();
    conn.execute(
        "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
         VALUES (?1, '2026-04-17T08:00:00Z', '2026-04-17T08:00:00Z', 'fixture')",
        params![CHAIN_ID],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session_chain_segments
            (chain_id, provider_name, session_id, started_at, transition_reason)
         VALUES (?1, ?2, ?3, '2026-04-17T08:00:00Z', 'initial')",
        params![CHAIN_ID, PROVIDER_NAME, session_id],
    )
    .unwrap();
}

fn row_to_json_without_compaction(row: &OwnedTurnFixture) -> Value {
    serde_json::json!({
        "session_id": row.session_id,
        "turn_id": row.turn_id,
        "timestamp": row.timestamp,
        "role": row.role
    })
}

fn parse_timestamp(raw: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(raw)
        .unwrap()
        .with_timezone(&Utc)
}

fn parsed_fixture_timestamp(row: &OwnedTurnFixture) -> DateTime<Utc> {
    parse_timestamp(&row.timestamp)
}

fn serialized_fixture_body(body: Value) -> String {
    serde_json::to_string(&body).unwrap()
}

fn turn_flags(conn: &Connection) -> Vec<(String, i64)> {
    turn_flag_rows(conn)
        .into_iter()
        .map(turn_flag_tuple)
        .collect()
}

fn turn_flag_rows(conn: &Connection) -> Vec<(String, i64)> {
    let mut stmt = conn
        .prepare("SELECT turn_id, is_compaction_boundary FROM session_turns ORDER BY turn_id")
        .unwrap();
    stmt.query_map([], turn_flag_row)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn turn_flag_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(String, i64)> {
    Ok((row.get(0)?, row.get(1)?))
}

fn turn_flag_tuple(row: (String, i64)) -> (String, i64) {
    row
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn shell_quote(input: &str) -> String {
    format!("'{}'", input.replace('\'', r#"'\''"#))
}
