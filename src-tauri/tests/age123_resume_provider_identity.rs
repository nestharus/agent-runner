#![cfg(unix)]

use oulipoly_state::{InvocationStatus, StateDb};
use rusqlite::{Connection, params};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

const CHAIN_ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const SESSION_A: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
const SESSION_B: &str = "6169694d-de0f-40d1-890c-6e28e55bab28";

struct ProviderFixture<'a> {
    name: &'a str,
    body: &'a str,
}

struct Fixture {
    dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
}

#[derive(Debug)]
struct InvocationIdentityRow {
    provider_name: String,
    provider_session_id: Option<String>,
    resume_input_id: Option<String>,
    provider_session_capture_method: Option<String>,
    provider_session_resolved_account: Option<String>,
    resume_acceptance_status: Option<String>,
    resume_acceptance_evidence: Option<String>,
    status: String,
    success: Option<bool>,
    exit_code: Option<i64>,
    error_category: Option<String>,
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

    fn conn(&self) -> Connection {
        let _ = self.open_db();
        Connection::open(self.db_path()).unwrap()
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

    fn write_resume_pool(&self, model_name: &str, providers: &[ProviderFixture<'_>]) {
        let mut model = String::new();
        for provider in providers {
            model.push_str(&format!(
                r#"[[providers]]
name = "{}"
args = ["exec-{}"]

"#,
                provider.name, provider.name
            ));
        }
        fs::write(self.models_dir.join(format!("{model_name}.toml")), model).unwrap();

        fs::write(
            self.models_dir.join("diagnostic.toml"),
            r#"[[providers]]
name = "diagnostic-provider"
"#,
        )
        .unwrap();
        fs::write(
            self.app_config_dir.join("config.toml"),
            r#"diagnostics_model = "diagnostic"
"#,
        )
        .unwrap();

        let mut providers_toml = String::new();
        for provider in providers {
            let command = self.write_script(&format!("{}-resume.sh", provider.name), provider.body);
            providers_toml.push_str(&format!(
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
                provider.name,
                toml_string(&command.display().to_string()),
                provider.name,
                provider.name,
                provider.name,
                toml_string(
                    &self
                        .provider_projects_dir(provider.name)
                        .display()
                        .to_string()
                )
            ));
        }

        let diagnostic_command = self.write_script(
            "diagnostic-provider.sh",
            "cat >/dev/null\nprintf '%s\\n' 'quota_exhausted' 'diagnosed quota exhaustion'",
        );
        providers_toml.push_str(&format!(
            r#"[diagnostic-provider]
command = {}
args = []
prompt_mode = "stdin"
"#,
            toml_string(&diagnostic_command.display().to_string())
        ));
        fs::write(self.app_config_dir.join("providers.toml"), providers_toml).unwrap();
    }

    fn provider_projects_dir(&self, provider: &str) -> PathBuf {
        self.dir.path().join(format!("{provider}-projects"))
    }

    fn stage_claude_jsonl(&self, provider: &str, session_id: &str) {
        let source_dir = self.provider_projects_dir(provider).join("source-project");
        fs::create_dir_all(&source_dir).unwrap();
        fs::write(
            source_dir.join(format!("{session_id}.jsonl")),
            format!(
                r#"{{"sessionId":"{session_id}","turnId":"turn-1","timestamp":"2026-04-17T08:00:00Z","type":"assistant"}}"#
            ),
        )
        .unwrap();
    }

    fn seed_rotated_chain(&self, source_provider: &str, target_provider: &str) {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, '2026-04-17T08:00:00Z', '2026-04-17T08:00:00Z', 'age123-resume')",
            params![CHAIN_ID],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, ended_at, transition_reason)
             VALUES (?1, ?2, ?3, '2026-04-17T08:00:00Z', '2026-04-17T09:00:00Z', 'initial')",
            params![CHAIN_ID, source_provider, SESSION_A],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, '2026-04-17T09:00:00Z', 'quota_threshold')",
            params![CHAIN_ID, target_provider, SESSION_B],
        )
        .unwrap();
    }

    fn seed_active_chain(&self, provider: &str, session_id: &str) {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, '2026-04-17T08:00:00Z', '2026-04-17T08:00:00Z', 'age123-resume')",
            params![CHAIN_ID],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, '2026-04-17T08:00:00Z', 'initial')",
            params![CHAIN_ID, provider, session_id],
        )
        .unwrap();
    }

    fn run_resume(&self, resume_input: &str) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("-m")
            .arg("age123-resume")
            .arg("--resume")
            .arg(resume_input)
            .arg("--models-dir")
            .arg(&self.models_dir)
            .arg("continue after rotation");
        self.run(cmd)
    }

    fn run_resume_with_migration(&self, resume_input: &str, target_provider: &str) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("-m")
            .arg("age123-resume")
            .arg("--resume")
            .arg(resume_input)
            .arg("--migrate")
            .arg(target_provider)
            .arg("--models-dir")
            .arg(&self.models_dir)
            .arg("continue after manual migration");
        self.run(cmd)
    }

    fn run(&self, mut cmd: Command) -> Output {
        cmd.current_dir(self.dir.path());
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd.output().unwrap()
    }

    fn latest_invocation(&self) -> InvocationIdentityRow {
        self.conn()
            .query_row(
                "SELECT provider_name, provider_session_id, resume_input_id,
                        provider_session_capture_method, provider_session_resolved_account,
                        resume_acceptance_status, resume_acceptance_evidence, status, success,
                        exit_code, error_category
                 FROM invocations
                 ORDER BY id DESC
                 LIMIT 1",
                [],
                map_identity_row,
            )
            .unwrap()
    }

    fn invocation_for_provider(&self, provider: &str) -> InvocationIdentityRow {
        self.conn()
            .query_row(
                "SELECT provider_name, provider_session_id, resume_input_id,
                        provider_session_capture_method, provider_session_resolved_account,
                        resume_acceptance_status, resume_acceptance_evidence, status, success,
                        exit_code, error_category
                 FROM invocations
                 WHERE provider_name = ?1
                 ORDER BY id DESC
                 LIMIT 1",
                params![provider],
                map_identity_row,
            )
            .unwrap()
    }
}

