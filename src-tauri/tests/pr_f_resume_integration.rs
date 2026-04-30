#![cfg(unix)]

use agent_runner_lib::state::{CompositeInvocationId, SessionTurnIngest, StateDb};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, params};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    models_dir: PathBuf,
}

impl Fixture {
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
            models_dir,
        }
    }

    fn db_path(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("state.db")
    }

    fn open_db(&self) -> StateDb {
        StateDb::open(&self.db_path()).unwrap()
    }

    fn conn(&self) -> Connection {
        let _ = self.open_db();
        Connection::open(self.db_path()).unwrap()
    }

    fn write_sessions_config(&self, provider_name: &str, transcript_path: &Path) {
        let app_config_dir = self.config_home.join("oulipoly-agent-runner");
        fs::create_dir_all(&app_config_dir).unwrap();
        fs::write(
            app_config_dir.join("sessions.toml"),
            format!(
                r#"[{provider_name}]
turn_script = 'cat "{}"'
state_dir = '{}'
"#,
                transcript_path.display(),
                self.dir.path().join("session-state").display()
            ),
        )
        .unwrap();
    }

    fn write_script(&self, name: &str, body: &str) -> PathBuf {
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

    fn write_model_body(&self, model_name: &str, body: &str) {
        fs::write(self.models_dir.join(format!("{model_name}.toml")), body).unwrap();
    }

    fn write_single_provider_model(
        &self,
        model_name: &str,
        provider_name: &str,
        script_path: &Path,
        resume_block: &str,
    ) {
        self.write_model_body(
            model_name,
            &format!(
                r#"prompt_mode = "arg"

[[providers]]
name = "{provider_name}"
command = "{}"
args = ["one-shot-only"]
interactive_args = ["launch"]
{resume_block}
"#,
                script_path.display()
            ),
        );
    }

    fn write_two_provider_model(
        &self,
        model_name: &str,
        provider_a_name: &str,
        provider_a_script: &Path,
        provider_b_name: &str,
        provider_b_script: &Path,
    ) {
        self.write_model_body(
            model_name,
            &format!(
                r#"prompt_mode = "arg"

[[providers]]
name = "{provider_a_name}"
command = "{}"
args = ["exec-a"]
interactive_args = ["launch-a"]

[providers.resume]
kind = "flag"
flag = "--resume"

[[providers]]
name = "{provider_b_name}"
command = "{}"
args = ["exec-b"]
interactive_args = ["launch-b"]

[providers.resume]
kind = "flag"
flag = "--resume"
"#,
                provider_a_script.display(),
                provider_b_script.display()
            ),
        );
    }

    fn write_migratable_two_provider_model(
        &self,
        model_name: &str,
        provider_a_script: &Path,
        provider_b_script: &Path,
        provider_a_projects: &Path,
        provider_b_projects: &Path,
    ) {
        self.write_model_body(
            model_name,
            &format!(
                r#"prompt_mode = "arg"

[[providers]]
name = "claude-a"
command = "{}"
args = ["exec-a"]
interactive_args = ["launch-a"]

[providers.resume]
kind = "flag"
flag = "--resume"

[providers.session_storage]
kind = "claude_code"
projects_dir = "{}"

[[providers]]
name = "claude-b"
command = "{}"
args = ["exec-b"]
interactive_args = ["launch-b"]

[providers.resume]
kind = "flag"
flag = "--resume"

[providers.session_storage]
kind = "claude_code"
projects_dir = "{}"
"#,
                provider_a_script.display(),
                provider_a_projects.display(),
                provider_b_script.display(),
                provider_b_projects.display()
            ),
        );
    }

    fn stage_claude_jsonl(&self, projects_dir: &Path, session_id: &str) -> PathBuf {
        let cwd_dir = projects_dir.join("cwd-hash-fixture");
        fs::create_dir_all(&cwd_dir).unwrap();
        let target = cwd_dir.join(format!("{session_id}.jsonl"));
        fs::write(
            &target,
            format!(
                r#"{{"sessionId":"{session_id}","turnId":"turn-1","timestamp":"2026-04-17T08:00:00Z","type":"assistant"}}"#
            ),
        )
        .unwrap();
        target
    }

    fn seed_active_chain(&self, chain_id: &str, provider: &str, session_id: &str, model: &str) {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, '2026-04-17T08:00:00Z', '2026-04-17T08:00:00Z', ?2)",
            params![chain_id, model],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, '2026-04-17T08:00:00Z', 'initial')",
            params![chain_id, provider, session_id],
        )
        .unwrap();
    }

    fn seed_quota_window(&self, provider: &str, used_percent: f64) {
        let conn = self.conn();
        let refreshed_at = Utc::now().to_rfc3339();
        let resets_at = (Utc::now() + Duration::hours(24)).to_rfc3339();
        conn.execute(
            "INSERT INTO provider_quotas
                (provider_name, used_percent, resets_at, calls_since_refresh, refreshed_at)
             VALUES (?1, ?2, ?3, 0, ?4)
             ON CONFLICT (provider_name) DO UPDATE SET
                used_percent = ?2,
                resets_at = ?3,
                calls_since_refresh = 0,
                refreshed_at = ?4,
                exhausted_at = NULL",
            params![provider, used_percent, resets_at, refreshed_at],
        )
        .unwrap();
        conn.execute(
            "DELETE FROM provider_quota_windows WHERE provider_name = ?1",
            params![provider],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO provider_quota_windows
                (provider_name, window_id, used_percent, resets_at, last_delta_percent, last_delta_calls)
             VALUES (?1, 0, ?2, ?3, 0.01, 22)",
            params![provider, used_percent, resets_at],
        )
        .unwrap();
    }

    fn active_segment(&self, chain_id: &str) -> (String, String) {
        self.conn()
            .query_row(
                "SELECT provider_name, session_id
                 FROM session_chain_segments
                 WHERE chain_id = ?1 AND ended_at IS NULL",
                params![chain_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
    }

    fn base_model_command(&self, model_name: &str) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("-m")
            .arg(model_name)
            .arg("--models-dir")
            .arg(&self.models_dir);
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd
    }

    fn seed_session_turns(&self, provider_name: &str, session_id: &str, turns: &[(&str, &str)]) {
        let db = self.open_db();
        let turns: Vec<SessionTurnIngest> = turns
            .iter()
            .map(|(turn_id, timestamp)| SessionTurnIngest {
                session_id: session_id.to_string(),
                turn_id: (*turn_id).to_string(),
                timestamp: ts(timestamp),
                role: "assistant".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
                is_compaction_boundary: false,
            })
            .collect();
        db.ingest_session_turns_batch(provider_name, &turns)
            .unwrap();
    }

    fn base_repl_command(&self, model_name: &str, resume: Option<&str>) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("repl")
            .arg("--models-dir")
            .arg(&self.models_dir)
            .arg(model_name);
        if let Some(resume) = resume {
            cmd.arg("--resume").arg(resume);
        }
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd
    }

    fn run_repl(&self, model_name: &str, resume: Option<&str>) -> Output {
        self.base_repl_command(model_name, resume).output().unwrap()
    }

    fn base_resume_command(&self, model_name: &str, session_id: &str) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("resume")
            .arg("-m")
            .arg(model_name)
            .arg("--session-id")
            .arg(session_id)
            .arg("--models-dir")
            .arg(&self.models_dir);
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd
    }

    fn base_top_level_resume_command(&self, model_name: &str, session_id: &str) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("-m")
            .arg(model_name)
            .arg("--resume")
            .arg(session_id)
            .arg("--models-dir")
            .arg(&self.models_dir);
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd
    }

    fn base_top_level_resume_without_model_command(&self, session_id: &str) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("--resume")
            .arg(session_id)
            .arg("--models-dir")
            .arg(&self.models_dir);
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd
    }
}

