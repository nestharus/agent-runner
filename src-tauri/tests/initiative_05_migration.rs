#![cfg(unix)]

use chrono::{DateTime, Utc};
use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, ResumeKind, ResumeStrategy, SessionSourceEntry,
    SessionStorage, SessionsConfig,
};
use oulipoly_runtime::balancer::TransitionReason;
use oulipoly_runtime::migration::{MigrationError, migrate_chain_segment};
use oulipoly_runtime::sessions::scan_provider;
use oulipoly_state::{ResolvedResume, SessionTurnIngest, StateDb};
use rusqlite::{Connection, params};
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SESSION_A: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
const CHAIN_A: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const CHAIN_B: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";

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
        let models_dir = config_home.join("oulipoly-agent-runner").join("models");
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

    fn write_turn_emitter(&self, name: &str, session_id: &str, turn_id: &str) -> PathBuf {
        self.write_script(name, &static_turn_stdout(session_id, turn_id))
    }

    fn write_session_appending_provider(
        &self,
        name: &str,
        transcript_path: &Path,
        argv_dump: Option<&Path>,
    ) -> PathBuf {
        self.write_script(
            name,
            &session_appending_provider_body(transcript_path, argv_dump),
        )
    }

    fn write_model(&self, model_name: &str, body: &str) {
        fs::write(self.models_dir.join(format!("{model_name}.toml")), body).unwrap();
    }

    fn write_providers_config(&self, providers: &[&str]) {
        let app_dir = self.config_home.join("oulipoly-agent-runner");
        fs::create_dir_all(&app_dir).unwrap();
        let mut body = String::new();
        for provider in providers {
            body.push_str(&format!("[{provider}]\n"));
            body.push('\n');
        }
        fs::write(app_dir.join("providers.toml"), body).unwrap();
    }

    fn write_runtime_provider(&self, provider: &str, command: &Path) {
        self.write_runtime_provider_with_storage(provider, command, None);
    }

    fn write_runtime_provider_with_storage(
        &self,
        provider: &str,
        command: &Path,
        storage: Option<(&str, &Path)>,
    ) {
        let app_dir = self.config_home.join("oulipoly-agent-runner");
        fs::create_dir_all(&app_dir).unwrap();
        let storage = storage
            .map(|(kind, path)| match kind {
                "claude_code" => format!(
                    r#"
[{provider}.session_storage]
kind = "claude_code"
projects_dir = "{}"
"#,
                    path.display()
                ),
                "codex" => format!(
                    r#"
[{provider}.session_storage]
kind = "codex"
sessions_dir = "{}"
"#,
                    path.display()
                ),
                other => panic!("unknown storage kind {other}"),
            })
            .unwrap_or_default();
        let body = format!(
            r#"
[{provider}]
command = "{}"
args = ["-p"]
interactive_args = ["launch"]
prompt_mode = "arg"

[{provider}.resume]
kind = "flag"
flag = "--resume"
{storage}
"#,
            command.display()
        );
        fs::write(app_dir.join("providers.toml"), body).unwrap();
    }

    fn write_sessions_config(&self, provider: &str, script: &Path) -> SessionsConfig {
        self.write_sessions_entry(provider, &script.to_string_lossy())
    }

    fn write_sessions_config_from_transcript(
        &self,
        provider: &str,
        transcript_path: &Path,
    ) -> SessionsConfig {
        self.write_sessions_entry(provider, &format!("cat {}", sh_path(transcript_path)))
    }

    fn write_sessions_entry(&self, provider: &str, turn_script: &str) -> SessionsConfig {
        let app_dir = self.config_home.join("oulipoly-agent-runner");
        fs::create_dir_all(&app_dir).unwrap();
        let state_dir = self.dir.path().join("session-state");
        fs::write(
            app_dir.join("sessions.toml"),
            format!(
                "[{provider}]\nturn_script = {:?}\nstate_dir = {:?}\n",
                turn_script,
                state_dir.to_string_lossy()
            ),
        )
        .unwrap();
        let mut entries = HashMap::new();
        entries.insert(
            provider.to_string(),
            SessionSourceEntry {
                turn_script: turn_script.to_string(),
                transcript_locator: None,
                state_dir: Some(state_dir),
            },
        );
        SessionsConfig { entries }
    }

    fn sessions_config_with_locator(&self, provider: &str, locator: &Path) -> SessionsConfig {
        let mut entries = HashMap::new();
        entries.insert(
            provider.to_string(),
            SessionSourceEntry {
                turn_script: "true".to_string(),
                transcript_locator: Some(locator.to_string_lossy().into_owned()),
                state_dir: Some(self.dir.path().join(format!("{provider}-state"))),
            },
        );
        SessionsConfig { entries }
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd
    }

    fn stage_claude_jsonl(&self, projects_dir: &Path, session_id: &str) -> PathBuf {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("jsonl")
            .join("claude")
            .join("full_session.jsonl");
        let cwd_dir = projects_dir.join("cwd-hash-fixture");
        fs::create_dir_all(&cwd_dir).unwrap();
        let target = cwd_dir.join(format!("{session_id}.jsonl"));
        fs::copy(source, &target).unwrap();
        target
    }

    fn seed_active_chain(&self, chain_id: &str, provider: &str, session_id: &str, model: &str) {
        let _ = self.open_db();
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

    fn seed_turns(&self, provider: &str, session_id: &str, boundaries: &[&str]) {
        let db = self.open_db();
        let turns: Vec<_> = (1..=10)
            .map(|i| {
                let turn_id = format!("turn-{i}");
                SessionTurnIngest {
                    session_id: session_id.to_string(),
                    turn_id: turn_id.clone(),
                    timestamp: ts(&format!("2026-04-17T08:00:0{}Z", i - 1)),
                    role: if i % 2 == 0 { "assistant" } else { "user" }.to_string(),
                    parent_turn_id: None,
                    is_sidechain: false,
                    is_compaction_boundary: boundaries.contains(&turn_id.as_str()),
                    body: None,
                }
            })
            .collect();
        db.ingest_session_turns_batch(provider, &turns).unwrap();
    }

    fn seed_missing_compaction_boundary(&self, provider: &str, session_id: &str) {
        self.conn()
            .execute(
                "INSERT INTO session_turns
                    (provider_name, session_id, turn_id, timestamp, role, source_file, ingested_at, is_compaction_boundary)
                 VALUES (?1, ?2, 'missing-turn', '2026-04-17T08:00:10Z', 'assistant', '', '2026-04-17T08:00:10Z', 1)",
                params![provider, session_id],
            )
            .unwrap();
    }

    fn migration_model(&self, source_projects: &Path, target_projects: &Path) -> ModelConfig {
        ModelConfig {
            name: "claude-opus".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![
                runtime_provider(
                    "claude",
                    SessionStorage::ClaudeCode {
                        projects_dir: source_projects.to_path_buf(),
                    },
                ),
                runtime_provider(
                    "claude2",
                    SessionStorage::ClaudeCode {
                        projects_dir: target_projects.to_path_buf(),
                    },
                ),
            ],
            inputs: Vec::new(),
            provider: None,
        }
    }

    fn codex_source_model(&self, codex_sessions: &Path, target_projects: &Path) -> ModelConfig {
        let mut codex = runtime_provider(
            "codex",
            SessionStorage::Codex {
                sessions_dir: codex_sessions.to_path_buf(),
            },
        );
        codex.resume = Some(ResumeStrategy {
            kind: ResumeKind::Subcommand,
            flag: None,
            subcommand: Some(vec!["resume".to_string()]),
        });
        ModelConfig {
            name: "codex-high".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![
                codex,
                runtime_provider(
                    "claude",
                    SessionStorage::ClaudeCode {
                        projects_dir: target_projects.to_path_buf(),
                    },
                ),
            ],
            inputs: Vec::new(),
            provider: None,
        }
    }

    fn resolved(&self, model: &ModelConfig, provider_index: usize) -> ResolvedResume {
        let provider = &model.providers[provider_index];
        ResolvedResume {
            chain_id: CHAIN_A.to_string(),
            model_name: Some(model.name.clone()),
            model: Some(model.clone()),
            active_provider: provider.name.clone(),
            active_session_id: SESSION_A.to_string(),
        }
    }
}

