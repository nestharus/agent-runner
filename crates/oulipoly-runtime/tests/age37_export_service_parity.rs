use oulipoly_runtime::services::{
    ProductionSessionExportService, SessionExportServicePort, SessionExportServiceRequest,
};
use oulipoly_runtime::session_export::{
    ExportError, ExportSessionMetadata, SessionStorageType, canonical_jsonl_bytes,
    read_canonical_transcript, resolve_export_session_metadata,
};
use oulipoly_state::StateDb;
use rusqlite::params;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const SESSION_A: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
const SESSION_B: &str = "8f0a6a1f-9cd2-4c91-b6c6-1f0a0a8c9e22";
const CHAIN_A: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const CHAIN_B: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const MODEL: &str = "claude-opus";
const CLAUDE_PROVIDER: &str = "claude";
const CODEX_PROVIDER: &str = "codex";

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    old_config: Option<OsString>,
    old_data: Option<OsString>,
    old_home: Option<OsString>,
}

impl EnvGuard {
    fn new(config_home: &Path, data_home: &Path) -> Self {
        let guard = Self {
            old_config: std::env::var_os("XDG_CONFIG_HOME"),
            old_data: std::env::var_os("XDG_DATA_HOME"),
            old_home: std::env::var_os("HOME"),
        };
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", config_home);
            std::env::set_var("XDG_DATA_HOME", data_home);
            std::env::set_var("HOME", data_home);
        }
        guard
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            restore_env("XDG_CONFIG_HOME", self.old_config.take());
            restore_env("XDG_DATA_HOME", self.old_data.take());
            restore_env("HOME", self.old_home.take());
        }
    }
}

unsafe fn restore_env(key: &str, value: Option<OsString>) {
    match value {
        Some(value) => unsafe { std::env::set_var(key, value) },
        None => unsafe { std::env::remove_var(key) },
    }
}

struct ExportFixture {
    _dir: tempfile::TempDir,
    _env: EnvGuard,
    config_root: PathBuf,
    models_dir: PathBuf,
    data_root: PathBuf,
}

impl ExportFixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config_home = dir.path().join("config");
        let data_home = dir.path().join("data");
        let config_root = config_home.join("oulipoly-agent-runner");
        let models_dir = config_root.join("models");
        let data_root = data_home.join("oulipoly-agent-runner");
        fs::create_dir_all(&models_dir).unwrap();
        fs::create_dir_all(&data_root).unwrap();
        let env = EnvGuard::new(&config_home, &data_home);
        Self {
            _dir: dir,
            _env: env,
            config_root,
            models_dir,
            data_root,
        }
    }

    fn db(&self) -> StateDb {
        StateDb::open(&self.data_root.join("state.db")).unwrap()
    }

    fn write_model(&self, model_name: &str, providers: &[&str]) {
        let mut body = String::new();
        for provider in providers {
            body.push_str("[[providers]]\n");
            body.push_str(&format!("name = \"{provider}\"\n\n"));
        }
        fs::write(self.models_dir.join(format!("{model_name}.toml")), body).unwrap();
    }

    fn write_provider(&self, provider_name: &str, storage_kind: &str, include_storage: bool) {
        let storage = if include_storage {
            let storage_root = match storage_kind {
                "claude_code" => format!(
                    "projects_dir = {:?}",
                    self.data_root.join("claude-projects").to_string_lossy()
                ),
                "codex" => format!(
                    "sessions_dir = {:?}",
                    self.data_root.join("codex-sessions").to_string_lossy()
                ),
                _ => String::new(),
            };
            format!(
                r#"
[{provider_name}.session_storage]
kind = "{storage_kind}"
{storage_root}
"#
            )
        } else {
            String::new()
        };
        let body = format!(
            r#"[{provider_name}]
command = "provider-command-that-must-not-run"
args = []
interactive_args = []
prompt_mode = "arg"

[{provider_name}.resume]
kind = "flag"
flag = "--resume"
{storage}
"#
        );
        let providers_path = self.config_root.join("providers.toml");
        let mut existing = fs::read_to_string(&providers_path).unwrap_or_default();
        existing.push_str(&body);
        fs::write(providers_path, existing).unwrap();
    }

    fn write_sessions_locator(&self, provider_name: &str, path: &Path) {
        fs::write(
            self.config_root.join("sessions.toml"),
            format!(
                "[{provider_name}]\nturn_script = \"true\"\ntranscript_locator = {:?}\nstate_dir = {:?}\n",
                format!("printf '%s\\n' {}", path.display()),
                self.data_root.join("locator-state").to_string_lossy()
            ),
        )
        .unwrap();
    }

    fn seed_active_chain(&self, chain_id: &str, provider_name: &str, session_id: &str) {
        let db = self.db();
        let now = chrono::Utc::now().to_rfc3339();
        db.connection()
            .execute(
                "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
                 VALUES (?1, ?2, ?2, ?3)",
                params![chain_id, now, MODEL],
            )
            .unwrap();
        db.connection()
            .execute(
                "INSERT INTO session_chain_segments
                    (chain_id, provider_name, session_id, started_at, transition_reason)
                 VALUES (?1, ?2, ?3, '2026-04-17T08:00:00Z', 'initial')",
                params![chain_id, provider_name, session_id],
            )
            .unwrap();
    }

    fn seed_db_turn(&self, provider_name: &str, session_id: &str, turn_id: &str, text: &str) {
        let db = self.db();
        db.connection()
            .execute(
                "INSERT INTO session_turns
                    (provider_name, session_id, turn_id, timestamp, role,
                     parent_turn_id, is_sidechain, is_compaction_boundary, source_file, ingested_at, body)
                 VALUES (?1, ?2, ?3, '2026-04-17T08:00:00Z', 'assistant', NULL, 0, 0, '', '2026-04-17T08:00:00Z', ?4)",
                params![
                    provider_name,
                    session_id,
                    turn_id,
                    format!(r#"[{{"type":"text","text":"{text}"}}]"#)
                ],
            )
            .unwrap();
    }

    fn stage_claude_jsonl(&self, session_id: &str) -> PathBuf {
        let path = self.data_root.join(format!("{session_id}.jsonl"));
        fs::write(
            &path,
            format!(
                "{{\"sessionId\":\"{session_id}\",\"type\":\"user\",\"uuid\":\"turn-1\",\"timestamp\":\"2026-04-17T08:00:00Z\",\"message\":\"hello\"}}\n"
            ),
        )
        .unwrap();
        path
    }
}