fn map_identity_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<InvocationIdentityRow> {
    Ok(InvocationIdentityRow {
        provider_name: row.get(0)?,
        provider_session_id: row.get(1)?,
        resume_input_id: row.get(2)?,
        provider_session_capture_method: row.get(3)?,
        provider_session_resolved_account: row.get(4)?,
        resume_acceptance_status: row.get(5)?,
        resume_acceptance_evidence: row.get(6)?,
        status: row.get(7)?,
        success: row.get(8)?,
        exit_code: row.get(9)?,
        error_category: row.get(10)?,
    })
}

fn toml_string(value: &str) -> String {
    format!("{value:?}")
}

fn identity_for(fixture: &Fixture, provider: &str) -> String {
    fixture
        .provider_projects_dir(provider)
        .display()
        .to_string()
}

#[test]
fn resume_session_mismatch_records_expected_segment_and_resolved_provider_identity() {
    let fixture = Fixture::new();
    fixture.write_resume_pool(
        "age123-resume",
        &[
            ProviderFixture {
                name: "claude-a",
                body: "printf '%s\\n' 'unexpected provider a launch' >&2\nexit 99",
            },
            ProviderFixture {
                name: "claude-b",
                body: "printf '%s\\n' 'No conversation found with session ID: 6169694d-de0f-40d1-890c-6e28e55bab28' >&2\nexit 7",
            },
        ],
    );
    fixture.seed_rotated_chain("claude-a", "claude-b");

    let output = fixture.run_resume(CHAIN_ID);

    assert_eq!(output.status.code(), Some(7), "{output:?}");
    let row = fixture.latest_invocation();
    assert_eq!(row.provider_name, "claude-b");
    assert_eq!(row.provider_session_id.as_deref(), Some(SESSION_B));
    assert_eq!(row.resume_input_id.as_deref(), Some(CHAIN_ID));
    assert_eq!(
        row.provider_session_capture_method.as_deref(),
        Some("resumed")
    );
    assert_eq!(
        row.provider_session_resolved_account,
        Some(identity_for(&fixture, "claude-b"))
    );
    assert_eq!(row.resume_acceptance_status.as_deref(), Some("rejected"));
    assert!(
        row.resume_acceptance_evidence
            .as_deref()
            .is_some_and(|evidence| evidence.contains("resume_session_mismatch")),
        "{row:?}"
    );
    assert_eq!(
        row.error_category.as_deref(),
        Some("resume_session_mismatch")
    );
}