fn sh_path(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

fn static_turn_stdout(session_id: &str, turn_id: &str) -> String {
    format!(
        r#"printf '{{"session_id":"{session_id}","turn_id":"{turn_id}","timestamp":"2026-04-17T08:00:00Z","role":"assistant"}}\n'"#
    )
}

fn session_appending_provider_body(transcript_path: &Path, argv_dump: Option<&Path>) -> String {
    let argv_dump = argv_dump
        .map(|path| format!("printf '%s\n' \"$@\" > {}\n", sh_path(path)))
        .unwrap_or_default();
    format!(
        r#"{argv_dump}ts="$(date -u +%Y-%m-%dT%H:%M:%S.%NZ)"
turn_id="t-$$-$RANDOM"
printf '{{"session_id":"{SESSION_A}","turn_id":"%s","timestamp":"%s","role":"assistant"}}\n' "$turn_id" "$ts" >> {}
printf 'ok\n'"#,
        sh_path(transcript_path)
    )
}

fn argv_dump_provider_body(argv_dump: &Path) -> String {
    format!(
        r#"printf '%s\n' "$@" > {}
printf 'ok\n'"#,
        sh_path(argv_dump)
    )
}

fn ok_provider_body() -> &'static str {
    "printf 'ok\n'"
}

fn runtime_provider(name: &str, session_storage: SessionStorage) -> ProviderConfig {
    ProviderConfig {
        name: name.to_string(),
        command: name.to_string(),
        args: Vec::new(),
        interactive_args: Some(vec!["launch".to_string()]),
        resume: Some(ResumeStrategy {
            kind: ResumeKind::Flag,
            flag: Some("--resume".to_string()),
            subcommand: None,
        }),
        session_capture: None,
        resume_acceptance: None,
        session_storage: Some(session_storage),
        system_prompt_override: None,
        tool_restrictions: None,
        invocation_mode: Default::default(),
    }
}

fn single_resume_provider_model_toml(command: &Path) -> String {
    let _ = command;
    r#"
[[providers]]
name = "claude"
"#
    .to_string()
}

fn resume_provider_with_model_flags_toml(command: &Path) -> String {
    let _ = command;
    r#"
[[providers]]
name = "claude"
args = ["-p", "--model", "opus"]
interactive_args = ["-m", "opus"]
"#
    .to_string()
}

fn manual_migrate_cli_model_toml(
    command: &Path,
    source_projects: &Path,
    target_projects: &Path,
) -> String {
    let _ = (command, source_projects, target_projects);
    r#"
[[providers]]
name = "claude"

[[providers]]
name = "claude2"
"#
    .to_string()
}