fn ts(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}

fn parse_invocation(stderr: &str) -> CompositeInvocationId {
    let lines: Vec<&str> = stderr
        .lines()
        .filter(|line| line.starts_with("OULIPOLY_INVOCATION="))
        .collect();
    assert_eq!(
        lines.len(),
        1,
        "stderr should contain exactly one invocation line: {stderr}"
    );
    let raw = lines[0].strip_prefix("OULIPOLY_INVOCATION=").unwrap();
    CompositeInvocationId::parse_env_value(raw).unwrap()
}

fn parse_session_line(stderr: &str, invocation_uuid: &str) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .filter(|line| line.starts_with("OULIPOLY_SESSION="))
        .collect();
    assert_eq!(
        lines.len(),
        1,
        "stderr should contain exactly one session line: {stderr}"
    );
    let raw = lines[0].strip_prefix("OULIPOLY_SESSION=").unwrap();
    let value: serde_json::Value = serde_json::from_str(raw).unwrap();
    assert_eq!(value["id"].as_str(), Some(invocation_uuid));
    value["session_id"].as_str().unwrap().to_string()
}

#[test]
fn noninteractive_invocation_ingests_session_and_emits_oulipoly_session_line() {
    let fixture = Fixture::new();
    let transcript_path = fixture.dir.path().join("turns.jsonl");
    fs::write(&transcript_path, "").unwrap();
    fixture.write_sessions_config("claude2", &transcript_path);

    let expected_session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    let script = fixture.write_script(
        "claude-session-writer.sh",
        &format!(
            r#"ts="$(date -u +%Y-%m-%dT%H:%M:%S.%NZ)"
turn_id="turn-$(date +%s%N)-$$"
printf '{{"session_id":"{expected_session_id}","turn_id":"%s","timestamp":"%s","role":"assistant"}}\n' "$turn_id" "$ts" >> "{}"
printf 'mock answer\n'
"#,
            transcript_path.display()
        ),
    );
    fixture.write_single_provider_model("claude-opus", "claude2", &script, "");

    let output = fixture
        .base_model_command("claude-opus")
        .arg("answer once")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let invocation = parse_invocation(&stderr);
    let emitted_session_id = parse_session_line(&stderr, &invocation.id);
    assert_eq!(emitted_session_id, expected_session_id);

    let row = fixture
        .open_db()
        .get_invocation_by_uuid(&invocation.id)
        .unwrap()
        .unwrap();
    assert_eq!(row.session_id.as_deref(), Some(expected_session_id));
}

