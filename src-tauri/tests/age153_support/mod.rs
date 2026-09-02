#![cfg(unix)]
#![allow(dead_code)]

#[path = "../provider_authority_fixture.rs"]
mod provider_authority_fixture;

use oulipoly_state::{CompositeInvocationId, InvocationStatus, StateDb};
use rusqlite::{Connection, params};
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub const SESSION_ID: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
pub const CHAIN_ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
pub const FIRST_PARENT_ROW_ID_IN_FRESH_FIXTURE: i64 = 2;
pub const FORCE_TERMINAL_SIGNAL_KIND: &str = "OULIPOLY_AGE153_FORCE_TERMINAL_SIGNAL_KIND";

pub struct Age153Fixture {
    pub dir: tempfile::TempDir,
    pub config_home: PathBuf,
    pub data_home: PathBuf,
    pub app_config_dir: PathBuf,
    pub models_dir: PathBuf,
}

struct FixturePaths {
    config_home: PathBuf,
    data_home: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
}

impl Age153Fixture {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let paths = fixture_paths(dir.path());
        create_fixture_dirs(&paths);
        fixture_from_paths(dir, paths)
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

    pub fn write_script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.script_path(name);
        write_executable_script(&path, &script_body(body));
        path
    }

    fn script_path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn write_model_toml(&self, model_name: &str, body: &str) {
        fs::write(self.model_toml_path(model_name), body).unwrap();
    }

    fn model_toml_path(&self, model_name: &str) -> PathBuf {
        self.models_dir.join(model_toml_filename(model_name))
    }

    fn write_providers_toml(&self, body: &str) {
        fs::write(
            self.app_config_dir.join("providers.toml"),
            provider_authority_fixture::with_explicit_provider_authority(body),
        )
        .unwrap();
    }

    fn write_provider_command_scripts<'a>(
        &self,
        providers: &'a [(&'a str, &'a str)],
    ) -> Vec<(&'a str, PathBuf)> {
        providers
            .iter()
            .map(|(provider, command_body)| {
                self.provider_command_script_entry(provider, command_body)
            })
            .collect()
    }

    fn write_resume_provider_scripts<'a>(
        &self,
        providers: &'a [(&'a str, String)],
    ) -> Vec<ResumeProviderPaths<'a>> {
        providers
            .iter()
            .map(|(provider, command_body)| {
                self.resume_provider_paths_entry(provider, command_body)
            })
            .collect()
    }

    fn write_project_session_jsonl(&self, provider: &str, session_id: &str, body: &str) {
        let path = self.project_session_jsonl_path(provider, session_id);
        write_project_session_jsonl_at(&path, body);
    }

    fn provider_command_script_entry<'a>(
        &self,
        provider: &'a str,
        command_body: &str,
    ) -> (&'a str, PathBuf) {
        provider_command_script_entry(
            provider,
            self.write_provider_command_script(provider, command_body),
        )
    }

    fn write_provider_command_script(&self, provider: &str, command_body: &str) -> PathBuf {
        self.write_script(&provider_script_name(provider, "command"), command_body)
    }

    fn resume_provider_paths_entry<'a>(
        &self,
        provider: &'a str,
        command_body: &str,
    ) -> ResumeProviderPaths<'a> {
        resume_provider_paths(
            provider,
            self.write_resume_provider_script(provider, command_body),
            self.provider_projects_dir(provider),
        )
    }

    fn write_resume_provider_script(&self, provider: &str, command_body: &str) -> PathBuf {
        self.write_script(&provider_script_name(provider, "resume"), command_body)
    }

    fn project_session_jsonl_path(&self, provider: &str, session_id: &str) -> PathBuf {
        self.provider_projects_dir(provider)
            .join("source-project")
            .join(session_jsonl_filename(session_id))
    }

    fn stage_active_provider_session_jsonl(&self, provider: &str) {
        self.write_project_session_jsonl(provider, SESSION_ID, &active_provider_jsonl());
    }
}

fn fixture_paths(dir: &Path) -> FixturePaths {
    let config_home = dir.join("config");
    let data_home = dir.join("data");
    let app_config_dir = config_home.join("oulipoly-agent-runner");
    let models_dir = app_config_dir.join("models");

    FixturePaths {
        config_home,
        data_home,
        app_config_dir,
        models_dir,
    }
}

