#![cfg(unix)]
#![allow(dead_code)]

use oulipoly_state::{CompositeInvocationId, InvocationStatus, StateDb};
use rusqlite::{Connection, params};
use serde_json::Value;
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

impl Age153Fixture {
    pub fn new() -> Self {
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
            models_dir,
        }
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
        let path = self.dir.path().join(name);
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

    pub fn write_model(&self, model_name: &str, providers: &[&str]) {
        let mut body = String::new();
        for provider in providers {
            body.push_str(&format!(
                r#"[[providers]]
name = "{provider}"
args = []
interactive_args = ["interactive"]

"#
            ));
        }
        fs::write(self.models_dir.join(format!("{model_name}.toml")), body).unwrap();
    }

    pub fn write_providers_with_bodies(&self, providers: &[(&str, &str)]) {
        let mut body = String::new();
        for (provider, command_body) in providers {
            let command = self.write_script(&format!("{provider}-command.sh"), command_body);
            body.push_str(&format!(
                r#"[{provider}]
command = {}
args = []
interactive_args = ["interactive"]
prompt_mode = "arg"

"#,
                toml_string(&command.display().to_string())
            ));
        }
        fs::write(self.app_config_dir.join("providers.toml"), body).unwrap();
    }

    pub fn write_providers_with_command_paths(&self, providers: &[(&str, &Path)]) {
        let mut body = String::new();
        for (provider, command) in providers {
            body.push_str(&format!(
                r#"[{provider}]
command = {}
args = []
interactive_args = ["interactive"]
prompt_mode = "arg"

"#,
                toml_string(&command.display().to_string())
            ));
        }
        fs::write(self.app_config_dir.join("providers.toml"), body).unwrap();
    }

    pub fn write_resume_pool(&self, model_name: &str, providers: &[(&str, String)]) {
        let mut model = String::new();
        let mut providers_toml = String::new();
        for (provider, command_body) in providers {
            model.push_str(&format!(
                r#"[[providers]]
name = "{provider}"
args = ["exec-{provider}"]

"#
            ));
            let command = self.write_script(&format!("{provider}-resume.sh"), command_body);
            let projects_dir = self.provider_projects_dir(provider);
            providers_toml.push_str(&format!(
                r#"[{provider}]
command = {}
args = []
interactive_args = ["launch-{provider}"]
prompt_mode = "arg"

[{provider}.resume]
kind = "flag"
flag = "--resume"

[{provider}.session_storage]
kind = "claude_code"
projects_dir = {}

"#,
                toml_string(&command.display().to_string()),
                toml_string(&projects_dir.display().to_string())
            ));
        }
        fs::write(self.models_dir.join(format!("{model_name}.toml")), model).unwrap();
        fs::write(self.app_config_dir.join("providers.toml"), providers_toml).unwrap();
    }

    pub fn provider_projects_dir(&self, provider: &str) -> PathBuf {
        self.dir.path().join(format!("{provider}-projects"))
    }

    pub fn stage_active_claude_jsonl(&self, provider: &str) {
        let source_dir = self.provider_projects_dir(provider).join("source-project");
        fs::create_dir_all(&source_dir).unwrap();
        fs::write(
            source_dir.join(format!("{SESSION_ID}.jsonl")),
            format!(
                r#"{{"sessionId":"{SESSION_ID}","turnId":"turn-1","timestamp":"2026-04-17T08:00:00Z","type":"assistant"}}"#
            ),
        )
        .unwrap();
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
    let value = parse_terminal_signal_line(line);
    let object = value
        .as_object()
        .expect("terminal signal marker body object");
    let keys: BTreeSet<_> = object.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        BTreeSet::from(["evidence", "invocation_id", "kind", "session_id"]),
        "AGE-153 marker schema from /home/nes/projects/agent-runner/planning/age-153-terminal-signal-wiring/contracts/age-153-terminal-signal-wiring.md"
    );
    assert_eq!(value["kind"], expected_kind);
    assert!(value["evidence"].is_object(), "{value}");
    let invocation_id = value["invocation_id"]
        .as_str()
        .expect("invocation_id string");
    uuid::Uuid::parse_str(invocation_id).expect("invocation_id uuid");
    if expect_session {
        let session_id = value["session_id"].as_str().expect("session_id string");
        uuid::Uuid::parse_str(session_id).expect("session_id uuid");
    } else {
        assert!(value["session_id"].is_null(), "{value}");
    }
}

pub fn assert_no_terminal_marker_on_stdout(output: &Output) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("OULIPOLY_TERMINAL_SIGNAL="),
        "terminal-signal marker must be stderr-only, stdout was:\n{stdout}"
    );
}

