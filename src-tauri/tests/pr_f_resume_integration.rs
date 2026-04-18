#![cfg(unix)]

use agent_runner_lib::state::{CompositeInvocationId, SessionTurnIngest, StateDb};
use chrono::{DateTime, Utc};
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
fn resume_multiple_matches_emit_duplicate_detail_line_on_non_tty_stderr() {
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

    assert!(
        stderr.contains(&format!(
            "[resume] session {session_id} matched claude2, codex; selected claude2 by latest turn timestamp"
        )),
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