#[test]
fn resume_invocation_re_emits_session_line_with_resumed_session_id() {
    let fixture = Fixture::new();
    let transcript_path = fixture.dir.path().join("resume-turns.jsonl");
    fs::write(&transcript_path, "").unwrap();
    fixture.write_sessions_config("claude2", &transcript_path);

    let initial_session_id = "8f0a6a1f-9cd2-4c91-b6c6-1f0a0a8c9e22";
    let script = fixture.write_script(
        "claude-resume-session-writer.sh",
        &format!(
            r#"sid="{initial_session_id}"
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--resume" ]; then
    shift
    sid="$1"
  fi
  shift || true
done
ts="$(date -u +%Y-%m-%dT%H:%M:%S.%NZ)"
turn_id="turn-$(date +%s%N)-$$"
printf '{{"session_id":"%s","turn_id":"%s","timestamp":"%s","role":"assistant"}}\n' "$sid" "$turn_id" "$ts" >> "{}"
printf 'mock answer for %s\n' "$sid"
"#,
            transcript_path.display()
        ),
    );
    fixture.write_single_provider_model(
        "claude-opus",
        "claude2",
        &script,
        r#"
[providers.resume]
kind = "flag"
flag = "--resume"
"#,
    );

    let initial_output = fixture
        .base_model_command("claude-opus")
        .arg("start session")
        .output()
        .unwrap();
    assert_eq!(initial_output.status.code(), Some(0), "{initial_output:?}");
    let initial_stderr = String::from_utf8_lossy(&initial_output.stderr);
    let initial_invocation = parse_invocation(&initial_stderr);
    let captured_session_id = parse_session_line(&initial_stderr, &initial_invocation.id);
    assert_eq!(captured_session_id, initial_session_id);

    let resume_output = fixture
        .base_top_level_resume_command("claude-opus", &captured_session_id)
        .arg("continue session")
        .output()
        .unwrap();
    assert_eq!(resume_output.status.code(), Some(0), "{resume_output:?}");
    let resume_stderr = String::from_utf8_lossy(&resume_output.stderr);
    let resume_invocation = parse_invocation(&resume_stderr);
    let resumed_session_id = parse_session_line(&resume_stderr, &resume_invocation.id);
    assert_eq!(resumed_session_id, captured_session_id);

    let row = fixture
        .open_db()
        .get_invocation_by_uuid(&resume_invocation.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        row.session_id.as_deref(),
        Some(captured_session_id.as_str())
    );
}

#[test]
fn top_level_resume_without_prompt_routes_to_interactive_repl() {
    let fixture = Fixture::new();
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    let provider_a_marker = fixture.dir.path().join("top-level-repl-provider-a.txt");
    let provider_b_argv = fixture
        .dir
        .path()
        .join("top-level-repl-provider-b-argv.txt");
    let provider_a = fixture.write_script(
        "top-level-repl-provider-a.sh",
        &format!(
            r#"printf 'provider-a\n' > "{}"; exit 0"#,
            provider_a_marker.display()
        ),
    );
    let provider_b = fixture.write_script(
        "top-level-repl-provider-b.sh",
        &format!(
            r#"printf 'TOP_LEVEL_REPL_RESUME_MARKER\n'; printf '%s\n' "$@" > "{}"; exit 0"#,
            provider_b_argv.display()
        ),
    );
    fixture.write_two_provider_model(
        "balanced-model",
        "claude-default",
        &provider_a,
        "claude-owner",
        &provider_b,
    );
    fixture.seed_session_turns(
        "claude-owner",
        session_id,
        &[("owner-turn", "2026-04-17T08:00:00Z")],
    );

    let output = fixture
        .base_top_level_resume_command("balanced-model", session_id)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        !provider_a_marker.exists(),
        "top-level --resume must route to the session owner"
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("TOP_LEVEL_REPL_RESUME_MARKER"),
        "{output:?}"
    );
    assert_eq!(
        fs::read_to_string(&provider_b_argv).unwrap(),
        "launch-b\n--resume\n5169694d-de0f-40d1-890c-6e28e55bab27\n"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[resume] -> claude-owner"), "{stderr}");
    let invocation = parse_invocation(&stderr);
    let row = fixture
        .open_db()
        .get_invocation_by_uuid(&invocation.id)
        .unwrap()
        .unwrap();
    assert_eq!(row.session_id.as_deref(), Some(session_id));
    assert_eq!(row.session_capture_method.as_deref(), Some("resumed"));
}

