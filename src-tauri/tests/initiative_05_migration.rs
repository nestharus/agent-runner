#![cfg(unix)]

use agent_runner_lib::balancer::TransitionReason;
use agent_runner_lib::config::{
    ModelConfig, ProviderEntry, ProvidersConfig, SessionSourceEntry, SessionsConfig,
};
use agent_runner_lib::migration::{MigrationError, migrate_chain_segment};
use agent_runner_lib::sessions::scan_provider;
use agent_runner_lib::state::{ResolvedResume, SessionTurnIngest, StateDb};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SESSION_A: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
const CHAIN_A: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

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

    fn write_model(&self, model_name: &str, body: &str) {
        fs::write(self.models_dir.join(format!("{model_name}.toml")), body).unwrap();
    }

    fn write_providers_config(&self, defaults: &[(&str, Option<&str>)]) -> ProvidersConfig {
        let app_dir = self.config_home.join("oulipoly-agent-runner");
        fs::create_dir_all(&app_dir).unwrap();
        let mut body = String::new();
        let mut entries = HashMap::new();
        for (provider, default_model) in defaults {
            body.push_str(&format!("[{provider}]\n"));
            if let Some(model) = default_model {
                body.push_str(&format!("default_model = \"{model}\"\n"));
            }
            body.push('\n');
            entries.insert(
                (*provider).to_string(),
                ProviderEntry {
                    quota_script: None,
                    auth_refresh_command: None,
                    default_model: default_model.map(str::to_string),
                },
            );
        }
        fs::write(app_dir.join("providers.toml"), body).unwrap();
        ProvidersConfig { entries }
    }

    fn write_sessions_config(&self, provider: &str, script: &Path) -> SessionsConfig {
        let mut entries = HashMap::new();
        entries.insert(
            provider.to_string(),
            SessionSourceEntry {
                turn_script: script.to_string_lossy().into_owned(),
                transcript_locator: None,
                state_dir: Some(self.dir.path().join("session-state")),
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
                }
            })
            .collect();
        db.ingest_session_turns_batch(provider, &turns).unwrap();
    }

    fn migration_model(&self, source_projects: &Path, target_projects: &Path) -> ModelConfig {
        ModelConfig::from_toml(
            "claude-opus",
            &format!(
                r#"
prompt_mode = "arg"

[[providers]]
name = "claude"
command = "claude"

[providers.resume]
kind = "flag"
flag = "--resume"

[providers.session_storage]
kind = "claude_code"
projects_dir = "{}"

[[providers]]
name = "claude2"
command = "claude2"

[providers.resume]
kind = "flag"
flag = "--resume"

[providers.session_storage]
kind = "claude_code"
projects_dir = "{}"
"#,
                source_projects.display(),
                target_projects.display()
            ),
        )
        .unwrap()
    }

    fn codex_source_model(&self, codex_sessions: &Path, target_projects: &Path) -> ModelConfig {
        ModelConfig::from_toml(
            "codex-high",
            &format!(
                r#"
prompt_mode = "arg"

[[providers]]
name = "codex"
command = "codex"

[providers.resume]
kind = "subcommand"
subcommand = ["resume"]

[providers.session_storage]
kind = "codex"
sessions_dir = "{}"

[[providers]]
name = "claude"
command = "claude"

[providers.resume]
kind = "flag"
flag = "--resume"

[providers.session_storage]
kind = "claude_code"
projects_dir = "{}"
"#,
                codex_sessions.display(),
                target_projects.display()
            ),
        )
        .unwrap()
    }

    fn resolved(&self, model: &ModelConfig, provider_index: usize) -> ResolvedResume {
        let provider = &model.providers[provider_index];
        ResolvedResume {
            chain_id: CHAIN_A.to_string(),
            model_name: model.name.clone(),
            model: model.clone(),
            active_provider: provider.name.clone(),
            active_provider_index: provider_index,
            active_session_id: SESSION_A.to_string(),
        }
    }
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
        .expect("OULIPOLY_SESSION line");
    let value: serde_json::Value =
        serde_json::from_str(line.strip_prefix("OULIPOLY_SESSION=").unwrap()).unwrap();
    value["session_id"].as_str().unwrap().to_string()
}