fn create_fixture_dirs(paths: &FixturePaths) {
    fs::create_dir_all(&paths.models_dir).unwrap();
}

fn fixture_from_paths(dir: tempfile::TempDir, paths: FixturePaths) -> Age153Fixture {
    Age153Fixture {
        dir,
        config_home: paths.config_home,
        data_home: paths.data_home,
        app_config_dir: paths.app_config_dir,
        models_dir: paths.models_dir,
    }
}

fn provider_command_script_entry(provider: &str, command: PathBuf) -> (&str, PathBuf) {
    (provider, command)
}

fn resume_provider_paths<'a>(
    provider: &'a str,
    command: PathBuf,
    projects_dir: PathBuf,
) -> ResumeProviderPaths<'a> {
    ResumeProviderPaths {
        provider,
        command,
        projects_dir,
    }
}

fn write_project_session_jsonl_at(path: &Path, body: &str) {
    create_project_session_dir(project_session_dir(path));
    write_project_session_file(path, body);
}

fn project_session_dir(path: &Path) -> &Path {
    path.parent().expect("session jsonl parent")
}

fn create_project_session_dir(path: &Path) {
    fs::create_dir_all(path).unwrap();
}

fn write_project_session_file(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
}

fn write_executable_script(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    mark_executable(path);
}

fn mark_executable(path: &Path) {
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

fn script_body(body: &str) -> String {
    format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n")
}

fn model_providers_toml(providers: &[&str]) -> String {
    providers
        .iter()
        .map(|provider| model_provider_toml(provider))
        .collect()
}

fn model_provider_toml(provider: &str) -> String {
    format!(
        r#"[[providers]]
name = "{provider}"
args = []
interactive_args = ["interactive"]

"#
    )
}

fn provider_script_name(provider: &str, suffix: &str) -> String {
    format!("{provider}-{suffix}.sh")
}

fn model_toml_filename(model_name: &str) -> String {
    format!("{model_name}.toml")
}

fn session_jsonl_filename(session_id: &str) -> String {
    format!("{session_id}.jsonl")
}

fn providers_toml(providers: &[(&str, PathBuf)]) -> String {
    providers
        .iter()
        .map(|(provider, command)| provider_command_toml(provider, command))
        .collect()
}

fn providers_toml_from_paths(providers: &[(&str, &Path)]) -> String {
    providers
        .iter()
        .map(|(provider, command)| provider_command_toml(provider, command))
        .collect()
}

fn provider_command_toml(provider: &str, command: &Path) -> String {
    format!(
        r#"[{provider}]
command = {}
args = []
interactive_args = ["interactive"]
prompt_mode = "arg"

"#,
        toml_string(&command.display().to_string())
    )
}

fn resume_model_toml(providers: &[(&str, String)]) -> String {
    providers
        .iter()
        .map(|(provider, _)| resume_model_provider_toml(provider))
        .collect()
}

fn resume_model_provider_toml(provider: &str) -> String {
    format!(
        r#"[[providers]]
name = "{provider}"
args = ["exec-{provider}"]

"#
    )
}

struct ResumeProviderPaths<'a> {
    provider: &'a str,
    command: PathBuf,
    projects_dir: PathBuf,
}