#[test]
fn top_level_resume_with_positional_prompt_routes_to_headless() {
    let fixture = Fixture::new();
    let session_id = "8f0a6a1f-9cd2-4c91-b6c6-1f0a0a8c9e22";
    let argv_dump = fixture.dir.path().join("top-level-positional-argv.txt");
    let script = fixture.write_script(
        "top-level-positional.sh",
        &format!(
            r#"printf 'TOP_LEVEL_HEADLESS_RESUME_MARKER\n'; printf '%s\n' "$@" > "{}"; exit 0"#,
            argv_dump.display()
        ),
    );
    fixture.write_single_provider_model(
        "claude-opus",
        "claude2",
        &script,
        r#"
[providers.resume]
kind = "flag"
flag = "--resume"
"#,
    );
    fixture.seed_session_turns("claude2", session_id, &[("turn-1", "2026-04-17T08:00:00Z")]);

    let output = fixture
        .base_top_level_resume_command("claude-opus", session_id)
        .arg("continuation prompt")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("TOP_LEVEL_HEADLESS_RESUME_MARKER"),
        "{output:?}"
    );
    assert_eq!(
        fs::read_to_string(&argv_dump).unwrap(),
        "one-shot-only\n--resume\n8f0a6a1f-9cd2-4c91-b6c6-1f0a0a8c9e22\ncontinuation prompt\n"
    );

    let invocation = parse_invocation(&String::from_utf8_lossy(&output.stderr));
    let row = fixture
        .open_db()
        .get_invocation_by_uuid(&invocation.id)
        .unwrap()
        .unwrap();
    assert_eq!(row.session_id.as_deref(), Some(session_id));
    assert_eq!(row.session_capture_method.as_deref(), Some("resumed"));
}

#[test]
fn top_level_resume_with_file_prompt_routes_to_headless() {
    let fixture = Fixture::new();
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    let answer_path = fixture.dir.path().join("answer.md");
    fs::write(&answer_path, "answer from file\n").unwrap();
    let argv_dump = fixture.dir.path().join("top-level-file-argv.txt");
    let script = fixture.write_script(
        "top-level-file.sh",
        &format!(
            r#"printf 'TOP_LEVEL_FILE_RESUME_MARKER\n'; printf '%s\n' "$@" > "{}"; exit 0"#,
            argv_dump.display()
        ),
    );
    fixture.write_single_provider_model(
        "claude-opus",
        "claude2",
        &script,
        r#"
[providers.resume]
kind = "flag"
flag = "--resume"
"#,
    );
    fixture.seed_session_turns("claude2", session_id, &[("turn-1", "2026-04-17T08:00:00Z")]);

    let output = fixture
        .base_top_level_resume_command("claude-opus", session_id)
        .arg("-f")
        .arg(&answer_path)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("TOP_LEVEL_FILE_RESUME_MARKER"),
        "{output:?}"
    );
    assert_eq!(
        fs::read_to_string(&argv_dump).unwrap(),
        "one-shot-only\n--resume\n5169694d-de0f-40d1-890c-6e28e55bab27\nanswer from file\n\n"
    );

    let invocation = parse_invocation(&String::from_utf8_lossy(&output.stderr));
    let row = fixture
        .open_db()
        .get_invocation_by_uuid(&invocation.id)
        .unwrap()
        .unwrap();
    assert_eq!(row.session_id.as_deref(), Some(session_id));
    assert_eq!(row.session_capture_method.as_deref(), Some("resumed"));
}

#[test]
fn top_level_resume_without_model_reports_missing_session() {
    let fixture = Fixture::new();
    let output = fixture
        .base_top_level_resume_without_model_command("5169694d-de0f-40d1-890c-6e28e55bab27")
        .output()
        .unwrap();

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("No session found matching"), "{stderr}");
}