fn assert_success(output: &Output) {
    assert_eq!(output.status.code(), Some(0), "{output:?}");
}

fn run_session_capture_chain_fixture() -> Fixture {
    let fixture = Fixture::new();
    let transcript_path = fixture.dir.path().join("turns.jsonl");
    fs::write(&transcript_path, "").unwrap();
    let script = fixture.write_script(
        "session-writer.sh",
        &format!(
            r#"printf '{{"session_id":"{SESSION_A}","turn_id":"t1","timestamp":"2026-04-17T08:00:00Z","role":"assistant"}}\n' >> "{}"
printf 'ok\n'"#,
            transcript_path.display()
        ),
    );
    fixture.write_model(
        "claude-opus",
        &format!(
            r#"
prompt_mode = "arg"

[[providers]]
name = "claude"
command = "{}"
"#,
            script.display()
        ),
    );
    fixture.write_sessions_config("claude", &script);

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
fn ui_session_chain_minted_at_ingestion_uses_provider_default() {
    let fixture = Fixture::new();
    let db = fixture.open_db();
    let script = fixture.write_script(
        "turns.sh",
        &format!(
            r#"printf '{{"session_id":"{SESSION_A}","turn_id":"ui-1","timestamp":"2026-04-17T08:00:00Z","role":"assistant"}}\n'"#
        ),
    );
    let sessions = fixture.write_sessions_config("claude", &script);
    let providers = fixture.write_providers_config(&[("claude", Some("claude-opus"))]);

    let result = scan_provider("claude", &sessions, &providers, &db);

    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!(chain_model_name(&fixture, SESSION_A), "claude-opus");
}

// risk: Chain identity write paths; level: particular-integration; source: proposal §11.1 Chain identity write paths / A3, A8.
#[test]
fn ui_session_chain_minted_with_unknown_when_no_provider_default() {
    let fixture = Fixture::new();
    let db = fixture.open_db();
    let script = fixture.write_script(
        "turns.sh",
        &format!(
            r#"printf '{{"session_id":"{SESSION_A}","turn_id":"ui-1","timestamp":"2026-04-17T08:00:00Z","role":"assistant"}}\n'"#
        ),
    );
    let sessions = fixture.write_sessions_config("claude", &script);
    let providers = fixture.write_providers_config(&[("claude", None)]);

    let result = scan_provider("claude", &sessions, &providers, &db);

    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!(chain_model_name(&fixture, SESSION_A), "<unknown>");
}

// risk: Chain identity write paths; level: particular-integration; source: proposal §11.1 Chain identity write paths / A7.
#[test]
fn chain_mint_works_for_codex_ingestion() {
    let fixture = Fixture::new();
    let db = fixture.open_db();
    let script = fixture.write_script(
        "codex-turns.sh",
        &format!(
            r#"printf '{{"session_id":"{SESSION_A}","turn_id":"codex-1","timestamp":"2026-04-17T08:00:00Z","role":"assistant"}}\n'"#
        ),
    );
    let sessions = fixture.write_sessions_config("codex", &script);
    let providers = fixture.write_providers_config(&[("codex", Some("codex-high"))]);

    let result = scan_provider("codex", &sessions, &providers, &db);

    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!(chain_model_name(&fixture, SESSION_A), "codex-high");
    assert_eq!(segment_count(&fixture, "codex"), 1);
}

// risk: Resolver disambiguation and model inference; level: end-to-end; source: proposal §11.1 Resolver disambiguation and model inference / A8.
#[test]
fn agent_resume_no_dash_m_uses_session_recorded_model() {
    let fixture = Fixture::new();
    let argv_dump = fixture.dir.path().join("argv.txt");
    let script = fixture.write_script(
        "resume-provider.sh",
        &format!(
            r#"printf '%s\n' "$@" > "{}"
printf '{{"session_id":"{SESSION_A}","turn_id":"t-$RANDOM","timestamp":"2026-04-17T08:00:00Z","role":"assistant"}}\n' >> "{}/turns.jsonl"
printf 'ok\n'"#,
            argv_dump.display(),
            fixture.dir.path().display()
        ),
    );
    fs::write(fixture.dir.path().join("turns.jsonl"), "").unwrap();
    fixture.write_model(
        "claude-opus",
        &format!(
            r#"
prompt_mode = "arg"

[[providers]]
name = "claude"
command = "{}"

[providers.resume]
kind = "flag"
flag = "--resume"
"#,
            script.display()
        ),
    );
    fixture.write_sessions_config("claude", &script);
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

// risk: Migration mechanic: Claude JSONL copy, Codex deferred guard, segment ledger, and races; level: particular-integration; source: proposal §11.1 Migration mechanic / A1, A3.
#[test]
fn migration_copies_claude_jsonl_to_target_projects_dir() {
    let (fixture, model, sessions, _source_projects, target_projects, source_jsonl) =
        migration_fixture();
    fixture.seed_turns("claude", SESSION_A, &[]);
    let db = fixture.open_db();
    let mut stderr = Vec::new();

    let migrated = migrate_chain_segment(
        &db,
        &sessions,
        &model,
        &fixture.resolved(&model, 0),
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

// risk: Migration mechanic: Claude JSONL copy, Codex deferred guard, segment ledger, and races; level: particular-integration; source: proposal §11.1 Migration mechanic / A1, A3.
#[test]
fn migration_appends_chain_segment_with_correct_reason() {
    let (fixture, model, sessions, _source_projects, _target_projects, _source_jsonl) =
        migration_fixture();
    fixture.seed_turns("claude", SESSION_A, &[]);
    let db = fixture.open_db();
    let mut stderr = Vec::new();

    let migrated = migrate_chain_segment(
        &db,
        &sessions,
        &model,
        &fixture.resolved(&model, 0),
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
    let missing = source_projects
        .join("cwd")
        .join(format!("{SESSION_A}.jsonl"));
    let locator = fixture.write_script(
        "missing-locator.sh",
        &format!(r#"printf '%s\n' "{}""#, missing.display()),
    );
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
    let bare = PathBuf::from("bare-session.jsonl");
    fs::write(&bare, "{}\n").unwrap();
    let locator = fixture.write_script(
        "bare-locator.sh",
        &format!(r#"printf '%s\n' "{}""#, bare.display()),
    );
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
        1,
        TransitionReason::Manual,
        &mut stderr,
    )
    .unwrap_err();
    let _ = fs::remove_file(&bare);

    assert!(
        matches!(err, MigrationError::SourcePathMalformed { provider, .. } if provider == "claude")
    );
}

// risk: Migration mechanic: compaction-aware Claude target build; level: particular-integration; source: proposal §11.1 Migration mechanic: compaction-aware Claude target build / A3, A6.
#[test]
fn migration_truncates_target_jsonl_at_latest_compaction_boundary() {
    let (fixture, model, sessions, _source_projects, _target_projects, _source_jsonl) =
        migration_fixture();
    fixture.seed_turns("claude", SESSION_A, &["turn-6"]);
    let db = fixture.open_db();
    let mut stderr = Vec::new();

    let migrated = migrate_chain_segment(
        &db,
        &sessions,
        &model,
        &fixture.resolved(&model, 0),
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
    fixture.seed_turns("claude", SESSION_A, &[]);
    let db = fixture.open_db();
    let mut stderr = Vec::new();

    let migrated = migrate_chain_segment(
        &db,
        &sessions,
        &model,
        &fixture.resolved(&model, 0),
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
    fixture.seed_turns("claude", SESSION_A, &["turn-4", "turn-8"]);
    let db = fixture.open_db();
    let mut stderr = Vec::new();

    let migrated = migrate_chain_segment(
        &db,
        &sessions,
        &model,
        &fixture.resolved(&model, 0),
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
    fixture.seed_turns("claude", SESSION_A, &[]);
    fixture
        .conn()
        .execute(
            "INSERT INTO session_turns
                (provider_name, session_id, turn_id, timestamp, role, source_file, ingested_at, is_compaction_boundary)
             VALUES ('claude', ?1, 'missing-turn', '2026-04-17T08:00:10Z', 'assistant', '', '2026-04-17T08:00:10Z', 1)",
            params![SESSION_A],
        )
        .unwrap();
    let db = fixture.open_db();
    let mut stderr = Vec::new();

    let err = migrate_chain_segment(
        &db,
        &sessions,
        &model,
        &fixture.resolved(&model, 0),
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
    fixture.seed_turns("claude", SESSION_A, &["turn-6"]);
    let db = fixture.open_db();
    let mut stderr = Vec::new();

    migrate_chain_segment(
        &db,
        &sessions,
        &model,
        &fixture.resolved(&model, 0),
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
    let script = fixture.write_script(
        "provider.sh",
        &format!(
            r#"printf '%s\n' "$@" > "{}"; printf 'ok\n'"#,
            argv.display()
        ),
    );
    fixture.write_model(
        "claude-opus",
        &format!(
            r#"
prompt_mode = "arg"

[[providers]]
name = "claude"
command = "{}"

[providers.resume]
kind = "flag"
flag = "--resume"
"#,
            script.display()
        ),
    );
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

// risk: CLI surface; level: end-to-end; source: proposal §11.1 CLI surface / A8.
#[test]
fn top_level_resume_without_model_errors_when_no_invocation_history() {
    let fixture = Fixture::new();
    let script = fixture.write_script("provider.sh", "printf 'ok\n'");
    fixture.write_model(
        "claude-opus",
        &format!(
            r#"
prompt_mode = "arg"

[[providers]]
name = "claude"
command = "{}"

[providers.resume]
kind = "flag"
flag = "--resume"
"#,
            script.display()
        ),
    );
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

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Cannot infer model"), "{stderr}");
    assert!(stderr.contains("default_model"), "{stderr}");
}

// risk: CLI surface / Sticky-then-migrate decision; level: end-to-end; source: proposal §11.1 CLI surface and Sticky-then-migrate decision / A2, A4.
#[test]
fn manual_migrate_flag_overrides_threshold_via_cli() {
    let fixture = Fixture::new();
    let source_projects = fixture.dir.path().join("source-projects");
    let target_projects = fixture.dir.path().join("target-projects");
    fixture.stage_claude_jsonl(&source_projects, SESSION_A);
    let script = fixture.write_script("provider.sh", "printf 'ok\n'");
    fixture.write_model(
        "claude-opus",
        &format!(
            r#"
prompt_mode = "arg"

[[providers]]
name = "claude"
command = "{}"

[providers.resume]
kind = "flag"
flag = "--resume"

[providers.session_storage]
kind = "claude_code"
projects_dir = "{}"

[[providers]]
name = "claude2"
command = "{}"

[providers.resume]
kind = "flag"
flag = "--resume"

[providers.session_storage]
kind = "claude_code"
projects_dir = "{}"
"#,
            script.display(),
            source_projects.display(),
            script.display(),
            target_projects.display()
        ),
    );
    fixture.seed_active_chain(CHAIN_A, "claude", SESSION_A, "claude-opus");
    fixture.seed_turns("claude", SESSION_A, &[]);

    let output = fixture
        .command()
        .args([
            "resume",
            "--session-id",
            SESSION_A,
            "--migrate",
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
fn startup_refuses_chain_ops_on_backfill_failure() {
    let fixture = Fixture::new();
    seed_pre_backfill_db(&fixture.db_path());
    let db_file = fixture.db_path();
    let mut perms = fs::metadata(&db_file).unwrap().permissions();
    perms.set_mode(0o444);
    fs::set_permissions(&db_file, perms).unwrap();

    let output = fixture
        .command()
        .args(["--resume", SESSION_A, "continue"])
        .output()
        .unwrap();

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("agents migrate-db"), "{stderr}");
}