fn resume_providers_toml(providers: &[ResumeProviderPaths<'_>]) -> String {
    providers.iter().map(resume_provider_toml).collect()
}

fn resume_provider_toml(provider: &ResumeProviderPaths<'_>) -> String {
    format!(
        r#"[{}]
command = {}
args = []
interactive_args = ["launch-{}"]
prompt_mode = "arg"

[{}.resume]
kind = "flag"
flag = "--resume"

[{}.session_storage]
kind = "claude_code"
projects_dir = {}

"#,
        provider.provider,
        toml_string(&provider.command.display().to_string()),
        provider.provider,
        provider.provider,
        provider.provider,
        toml_string(&provider.projects_dir.display().to_string())
    )
}

fn active_provider_jsonl() -> String {
    format!(
        r#"{{"sessionId":"{SESSION_ID}","turnId":"turn-1","timestamp":"2026-04-17T08:00:00Z","type":"assistant"}}"#
    )
}

impl Age153Fixture {
    pub fn write_model(&self, model_name: &str, providers: &[&str]) {
        self.write_model_toml(model_name, &model_providers_toml(providers));
    }

    pub fn write_providers_with_bodies(&self, providers: &[(&str, &str)]) {
        let commands = self.write_provider_command_scripts(providers);
        self.write_providers_toml(&providers_toml(&commands));
    }

    pub fn write_providers_with_command_paths(&self, providers: &[(&str, &Path)]) {
        self.write_providers_toml(&providers_toml_from_paths(providers));
    }

    pub fn write_resume_pool(&self, model_name: &str, providers: &[(&str, String)]) {
        let provider_paths = self.write_resume_provider_scripts(providers);
        self.write_model_toml(model_name, &resume_model_toml(providers));
        self.write_providers_toml(&resume_providers_toml(&provider_paths));
    }

    pub fn provider_projects_dir(&self, provider: &str) -> PathBuf {
        self.dir.path().join(format!("{provider}-projects"))
    }

    pub fn stage_active_claude_jsonl(&self, provider: &str) {
        self.stage_active_provider_session_jsonl(provider);
    }

    pub fn seed_active_chain(&self, provider: &str, model: &str) {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, '2026-04-17T08:00:00Z', '2026-04-17T08:00:00Z', ?2)",
            params![CHAIN_ID, model],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, '2026-04-17T08:00:00Z', 'initial')",
            params![CHAIN_ID, provider, SESSION_ID],
        )
        .unwrap();
        provider_authority_fixture::bind_session_authority_with_cwd(
            &conn,
            provider,
            SESSION_ID,
            self.dir.path(),
        );
    }

    pub fn run_one_shot(&self, model_name: &str) -> Output {
        self.run_one_shot_with_env(model_name, &[])
    }

    pub fn run_one_shot_with_env(&self, model_name: &str, envs: &[(&str, &str)]) -> Output {
        let mut cmd = self.command();
        for (key, value) in envs {
            cmd.env(key, value);
        }
        cmd.arg("--models-dir")
            .arg(&self.models_dir)
            .arg("--model")
            .arg(model_name)
            .arg("prompt");
        cmd.output().unwrap()
    }

    pub fn run_resume(&self, model_name: &str) -> Output {
        self.run_resume_with_env(model_name, &[])
    }

    pub fn run_resume_with_env(&self, model_name: &str, envs: &[(&str, &str)]) -> Output {
        let mut cmd = self.command();
        for (key, value) in envs {
            cmd.env(key, value);
        }
        cmd.arg("-m")
            .arg(model_name)
            .arg("--resume")
            .arg(SESSION_ID)
            .arg("--models-dir")
            .arg(&self.models_dir)
            .arg("continue after quota");
        cmd.current_dir(self.dir.path());
        cmd.output().unwrap()
    }

    pub fn run_repl(&self, model_name: &str) -> Output {
        let mut cmd = self.command();
        cmd.arg("repl")
            .arg("--models-dir")
            .arg(&self.models_dir)
            .arg(model_name);
        cmd.output().unwrap()
    }

    pub fn command(&self) -> Command {
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

    pub fn exhausted_row_count(&self, provider: &str) -> i64 {
        self.conn()
            .query_row(
                "SELECT COUNT(*)
                 FROM provider_quotas
                 WHERE provider_name = ?1 AND exhausted_at IS NOT NULL",
                params![provider],
                |row| row.get(0),
            )
            .unwrap()
    }

    /// AGE-163 WU-A.4: typed forensics lands durable unavailability on
    /// `next_available_at`. Use this for "the provider was marked
    /// unavailable for routing" assertions.
    pub fn next_available_at_row_count(&self, provider: &str) -> i64 {
        self.conn()
            .query_row(
                "SELECT COUNT(*)
                 FROM provider_quotas
                 WHERE provider_name = ?1 AND next_available_at IS NOT NULL",
                params![provider],
                |row| row.get(0),
            )
            .unwrap()
    }

    pub fn failed_invocation_count(&self, provider: &str, terminal_reason: &str) -> i64 {
        self.conn()
            .query_row(
                "SELECT COUNT(*)
                 FROM invocations
                 WHERE provider_name = ?1
                   AND status = ?2
                   AND success = 0
                   AND terminal_reason = ?3
                   AND finished_at IS NOT NULL",
                params![provider, InvocationStatus::Failed.as_str(), terminal_reason],
                |row| row.get(0),
            )
            .unwrap()
    }

    pub fn successful_invocation_count_without_terminal_reason(&self, provider: &str) -> i64 {
        self.conn()
            .query_row(
                "SELECT COUNT(*)
                 FROM invocations
                 WHERE provider_name = ?1
                   AND status = ?2
                   AND success = 1
                   AND terminal_reason IS NULL
                   AND finished_at IS NOT NULL",
                params![provider, InvocationStatus::Succeeded.as_str()],
                |row| row.get(0),
            )
            .unwrap()
    }

    pub fn invocation_count_with_terminal_reason(&self, terminal_reason: &str) -> i64 {
        self.conn()
            .query_row(
                "SELECT COUNT(*)
                 FROM invocations
                 WHERE terminal_reason = ?1
                   AND finished_at IS NOT NULL",
                params![terminal_reason],
                |row| row.get(0),
            )
            .unwrap()
    }

    pub fn active_segment_provider(&self) -> String {
        self.conn()
            .query_row(
                "SELECT provider_name
                 FROM session_chain_segments
                 WHERE chain_id = ?1 AND ended_at IS NULL",
                params![CHAIN_ID],
                |row| row.get(0),
            )
            .unwrap()
    }

    pub fn seed_running_child_for_first_parent(&self, child_uuid: &str) -> i64 {
        drop(self.open_db());
        let conn = Connection::open(self.db_path()).unwrap();
        conn.pragma_update(None, "foreign_keys", false).unwrap();
        conn.execute(
            "INSERT INTO invocations (
                invocation_uuid, model_name, provider_name, provider_index,
                parent_invocation_id, status, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                child_uuid,
                "age153-child-model",
                "fixture-child",
                0,
                FIRST_PARENT_ROW_ID_IN_FRESH_FIXTURE,
                "running",
                "2026-04-17T08:00:01Z"
            ],
        )
        .unwrap();
        conn.last_insert_rowid()
    }
}