fn prepared_export() -> (ExportFixture, PathBuf) {
    let fixture = ExportFixture::new();
    fixture.write_model(MODEL, &[CLAUDE_PROVIDER]);
    fixture.write_provider(CLAUDE_PROVIDER, "claude_code", true);
    fixture.seed_active_chain(CHAIN_A, CLAUDE_PROVIDER, SESSION_A);
    let transcript = fixture.stage_claude_jsonl(SESSION_A);
    fixture.write_sessions_locator(CLAUDE_PROVIDER, &transcript);
    (fixture, transcript)
}

fn invocation_count(path: &Path) -> i64 {
    let db = StateDb::open(path).unwrap();
    db.connection()
        .query_row("SELECT COUNT(*) FROM invocations", [], |row| row.get(0))
        .unwrap()
}

#[test]
fn export_service_returns_canonical_bytes_matching_direct_path() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let (_fixture, transcript) = prepared_export();
    let metadata = ExportSessionMetadata {
        session_id: SESSION_A.to_string(),
        chain_id: CHAIN_A.to_string(),
        provider_name: CLAUDE_PROVIDER.to_string(),
        storage_type: SessionStorageType::ClaudeCode,
        jsonl_path: transcript,
    };
    let direct = canonical_jsonl_bytes(&read_canonical_transcript(&metadata).unwrap()).unwrap();

    let service = ProductionSessionExportService::default();
    let output = service
        .export_session(SessionExportServiceRequest {
            session_id: SESSION_A.to_string(),
        })
        .unwrap();

    assert_eq!(output.result.unwrap(), direct);
}

#[test]
fn export_service_preserves_invocation_count() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let (fixture, _transcript) = prepared_export();
    let state_path = fixture.data_root.join("state.db");
    let before = invocation_count(&state_path);
    let service = ProductionSessionExportService::default();

    let output = service
        .export_session(SessionExportServiceRequest {
            session_id: SESSION_A.to_string(),
        })
        .unwrap();

    assert!(output.result.is_ok());
    assert_eq!(invocation_count(&state_path), before);
}

#[test]
fn export_service_preserves_export_error_variants() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    assert_invalid_session_id();
    assert_session_not_found();
    assert_ambiguous_session();
    assert_unsupported_storage();
    assert_malformed_transcript();
    assert_operational();
}

#[test]
fn locator_contract_allow_missing_export_metadata_uses_db_fallback() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let fixture = ExportFixture::new();
    fixture.write_model(MODEL, &[CLAUDE_PROVIDER]);
    fixture.write_provider(CLAUDE_PROVIDER, "claude_code", true);
    fixture.seed_active_chain(CHAIN_A, CLAUDE_PROVIDER, SESSION_A);
    fixture.seed_db_turn(CLAUDE_PROVIDER, SESSION_A, "db-turn", "fallback body");
    let missing = fixture.data_root.join("missing-provider-transcript.jsonl");
    fixture.write_sessions_locator(CLAUDE_PROVIDER, &missing);

    let metadata = resolve_export_session_metadata(SESSION_A).unwrap();
    let records = read_canonical_transcript(&metadata).unwrap();

    assert_eq!(metadata.jsonl_path, missing);
    assert!(!metadata.jsonl_path.exists());
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].turn_id, "db-turn");
    assert_eq!(records[0].content[0].text.as_deref(), Some("fallback body"));
    assert_eq!(records[0].source.storage_type, "state_db");
}