#[test]
fn noninteractive_resume_reads_answer_file_and_records_resumed_target() {
    let fixture = Fixture::new();
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    let answer_path = fixture.dir.path().join("answer.md");
    fs::write(&answer_path, "answer from root\n").unwrap();
    let stdin_dump = fixture.dir.path().join("stdin.txt");
    let argv_dump = fixture.dir.path().join("argv.txt");
    let script = fixture.write_script(
        "claude.sh",
        &format!(
            r#"printf '%s\n' "$@" > "{argv}"; cat > "{stdin}"; exit 0"#,
            argv = argv_dump.display(),
            stdin = stdin_dump.display()
        ),
    );
    fixture.write_single_provider_model(
        "claude-opus",
        "claude2",
        &script,
        r#"
[providers.resume]
kind = "flag"
flag = "--resume"
"#,
    );
    fixture.seed_session_turns("claude2", session_id, &[("turn-1", "2026-04-17T08:00:00Z")]);

    let output = fixture
        .base_resume_command("claude-opus", session_id)
        .arg("-f")
        .arg(&answer_path)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        fs::read_to_string(&argv_dump).unwrap(),
        "one-shot-only\n--resume\n5169694d-de0f-40d1-890c-6e28e55bab27\nanswer from root\n\n"
    );
    assert_eq!(fs::read_to_string(&stdin_dump).unwrap(), "");

    let invocation = parse_invocation(&String::from_utf8_lossy(&output.stderr));
    let row = fixture
        .open_db()
        .get_invocation_by_uuid(&invocation.id)
        .unwrap()
        .unwrap();
    assert_eq!(row.session_id.as_deref(), Some(session_id));
    assert_eq!(row.session_capture_method.as_deref(), Some("resumed"));
}

#[test]
fn noninteractive_resume_routes_to_session_owner_in_multi_provider_model() {
    let fixture = Fixture::new();
    let session_id = "8f0a6a1f-9cd2-4c91-b6c6-1f0a0a8c9e22";
    let provider_a_marker = fixture.dir.path().join("provider-a.txt");
    let provider_b_marker = fixture.dir.path().join("provider-b.txt");
    let provider_a = fixture.write_script(
        "provider-a.sh",
        &format!(
            r#"printf 'provider-a\n' > "{}"; exit 0"#,
            provider_a_marker.display()
        ),
    );
    let provider_b = fixture.write_script(
        "provider-b.sh",
        &format!(
            r#"printf '%s\n' "$@" > "{}"; exit 0"#,
            provider_b_marker.display()
        ),
    );
    fixture.write_two_provider_model(
        "balanced-model",
        "claude-default",
        &provider_a,
        "claude-owner",
        &provider_b,
    );
    fixture.seed_session_turns(
        "claude-owner",
        session_id,
        &[("owner-turn", "2026-04-17T08:00:00Z")],
    );

    let output = fixture
        .base_resume_command("balanced-model", session_id)
        .arg("--prompt")
        .arg("answer text")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        !provider_a_marker.exists(),
        "resume must not launch provider A/default provider"
    );
    assert_eq!(
        fs::read_to_string(&provider_b_marker).unwrap(),
        "exec-b\n--resume\n8f0a6a1f-9cd2-4c91-b6c6-1f0a0a8c9e22\nanswer text\n"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[resume] -> claude-owner"), "{stderr}");

    let invocation = parse_invocation(&stderr);
    let row = fixture
        .open_db()
        .get_invocation_by_uuid(&invocation.id)
        .unwrap()
        .unwrap();
    assert_eq!(row.provider_name.as_deref(), Some("claude-owner"));
    assert_eq!(row.provider_index, 1);
}

#[test]
fn interactive_repl_resume_routes_to_session_owner_in_multi_provider_model() {
    let fixture = Fixture::new();
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    let provider_a_marker = fixture.dir.path().join("interactive-provider-a.txt");
    let provider_b_marker = fixture.dir.path().join("interactive-provider-b.txt");
    let provider_a = fixture.write_script(
        "interactive-provider-a.sh",
        &format!(
            r#"printf 'provider-a\n' > "{}"; exit 0"#,
            provider_a_marker.display()
        ),
    );
    let provider_b = fixture.write_script(
        "interactive-provider-b.sh",
        &format!(
            r#"printf '%s\n' "$@" > "{}"; exit 0"#,
            provider_b_marker.display()
        ),
    );
    fixture.write_two_provider_model(
        "balanced-model",
        "claude-default",
        &provider_a,
        "claude-owner",
        &provider_b,
    );
    fixture.seed_session_turns(
        "claude-owner",
        session_id,
        &[("owner-turn", "2026-04-17T08:00:00Z")],
    );

    let output = fixture.run_repl("balanced-model", Some(session_id));

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        !provider_a_marker.exists(),
        "repl --resume must not launch provider A/default provider"
    );
    assert_eq!(
        fs::read_to_string(&provider_b_marker).unwrap(),
        "launch-b\n--resume\n5169694d-de0f-40d1-890c-6e28e55bab27\n"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[resume] -> claude-owner"), "{stderr}");

    let invocation = parse_invocation(&stderr);
    let row = fixture
        .open_db()
        .get_invocation_by_uuid(&invocation.id)
        .unwrap()
        .unwrap();
    assert_eq!(row.provider_name.as_deref(), Some("claude-owner"));
    assert_eq!(row.provider_index, 1);
}