pub fn toml_string(value: &str) -> String {
    format!("{value:?}")
}

pub fn quota_body(marker: &Path, exit_code: i32) -> String {
    format!(
        "printf '%s\\n' ran >> {}\nprintf '%s\\n' 'claude usage limit reached' >&2\nexit {exit_code}",
        toml_string(&marker.display().to_string())
    )
}

pub fn success_body(marker: &Path, stdout: &str) -> String {
    format!(
        "printf '%s\\n' ran >> {}\nprintf '%s\\n' {}",
        toml_string(&marker.display().to_string()),
        toml_string(stdout)
    )
}

pub fn legacy_quota_like_non_signal_body(marker: &Path) -> String {
    format!(
        "printf '%s\\n' ran >> {}\nprintf '%s\\n' 'quota billing usage limit reached' >&2\nexit 42",
        toml_string(&marker.display().to_string())
    )
}

pub fn signal_exit_with_non_quota_error_body(marker: &Path) -> String {
    format!(
        "printf '%s\\n' ran >> {}\nprintf '%s\\n' 'permission denied: subprocess crashed' >&2\nkill -TERM $$\nsleep 1",
        toml_string(&marker.display().to_string())
    )
}

pub fn nonzero_exit_with_non_quota_error_body(marker: &Path) -> String {
    format!(
        "printf '%s\\n' ran >> {}\nprintf '%s\\n' 'unhandled error: aborting' >&2\nexit 1",
        toml_string(&marker.display().to_string())
    )
}

pub fn unknown_with_non_quota_error_body(marker: &Path) -> String {
    format!(
        "printf '%s\\n' ran >> {}\nprintf '%s\\n' 'unclassified provider failure' >&2\nexit 42",
        toml_string(&marker.display().to_string())
    )
}