#[test]
fn session_export_parity_unchanged_under_contract() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let (_fixture, transcript) = prepared_export();
    let service = ProductionSessionExportService::default();

    let output = service
        .export_session(SessionExportServiceRequest {
            session_id: SESSION_A.to_string(),
        })
        .unwrap();

    assert!(output.result.is_ok());
    assert_eq!(
        transcript,
        resolve_export_session_metadata(SESSION_A)
            .unwrap()
            .jsonl_path
    );
}

fn export_error_for(fixture: ExportFixture, session_id: &str) -> ExportError {
    let service = ProductionSessionExportService::default();
    let err = service
        .export_session(SessionExportServiceRequest {
            session_id: session_id.to_string(),
        })
        .unwrap()
        .result
        .unwrap_err();
    drop(fixture);
    err
}

fn assert_invalid_session_id() {
    let fixture = ExportFixture::new();
    fixture.write_model(MODEL, &[CLAUDE_PROVIDER]);
    fixture.write_provider(CLAUDE_PROVIDER, "claude_code", true);
    match export_error_for(fixture, "not-a-uuid") {
        ExportError::InvalidSessionId { input } => assert_eq!(input, "not-a-uuid"),
        other => panic!("expected InvalidSessionId, got {other:?}"),
    }
}

fn assert_session_not_found() {
    let fixture = ExportFixture::new();
    fixture.write_model(MODEL, &[CLAUDE_PROVIDER]);
    fixture.write_provider(CLAUDE_PROVIDER, "claude_code", true);
    match export_error_for(fixture, SESSION_A) {
        ExportError::SessionNotFound { input } => assert_eq!(input, SESSION_A),
        other => panic!("expected SessionNotFound, got {other:?}"),
    }
}

fn assert_ambiguous_session() {
    let fixture = ExportFixture::new();
    fixture.write_model(MODEL, &[CLAUDE_PROVIDER, CODEX_PROVIDER]);
    fixture.write_provider(CLAUDE_PROVIDER, "claude_code", true);
    fixture.write_provider(CODEX_PROVIDER, "codex", true);
    fixture.seed_active_chain(CHAIN_A, CLAUDE_PROVIDER, SESSION_A);
    fixture.seed_active_chain(CHAIN_B, CODEX_PROVIDER, SESSION_A);
    match export_error_for(fixture, SESSION_A) {
        ExportError::AmbiguousSession { input } => assert_eq!(input, SESSION_A),
        other => panic!("expected AmbiguousSession, got {other:?}"),
    }
}

fn assert_unsupported_storage() {
    let fixture = ExportFixture::new();
    fixture.write_model(MODEL, &[CLAUDE_PROVIDER]);
    fixture.write_provider(CLAUDE_PROVIDER, "claude_code", false);
    fixture.seed_active_chain(CHAIN_A, CLAUDE_PROVIDER, SESSION_A);
    match export_error_for(fixture, SESSION_A) {
        ExportError::UnsupportedStorage {
            provider_name,
            reason,
        } => {
            assert_eq!(provider_name, CLAUDE_PROVIDER);
            assert!(!reason.is_empty());
        }
        other => panic!("expected UnsupportedStorage, got {other:?}"),
    }
}

fn assert_malformed_transcript() {
    let fixture = ExportFixture::new();
    fixture.write_model(MODEL, &[CLAUDE_PROVIDER]);
    fixture.write_provider(CLAUDE_PROVIDER, "claude_code", true);
    fixture.seed_active_chain(CHAIN_A, CLAUDE_PROVIDER, SESSION_A);
    let path = fixture.data_root.join("bad.jsonl");
    fs::write(
        &path,
        format!(
            "{{\"sessionId\":\"{SESSION_B}\",\"type\":\"user\",\"uuid\":\"turn-1\",\"timestamp\":\"2026-04-17T08:00:00Z\",\"message\":\"wrong\"}}\n"
        ),
    )
    .unwrap();
    fixture.write_sessions_locator(CLAUDE_PROVIDER, &path);
    match export_error_for(fixture, SESSION_A) {
        ExportError::MalformedTranscript { path: got, .. } => assert_eq!(got, path),
        other => panic!("expected MalformedTranscript, got {other:?}"),
    }
}

fn assert_operational() {
    let fixture = ExportFixture::new();
    fs::write(
        fixture.models_dir.join(format!("{MODEL}.toml")),
        "not = [toml",
    )
    .unwrap();
    fixture.write_provider(CLAUDE_PROVIDER, "claude_code", true);
    match export_error_for(fixture, SESSION_A) {
        ExportError::Operational { message } => assert!(!message.is_empty()),
        other => panic!("expected Operational, got {other:?}"),
    }
}