#[test]
fn repl_resume_migrates_to_best_scored_provider() {
    let fixture = Fixture::new();
    let chain_id = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    let source_projects = fixture.dir.path().join("source-projects");
    let target_projects = fixture.dir.path().join("target-projects");
    fixture.stage_claude_jsonl(&source_projects, session_id);
    let provider_a_marker = fixture.dir.path().join("migrate-provider-a.txt");
    let provider_b_marker = fixture.dir.path().join("migrate-provider-b.txt");
    let provider_a = fixture.write_script(
        "migrate-provider-a.sh",
        &format!(
            r#"printf '%s\n' "$@" > "{}"; exit 0"#,
            provider_a_marker.display()
        ),
    );
    let provider_b = fixture.write_script(
        "migrate-provider-b.sh",
        &format!(
            r#"printf '%s\n' "$@" > "{}"; exit 0"#,
            provider_b_marker.display()
        ),
    );
    fixture.write_migratable_two_provider_model(
        "balanced-model",
        &provider_a,
        &provider_b,
        &source_projects,
        &target_projects,
    );
    fixture.seed_active_chain(chain_id, "claude-a", session_id, "balanced-model");
    fixture.seed_quota_window("claude-a", 0.83);
    fixture.seed_quota_window("claude-b", 0.12);

    let output = fixture.run_repl("balanced-model", Some(session_id));

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        !provider_a_marker.exists(),
        "repl --resume should launch migrated provider, not original active provider"
    );
    assert_eq!(
        fs::read_to_string(&provider_b_marker).unwrap(),
        "launch-b\n--resume\n5169694d-de0f-40d1-890c-6e28e55bab27\n"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[resume] -> claude-a"), "{stderr}");
    assert!(
        stderr.contains("[migrate] claude-a -> claude-b reason=quota_threshold"),
        "{stderr}"
    );
    assert_eq!(
        fixture.active_segment(chain_id),
        ("claude-b".to_string(), session_id.to_string())
    );
    assert!(
        target_projects
            .join("cwd-hash-fixture")
            .join(format!("{session_id}.jsonl"))
            .exists()
    );
}

#[test]
fn repl_resume_stays_when_active_is_best_scored() {
    let fixture = Fixture::new();
    let chain_id = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    let source_projects = fixture.dir.path().join("source-projects");
    let target_projects = fixture.dir.path().join("target-projects");
    fixture.stage_claude_jsonl(&source_projects, session_id);
    let provider_a_marker = fixture.dir.path().join("stay-provider-a.txt");
    let provider_b_marker = fixture.dir.path().join("stay-provider-b.txt");
    let provider_a = fixture.write_script(
        "stay-provider-a.sh",
        &format!(
            r#"printf '%s\n' "$@" > "{}"; exit 0"#,
            provider_a_marker.display()
        ),
    );
    let provider_b = fixture.write_script(
        "stay-provider-b.sh",
        &format!(
            r#"printf '%s\n' "$@" > "{}"; exit 0"#,
            provider_b_marker.display()
        ),
    );
    fixture.write_migratable_two_provider_model(
        "balanced-model",
        &provider_a,
        &provider_b,
        &source_projects,
        &target_projects,
    );
    fixture.seed_active_chain(chain_id, "claude-a", session_id, "balanced-model");
    fixture.seed_quota_window("claude-a", 0.12);
    fixture.seed_quota_window("claude-b", 0.83);

    let output = fixture.run_repl("balanced-model", Some(session_id));

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        !provider_b_marker.exists(),
        "repl --resume should stay on active provider when it is best scored"
    );
    assert_eq!(
        fs::read_to_string(&provider_a_marker).unwrap(),
        "launch-a\n--resume\n5169694d-de0f-40d1-890c-6e28e55bab27\n"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[resume] -> claude-a"), "{stderr}");
    assert!(!stderr.contains("[migrate]"), "{stderr}");
    assert_eq!(
        fixture.active_segment(chain_id),
        ("claude-a".to_string(), session_id.to_string())
    );
}

#[test]
fn resume_happy_path_records_resumed_capture_on_invocation_row() {
    let fixture = Fixture::new();
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    let script = fixture.write_script("claude.sh", "exit 0");
    fixture.write_single_provider_model(
        "claude-opus",
        "claude2",
        &script,
        r#"
[providers.resume]
kind = "flag"
flag = "--resume"
"#,
    );
    fixture.seed_session_turns("claude2", session_id, &[("turn-1", "2026-04-17T08:00:00Z")]);

    let output = fixture.run_repl("claude-opus", Some(session_id));

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let invocation = parse_invocation(&String::from_utf8_lossy(&output.stderr));
    let row = fixture
        .open_db()
        .get_invocation_by_uuid(&invocation.id)
        .unwrap()
        .unwrap();

    assert_eq!(row.session_id.as_deref(), Some(session_id));
    assert_eq!(row.session_capture_method.as_deref(), Some("resumed"));
}