pub fn line_count(path: &Path) -> usize {
    fs::read_to_string(path)
        .map(|content| content.lines().count())
        .unwrap_or(0)
}

pub fn terminal_signal_lines(stderr: &str) -> Vec<&str> {
    stderr
        .lines()
        .filter(|line| line.starts_with("OULIPOLY_TERMINAL_SIGNAL="))
        .collect()
}

pub fn invocation_lines(stderr: &str) -> Vec<&str> {
    stderr
        .lines()
        .filter(|line| line.starts_with("OULIPOLY_INVOCATION="))
        .collect()
}

pub fn parse_terminal_signal_line(line: &str) -> Value {
    let raw = line.strip_prefix("OULIPOLY_TERMINAL_SIGNAL=").unwrap();
    serde_json::from_str(raw).unwrap()
}

pub fn assert_single_terminal_signal(stderr: &str, expected_kind: &str, expect_session: bool) {
    let lines = terminal_signal_lines(stderr);
    assert_eq!(
        lines.len(),
        1,
        "expected exactly one OULIPOLY_TERMINAL_SIGNAL marker in stderr:\n{stderr}"
    );
    assert_terminal_signal_shape(lines[0], expected_kind, expect_session);
}

pub fn assert_terminal_signal_shape(line: &str, expected_kind: &str, expect_session: bool) {
    assert_terminal_signal_facts(terminal_signal_facts(line), expected_kind, expect_session);
}

fn terminal_signal_facts(line: &str) -> TerminalSignalFacts {
    let value = parse_terminal_signal_line(line);
    let object = expect_terminal_signal_object(terminal_signal_object(&value));
    terminal_signal_facts_from_parts(
        terminal_signal_evidence_is_object(&value),
        terminal_signal_invocation_id_is_uuid(&value),
        terminal_signal_keys(object),
        terminal_signal_kind(&value),
        terminal_signal_raw(&value),
        terminal_signal_session_id_is_uuid(&value),
        terminal_signal_session_is_null(&value),
    )
}

fn terminal_signal_object(value: &Value) -> Option<&Map<String, Value>> {
    value.as_object()
}

fn expect_terminal_signal_object(object: Option<&Map<String, Value>>) -> &Map<String, Value> {
    object.expect("terminal signal marker body object")
}

fn terminal_signal_keys(object: &Map<String, Value>) -> BTreeSet<String> {
    object.keys().cloned().collect()
}

fn terminal_signal_evidence_is_object(value: &Value) -> bool {
    value["evidence"].is_object()
}

fn terminal_signal_invocation_id_is_uuid(value: &Value) -> bool {
    uuid_value_is_valid(&value["invocation_id"])
}

fn terminal_signal_kind(value: &Value) -> Value {
    value["kind"].clone()
}

fn terminal_signal_raw(value: &Value) -> Value {
    value.clone()
}

fn terminal_signal_session_id_is_uuid(value: &Value) -> bool {
    uuid_value_is_valid(&value["session_id"])
}

fn terminal_signal_session_is_null(value: &Value) -> bool {
    value["session_id"].is_null()
}

fn terminal_signal_facts_from_parts(
    evidence_is_object: bool,
    invocation_id_is_uuid: bool,
    keys: BTreeSet<String>,
    kind: Value,
    raw: Value,
    session_id_is_uuid: bool,
    session_is_null: bool,
) -> TerminalSignalFacts {
    TerminalSignalFacts {
        evidence_is_object,
        invocation_id_is_uuid,
        keys,
        kind,
        raw,
        session_id_is_uuid,
        session_is_null,
    }
}

fn uuid_value_is_valid(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|raw| uuid::Uuid::parse_str(raw).is_ok())
}

fn assert_terminal_signal_facts(
    facts: TerminalSignalFacts,
    expected_kind: &str,
    expect_session: bool,
) {
    assert_eq!(
        facts.keys,
        terminal_signal_expected_keys(),
        "AGE-153 marker schema from /home/nes/projects/agent-runner/planning/age-153-terminal-signal-wiring/contracts/age-153-terminal-signal-wiring.md"
    );
    assert_eq!(facts.kind, expected_kind);
    assert!(facts.evidence_is_object, "{}", facts.raw);
    assert!(facts.invocation_id_is_uuid, "invocation_id uuid");
    if expect_session {
        assert!(facts.session_id_is_uuid, "session_id uuid");
    } else {
        assert!(facts.session_is_null, "{}", facts.raw);
    }
}