#[test]
fn successful_resume_records_resolved_provider_identity() {
    let fixture = Fixture::new();
    fixture.write_resume_pool(
        "age123-resume",
        &[
            ProviderFixture {
                name: "claude-a",
                body: "printf '%s\\n' 'unexpected provider a launch' >&2\nexit 99",
            },
            ProviderFixture {
                name: "claude-b",
                body: "printf '%s\\n' 'resume accepted'\nexit 0",
            },
        ],
    );
    fixture.seed_rotated_chain("claude-a", "claude-b");

    let output = fixture.run_resume(CHAIN_ID);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let row = fixture.latest_invocation();
    assert_eq!(row.provider_name, "claude-b");
    assert_eq!(row.provider_session_id.as_deref(), Some(SESSION_B));
    assert_eq!(
        row.provider_session_resolved_account,
        Some(identity_for(&fixture, "claude-b"))
    );
    assert_eq!(row.status, InvocationStatus::Succeeded.as_str());
    assert_eq!(row.success, Some(true));
    assert_eq!(row.exit_code, Some(0));
    assert_eq!(row.resume_acceptance_status.as_deref(), Some("unconfirmed"));
}

#[test]
fn quota_retry_records_resolved_provider_identity_per_attempt() {
    let fixture = Fixture::new();
    fixture.write_resume_pool(
        "age123-resume",
        &[
            ProviderFixture {
                name: "claude-a",
                body: "printf '%s\\n' 'quota exhausted for active resume provider' >&2\nexit 42",
            },
            ProviderFixture {
                name: "claude-b",
                body: "printf '%s\\n' 'retried resume accepted'\nexit 0",
            },
        ],
    );
    fixture.stage_claude_jsonl("claude-a", SESSION_A);
    fixture.seed_active_chain("claude-a", SESSION_A);

    let output = fixture.run_resume(CHAIN_ID);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let first = fixture.invocation_for_provider("claude-a");
    let second = fixture.invocation_for_provider("claude-b");
    assert_eq!(
        first.provider_session_resolved_account,
        Some(identity_for(&fixture, "claude-a"))
    );
    assert_eq!(
        second.provider_session_resolved_account,
        Some(identity_for(&fixture, "claude-b"))
    );
    assert_eq!(first.error_category.as_deref(), Some("quota_exhausted"));
    assert_eq!(first.status, InvocationStatus::Failed.as_str());
    assert_eq!(second.status, InvocationStatus::Succeeded.as_str());
    assert_eq!(second.success, Some(true));
}

#[test]
fn manual_migration_records_target_resolved_provider_identity() {
    let fixture = Fixture::new();
    fixture.write_resume_pool(
        "age123-resume",
        &[
            ProviderFixture {
                name: "claude-a",
                body: "printf '%s\\n' 'unexpected source launch' >&2\nexit 99",
            },
            ProviderFixture {
                name: "claude-b",
                body: "printf '%s\\n' 'manual migration accepted'\nexit 0",
            },
        ],
    );
    fixture.stage_claude_jsonl("claude-a", SESSION_A);
    fixture.seed_active_chain("claude-a", SESSION_A);

    let output = fixture.run_resume_with_migration(CHAIN_ID, "claude-b");

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let row = fixture.latest_invocation();
    assert_eq!(row.provider_name, "claude-b");
    assert_eq!(row.resume_input_id.as_deref(), Some(CHAIN_ID));
    assert_eq!(row.provider_session_id.as_deref(), Some(SESSION_A));
    assert_eq!(
        row.provider_session_resolved_account,
        Some(identity_for(&fixture, "claude-b"))
    );
}