fn missing_source_locator_body(missing: &Path) -> String {
    format!(r#"printf '%s\n' "{}""#, missing.display())
}

fn malformed_source_locator_body(path: &Path) -> String {
    format!(r#"printf '%s\n' "{}""#, path.display())
}

fn ts(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}

fn fixture_lines(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

fn segment_count(fixture: &Fixture, provider: &str) -> i64 {
    fixture
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM session_chain_segments WHERE provider_name = ?1",
            params![provider],
            |row| row.get(0),
        )
        .unwrap()
}

fn chain_model_name(fixture: &Fixture, session_id: &str) -> String {
    fixture
        .conn()
        .query_row(
            "SELECT c.model_name
             FROM session_chains c
             JOIN session_chain_segments s ON s.chain_id = c.chain_id
             WHERE s.session_id = ?1",
            params![session_id],
            |row| row.get(0),
        )
        .unwrap()
}

fn parse_session(stderr: &str) -> String {
    let line = stderr
        .lines()
        .find(|line| line.starts_with("OULIPOLY_SESSION="))
        .unwrap_or_else(|| panic!("OULIPOLY_SESSION line in stderr: {stderr}"));
    let value: serde_json::Value =
        serde_json::from_str(line.strip_prefix("OULIPOLY_SESSION=").unwrap()).unwrap();
    value["session_id"].as_str().unwrap().to_string()
}

fn assert_success(output: &Output) {
    assert_eq!(output.status.code(), Some(0), "{output:?}");
}

fn data_home_file_fixture(fixture: &Fixture) -> PathBuf {
    let path = fixture.dir.path().join("data-home-file");
    fs::write(&path, "not a directory").unwrap();
    path
}

fn run_session_capture_chain_fixture() -> Fixture {
    let fixture = Fixture::new();
    let transcript_path = fixture.dir.path().join("turns.jsonl");
    fs::write(&transcript_path, "").unwrap();
    let script =
        fixture.write_session_appending_provider("session-writer.sh", &transcript_path, None);
    fixture.write_model(
        "claude-opus",
        r#"
[[providers]]
name = "claude"
"#,
    );
    fixture.write_runtime_provider("claude", &script);
    fixture.write_sessions_config_from_transcript("claude", &transcript_path);

    let output = fixture
        .command()
        .args(["--models-dir"])
        .arg(&fixture.models_dir)
        .args(["--model", "claude-opus", "start"])
        .output()
        .unwrap();

    assert_success(&output);
    fixture
}

// risk: Chain identity write paths; level: particular-integration; source: proposal §11.1 Chain identity write paths / A3, A8.
#[test]
fn mint_chain_on_first_session_capture() {
    let fixture = run_session_capture_chain_fixture();

    assert_eq!(chain_model_name(&fixture, SESSION_A), "claude-opus");
    assert_eq!(segment_count(&fixture, "claude"), 1);
}

// risk: Chain identity write paths; level: particular-integration; source: proposal §11.1 Chain identity write paths / A3, A8.
#[test]
fn agent_session_chain_records_model_at_mint() {
    let fixture = run_session_capture_chain_fixture();

    assert_eq!(chain_model_name(&fixture, SESSION_A), "claude-opus");
}

// risk: Chain identity write paths; level: particular-integration; source: proposal §11.1 Chain identity write paths / A3, A8.
#[test]
fn ui_session_chain_minted_with_unknown() {
    let fixture = Fixture::new();
    let db = fixture.open_db();
    let script = fixture.write_turn_emitter("turns.sh", SESSION_A, "ui-1");
    let sessions = fixture.write_sessions_config("claude", &script);
    fixture.write_providers_config(&["claude"]);

    let result = scan_provider("claude", &sessions, &db);

    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!(chain_model_name(&fixture, SESSION_A), "<unknown>");
}

// risk: Chain identity write paths; level: particular-integration; source: proposal §11.1 Chain identity write paths / A7.
#[test]
fn chain_mint_works_for_codex_ingestion() {
    let fixture = Fixture::new();
    let db = fixture.open_db();
    let script = fixture.write_turn_emitter("codex-turns.sh", SESSION_A, "codex-1");
    let sessions = fixture.write_sessions_config("codex", &script);
    fixture.write_providers_config(&["codex"]);

    let result = scan_provider("codex", &sessions, &db);

    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!(chain_model_name(&fixture, SESSION_A), "<unknown>");
    assert_eq!(segment_count(&fixture, "codex"), 1);
}

// risk: Resolver disambiguation and model inference; level: end-to-end; source: proposal §11.1 Resolver disambiguation and model inference / A8.
#[test]
fn agent_resume_no_dash_m_uses_session_recorded_model() {
    let fixture = Fixture::new();
    let argv_dump = fixture.dir.path().join("argv.txt");
    let transcript_path = fixture.dir.path().join("turns.jsonl");
    fs::write(&transcript_path, "").unwrap();
    let script = fixture.write_session_appending_provider(
        "resume-provider.sh",
        &transcript_path,
        Some(&argv_dump),
    );
    fixture.write_model("claude-opus", &single_resume_provider_model_toml(&script));
    fixture.write_runtime_provider("claude", &script);
    fixture.write_sessions_config_from_transcript("claude", &transcript_path);
    let initial = fixture
        .command()
        .args(["--models-dir"])
        .arg(&fixture.models_dir)
        .args(["--model", "claude-opus", "start"])
        .output()
        .unwrap();
    assert_success(&initial);
    let session_id = parse_session(&String::from_utf8_lossy(&initial.stderr));

    let resumed = fixture
        .command()
        .arg("--resume")
        .arg(&session_id)
        .args(["--models-dir"])
        .arg(&fixture.models_dir)
        .arg("continue")
        .output()
        .unwrap();

    assert_success(&resumed);
    assert!(fs::read_to_string(argv_dump).unwrap().contains("--resume"));
}

fn migration_fixture() -> (
    Fixture,
    ModelConfig,
    SessionsConfig,
    PathBuf,
    PathBuf,
    PathBuf,
) {
    let fixture = Fixture::new();
    let source_projects = fixture.dir.path().join("source-projects");
    let target_projects = fixture.dir.path().join("target-projects");
    let source_jsonl = fixture.stage_claude_jsonl(&source_projects, SESSION_A);
    let locator = fixture.write_script(
        "locator.sh",
        &format!(r#"printf '%s\n' "{}""#, source_jsonl.display()),
    );
    let sessions = fixture.sessions_config_with_locator("claude", &locator);
    let model = fixture.migration_model(&source_projects, &target_projects);
    fixture.seed_active_chain(CHAIN_A, "claude", SESSION_A, "claude-opus");
    (
        fixture,
        model,
        sessions,
        source_projects,
        target_projects,
        source_jsonl,
    )
}

fn claude_project_dir_name(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| match c {
            '/' | '\\' => '-',
            c if (c.is_ascii() && c.is_alphanumeric()) || c == '-' => c,
            _ => '-',
        })
        .collect()
}

// risk: Migration mechanic: Claude JSONL copy, Codex deferred guard, segment ledger, and races; level: particular-integration; source: proposal §11.1 Migration mechanic / A1, A3.
#[test]
fn migration_copies_claude_jsonl_to_target_projects_dir() {
    let (fixture, model, sessions, _source_projects, target_projects, source_jsonl) =
        migration_fixture();
    let resume_working_dir = fixture.dir.path().join("resume-workspace");
    fs::create_dir_all(&resume_working_dir).unwrap();
    fixture.seed_turns("claude", SESSION_A, &[]);
    let db = fixture.open_db();
    let mut stderr = Vec::new();

    let migrated = migrate_chain_segment(
        &db,
        &sessions,
        &model,
        &fixture.resolved(&model, 0),
        &resume_working_dir,
        1,
        TransitionReason::Manual,
        &mut stderr,
    )
    .unwrap();

    assert!(migrated.target_jsonl_path.starts_with(&target_projects));
    assert!(migrated.target_jsonl_path.exists());
    assert_eq!(
        fixture_lines(&migrated.target_jsonl_path),
        fixture_lines(&source_jsonl)
    );
}

// risk: Migration mechanic: same-chain provider rejoin overwrites stale target transcript; level: particular-integration; source: live QA TargetAlreadyExists regression.
#[test]
fn migration_overwrites_target_when_same_chain_revisits_provider() {
    let fixture = Fixture::new();
    let source_projects = fixture.dir.path().join("source-projects");
    let target_projects = fixture.dir.path().join("target-projects");
    let resume_working_dir = fixture.dir.path().join("resume-workspace");
    let cwd_hash = "cwd-hash-fixture";
    let stale_target = source_projects
        .join(claude_project_dir_name(&resume_working_dir))
        .join(format!("{SESSION_A}.jsonl"));
    let current_source = target_projects
        .join(cwd_hash)
        .join(format!("{SESSION_A}.jsonl"));
    fs::create_dir_all(stale_target.parent().unwrap()).unwrap();
    fs::create_dir_all(current_source.parent().unwrap()).unwrap();
    fs::write(&stale_target, "{\"turn\":\"stale\"}\n").unwrap();
    fs::write(
        &current_source,
        "{\"turn\":\"current-1\"}\n{\"turn\":\"current-2\"}\n",
    )
    .unwrap();
    let locator = fixture.write_script(
        "claude2-locator.sh",
        &format!(r#"printf '%s\n' "{}""#, current_source.display()),
    );
    let sessions = fixture.sessions_config_with_locator("claude2", &locator);
    let model = fixture.migration_model(&source_projects, &target_projects);
    let _ = fixture.open_db();
    let conn = fixture.conn();
    conn.execute(
        "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
         VALUES (?1, '2026-04-17T08:00:00Z', '2026-04-17T08:10:00Z', 'claude-opus')",
        params![CHAIN_A],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session_chain_segments
            (chain_id, provider_name, session_id, started_at, ended_at, transition_reason)
         VALUES (?1, 'claude', ?2, '2026-04-17T08:00:00Z', '2026-04-17T08:05:00Z', 'initial')",
        params![CHAIN_A, SESSION_A],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session_chain_segments
            (chain_id, provider_name, session_id, started_at, transition_reason)
         VALUES (?1, 'claude2', ?2, '2026-04-17T08:05:00Z', 'quota_threshold')",
        params![CHAIN_A, SESSION_A],
    )
    .unwrap();
    drop(conn);
    let db = fixture.open_db();
    let resolved = ResolvedResume {
        chain_id: CHAIN_A.to_string(),
        model_name: Some(model.name.clone()),
        model: Some(model.clone()),
        active_provider: "claude2".to_string(),
        active_session_id: SESSION_A.to_string(),
    };
    let mut stderr = Vec::new();

    let migrated = migrate_chain_segment(
        &db,
        &sessions,
        &model,
        &resolved,
        &resume_working_dir,
        0,
        TransitionReason::Manual,
        &mut stderr,
    )
    .unwrap();

    assert_eq!(migrated.target_jsonl_path, stale_target);
    assert_eq!(
        fixture_lines(&migrated.target_jsonl_path),
        fixture_lines(&current_source)
    );
    let active_provider: String = fixture
        .conn()
        .query_row(
            "SELECT provider_name FROM session_chain_segments WHERE chain_id = ?1 AND ended_at IS NULL",
            params![CHAIN_A],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(active_provider, "claude");
}

// risk: Migration mechanic: target session UUID conflict detection; level: particular-integration; source: live QA TargetAlreadyExists regression / Q5.
#[test]
fn migration_refuses_when_other_chain_owns_target_session() {
    let fixture = Fixture::new();
    let source_projects = fixture.dir.path().join("source-projects");
    let target_projects = fixture.dir.path().join("target-projects");
    let resume_working_dir = fixture.dir.path().join("resume-workspace");
    let current_source = target_projects
        .join("cwd-hash-fixture")
        .join(format!("{SESSION_A}.jsonl"));
    fs::create_dir_all(current_source.parent().unwrap()).unwrap();
    fs::write(&current_source, "{\"turn\":\"current\"}\n").unwrap();
    let locator = fixture.write_script(
        "conflict-locator.sh",
        &format!(r#"printf '%s\n' "{}""#, current_source.display()),
    );
    let sessions = fixture.sessions_config_with_locator("claude2", &locator);
    let model = fixture.migration_model(&source_projects, &target_projects);
    let _ = fixture.open_db();
    let conn = fixture.conn();
    for chain_id in [CHAIN_A, CHAIN_B] {
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, '2026-04-17T08:00:00Z', '2026-04-17T08:00:00Z', 'claude-opus')",
            params![chain_id],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO session_chain_segments
            (chain_id, provider_name, session_id, started_at, transition_reason)
         VALUES (?1, 'claude2', ?2, '2026-04-17T08:05:00Z', 'initial')",
        params![CHAIN_A, SESSION_A],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session_chain_segments
            (chain_id, provider_name, session_id, started_at, transition_reason)
         VALUES (?1, 'claude', ?2, '2026-04-17T08:05:00Z', 'initial')",
        params![CHAIN_B, SESSION_A],
    )
    .unwrap();
    drop(conn);
    let db = fixture.open_db();
    let resolved = ResolvedResume {
        chain_id: CHAIN_A.to_string(),
        model_name: Some(model.name.clone()),
        model: Some(model.clone()),
        active_provider: "claude2".to_string(),
        active_session_id: SESSION_A.to_string(),
    };
    let mut stderr = Vec::new();

    let err = migrate_chain_segment(
        &db,
        &sessions,
        &model,
        &resolved,
        &resume_working_dir,
        0,
        TransitionReason::Manual,
        &mut stderr,
    )
    .unwrap_err();

    assert!(matches!(
        err,
        MigrationError::TargetSessionInUseByOtherChain {
            provider,
            session_id,
            conflicting_chain_id
        } if provider == "claude"
            && session_id == SESSION_A
            && conflicting_chain_id == CHAIN_B
    ));
}

// risk: Migration mechanic: closed segments do not block target UUID overwrite; level: particular-integration; source: live QA TargetAlreadyExists regression / Q5.
#[test]
fn migration_overwrites_when_other_chain_segment_is_closed() {
    let fixture = Fixture::new();
    let source_projects = fixture.dir.path().join("source-projects");
    let target_projects = fixture.dir.path().join("target-projects");
    let resume_working_dir = fixture.dir.path().join("resume-workspace");
    let cwd_hash = "cwd-hash-fixture";
    let target_path = source_projects
        .join(claude_project_dir_name(&resume_working_dir))
        .join(format!("{SESSION_A}.jsonl"));
    let current_source = target_projects
        .join(cwd_hash)
        .join(format!("{SESSION_A}.jsonl"));
    fs::create_dir_all(target_path.parent().unwrap()).unwrap();
    fs::create_dir_all(current_source.parent().unwrap()).unwrap();
    fs::write(&target_path, "{\"turn\":\"closed-chain-stale\"}\n").unwrap();
    fs::write(&current_source, "{\"turn\":\"current\"}\n").unwrap();
    let locator = fixture.write_script(
        "closed-conflict-locator.sh",
        &format!(r#"printf '%s\n' "{}""#, current_source.display()),
    );
    let sessions = fixture.sessions_config_with_locator("claude2", &locator);
    let model = fixture.migration_model(&source_projects, &target_projects);
    let _ = fixture.open_db();
    let conn = fixture.conn();
    for chain_id in [CHAIN_A, CHAIN_B] {
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, '2026-04-17T08:00:00Z', '2026-04-17T08:00:00Z', 'claude-opus')",
            params![chain_id],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO session_chain_segments
            (chain_id, provider_name, session_id, started_at, transition_reason)
         VALUES (?1, 'claude2', ?2, '2026-04-17T08:05:00Z', 'initial')",
        params![CHAIN_A, SESSION_A],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session_chain_segments
            (chain_id, provider_name, session_id, started_at, ended_at, transition_reason)
         VALUES (?1, 'claude', ?2, '2026-04-17T08:00:00Z', '2026-04-17T08:04:00Z', 'initial')",
        params![CHAIN_B, SESSION_A],
    )
    .unwrap();
    drop(conn);
    let db = fixture.open_db();
    let resolved = ResolvedResume {
        chain_id: CHAIN_A.to_string(),
        model_name: Some(model.name.clone()),
        model: Some(model.clone()),
        active_provider: "claude2".to_string(),
        active_session_id: SESSION_A.to_string(),
    };
    let mut stderr = Vec::new();

    let migrated = migrate_chain_segment(
        &db,
        &sessions,
        &model,
        &resolved,
        &resume_working_dir,
        0,
        TransitionReason::Manual,
        &mut stderr,
    )
    .unwrap();

    assert_eq!(migrated.target_jsonl_path, target_path);
    assert_eq!(
        fixture_lines(&migrated.target_jsonl_path),
        fixture_lines(&current_source)
    );
}

// risk: Migration mechanic: Claude JSONL copy, Codex deferred guard, segment ledger, and races; level: particular-integration; source: proposal §11.1 Migration mechanic / A1, A3.
#[test]
fn migration_appends_chain_segment_with_correct_reason() {
    let (fixture, model, sessions, _source_projects, _target_projects, _source_jsonl) =
        migration_fixture();
    let resume_working_dir = fixture.dir.path().join("resume-workspace");
    fixture.seed_turns("claude", SESSION_A, &[]);
    let db = fixture.open_db();
    let mut stderr = Vec::new();

    let migrated = migrate_chain_segment(
        &db,
        &sessions,
        &model,
        &fixture.resolved(&model, 0),
        &resume_working_dir,
        1,
        TransitionReason::QuotaThreshold,
        &mut stderr,
    )
    .unwrap();

    let conn = fixture.conn();
    let closed: (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT ended_at, last_turn_id FROM session_chain_segments WHERE chain_id = ?1 AND provider_name = 'claude'",
            params![CHAIN_A],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!(closed.0.is_some());
    assert_eq!(closed.1.as_deref(), Some("turn-10"));
    let reason: String = conn
        .query_row(
            "SELECT transition_reason FROM session_chain_segments WHERE chain_id = ?1 AND provider_name = ?2 AND ended_at IS NULL",
            params![CHAIN_A, migrated.target_provider],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(reason, "quota_threshold");
}

// risk: Migration mechanic: Claude JSONL copy, Codex deferred guard, segment ledger, and races; level: particular-integration; source: proposal §11.1 Migration mechanic / A1.
#[test]
fn migration_errors_on_source_jsonl_missing() {
    let fixture = Fixture::new();
    let source_projects = fixture.dir.path().join("source-projects");
    let target_projects = fixture.dir.path().join("target-projects");
    let resume_working_dir = fixture.dir.path().join("resume-workspace");
    let missing = source_projects
        .join("cwd")
        .join(format!("{SESSION_A}.jsonl"));
    let locator =
        fixture.write_script("missing-locator.sh", &missing_source_locator_body(&missing));
    let sessions = fixture.sessions_config_with_locator("claude", &locator);
    let model = fixture.migration_model(&source_projects, &target_projects);
    fixture.seed_active_chain(CHAIN_A, "claude", SESSION_A, "claude-opus");
    let db = fixture.open_db();
    let mut stderr = Vec::new();

    let err = migrate_chain_segment(
        &db,
        &sessions,
        &model,
        &fixture.resolved(&model, 0),
        &resume_working_dir,
        1,
        TransitionReason::Manual,
        &mut stderr,
    )
    .unwrap_err();

    assert!(
        matches!(err, MigrationError::SourceMissing { provider, session_id } if provider == "claude" && session_id == SESSION_A)
    );
}

// risk: Migration mechanic: Claude JSONL copy, Codex deferred guard, segment ledger, and races; level: particular-integration; source: proposal §11.1 Migration mechanic / A1.
#[test]
fn migration_errors_on_source_path_malformed() {
    let fixture = Fixture::new();
    let source_projects = fixture.dir.path().join("source-projects");
    let target_projects = fixture.dir.path().join("target-projects");
    let resume_working_dir = fixture.dir.path().join("resume-workspace");
    let bare = PathBuf::from("bare-session.jsonl");
    let locator = fixture.write_script("bare-locator.sh", &malformed_source_locator_body(&bare));
    let sessions = fixture.sessions_config_with_locator("claude", &locator);
    let model = fixture.migration_model(&source_projects, &target_projects);
    fixture.seed_active_chain(CHAIN_A, "claude", SESSION_A, "claude-opus");
    let db = fixture.open_db();
    let mut stderr = Vec::new();

    let err = migrate_chain_segment(
        &db,
        &sessions,
        &model,
        &fixture.resolved(&model, 0),
        &resume_working_dir,
        1,
        TransitionReason::Manual,
        &mut stderr,
    )
    .unwrap_err();
    assert!(
        matches!(err, MigrationError::SourcePathMalformed { provider, .. } if provider == "claude")
    );
}

// risk: Migration mechanic: compaction-aware Claude target build; level: particular-integration; source: proposal §11.1 Migration mechanic: compaction-aware Claude target build / A3, A6.
#[test]
fn migration_truncates_target_jsonl_at_latest_compaction_boundary() {
    let (fixture, model, sessions, _source_projects, _target_projects, _source_jsonl) =
        migration_fixture();
    let resume_working_dir = fixture.dir.path().join("resume-workspace");
    fixture.seed_turns("claude", SESSION_A, &["turn-6"]);
    let db = fixture.open_db();
    let mut stderr = Vec::new();

    let migrated = migrate_chain_segment(
        &db,
        &sessions,
        &model,
        &fixture.resolved(&model, 0),
        &resume_working_dir,
        1,
        TransitionReason::Manual,
        &mut stderr,
    )
    .unwrap();
    let target = fixture_lines(&migrated.target_jsonl_path);

    assert_eq!(target.len(), 5);
    assert!(target.first().unwrap().contains(r#""turn_id":"turn-6""#));
    assert!(
        target
            .iter()
            .all(|line| !line.contains(r#""turn_id":"turn-5""#))
    );
}

// risk: Migration mechanic: compaction-aware Claude target build; level: particular-integration; source: proposal §11.1 Migration mechanic: compaction-aware Claude target build / A3, A6.
#[test]
fn migration_copies_full_jsonl_when_no_compaction_boundary() {
    let (fixture, model, sessions, _source_projects, _target_projects, source_jsonl) =
        migration_fixture();
    let resume_working_dir = fixture.dir.path().join("resume-workspace");
    fixture.seed_turns("claude", SESSION_A, &[]);
    let db = fixture.open_db();
    let mut stderr = Vec::new();

    let migrated = migrate_chain_segment(
        &db,
        &sessions,
        &model,
        &fixture.resolved(&model, 0),
        &resume_working_dir,
        1,
        TransitionReason::Manual,
        &mut stderr,
    )
    .unwrap();

    assert_eq!(
        fixture_lines(&migrated.target_jsonl_path),
        fixture_lines(&source_jsonl)
    );
}

// risk: Migration mechanic: compaction-aware Claude target build; level: particular-integration; source: proposal §11.1 Migration mechanic: compaction-aware Claude target build / A3, A6.
#[test]
fn migration_picks_latest_of_multiple_compaction_boundaries() {
    let (fixture, model, sessions, _source_projects, _target_projects, _source_jsonl) =
        migration_fixture();
    let resume_working_dir = fixture.dir.path().join("resume-workspace");
    fixture.seed_turns("claude", SESSION_A, &["turn-4", "turn-8"]);
    let db = fixture.open_db();
    let mut stderr = Vec::new();

    let migrated = migrate_chain_segment(
        &db,
        &sessions,
        &model,
        &fixture.resolved(&model, 0),
        &resume_working_dir,
        1,
        TransitionReason::Manual,
        &mut stderr,
    )
    .unwrap();
    let target = fixture_lines(&migrated.target_jsonl_path);

    assert_eq!(target.len(), 3);
    assert!(target.first().unwrap().contains(r#""turn_id":"turn-8""#));
}

// risk: Migration mechanic: compaction-aware Claude target build; level: particular-integration; source: proposal §11.1 Migration mechanic: compaction-aware Claude target build / A3, A6.
#[test]
fn migration_errors_when_compaction_boundary_not_in_jsonl() {
    let (fixture, model, sessions, _source_projects, target_projects, _source_jsonl) =
        migration_fixture();
    let resume_working_dir = fixture.dir.path().join("resume-workspace");
    fixture.seed_turns("claude", SESSION_A, &[]);
    fixture.seed_missing_compaction_boundary("claude", SESSION_A);
    let db = fixture.open_db();
    let mut stderr = Vec::new();

    let err = migrate_chain_segment(
        &db,
        &sessions,
        &model,
        &fixture.resolved(&model, 0),
        &resume_working_dir,
        1,
        TransitionReason::Manual,
        &mut stderr,
    )
    .unwrap_err();

    assert!(
        matches!(err, MigrationError::CompactionBoundaryNotInJsonl { session_id, turn_id } if session_id == SESSION_A && turn_id == "missing-turn")
    );
    assert!(!target_projects.exists() || fs::read_dir(target_projects).unwrap().next().is_none());
}

// risk: Migration mechanic: compaction-aware Claude target build; level: particular-integration; source: proposal §11.1 Migration mechanic: compaction-aware Claude target build / A3, A6.
#[test]
fn pre_compaction_turns_remain_queryable_after_migration() {
    let (fixture, model, sessions, _source_projects, _target_projects, _source_jsonl) =
        migration_fixture();
    let resume_working_dir = fixture.dir.path().join("resume-workspace");
    fixture.seed_turns("claude", SESSION_A, &["turn-6"]);
    let db = fixture.open_db();
    let mut stderr = Vec::new();

    migrate_chain_segment(
        &db,
        &sessions,
        &model,
        &fixture.resolved(&model, 0),
        &resume_working_dir,
        1,
        TransitionReason::Manual,
        &mut stderr,
    )
    .unwrap();

    let pre_count: i64 = fixture
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM session_turns WHERE provider_name = 'claude' AND session_id = ?1 AND turn_id IN ('turn-1', 'turn-5')",
            params![SESSION_A],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pre_count, 2);
}

// risk: Migration mechanic: Claude JSONL copy, Codex deferred guard, segment ledger, and races; level: particular-integration; source: proposal §11.1 Migration mechanic / A7.
#[test]
fn migration_mechanic_errors_codex_deferred_on_codex_active_provider() {
    let fixture = Fixture::new();
    let resume_working_dir = fixture.dir.path().join("resume-workspace");
    let model = fixture.codex_source_model(
        &fixture.dir.path().join("codex-sessions"),
        &fixture.dir.path().join("target-projects"),
    );
    fixture.seed_active_chain(CHAIN_A, "codex", SESSION_A, "codex-high");
    let db = fixture.open_db();
    let sessions = SessionsConfig::default();
    let mut stderr = Vec::new();

    let err = migrate_chain_segment(
        &db,
        &sessions,
        &model,
        &fixture.resolved(&model, 0),
        &resume_working_dir,
        1,
        TransitionReason::Manual,
        &mut stderr,
    )
    .unwrap_err();

    assert!(
        matches!(err, MigrationError::CodexMigrationDeferred { provider } if provider == "codex")
    );
    assert_eq!(segment_count(&fixture, "claude"), 0);
}

// risk: Migration mechanic: Codex deferred negative emission; level: particular-integration; source: proposal §11.1 Migration mechanic: Codex deferred negative emission / A7.
#[test]
fn migration_does_not_emit_migrate_stderr_on_codex_deferred() {
    let fixture = Fixture::new();
    let resume_working_dir = fixture.dir.path().join("resume-workspace");
    let model = fixture.codex_source_model(
        &fixture.dir.path().join("codex-sessions"),
        &fixture.dir.path().join("target-projects"),
    );
    fixture.seed_active_chain(CHAIN_A, "codex", SESSION_A, "codex-high");
    let db = fixture.open_db();
    let sessions = SessionsConfig::default();
    let mut stderr = Vec::new();

    let err = migrate_chain_segment(
        &db,
        &sessions,
        &model,
        &fixture.resolved(&model, 0),
        &resume_working_dir,
        1,
        TransitionReason::Manual,
        &mut stderr,
    )
    .unwrap_err();

    assert!(matches!(err, MigrationError::CodexMigrationDeferred { .. }));
    assert!(!String::from_utf8_lossy(&stderr).contains("[migrate]"));
    assert_eq!(segment_count(&fixture, "claude"), 0);
}

// risk: CLI surface; level: end-to-end; source: proposal §11.1 CLI surface / A8.
#[test]
fn top_level_resume_without_model_succeeds_when_chain_exists() {
    let fixture = Fixture::new();
    let argv = fixture.dir.path().join("argv.txt");
    let script = fixture.write_script("provider.sh", &argv_dump_provider_body(&argv));
    fixture.write_model("claude-opus", &single_resume_provider_model_toml(&script));
    fixture.write_runtime_provider("claude", &script);
    fixture.seed_active_chain(CHAIN_A, "claude", SESSION_A, "claude-opus");

    let output = fixture
        .command()
        .arg("--resume")
        .arg(SESSION_A)
        .args(["--models-dir"])
        .arg(&fixture.models_dir)
        .arg("continue")
        .output()
        .unwrap();

    assert_success(&output);
    assert!(fs::read_to_string(argv).unwrap().contains("--resume"));
}

#[test]
fn top_level_resume_with_migration_preserves_raw_supplied_session_id() {
    let fixture = Fixture::new();
    let source_projects = fixture.dir.path().join("source-projects");
    let target_projects = fixture.dir.path().join("target-projects");
    fixture.stage_claude_jsonl(&source_projects, SESSION_A);
    let transcript_path = fixture.dir.path().join("migrated-turns.jsonl");
    let argv = fixture.dir.path().join("migrated-argv.txt");
    fs::write(&transcript_path, "").unwrap();
    let fresh_session_id = "8f0a6a1f-9cd2-4c91-b6c6-1f0a0a8c9e22";
    let script = fixture.write_script(
        "migrated-provider.sh",
        &format!(
            r#"printf '%s\n' "$@" > {}
ts="$(date -u +%Y-%m-%dT%H:%M:%S.%NZ)"
turn_id="turn-$(date +%s%N)-$$"
printf '{{"session_id":"{fresh_session_id}","turn_id":"%s","timestamp":"%s","role":"assistant"}}\n' "$turn_id" "$ts" >> {}
printf 'ok\n'"#,
            sh_path(&argv),
            sh_path(&transcript_path)
        ),
    );
    fixture.write_model(
        "claude-opus",
        &manual_migrate_cli_model_toml(&script, &source_projects, &target_projects),
    );
    let app_dir = fixture.config_home.join("oulipoly-agent-runner");
    fs::create_dir_all(&app_dir).unwrap();
    fs::write(
        app_dir.join("providers.toml"),
        format!(
            r#"
[claude]
command = "{}"
args = []
interactive_args = ["launch"]
prompt_mode = "arg"

[claude.resume]
kind = "flag"
flag = "--resume"

[claude.session_storage]
kind = "claude_code"
projects_dir = "{}"

[claude2]
command = "{}"
args = []
interactive_args = ["launch"]
prompt_mode = "arg"

[claude2.resume]
kind = "flag"
flag = "--resume"

[claude2.session_storage]
kind = "claude_code"
projects_dir = "{}"
"#,
            script.display(),
            source_projects.display(),
            script.display(),
            target_projects.display()
        ),
    )
    .unwrap();
    fixture.write_sessions_config_from_transcript("claude2", &transcript_path);
    fixture.seed_active_chain(CHAIN_A, "claude", SESSION_A, "claude-opus");
    fixture.seed_turns("claude", SESSION_A, &[]);

    let output = fixture
        .command()
        .arg("--resume")
        .arg(CHAIN_A)
        .arg("--rotate-provider")
        .arg("claude2")
        .args(["--models-dir"])
        .arg(&fixture.models_dir)
        .arg("continue")
        .output()
        .unwrap();

    assert_success(&output);
    let argv = fs::read_to_string(argv).unwrap();
    assert!(argv.contains("--resume\n"), "{argv}");
    assert!(argv.contains(&format!("{SESSION_A}\n")), "{argv}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(parse_session(&stderr), CHAIN_A);
    let recorded_session_id: String = fixture
        .conn()
        .query_row(
            "SELECT session_id FROM invocations ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(recorded_session_id, CHAIN_A);
}

// risk: CLI surface; level: end-to-end; source: proposal §11.1 CLI surface / A8.
#[test]
fn model_none_resume_uses_providers_toml_only() {
    let fixture = Fixture::new();
    let argv = fixture.dir.path().join("argv.txt");
    let script = fixture.write_script("provider.sh", &argv_dump_provider_body(&argv));
    fixture.write_model(
        "claude-opus",
        &resume_provider_with_model_flags_toml(&script),
    );
    fixture.write_runtime_provider("claude", &script);
    fixture.seed_active_chain(CHAIN_A, "claude", SESSION_A, "<unknown>");

    let output = fixture
        .command()
        .arg("--resume")
        .arg(SESSION_A)
        .args(["--models-dir"])
        .arg(&fixture.models_dir)
        .arg("continue")
        .output()
        .unwrap();

    assert_success(&output);
    let argv = fs::read_to_string(argv).unwrap();
    assert!(!argv.contains("--model\n"), "{argv}");
    assert!(!argv.contains("-m\n"), "{argv}");
    assert!(!argv.contains("opus\n"), "{argv}");
    assert!(argv.contains("-p\n"), "{argv}");
    assert!(argv.contains("--resume\n"), "{argv}");
    let recorded_model: String = fixture
        .conn()
        .query_row("SELECT model_name FROM invocations", [], |row| row.get(0))
        .unwrap();
    assert_eq!(recorded_model, "<unknown>");
}

// risk: CLI surface; level: end-to-end; source: proposal §11.1 CLI surface / A8.
#[test]
fn model_set_resume_combines_providers_and_model_args() {
    let fixture = Fixture::new();
    let argv = fixture.dir.path().join("argv.txt");
    let script = fixture.write_script("provider.sh", &argv_dump_provider_body(&argv));
    fixture.write_model(
        "claude-opus",
        &resume_provider_with_model_flags_toml(&script),
    );
    fixture.write_runtime_provider("claude", &script);
    fixture.seed_active_chain(CHAIN_A, "claude", SESSION_A, "claude-opus");

    let output = fixture
        .command()
        .arg("--resume")
        .arg(SESSION_A)
        .args(["--models-dir"])
        .arg(&fixture.models_dir)
        .arg("continue")
        .output()
        .unwrap();

    assert_success(&output);
    let argv = fs::read_to_string(argv).unwrap();
    assert!(argv.contains("-p\n"), "{argv}");
    assert!(argv.contains("--model\nopus\n"), "{argv}");
    assert!(argv.contains("--resume\n"), "{argv}");
}

// risk: CLI surface; level: end-to-end; source: proposal §11.1 CLI surface / A8.
#[test]
fn run_repl_spawns_without_model_flag_when_model_none() {
    let fixture = Fixture::new();
    let argv = fixture.dir.path().join("argv.txt");
    let script = fixture.write_script("provider.sh", &argv_dump_provider_body(&argv));
    fixture.write_model(
        "claude-opus",
        &resume_provider_with_model_flags_toml(&script),
    );
    fixture.write_runtime_provider("claude", &script);
    fixture.seed_active_chain(CHAIN_A, "claude", SESSION_A, "<unknown>");

    let output = fixture
        .command()
        .args(["repl", "--resume", SESSION_A, "--models-dir"])
        .arg(&fixture.models_dir)
        .output()
        .unwrap();

    assert_success(&output);
    let argv = fs::read_to_string(argv).unwrap();
    assert!(!argv.contains("--model\n"), "{argv}");
    assert!(!argv.contains("-m\n"), "{argv}");
    assert!(!argv.contains("opus\n"), "{argv}");
    assert!(argv.contains("launch\n"), "{argv}");
    assert!(argv.contains("--resume\n"), "{argv}");
}

// risk: CLI surface / Best-on-resume decision; level: end-to-end; source: proposal §11.1 CLI surface and Best-on-resume decision / A2, A4.
#[test]
fn manual_migrate_flag_overrides_best_score_via_cli() {
    let fixture = Fixture::new();
    let source_projects = fixture.dir.path().join("source-projects");
    let target_projects = fixture.dir.path().join("target-projects");
    fixture.stage_claude_jsonl(&source_projects, SESSION_A);
    let script = fixture.write_script("provider.sh", ok_provider_body());
    fixture.write_model(
        "claude-opus",
        &manual_migrate_cli_model_toml(&script, &source_projects, &target_projects),
    );
    let app_dir = fixture.config_home.join("oulipoly-agent-runner");
    fs::create_dir_all(&app_dir).unwrap();
    fs::write(
        app_dir.join("providers.toml"),
        format!(
            r#"
[claude]
command = "{}"
args = []
interactive_args = ["launch"]
prompt_mode = "arg"

[claude.resume]
kind = "flag"
flag = "--resume"

[claude.session_storage]
kind = "claude_code"
projects_dir = "{}"

[claude2]
command = "{}"
args = []
interactive_args = ["launch"]
prompt_mode = "arg"

[claude2.resume]
kind = "flag"
flag = "--resume"

[claude2.session_storage]
kind = "claude_code"
projects_dir = "{}"
"#,
            script.display(),
            source_projects.display(),
            script.display(),
            target_projects.display()
        ),
    )
    .unwrap();
    fixture.seed_active_chain(CHAIN_A, "claude", SESSION_A, "claude-opus");
    fixture.seed_turns("claude", SESSION_A, &[]);

    let output = fixture
        .command()
        .args([
            "resume",
            "--session-id",
            SESSION_A,
            "--rotate-provider",
            "claude2",
            "--prompt",
            "continue",
        ])
        .args(["--models-dir"])
        .arg(&fixture.models_dir)
        .output()
        .unwrap();

    assert_success(&output);
    let reason: String = fixture
        .conn()
        .query_row(
            "SELECT transition_reason FROM session_chain_segments WHERE provider_name = 'claude2'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(reason, "manual");
}

// risk: CLI surface; level: end-to-end; source: proposal §11.1 CLI surface / A8.
#[test]
fn resume_list_subcommand_prints_all_chains_for_session_id() {
    let fixture = Fixture::new();
    fixture.seed_active_chain(CHAIN_A, "claude", SESSION_A, "claude-opus");
    fixture.seed_active_chain(
        "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
        "claude2",
        SESSION_A,
        "claude-opus",
    );

    let output = fixture
        .command()
        .args(["resume", "--list", SESSION_A])
        .output()
        .unwrap();

    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(CHAIN_A), "{stdout}");
    assert!(
        stdout.contains("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"),
        "{stdout}"
    );
    assert!(stdout.contains("claude"), "{stdout}");
    assert!(stdout.contains("turns"), "{stdout}");
}

// risk: CLI surface; level: end-to-end; source: proposal §11.1 CLI surface / A5, A8.
#[test]
fn resume_list_subcommand_rejects_malformed_uuid() {
    let fixture = Fixture::new();

    let output = fixture
        .command()
        .args(["resume", "--list", "not-a-uuid"])
        .output()
        .unwrap();

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid session UUID"), "{stderr}");
}

fn seed_pre_backfill_db(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE session_turns (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            provider_name TEXT NOT NULL,
            session_id TEXT NOT NULL,
            turn_id TEXT NOT NULL,
            timestamp TEXT NOT NULL,
            role TEXT NOT NULL,
            parent_turn_id TEXT,
            source_file TEXT NOT NULL,
            ingested_at TEXT NOT NULL,
            UNIQUE (provider_name, session_id, turn_id)
        );
        CREATE TABLE invocations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            invocation_uuid TEXT NOT NULL UNIQUE,
            model_name TEXT NOT NULL,
            provider_name TEXT,
            provider_index INTEGER NOT NULL,
            parent_invocation_id INTEGER,
            status TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed', 'legacy')),
            success INTEGER,
            exit_code INTEGER,
            error_category TEXT,
            session_id TEXT,
            session_capture_method TEXT,
            resume_acceptance_status TEXT,
            resume_acceptance_evidence TEXT,
            created_at TEXT NOT NULL,
            finished_at TEXT
        );
        CREATE TABLE providers (
            model_name TEXT NOT NULL,
            provider_name TEXT NOT NULL,
            invocation_count INTEGER NOT NULL DEFAULT 0,
            error_count INTEGER NOT NULL DEFAULT 0,
            last_error TEXT,
            last_error_at TEXT,
            last_invoked_at TEXT,
            PRIMARY KEY (model_name, provider_name)
        );
        CREATE TABLE provider_quotas (
            provider_name TEXT PRIMARY KEY,
            used_percent REAL NOT NULL DEFAULT 0,
            resets_at TEXT,
            calls_since_refresh INTEGER NOT NULL DEFAULT 0,
            refreshed_at TEXT,
            last_empty_refresh_at TEXT,
            exhausted_at TEXT NULL,
            topology_peak_live_window_count INTEGER NOT NULL DEFAULT 0,
            last_topology_probe_at TEXT
        );
        CREATE TABLE provider_quota_windows (
            provider_name TEXT NOT NULL,
            window_id INTEGER NOT NULL,
            used_percent REAL NOT NULL DEFAULT 0,
            resets_at TEXT NOT NULL,
            last_delta_percent REAL,
            last_delta_calls INTEGER,
            PRIMARY KEY (provider_name, window_id)
        );
        CREATE TABLE session_chains (
            chain_id TEXT PRIMARY KEY,
            created_at TEXT NOT NULL,
            last_used_at TEXT NOT NULL,
            model_name TEXT NOT NULL
        );
        CREATE TABLE session_chain_segments (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            chain_id TEXT NOT NULL REFERENCES session_chains(chain_id),
            provider_name TEXT NOT NULL,
            session_id TEXT NOT NULL,
            started_at TEXT NOT NULL,
            ended_at TEXT,
            last_turn_id TEXT,
            transition_reason TEXT NOT NULL CHECK (transition_reason IN
                ('initial', 'manual', 'quota_threshold', 'exhausted', 'imported')),
            UNIQUE(chain_id, provider_name, session_id)
        );",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session_turns
            (provider_name, session_id, turn_id, timestamp, role, source_file, ingested_at)
         VALUES ('claude', ?1, 't1', '2026-04-17T08:00:00Z', 'assistant', '', '2026-04-17T08:00:00Z')",
        params![SESSION_A],
    )
    .unwrap();
}

fn seed_current_backfill_failure_db(path: &Path) {
    seed_pre_backfill_db(path);
    StateDb::open(path).unwrap();
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "DELETE FROM session_chain_segments;
        DELETE FROM session_chains;
        CREATE TRIGGER fail_session_chain_backfill
        BEFORE INSERT ON session_chains
        BEGIN
            SELECT RAISE(FAIL, 'forced session chain backfill failure');
        END;",
    )
    .unwrap();
}

// risk: Schema migration and backfill; level: end-to-end; source: proposal §11.1 Schema migration and backfill / A5.
#[test]
fn migrate_db_command_runs_backfill_to_completion() {
    let fixture = Fixture::new();
    seed_pre_backfill_db(&fixture.db_path());

    let output = fixture.command().arg("migrate-db").output().unwrap();

    assert_success(&output);
    let count: i64 = fixture
        .conn()
        .query_row("SELECT COUNT(*) FROM session_chains", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

// risk: Schema migration and backfill; level: end-to-end; source: proposal §11.1 Schema migration and backfill / A5.
#[test]
fn migrate_db_command_idempotent_on_second_run() {
    let fixture = Fixture::new();
    seed_pre_backfill_db(&fixture.db_path());

    assert_success(&fixture.command().arg("migrate-db").output().unwrap());
    let first_count: i64 = fixture
        .conn()
        .query_row("SELECT COUNT(*) FROM session_chains", [], |row| row.get(0))
        .unwrap();
    assert_success(&fixture.command().arg("migrate-db").output().unwrap());
    let second_count: i64 = fixture
        .conn()
        .query_row("SELECT COUNT(*) FROM session_chains", [], |row| row.get(0))
        .unwrap();

    assert_eq!(second_count, first_count);
}

// risk: Schema migration and backfill; level: end-to-end; source: proposal §11.1 Schema migration and backfill / A5.
#[test]
fn migrate_db_command_reports_open_error() {
    let fixture = Fixture::new();
    let data_home = data_home_file_fixture(&fixture);
    let mut command = fixture.command();
    command.env("XDG_DATA_HOME", data_home);

    let output = command.arg("migrate-db").output().unwrap();

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Failed to create state directory")
            || stderr.contains("Failed to open state DB"),
        "{stderr}"
    );
}

// risk: Schema migration and backfill; level: end-to-end; source: proposal §11.1 Schema migration and backfill / A5.
#[test]
fn startup_refuses_chain_ops_on_backfill_failure() {
    let fixture = Fixture::new();
    seed_current_backfill_failure_db(&fixture.db_path());

    let output = fixture
        .command()
        .args(["--resume", SESSION_A, "continue"])
        .output()
        .unwrap();

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("agents migrate-db"), "{stderr}");
}