fn terminal_signal_expected_keys() -> BTreeSet<String> {
    BTreeSet::from([
        "evidence".to_string(),
        "invocation_id".to_string(),
        "kind".to_string(),
        "session_id".to_string(),
    ])
}

struct TerminalSignalFacts {
    evidence_is_object: bool,
    invocation_id_is_uuid: bool,
    keys: BTreeSet<String>,
    kind: Value,
    raw: Value,
    session_id_is_uuid: bool,
    session_is_null: bool,
}

pub fn assert_no_terminal_marker_on_stdout(output: &Output) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("OULIPOLY_TERMINAL_SIGNAL="),
        "terminal-signal marker must be stderr-only, stdout was:\n{stdout}"
    );
}

pub fn assert_result_envelope_shape(stdout: &str) -> Value {
    let line = single_result_envelope_line(stdout);
    let value = parse_result_envelope_line(line);
    assert_result_envelope_value_shape(&value);
    value
}

fn single_result_envelope_line(stdout: &str) -> &str {
    let lines = result_envelope_lines(stdout);
    assert_eq!(
        lines.len(),
        1,
        "stdout must contain one result envelope:\n{stdout}"
    );
    lines[0]
}

fn result_envelope_lines(stdout: &str) -> Vec<&str> {
    stdout
        .lines()
        .filter(|line| line.starts_with("OULIPOLY_RESULT="))
        .collect()
}

fn parse_result_envelope_line(line: &str) -> Value {
    serde_json::from_str(result_envelope_payload(line)).unwrap()
}

fn result_envelope_payload(line: &str) -> &str {
    line.strip_prefix("OULIPOLY_RESULT=").unwrap()
}

fn assert_result_envelope_value_shape(value: &Value) {
    let object = value.as_object().expect("result envelope object");
    let keys: BTreeSet<_> = object.keys().map(String::as_str).collect();
    let base_keys = result_envelope_base_keys();
    if value["success"] == true {
        assert_eq!(keys, base_keys);
    } else {
        assert_eq!(keys, failure_result_envelope_keys(base_keys));
        assert_eq!(
            value["agent_runner_invocation_id"], value["id"],
            "failure result identity must repeat the runner invocation id"
        );
    }
}

fn result_envelope_base_keys() -> BTreeSet<&'static str> {
    BTreeSet::from([
        "error_category",
        "exit_code",
        "finished_at",
        "id",
        "status",
        "success",
        "terminal_reason",
    ])
}

fn failure_result_envelope_keys(mut base_keys: BTreeSet<&'static str>) -> BTreeSet<&'static str> {
    base_keys.extend([
        "agent_runner_invocation_id",
        "provider_name",
        "provider_session_id",
        "agent_runner_chain_id",
    ]);
    base_keys
}

pub fn normalized_result_stdout(stdout: &str) -> String {
    normalized_stdout_lines(stdout).join("\n") + "\n"
}

fn normalized_stdout_lines(stdout: &str) -> Vec<String> {
    stdout.lines().map(normalized_stdout_line).collect()
}

fn normalized_stdout_line(line: &str) -> String {
    let Some(raw) = result_envelope_payload_opt(line) else {
        return line.to_string();
    };
    format_normalized_result_line(normalized_result_value(parse_result_value(raw)))
}

fn result_envelope_payload_opt(line: &str) -> Option<&str> {
    line.strip_prefix("OULIPOLY_RESULT=")
}

fn parse_result_value(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap()
}

fn normalized_result_value(mut value: Value) -> Value {
    value["id"] = Value::String("<uuid>".to_string());
    value["finished_at"] = Value::String("<ts>".to_string());
    value
}

fn format_normalized_result_line(value: Value) -> String {
    format!("OULIPOLY_RESULT={}", serde_json::to_string(&value).unwrap())
}

pub fn assert_normalized_result_stdout_matches_golden(stdout: &str, golden: &str) {
    assert_eq!(normalized_result_stdout(stdout), golden);
}

pub fn assert_ordered(haystack: &str, first: &str, second: &str) {
    assert_ordered_positions(
        ordered_marker_positions(haystack, first, second),
        haystack,
        first,
        second,
    );
}

fn ordered_marker_positions(
    haystack: &str,
    first: &str,
    second: &str,
) -> (Option<usize>, Option<usize>) {
    (haystack.find(first), haystack.find(second))
}

fn assert_ordered_positions(
    positions: (Option<usize>, Option<usize>),
    haystack: &str,
    first: &str,
    second: &str,
) {
    let first_index = positions
        .0
        .unwrap_or_else(|| panic!("missing first marker {first:?} in:\n{haystack}"));
    let second_index = positions
        .1
        .unwrap_or_else(|| panic!("missing second marker {second:?} in:\n{haystack}"));
    assert!(
        first_index < second_index,
        "expected {first:?} before {second:?} in:\n{haystack}"
    );
}

pub fn main_rs_source() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs");
    fs::read_to_string(&path).unwrap_or_else(|err| panic!("failed to read {path:?}: {err}"))
}

