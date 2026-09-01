#![allow(dead_code)]
//! Declared roles: accessor, formatter, mapper, orchestration, parser.

use oulipoly_state::StateDb;
use rusqlite::{Connection, params};
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub mod rc1_schema_contract;
pub mod rc3_export_db_source;
pub mod rc4_trace_inline_transcript;

#[cfg(test)]
pub mod age158_characterization;

pub const PROVIDER: &str = "codex";
pub const MODEL: &str = "codex-high";
pub const SESSION_ID: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
pub const CHAIN_ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
pub const INVOCATION_UUID: &str = "11111111-1111-4111-8111-111111111111";
pub const TS_USER: &str = "2026-04-17T08:00:00Z";
pub const TS_ASSISTANT: &str = "2026-04-17T08:00:01Z";

pub struct RcaFixture {
    dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
}

impl RcaFixture {
    pub fn new() -> Self {
        let paths = empty_body_fixture_paths();
        create_empty_body_model_dir(&paths.models_dir);
        assemble_empty_body_fixture(paths)
    }

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("state.db")
    }

    pub fn open_db(&self) -> StateDb {
        StateDb::open(&self.db_path()).unwrap()
    }

    pub fn conn(&self) -> Connection {
        let _ = self.open_db();
        Connection::open(self.db_path()).unwrap()
    }

    pub fn session_turns_columns(&self) -> Vec<String> {
        let conn = self.conn();
        query_session_turn_column_names(&conn)
    }

    pub fn seed_chain(&self) {
        let conn = self.conn();
        insert_session_chain(&conn);
        insert_session_chain_segment(&conn);
    }

    pub fn seed_body_turns(&self) {
        let conn = self.conn();
        insert_user_body_turn(&conn);
        insert_assistant_body_turn(&conn);
    }

    pub fn fetch_turn_body(&self, turn_id: &str) -> Result<Value, String> {
        fetch_turn_body_text(&self.conn(), turn_id).and_then(|raw| parse_turn_body_json(&raw))
    }

    pub fn write_cli_config_with_missing_transcript(&self) {
        write_empty_body_model_toml(&self.models_dir);
        write_empty_body_provider_toml(&self.app_config_dir, self.root());
        let locator = write_missing_transcript_locator_script(self);
        write_empty_body_sessions_toml(&self.app_config_dir, self.root(), &locator);
    }

    pub fn write_script(&self, name: &str, body: &str) -> PathBuf {
        let path = fixture_script_path(self.root(), name);
        let script = format_shell_script(body);
        write_script_file(&path, &script);
        mark_script_executable(&path);
        path
    }

    pub fn run_export(&self) -> Output {
        let mut cmd = export_command();
        apply_export_env(&mut cmd, self);
        run_export_command(cmd)
    }
}

struct EmptyBodyFixturePaths {
    dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
}

fn empty_body_fixture_paths() -> EmptyBodyFixturePaths {
    let dir = tempfile::tempdir().unwrap();
    let config_home = dir.path().join("config");
    let data_home = dir.path().join("data");
    let app_config_dir = config_home.join("oulipoly-agent-runner");
    let models_dir = app_config_dir.join("models");
    EmptyBodyFixturePaths {
        dir,
        config_home,
        data_home,
        app_config_dir,
        models_dir,
    }
}

fn create_empty_body_model_dir(models_dir: &Path) {
    fs::create_dir_all(models_dir).unwrap();
}

fn assemble_empty_body_fixture(paths: EmptyBodyFixturePaths) -> RcaFixture {
    RcaFixture {
        dir: paths.dir,
        config_home: paths.config_home,
        data_home: paths.data_home,
        app_config_dir: paths.app_config_dir,
        models_dir: paths.models_dir,
    }
}

fn query_session_turn_column_names(conn: &Connection) -> Vec<String> {
    let mut stmt = open_session_turns_statement(conn);
    collect_column_names(&mut stmt)
}

fn open_session_turns_statement(conn: &Connection) -> rusqlite::Statement<'_> {
    conn.prepare("PRAGMA table_info(session_turns)").unwrap()
}

fn collect_column_names(stmt: &mut rusqlite::Statement<'_>) -> Vec<String> {
    stmt.query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn insert_session_chain(conn: &Connection) {
    conn.execute(
        session_chain_insert_sql(),
        params![CHAIN_ID, TS_USER, TS_ASSISTANT, MODEL],
    )
    .unwrap();
}

fn session_chain_insert_sql() -> &'static str {
    "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
     VALUES (?1, ?2, ?3, ?4)"
}

fn insert_session_chain_segment(conn: &Connection) {
    conn.execute(
        session_chain_segment_insert_sql(),
        params![CHAIN_ID, PROVIDER, SESSION_ID, TS_USER],
    )
    .unwrap();
}

fn session_chain_segment_insert_sql() -> &'static str {
    "INSERT INTO session_chain_segments
        (chain_id, provider_name, session_id, started_at, transition_reason)
     VALUES (?1, ?2, ?3, ?4, 'initial')"
}

fn insert_user_body_turn(conn: &Connection) {
    let sql = session_turn_insert_sql("turn-user", "user", "NULL");
    conn.execute(
        &sql,
        params![
            PROVIDER,
            SESSION_ID,
            TS_USER,
            r#"[{"type":"text","text":"db stored user body"}]"#
        ],
    )
    .unwrap();
}

