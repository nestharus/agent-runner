#![cfg(unix)]

use chrono::{DateTime, Utc};
use oulipoly_state::{CompositeInvocationId, InvocationStatus, SessionTurnIngest, StateDb};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Fixture {
    _dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    app_config_dir: PathBuf,
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
            _dir: dir,
            config_home,
            data_home,
            app_config_dir,
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

    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd
    }

    fn write_model(&self, name: &str, body: &str) {
        fs::write(self.models_dir.join(format!("{name}.toml")), body).unwrap();
    }

    fn write_config(&self, body: &str) {
        fs::write(self.app_config_dir.join("config.toml"), body).unwrap();
    }

    fn write_providers(&self, body: &str) {
        fs::write(self.app_config_dir.join("providers.toml"), body).unwrap();
    }

    fn seed_session_turns(&self, provider_name: &str, session_id: &str) {
        let db = self.open_db();
        let turns = vec![SessionTurnIngest {
            session_id: session_id.to_string(),
            turn_id: "turn-1".to_string(),
            timestamp: ts("2026-04-17T08:00:00Z"),
            role: "assistant".to_string(),
            parent_turn_id: None,
            is_sidechain: false,
            is_compaction_boundary: false,
            body: None,
        }];
        db.ingest_session_turns_batch(provider_name, &turns)
            .unwrap();
    }
}

fn ts(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}

fn fixture_script(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tests")
        .join("fixtures")
        .join("age27")
        .join(name)
}

fn toml_command(command: &str) -> String {
    format!("'{command}'")
}

fn diagnostic_command(prompt_dump: &Path, category: &str, summary: &str) -> String {
    let script = fixture_script("diagnostic-provider.sh");
    toml_command(&format!(
        "env AGE27_DIAGNOSTIC_PROMPT_DUMP=\"{}\" AGE27_DIAGNOSTIC_CATEGORY={} AGE27_DIAGNOSTIC_SUMMARY=\"{}\" \"{}\"",
        prompt_dump.display(),
        category,
        summary,
        script.display()
    ))
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

fn assert_ordered(stderr: &str, first: &str, second: &str) {
    let first_index = stderr.find(first).unwrap_or_else(|| {
        panic!("missing first marker {first:?} in stderr:\n{stderr}");
    });
    let second_index = stderr.find(second).unwrap_or_else(|| {
        panic!("missing second marker {second:?} in stderr:\n{stderr}");
    });
    assert!(
        first_index < second_index,
        "expected {first:?} before {second:?} in stderr:\n{stderr}"
    );
}

fn write_one_shot_models(fixture: &Fixture) {
    fixture.write_model(
        "failing",
        r#"[[providers]]
name = "failure-provider"
"#,
    );
    fixture.write_model(
        "diagnostic",
        r#"[[providers]]
name = "diagnostic-provider"
"#,
    );
    fixture.write_config(
        r#"diagnostics_model = "diagnostic"
"#,
    );
}

#[test]
fn failed_one_shot_loads_migrated_diagnostic_model_via_effective_provider_and_persists_category() {
    let fixture = Fixture::new();
    write_one_shot_models(&fixture);
    let prompt_dump = fixture._dir.path().join("diagnostic-prompt.txt");
    fixture.write_providers(&format!(
        r#"[failure-provider]
command = {}
args = []
prompt_mode = "arg"

[diagnostic-provider]
command = {}
args = []
prompt_mode = "stdin"
"#,
        toml_command(&fixture_script("failure-provider.sh").display().to_string()),
        diagnostic_command(
            &prompt_dump,
            "network_error",
            "Diagnostic model saw network trouble"
        )
    ));

    let output = fixture
        .command()
        .arg("--models-dir")
        .arg(&fixture.models_dir)
        .arg("--model")
        .arg("failing")
        .arg("trigger failure")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(7), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[diagnostics] network_error: Diagnostic model saw network trouble"),
        "{stderr}"
    );
    assert!(stderr.contains("[diagnostics: network_error]"), "{stderr}");
    assert!(!stderr.contains("Empty command"), "{stderr}");
    assert_ordered(
        &stderr,
        "[diagnostics] network_error",
        "opaque child failure",
    );
    assert_ordered(
        &stderr,
        "opaque child failure",
        "[diagnostics: network_error]",
    );

    let prompt = fs::read_to_string(prompt_dump).unwrap();
    assert!(prompt.contains("Exit code: 7"), "{prompt}");
    assert!(prompt.contains("opaque child failure"), "{prompt}");
    assert!(
        prompt.contains("- network_error: Connection refused"),
        "{prompt}"
    );
    assert!(
        prompt.contains("Respond with ONLY the category name on the first line"),
        "{prompt}"
    );

    let invocation = parse_invocation(&stderr);
    let row = fixture
        .open_db()
        .get_invocation_by_uuid(&invocation.id)
        .unwrap()
        .unwrap();
    assert_eq!(row.status, InvocationStatus::Failed);
    assert_eq!(row.exit_code, Some(7));
    assert_eq!(row.error_category.as_deref(), Some("network_error"));
}