#[test]
fn resume_happy_path_emits_short_provider_line() {
    let fixture = Fixture::new();
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    let script = fixture.write_script("claude.sh", "exit 0");
    fixture.write_single_provider_model(
        "claude-opus",
        "claude2",
        &script,
        r#"
[providers.resume]
kind = "flag"
flag = "--resume"
"#,
    );
    fixture.seed_session_turns("claude2", session_id, &[("turn-1", "2026-04-17T08:00:00Z")]);

    let output = fixture.run_repl("claude-opus", Some(session_id));
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(stderr.contains("[resume] -> claude2"), "{stderr}");
}

#[test]
fn resume_single_match_omits_duplicate_detail_line() {
    let fixture = Fixture::new();
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    let script = fixture.write_script("claude.sh", "exit 0");
    fixture.write_single_provider_model(
        "claude-opus",
        "claude2",
        &script,
        r#"
[providers.resume]
kind = "flag"
flag = "--resume"
"#,
    );
    fixture.seed_session_turns("claude2", session_id, &[("turn-1", "2026-04-17T08:00:00Z")]);

    let output = fixture.run_repl("claude-opus", Some(session_id));
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!stderr.contains("matched claude2"), "{stderr}");
}

#[test]
fn resume_multiple_matches_omit_legacy_duplicate_detail_line() {
    let fixture = Fixture::new();
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    let script = fixture.write_script("claude.sh", "exit 0");
    fixture.write_single_provider_model(
        "claude-opus",
        "claude2",
        &script,
        r#"
[providers.resume]
kind = "flag"
flag = "--resume"
"#,
    );
    fixture.seed_session_turns("claude2", session_id, &[("turn-a", "2026-04-17T08:00:10Z")]);
    fixture.seed_session_turns("codex", session_id, &[("turn-b", "2026-04-17T08:00:05Z")]);

    let output = fixture.run_repl("claude-opus", Some(session_id));
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(stderr.contains("[resume] -> claude2"), "{stderr}");
    assert!(
        !stderr.contains(&format!("[resume] session {session_id} matched")),
        "{stderr}"
    );
}

#[test]
fn resume_unknown_session_exits_one_with_not_found_message() {
    let fixture = Fixture::new();
    let script = fixture.write_script("claude.sh", "exit 0");
    fixture.write_single_provider_model(
        "claude-opus",
        "claude2",
        &script,
        r#"
[providers.resume]
kind = "flag"
flag = "--resume"
"#,
    );

    let output = fixture.run_repl("claude-opus", Some("5169694d-de0f-40d1-890c-6e28e55bab27"));
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(stderr.contains("No session found matching"), "{stderr}");
}

#[test]
fn resume_model_pool_mismatch_suggests_other_model() {
    let fixture = Fixture::new();
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    let script = fixture.write_script("fixture.sh", "exit 0");
    fixture.write_single_provider_model(
        "wrong-model",
        "codex",
        &script,
        r#"
[providers.resume]
kind = "subcommand"
subcommand = ["resume"]
"#,
    );
    fixture.write_single_provider_model(
        "claude-opus",
        "claude2",
        &script,
        r#"
[providers.resume]
kind = "flag"
flag = "--resume"
"#,
    );
    fixture.seed_session_turns("claude2", session_id, &[("turn-1", "2026-04-17T08:00:00Z")]);

    let output = fixture.run_repl("wrong-model", Some(session_id));
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        stderr.contains(&format!(
            "session {session_id} belongs to provider claude2, which is not in model wrong-model's provider pool"
        )),
        "{stderr}"
    );
    assert!(stderr.contains("claude-opus"), "{stderr}");
}

#[test]
fn resume_model_pool_mismatch_says_no_other_model_when_no_suggestions() {
    let fixture = Fixture::new();
    let session_id = "0824f7d1-7a3d-4ff8-8e4b-8c1d3b0d3e2c";
    let script = fixture.write_script("fixture.sh", "exit 0");
    // Only one model exists, and its provider list does NOT include
    // the resolved provider. This exercises the
    // resume_model_pool_mismatch_message empty-suggestions branch.
    fixture.write_single_provider_model(
        "wrong-model",
        "codex",
        &script,
        r#"
[providers.resume]
kind = "subcommand"
subcommand = ["resume"]
"#,
    );
    fixture.seed_session_turns("claude2", session_id, &[("turn-1", "2026-04-17T08:00:00Z")]);

    let output = fixture.run_repl("wrong-model", Some(session_id));
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        stderr.contains("(no other model in the loaded config includes claude2)"),
        "{stderr}"
    );
}