fn insert_assistant_body_turn(conn: &Connection) {
    let sql = session_turn_insert_sql("turn-assistant", "assistant", "'turn-user'");
    conn.execute(
        &sql,
        params![
            PROVIDER,
            SESSION_ID,
            TS_ASSISTANT,
            r#"[{"type":"text","text":"db stored assistant body"}]"#
        ],
    )
    .unwrap();
}

fn session_turn_insert_sql(
    turn_id: &'static str,
    role: &'static str,
    parent: &'static str,
) -> String {
    format!(
        "INSERT INTO session_turns
            (provider_name, session_id, turn_id, timestamp, role,
             parent_turn_id, is_sidechain, is_compaction_boundary, source_file, ingested_at, body)
         VALUES (?1, ?2, '{turn_id}', ?3, '{role}', {parent}, 0, 0, '', ?3, ?4)"
    )
}

fn fetch_turn_body_text(conn: &Connection, turn_id: &str) -> Result<String, String> {
    conn.query_row(
        fetch_turn_body_sql(),
        params![PROVIDER, SESSION_ID, turn_id],
        |row| row.get(0),
    )
    .map_err(format_turn_body_query_error)
}

fn fetch_turn_body_sql() -> &'static str {
    "SELECT body FROM session_turns
     WHERE provider_name = ?1 AND session_id = ?2 AND turn_id = ?3"
}

fn format_turn_body_query_error(error: rusqlite::Error) -> String {
    format!("body column must be queryable from session_turns: {error}")
}

fn parse_turn_body_json(raw: &str) -> Result<Value, String> {
    serde_json::from_str(raw).map_err(format_turn_body_json_error)
}

fn format_turn_body_json_error(error: serde_json::Error) -> String {
    format!("body JSON must parse: {error}")
}

fn write_empty_body_model_toml(models_dir: &Path) {
    fs::create_dir_all(models_dir).unwrap();
    let path = empty_body_model_toml_path(models_dir);
    let contents = empty_body_model_toml();
    write_text_file(&path, &contents);
}

fn empty_body_model_toml_path(models_dir: &Path) -> PathBuf {
    models_dir.join(empty_body_model_toml_filename())
}

fn empty_body_model_toml_filename() -> String {
    format!("{MODEL}.toml")
}

fn empty_body_model_toml() -> String {
    format!("[[providers]]\nname = \"{PROVIDER}\"\n")
}

fn write_empty_body_provider_toml(app_config_dir: &Path, root: &Path) {
    let path = empty_body_provider_toml_path(app_config_dir);
    let contents = crate::provider_authority_fixture::with_explicit_provider_authority(
        &empty_body_provider_toml(root),
    );
    write_text_file(&path, &contents);
}

fn empty_body_provider_toml_path(app_config_dir: &Path) -> PathBuf {
    app_config_dir.join("providers.toml")
}

fn empty_body_provider_toml(root: &Path) -> String {
    format!(
        r#"[{PROVIDER}]
command = "provider-command-that-must-not-run"
args = []
interactive_args = ["launch"]
prompt_mode = "arg"

[{PROVIDER}.resume]
kind = "subcommand"
subcommand = ["resume"]

[{PROVIDER}.session_storage]
kind = "codex"
sessions_dir = "{}"
"#,
        root.join("missing-sessions-dir").display()
    )
}

fn write_missing_transcript_locator_script(fixture: &RcaFixture) -> PathBuf {
    let missing_jsonl = missing_transcript_rollout_path(fixture.root());
    let script = missing_transcript_locator_body(&missing_jsonl);
    fixture.write_script(locator_script_filename(), &script)
}

fn missing_transcript_rollout_path(root: &Path) -> PathBuf {
    root.join("missing-rollout.jsonl")
}

fn locator_script_filename() -> &'static str {
    "missing-transcript-locator.sh"
}

fn missing_transcript_locator_body(missing_jsonl: &Path) -> String {
    format!("printf '%s\\n' {}", sh_path(missing_jsonl))
}

fn write_empty_body_sessions_toml(app_config_dir: &Path, root: &Path, locator: &Path) {
    let path = empty_body_sessions_toml_path(app_config_dir);
    let contents = empty_body_sessions_toml(root, locator);
    write_text_file(&path, &contents);
}

fn empty_body_sessions_toml_path(app_config_dir: &Path) -> PathBuf {
    app_config_dir.join("sessions.toml")
}

fn empty_body_sessions_toml(root: &Path, locator: &Path) -> String {
    format!(
        "[{PROVIDER}]\nturn_script = \"true\"\ntranscript_locator = {:?}\nstate_dir = {:?}\n",
        locator.to_string_lossy(),
        root.join("locator-state").to_string_lossy()
    )
}

fn fixture_script_path(root: &Path, name: &str) -> PathBuf {
    root.join(name)
}

fn format_shell_script(body: &str) -> String {
    format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n")
}

fn write_script_file(path: &Path, script: &str) {
    fs::write(path, script).unwrap();
}

fn write_text_file(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
}

fn mark_script_executable(path: &Path) {
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

fn export_command() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
    cmd.arg("session").arg("export").arg(SESSION_ID);
    cmd
}

fn apply_export_env(cmd: &mut Command, fixture: &RcaFixture) {
    cmd.env("XDG_CONFIG_HOME", &fixture.config_home);
    cmd.env("XDG_DATA_HOME", &fixture.data_home);
    cmd.env(
        "OULIPOLY_DATA_DIR",
        fixture.data_home.join("oulipoly-agent-runner"),
    );
    cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
}

fn run_export_command(mut cmd: Command) -> Output {
    cmd.output().unwrap()
}

pub fn sh_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    format!("'{}'", s.replace('\'', "'\\''"))
}