pub fn balancing_source() -> String {
    combined_source(read_manifest_sources(balancing_source_files()))
}

fn balancing_source_files() -> &'static [&'static str] {
    &[
        "src/run/mod.rs",
        "src/run/balancing/mod.rs",
        "src/run/balancing/orchestration.rs",
        "src/run/balancing/accessor.rs",
        "src/run/balancing/mapper.rs",
        "src/run/balancing/mapper/attempt.rs",
        "src/run/balancing/mapper/attempt/completed.rs",
        "src/run/balancing/mapper/attempt/disposition.rs",
        "src/run/balancing/mapper/attempt/quota.rs",
        "src/run/balancing/mapper/attempt/shared.rs",
        "src/run/balancing/mapper/attempt/spawn.rs",
        "src/run/balancing/mapper/context.rs",
        "src/run/balancing/mapper/context/config.rs",
        "src/run/balancing/mapper/context/invocation.rs",
        "src/run/balancing/mapper/context/quota.rs",
        "src/run/balancing/mapper/context/routing.rs",
        "src/run/balancing/mapper/context/session.rs",
        "src/run/balancing/mapper/executor_request.rs",
        "src/run/balancing/mapper/failure.rs",
        "src/run/balancing/mapper/finalizer_request.rs",
        "src/run/balancing/mapper/session_ingest.rs",
        "src/run/balancing/mapper/terminal.rs",
        "src/run/balancing/parser.rs",
        "src/run/balancing/disposition.rs",
        "src/run/balancing/finalization.rs",
        "src/run/balancing/formatter.rs",
        "src/run/balancing/diagnostics.rs",
        "src/run/balancing/predicate.rs",
        "src/run/balancing/state_update.rs",
        "src/run/balancing/validator.rs",
    ]
}

pub fn repl_source() -> String {
    combined_source(read_manifest_sources(repl_source_files()))
}

fn repl_source_files() -> &'static [&'static str] {
    &[
        "src/run/mod.rs",
        "src/run/repl/mod.rs",
        "src/run/repl/orchestration.rs",
        "src/run/repl/execution.rs",
        "src/run/repl/migration.rs",
        "src/run/repl/terminal.rs",
        "src/run/repl/disposition.rs",
        "src/run/repl/finalization.rs",
        "src/run/repl/formatter.rs",
        "src/run/repl/mapper.rs",
        "src/run/repl/validator.rs",
    ]
}

pub fn terminal_outcome_adapter_source() -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut combined = String::new();
    for rel in [
        "src/terminal_outcome_adapter.rs",
        "src/terminal_outcome_adapter/category.rs",
        "src/terminal_outcome_adapter/disposition.rs",
        "src/terminal_outcome_adapter/fixture_override.rs",
        "src/terminal_outcome_adapter/marker.rs",
        "src/terminal_outcome_adapter/outcome.rs",
    ] {
        let path = root.join(rel);
        let extra = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {path:?}: {err}"));
        combined.push('\n');
        combined.push_str(&extra);
    }
    combined
}