pub fn assert_result_envelope_shape(stdout: &str) -> Value {
    let lines: Vec<_> = stdout
        .lines()
        .filter(|line| line.starts_with("OULIPOLY_RESULT="))
        .collect();
    assert_eq!(
        lines.len(),
        1,
        "stdout must contain one result envelope:\n{stdout}"
    );
    let raw = lines[0].strip_prefix("OULIPOLY_RESULT=").unwrap();
    let value: Value = serde_json::from_str(raw).unwrap();
    let object = value.as_object().expect("result envelope object");
    let keys: BTreeSet<_> = object.keys().map(String::as_str).collect();
    let base_keys = BTreeSet::from([
        "error_category",
        "exit_code",
        "finished_at",
        "id",
        "status",
        "success",
        "terminal_reason",
    ]);
    if value["success"] == true {
        assert_eq!(keys, base_keys);
    } else {
        let mut expected = base_keys;
        expected.extend([
            "agent_runner_invocation_id",
            "provider_name",
            "provider_session_id",
            "agent_runner_chain_id",
        ]);
        assert_eq!(keys, expected);
        assert_eq!(
            value["agent_runner_invocation_id"], value["id"],
            "failure result identity must repeat the runner invocation id"
        );
    }
    value
}

pub fn normalized_result_stdout(stdout: &str) -> String {
    stdout
        .lines()
        .map(|line| {
            let Some(raw) = line.strip_prefix("OULIPOLY_RESULT=") else {
                return line.to_string();
            };
            let mut value: Value = serde_json::from_str(raw).unwrap();
            value["id"] = Value::String("<uuid>".to_string());
            value["finished_at"] = Value::String("<ts>".to_string());
            format!("OULIPOLY_RESULT={}", serde_json::to_string(&value).unwrap())
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

pub fn assert_normalized_result_stdout_matches_golden(stdout: &str, golden: &str) {
    assert_eq!(normalized_result_stdout(stdout), golden);
}

pub fn assert_ordered(haystack: &str, first: &str, second: &str) {
    let first_index = haystack
        .find(first)
        .unwrap_or_else(|| panic!("missing first marker {first:?} in:\n{haystack}"));
    let second_index = haystack
        .find(second)
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
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut combined = String::new();
    for rel in [
        "src/run/mod.rs",
        "src/run/balancing/mod.rs",
        "src/run/balancing/orchestration.rs",
        "src/run/balancing/accessor.rs",
        "src/run/balancing/mapper.rs",
        "src/run/balancing/parser.rs",
        "src/run/balancing/disposition.rs",
        "src/run/balancing/finalization.rs",
        "src/run/balancing/formatter.rs",
        "src/run/balancing/diagnostics.rs",
        "src/run/balancing/predicate.rs",
        "src/run/balancing/state_update.rs",
        "src/run/balancing/validator.rs",
    ] {
        let path = root.join(rel);
        let extra = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {path:?}: {err}"));
        combined.push('\n');
        combined.push_str(&extra);
    }
    combined
}

pub fn repl_source() -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut combined = String::new();
    for rel in [
        "src/run/mod.rs",
        "src/run/repl/mod.rs",
        "src/run/repl/orchestration.rs",
        "src/run/repl/disposition.rs",
        "src/run/repl/finalization.rs",
        "src/run/repl/formatter.rs",
        "src/run/repl/mapper.rs",
        "src/run/repl/validator.rs",
    ] {
        let path = root.join(rel);
        let extra = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {path:?}: {err}"));
        combined.push('\n');
        combined.push_str(&extra);
    }
    combined
}

pub fn terminal_outcome_adapter_source() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/terminal_outcome_adapter.rs");
    fs::read_to_string(&path).unwrap_or_else(|err| panic!("failed to read {path:?}: {err}"))
}

pub fn source_block_after<'a>(source: &'a str, start: &str) -> &'a str {
    let start_idx = source
        .find(start)
        .unwrap_or_else(|| panic!("missing {start}"));
    let open_idx = source[start_idx..]
        .find('{')
        .map(|idx| start_idx + idx)
        .unwrap_or_else(|| panic!("missing opening brace after {start}"));
    let mut depth = 1usize;
    let mut idx = open_idx + 1;
    let bytes = source.as_bytes();

    while idx < bytes.len() {
        match bytes[idx] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[open_idx + 1..idx];
                }
            }
            _ => {}
        }
        idx += 1;
    }

    panic!("missing closing brace after {start}");
}

fn signal_consumer_source() -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut combined = main_rs_source();
    combined.push('\n');
    combined.push_str(&repl_source());
    for rel in [
        "src/invocation/result_envelope.rs",
        "src/invocation/finalize.rs",
    ] {
        let path = root.join(rel);
        let extra = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {path:?}: {err}"));
        combined.push('\n');
        combined.push_str(&extra);
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
    stderr
        .lines()
        .filter_map(|line| line.strip_prefix("OULIPOLY_INVOCATION="))
        .filter_map(|raw| CompositeInvocationId::parse_env_value(raw).ok())
        .collect()
}