#[test]
fn resume_rejects_malformed_uuid_before_lookup() {
    let fixture = Fixture::new();
    let script = fixture.write_script("claude.sh", "exit 0");
    fixture.write_single_provider_model(
        "claude-opus",
        "claude2",
        &script,
        r#"
[providers.resume]
kind = "flag"
flag = "--resume"
"#,
    );

    let output = fixture.run_repl("claude-opus", Some("not-a-uuid"));
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        stderr.contains("invalid session UUID: not-a-uuid"),
        "{stderr}"
    );
}

#[test]
fn resume_requires_provider_resume_block() {
    let fixture = Fixture::new();
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    let script = fixture.write_script("claude.sh", "exit 0");
    fixture.write_single_provider_model("claude-opus", "claude2", &script, "");
    fixture.seed_session_turns("claude2", session_id, &[("turn-1", "2026-04-17T08:00:00Z")]);

    let output = fixture.run_repl("claude-opus", Some(session_id));
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        stderr.contains("provider claude2 has no [providers.resume] block; cannot resume"),
        "{stderr}"
    );
}

#[test]
fn resume_marks_capture_as_resumed_before_nonzero_child_exit() {
    let fixture = Fixture::new();
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    let script = fixture.write_script("claude.sh", "exit 7");
    fixture.write_single_provider_model(
        "claude-opus",
        "claude2",
        &script,
        r#"
[providers.resume]
kind = "flag"
flag = "--resume"
"#,
    );
    fixture.seed_session_turns("claude2", session_id, &[("turn-1", "2026-04-17T08:00:00Z")]);

    let output = fixture.run_repl("claude-opus", Some(session_id));

    assert_eq!(output.status.code(), Some(7), "{output:?}");
    let invocation = parse_invocation(&String::from_utf8_lossy(&output.stderr));
    let row = fixture
        .open_db()
        .get_invocation_by_uuid(&invocation.id)
        .unwrap()
        .unwrap();
    assert_eq!(row.session_id.as_deref(), Some(session_id));
    assert_eq!(row.session_capture_method.as_deref(), Some("resumed"));
}

#[test]
fn resume_codex_subcommand_happy_path_executes_and_records_provenance() {
    // Per PR-F contract §test-contract item 6 + proposal §11 followup:
    // end-to-end runtime verification that kind = "subcommand" composition
    // actually launches and records resumed provenance, not just argv
    // assembly. Mirrors the Claude (kind = "flag") happy-path test but
    // exercises the Codex composition shape under a full repl --resume
    // lifecycle.
    let fixture = Fixture::new();
    let session_id = "8f0a6a1f-9cd2-4c91-b6c6-1f0a0a8c9e22";
    // Fixture script accepts the appended `resume <UUID>` argv shape and
    // exits cleanly; this proves the runner composed the subcommand
    // strategy correctly through to spawn.
    // The fixture model declares `interactive_args = ["launch"]`, so the
    // composed argv is `["launch", "resume", "<UUID>"]`: `$1` is the
    // interactive_args token and `$2`/`$3` are the resume strategy
    // composition. Asserting both halves proves the runner appended the
    // strategy after interactive_args, not the other way around.
    let script = fixture.write_script(
        "codex.sh",
        r#"if [ "$1" = "launch" ] && [ "$2" = "resume" ] && [ -n "$3" ]; then exit 0; else exit 99; fi"#,
    );
    fixture.write_single_provider_model(
        "codex-high",
        "codex",
        &script,
        r#"
[providers.resume]
kind = "subcommand"
subcommand = ["resume"]
"#,
    );
    fixture.seed_session_turns("codex", session_id, &[("turn-1", "2026-04-17T08:00:00Z")]);

    let output = fixture.run_repl("codex-high", Some(session_id));

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[resume] -> codex"), "{stderr}");

    let invocation = parse_invocation(&stderr);
    let row = fixture
        .open_db()
        .get_invocation_by_uuid(&invocation.id)
        .unwrap()
        .unwrap();
    assert_eq!(row.session_id.as_deref(), Some(session_id));
    assert_eq!(row.session_capture_method.as_deref(), Some("resumed"));
}

#[test]
fn repl_without_resume_records_none_capture_method_regression() {
    let fixture = Fixture::new();
    let script = fixture.write_script("claude.sh", "exit 0");
    fixture.write_single_provider_model(
        "claude-opus",
        "claude2",
        &script,
        r#"
[providers.resume]
kind = "flag"
flag = "--resume"
"#,
    );

    let output = fixture.run_repl("claude-opus", None);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let invocation = parse_invocation(&String::from_utf8_lossy(&output.stderr));
    let row = fixture
        .open_db()
        .get_invocation_by_uuid(&invocation.id)
        .unwrap()
        .unwrap();
    assert_eq!(row.session_id, None);
    assert_eq!(row.session_capture_method.as_deref(), Some("none"));
}