#[test]
fn diagnostic_quota_exhausted_marks_active_provider_exhausted() {
    let fixture = Fixture::new();
    fixture.write_model(
        "failing",
        r#"[[providers]]
name = "claude-failure-provider"
"#,
    );
    fixture.write_model(
        "diagnostic",
        r#"[[providers]]
name = "diagnostic-provider"
"#,
    );
    fixture.write_config(
        r#"diagnostics_model = "diagnostic"
"#,
    );
    let prompt_dump = fixture._dir.path().join("diagnostic-quota-prompt.txt");
    fixture.write_providers(&format!(
        r#"[claude-failure-provider]
command = 'bash -c "echo Claude usage limit reached for active provider >&2; exit 7"'
args = []
prompt_mode = "arg"

[diagnostic-provider]
command = {}
args = []
prompt_mode = "stdin"
"#,
        diagnostic_command(
            &prompt_dump,
            "quota_exhausted",
            "Diagnostic model saw exhausted quota"
        )
    ));

    let output = fixture
        .command()
        .arg("--models-dir")
        .arg(&fixture.models_dir)
        .arg("--model")
        .arg("failing")
        .arg("trigger quota failure")
        .env(
            "OULIPOLY_AGE153_FORCE_TERMINAL_SIGNAL_KIND",
            "QuotaExhaustedInband",
        )
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("OULIPOLY_TERMINAL_SIGNAL="), "{stderr}");
    assert!(
        stderr.contains("all providers in pool failing are quota-exhausted"),
        "{stderr}"
    );
    assert!(!stderr.contains("Empty command"), "{stderr}");
    assert!(
        !prompt_dump.exists(),
        "typed quota should not run diagnostics"
    );

    let db = fixture.open_db();
    let quota = db.get_quota("claude-failure-provider").unwrap().unwrap();
    // AGE-163 WU-A.4: durable working-set write moved from `exhausted_at`
    // to `next_available_at` via the typed forensics path.
    assert!(quota.next_available_at.is_some());
    let invocation = parse_invocation(&stderr);
    let row = db
        .get_invocation_by_uuid(&invocation.id)
        .unwrap()
        .expect("failed quota attempt should be recorded");
    assert_eq!(row.exit_code, Some(7));
    assert_eq!(row.error_category.as_deref(), Some("quota_exhausted"));
}

#[test]
fn resume_failure_runs_effective_diagnostics_and_preserves_finalization_order() {
    let fixture = Fixture::new();
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    fixture.write_model(
        "resumable",
        r#"[[providers]]
name = "resume-provider"
args = ["one-shot-only"]
"#,
    );
    fixture.write_model(
        "diagnostic",
        r#"[[providers]]
name = "diagnostic-provider"
"#,
    );
    fixture.write_config(
        r#"diagnostics_model = "diagnostic"
"#,
    );
    let prompt_dump = fixture._dir.path().join("resume-diagnostic-prompt.txt");
    fixture.write_providers(&format!(
        r#"[resume-provider]
command = {}
args = []
interactive_args = ["launch"]
prompt_mode = "arg"

[resume-provider.resume]
kind = "flag"
flag = "--resume"

[diagnostic-provider]
command = {}
args = []
prompt_mode = "stdin"
"#,
        toml_command(&fixture_script("resume-provider.sh").display().to_string()),
        diagnostic_command(&prompt_dump, "network_error", "resume diagnostic")
    ));
    fixture.seed_session_turns("resume-provider", session_id);

    let output = fixture
        .command()
        .arg("-m")
        .arg("resumable")
        .arg("--resume")
        .arg(session_id)
        .arg("--models-dir")
        .arg(&fixture.models_dir)
        .arg("continue after failure")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(7), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[diagnostics] network_error: resume diagnostic"),
        "{stderr}"
    );
    assert!(stderr.contains("[diagnostics: network_error]"), "{stderr}");
    assert!(!stderr.contains("Empty command"), "{stderr}");
    assert_ordered(
        &stderr,
        "[diagnostics] network_error",
        "resume child failure",
    );
    assert_ordered(
        &stderr,
        "resume child failure",
        "[diagnostics: network_error]",
    );
    let prompt = fs::read_to_string(prompt_dump).unwrap();
    assert!(prompt.contains("Exit code: 7"), "{prompt}");
    assert!(prompt.contains("resume child failure"), "{prompt}");

    let invocation = parse_invocation(&stderr);
    let db = fixture.open_db();
    let row = db.get_invocation_by_uuid(&invocation.id).unwrap().unwrap();
    assert_eq!(row.status, InvocationStatus::Failed);
    assert_eq!(row.exit_code, Some(7));
    assert_eq!(row.error_category.as_deref(), Some("network_error"));
    assert_eq!(row.session_capture_method.as_deref(), Some("resumed"));
    assert!(
        db.get_quota("resume-provider").unwrap().is_none(),
        "resume failure diagnostics must not create a one-shot quota marker"
    );
}