pub fn source_block_after<'a>(source: &'a str, start: &str) -> &'a str {
    source_block_from_parse_result(source, start, source_block_bounds(source, start))
}

fn source_block_bounds(source: &str, start: &str) -> Result<(usize, usize), SourceBlockError> {
    let start_idx = source.find(start).ok_or(SourceBlockError::Start)?;
    let open_idx = source[start_idx..]
        .find('{')
        .map(|idx| start_idx + idx)
        .ok_or(SourceBlockError::OpeningBrace)?;
    let mut depth = 1usize;
    let mut idx = open_idx + 1;
    let bytes = source.as_bytes();

    while idx < bytes.len() {
        match bytes[idx] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok((open_idx + 1, idx));
                }
            }
            _ => {}
        }
        idx += 1;
    }

    Err(SourceBlockError::ClosingBrace)
}

fn source_block_from_parse_result<'a>(
    source: &'a str,
    start: &str,
    result: Result<(usize, usize), SourceBlockError>,
) -> &'a str {
    source_block_slice(source, validated_source_block_bounds(start, result))
}

fn validated_source_block_bounds(
    start: &str,
    result: Result<(usize, usize), SourceBlockError>,
) -> (usize, usize) {
    result.unwrap_or_else(|error| panic!("{}", source_block_error_message(error, start)))
}

fn source_block_slice(source: &str, bounds: (usize, usize)) -> &str {
    &source[bounds.0..bounds.1]
}

enum SourceBlockError {
    Start,
    OpeningBrace,
    ClosingBrace,
}

fn source_block_error_message(error: SourceBlockError, start: &str) -> String {
    match error {
        SourceBlockError::Start => format!("missing {start}"),
        SourceBlockError::OpeningBrace => format!("missing opening brace after {start}"),
        SourceBlockError::ClosingBrace => format!("missing closing brace after {start}"),
    }
}

fn signal_consumer_source() -> String {
    combined_source(signal_consumer_source_parts())
}

fn signal_consumer_source_parts() -> Vec<String> {
    let mut sources = vec![main_rs_source(), repl_source()];
    sources.extend(read_manifest_sources(signal_consumer_source_files()));
    sources
}

fn signal_consumer_source_files() -> &'static [&'static str] {
    &[
        "src/invocation/result_envelope.rs",
        "src/invocation/finalize.rs",
    ]
}

fn read_manifest_sources(rels: &[&str]) -> Vec<String> {
    rels.iter().map(|rel| read_manifest_source(rel)).collect()
}

fn read_manifest_source(rel: &str) -> String {
    read_source_path(&manifest_source_path(rel))
}

fn manifest_source_path(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn read_source_path(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|err| panic!("failed to read {path:?}: {err}"))
}

fn combined_source(sources: Vec<String>) -> String {
    let mut combined = String::new();
    for source in sources {
        combined.push('\n');
        combined.push_str(&source);
    }
    combined
}

pub fn assert_signal_consumer_source_wired(function_name: &str, expected_fragments: &[&str]) {
    let source = signal_consumer_source();
    let body = source_block_after(&source, function_name);
    for fragment in expected_fragments {
        assert!(
            body.contains(fragment),
            "{function_name} must contain {fragment:?} per AGE-153 contract:\n/home/nes/projects/agent-runner/planning/age-153-terminal-signal-wiring/contracts/age-153-terminal-signal-wiring.md"
        );
    }
}

pub fn parse_valid_invocations(stderr: &str) -> Vec<CompositeInvocationId> {
    parse_invocation_payloads(invocation_payloads(stderr))
}

fn invocation_payloads(stderr: &str) -> Vec<&str> {
    stderr.lines().filter_map(invocation_payload).collect()
}

fn invocation_payload(line: &str) -> Option<&str> {
    line.strip_prefix("OULIPOLY_INVOCATION=")
}

fn parse_invocation_payloads(payloads: Vec<&str>) -> Vec<CompositeInvocationId> {
    payloads
        .into_iter()
        .filter_map(parse_invocation_payload)
        .collect()
}

fn parse_invocation_payload(raw: &str) -> Option<CompositeInvocationId> {
    CompositeInvocationId::parse_env_value(raw).ok()
}
