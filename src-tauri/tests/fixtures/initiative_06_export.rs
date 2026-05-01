#![cfg(unix)]
#![allow(dead_code)]

use agent_runner_lib::session_export::{ExportSessionMetadata, SessionStorageType};
use agent_runner_lib::state::StateDb;
use chrono::{Duration, Utc};
use rusqlite::{Connection, params};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub const SESSION_A: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
pub const SESSION_B: &str = "8f0a6a1f-9cd2-4c91-b6c6-1f0a0a8c9e22";
pub const CHAIN_A: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
pub const CHAIN_B: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
pub const MODEL: &str = "claude-opus";
pub const CLAUDE_PROVIDER: &str = "claude";
pub const CODEX_PROVIDER: &str = "codex";

pub const CLAUDE_LINE_1: &str = "{\"sessionId\":\"5169694d-de0f-40d1-890c-6e28e55bab27\",\"type\":\"user\",\"uuid\":\"claude-turn-1\",\"timestamp\":\"2026-04-17T08:00:00Z\",\"message\":\"hello from user\"}";
pub const CLAUDE_LINE_2: &str = "{\"sessionId\":\"5169694d-de0f-40d1-890c-6e28e55bab27\",\"type\":\"assistant\",\"uuid\":\"claude-turn-2\",\"timestamp\":\"2026-04-17T08:00:01Z\",\"message\":\"hello from assistant\"}";
pub const CLAUDE_UNSUPPORTED_LINE: &str = "{\"sessionId\":\"5169694d-de0f-40d1-890c-6e28e55bab27\",\"type\":\"system\",\"uuid\":\"claude-system-1\",\"timestamp\":\"2026-04-17T08:00:02Z\",\"message\":\"provider system event\"}";

#[derive(Clone, Copy)]
pub enum StorageKind<'a> {
    ClaudeCode { projects_dir: &'a Path },
    Codex { sessions_dir: &'a Path },
    None,
}

pub struct ExportFixture {
    dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
}

pub struct PreparedExport {
    pub fixture: ExportFixture,
    pub session_id: String,
    pub chain_id: String,
    pub provider_name: String,
    pub jsonl_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadOnlySnapshot {
    pub table_counts: BTreeMap<String, i64>,
    pub transcript_bytes: Vec<u8>,
    pub transcript_mtime: std::time::SystemTime,
    pub providers_toml: Option<Vec<u8>>,
    pub sessions_toml: Option<Vec<u8>>,
    pub model_toml: Option<Vec<u8>>,
}

pub struct ComponentExportFixture {
    dir: tempfile::TempDir,
    pub metadata: ExportSessionMetadata,
    pub jsonl_path: PathBuf,
}

impl ExportFixture {
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

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    pub fn data_home(&self) -> &Path {
        &self.data_home
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

    pub fn write_model(&self, model_name: &str, providers: &[&str]) {
        let mut body = String::new();
        for provider in providers {
            body.push_str("[[providers]]\n");
            body.push_str(&format!("name = \"{provider}\"\n\n"));
        }
        fs::write(self.models_dir.join(format!("{model_name}.toml")), body).unwrap();
    }

    pub fn write_provider(
        &self,
        provider_name: &str,
        storage: StorageKind<'_>,
        resume: bool,
        quota_script_marker: Option<&Path>,
    ) {
        let resume_block = if resume {
            format!(
                r#"
[{provider_name}.resume]
kind = "flag"
flag = "--resume"
"#
            )
        } else {
            String::new()
        };
        let storage_block = match storage {
            StorageKind::ClaudeCode { projects_dir } => format!(
                r#"
[{provider_name}.session_storage]
kind = "claude_code"
projects_dir = "{}"
"#,
                projects_dir.display()
            ),
            StorageKind::Codex { sessions_dir } => format!(
                r#"
[{provider_name}.session_storage]
kind = "codex"
sessions_dir = "{}"
"#,
                sessions_dir.display()
            ),
            StorageKind::None => String::new(),
        };
        let quota_script = quota_script_marker
            .map(|path| format!("quota_script = \"printf touched > {}\"\n", path.display()))
            .unwrap_or_default();
        fs::create_dir_all(&self.app_config_dir).unwrap();
        let providers_path = self.app_config_dir.join("providers.toml");
        let mut body = if providers_path.exists() {
            fs::read_to_string(&providers_path).unwrap()
        } else {
            String::new()
        };
        body.push_str(&format!(
            r#"[{provider_name}]
command = "provider-command-that-must-not-run"
args = []
interactive_args = ["launch"]
prompt_mode = "arg"
{quota_script}{resume_block}{storage_block}
"#
        ));
        fs::write(providers_path, body).unwrap();
    }

    pub fn write_sessions_with_locator_path(&self, provider_name: &str, transcript_path: &Path) {
        let locator = self.write_script(
            &format!("{provider_name}-locator.sh"),
            &format!("printf '%s\\n' {}", sh_path(transcript_path)),
        );
        self.write_sessions_with_locator_script(provider_name, &locator.to_string_lossy());
    }

    pub fn write_sessions_with_locator_script(&self, provider_name: &str, locator: &str) {
        fs::create_dir_all(&self.app_config_dir).unwrap();
        let sessions_path = self.app_config_dir.join("sessions.toml");
        let mut body = if sessions_path.exists() {
            fs::read_to_string(&sessions_path).unwrap()
        } else {
            String::new()
        };
        body.push_str(&format!(
            "[{provider_name}]\nturn_script = \"true\"\ntranscript_locator = {:?}\nstate_dir = {:?}\n",
            locator,
            self.dir
                .path()
                .join(format!("{provider_name}-locator-state"))
                .to_string_lossy()
        ));
        fs::write(sessions_path, body).unwrap();
    }

    pub fn write_sessions_without_locator(&self, provider_name: &str) {
        fs::create_dir_all(&self.app_config_dir).unwrap();
        let sessions_path = self.app_config_dir.join("sessions.toml");
        let mut body = if sessions_path.exists() {
            fs::read_to_string(&sessions_path).unwrap()
        } else {
            String::new()
        };
        body.push_str(&format!(
            "[{provider_name}]\nturn_script = \"true\"\nstate_dir = {:?}\n",
            self.dir
                .path()
                .join(format!("{provider_name}-locator-state"))
                .to_string_lossy()
        ));
        fs::write(sessions_path, body).unwrap();
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

    pub fn seed_active_chain(
        &self,
        chain_id: &str,
        provider_name: &str,
        session_id: &str,
        model_name: &str,
        last_used_at: &str,
    ) {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, ?2, ?2, ?3)",
            params![chain_id, last_used_at, model_name],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, ?4, 'initial')",
            params![chain_id, provider_name, session_id, last_used_at],
        )
        .unwrap();
    }

    pub fn seed_provider_quota_exhausted(&self, provider_name: &str) {
        self.conn()
            .execute(
                "INSERT INTO provider_quotas
                    (provider_name, used_percent, resets_at, calls_since_refresh, refreshed_at, exhausted_at)
                 VALUES (?1, 1.0, '2026-04-18T08:00:00Z', 0, '2026-04-17T08:00:00Z', '2026-04-17T08:00:00Z')",
                params![provider_name],
            )
            .unwrap();
    }

    pub fn stage_jsonl(&self, name: &str, body: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        fs::write(&path, body).unwrap();
        path
    }

    pub fn stage_claude_jsonl(
        &self,
        projects_dir: &Path,
        workspace_root: &Path,
        session_id: &str,
        body: &str,
    ) -> PathBuf {
        fs::create_dir_all(workspace_root).unwrap();
        let transcript_dir = projects_dir.join(claude_project_dir_name(workspace_root));
        fs::create_dir_all(&transcript_dir).unwrap();
        let path = transcript_dir.join(format!("{session_id}.jsonl"));
        fs::write(&path, body).unwrap();
        path
    }

    pub fn snapshot_read_only_state(&self, transcript_path: &Path) -> ReadOnlySnapshot {
        let conn = self.conn();
        let mut table_counts = BTreeMap::new();
        for table in [
            "invocations",
            "session_turns",
            "session_chains",
            "session_chain_segments",
            "provider_quotas",
            "provider_quota_windows",
        ] {
            table_counts.insert(table.to_string(), table_count(&conn, table));
        }
        ReadOnlySnapshot {
            table_counts,
            transcript_bytes: fs::read(transcript_path).unwrap(),
            transcript_mtime: fs::metadata(transcript_path).unwrap().modified().unwrap(),
            providers_toml: read_optional(self.app_config_dir.join("providers.toml")),
            sessions_toml: read_optional(self.app_config_dir.join("sessions.toml")),
            model_toml: read_optional(self.models_dir.join(format!("{MODEL}.toml"))),
        }
    }

    pub fn run_export(&self, session_id: &str, extra_args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("session").arg("export").arg(session_id);
        cmd.args(extra_args);
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd.output().unwrap()
    }
}

pub fn cli_claude_export_fixture() -> PreparedExport {
    let fixture = ExportFixture::new();
    let projects_dir = fixture.root().join("claude-projects");
    fs::create_dir_all(&projects_dir).unwrap();
    let workspace_root = fixture.root().join("workspace");
    let jsonl_path = fixture.stage_claude_jsonl(
        &projects_dir,
        &workspace_root,
        SESSION_A,
        &format!("{CLAUDE_LINE_1}\n{CLAUDE_LINE_2}\n"),
    );
    fixture.write_model(MODEL, &[CLAUDE_PROVIDER]);
    fixture.write_provider(
        CLAUDE_PROVIDER,
        StorageKind::ClaudeCode {
            projects_dir: &projects_dir,
        },
        true,
        None,
    );
    fixture.write_sessions_with_locator_path(CLAUDE_PROVIDER, &jsonl_path);
    fixture.seed_active_chain(
        CHAIN_A,
        CLAUDE_PROVIDER,
        SESSION_A,
        MODEL,
        "2026-04-17T08:00:00Z",
    );
    PreparedExport {
        fixture,
        session_id: SESSION_A.to_string(),
        chain_id: CHAIN_A.to_string(),
        provider_name: CLAUDE_PROVIDER.to_string(),
        jsonl_path,
    }
}

pub fn cli_codex_export_fixture() -> PreparedExport {
    let fixture = ExportFixture::new();
    let sessions_dir = fixture.root().join("codex-sessions");
    let workspace_root = fixture.root().join("workspace");
    fs::create_dir_all(&sessions_dir).unwrap();
    fs::create_dir_all(&workspace_root).unwrap();
    let jsonl_path = fixture.stage_jsonl(
        "rollout-2026-04-17.jsonl",
        &format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{SESSION_A}\",\"cwd\":\"{}\"}}}}\n\
             {{\"type\":\"response_item\",\"timestamp\":\"2026-04-17T08:00:00Z\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"codex user\"}}]}}}}\n\
             {{\"type\":\"response_item\",\"timestamp\":\"2026-04-17T08:00:01Z\",\"payload\":{{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{{\"type\":\"output_text\",\"text\":\"codex assistant\"}}]}}}}\n",
            workspace_root.display()
        ),
    );
    fixture.write_model(MODEL, &[CODEX_PROVIDER]);
    fixture.write_provider(
        CODEX_PROVIDER,
        StorageKind::Codex {
            sessions_dir: &sessions_dir,
        },
        true,
        None,
    );
    fixture.write_sessions_with_locator_path(CODEX_PROVIDER, &jsonl_path);
    fixture.seed_active_chain(
        CHAIN_A,
        CODEX_PROVIDER,
        SESSION_A,
        MODEL,
        "2026-04-17T08:00:00Z",
    );
    PreparedExport {
        fixture,
        session_id: SESSION_A.to_string(),
        chain_id: CHAIN_A.to_string(),
        provider_name: CODEX_PROVIDER.to_string(),
        jsonl_path,
    }
}

pub fn cli_malformed_transcript_fixture() -> PreparedExport {
    let prepared = cli_claude_export_fixture();
    fs::write(
        &prepared.jsonl_path,
        format!("{CLAUDE_LINE_1}\n{{this is not json}}\n{CLAUDE_LINE_2}\n"),
    )
    .unwrap();
    prepared
}

pub fn cli_read_only_fixture() -> PreparedExport {
    let prepared = cli_claude_export_fixture();
    prepared
        .fixture
        .seed_provider_quota_exhausted(CLAUDE_PROVIDER);
    prepared
}

pub fn cli_missing_uuid_fixture() -> ExportFixture {
    let fixture = ExportFixture::new();
    let _ = fixture.open_db();
    fixture.write_model(MODEL, &[CLAUDE_PROVIDER]);
    fixture.write_provider(CLAUDE_PROVIDER, StorageKind::None, true, None);
    fixture.write_sessions_without_locator(CLAUDE_PROVIDER);
    fixture
}

pub fn cli_ambiguous_session_fixture() -> PreparedExport {
    let prepared = cli_claude_export_fixture();
    prepared
        .fixture
        .write_model(MODEL, &[CLAUDE_PROVIDER, "claude2"]);
    prepared.fixture.write_provider(
        "claude2",
        StorageKind::ClaudeCode {
            projects_dir: &prepared.fixture.root().join("claude-projects"),
        },
        true,
        None,
    );
    prepared
        .fixture
        .write_sessions_with_locator_path("claude2", &prepared.jsonl_path);
    let recent_a = (Utc::now() - Duration::hours(1)).to_rfc3339();
    let recent_b = (Utc::now() - Duration::hours(2)).to_rfc3339();
    prepared
        .fixture
        .conn()
        .execute(
            "UPDATE session_chains SET last_used_at = ?2 WHERE chain_id = ?1",
            params![CHAIN_A, recent_a],
        )
        .unwrap();
    prepared
        .fixture
        .seed_active_chain(CHAIN_B, "claude2", SESSION_A, MODEL, &recent_b);
    prepared
}

pub fn cli_unsupported_storage_fixture() -> PreparedExport {
    let fixture = ExportFixture::new();
    let jsonl_path = fixture.stage_jsonl("other-transcript.jsonl", "{}\n");
    fixture.write_model(MODEL, &[CLAUDE_PROVIDER]);
    fixture.write_provider(CLAUDE_PROVIDER, StorageKind::None, true, None);
    fixture.write_sessions_with_locator_path(CLAUDE_PROVIDER, &jsonl_path);
    fixture.seed_active_chain(
        CHAIN_A,
        CLAUDE_PROVIDER,
        SESSION_A,
        MODEL,
        "2026-04-17T08:00:00Z",
    );
    PreparedExport {
        fixture,
        session_id: SESSION_A.to_string(),
        chain_id: CHAIN_A.to_string(),
        provider_name: CLAUDE_PROVIDER.to_string(),
        jsonl_path,
    }
}

pub fn component_source_preimage_fixture() -> ComponentExportFixture {
    component_claude_fixture_with_body(&format!(
        "\n{CLAUDE_LINE_1}\r\n{CLAUDE_LINE_2}\n{CLAUDE_UNSUPPORTED_LINE}"
    ))
}

pub fn component_ordering_fixture() -> ComponentExportFixture {
    component_claude_fixture_with_body(&format!("{CLAUDE_LINE_1}\n{CLAUDE_LINE_2}\n"))
}

pub fn component_unsupported_record_fixture() -> ComponentExportFixture {
    component_claude_fixture_with_body(&format!("{CLAUDE_LINE_1}\n{CLAUDE_UNSUPPORTED_LINE}\n"))
}

pub fn component_compaction_fixture() -> ComponentExportFixture {
    let boundary = "{\"sessionId\":\"5169694d-de0f-40d1-890c-6e28e55bab27\",\"type\":\"assistant\",\"uuid\":\"compact-summary\",\"timestamp\":\"2026-04-17T08:00:01Z\",\"isCompactSummary\":true,\"message\":\"summary\"}";
    let after = "{\"sessionId\":\"5169694d-de0f-40d1-890c-6e28e55bab27\",\"type\":\"user\",\"uuid\":\"post-compact\",\"timestamp\":\"2026-04-17T08:00:02Z\",\"message\":\"after summary\"}";
    let fixture =
        component_claude_fixture_with_body(&format!("{CLAUDE_LINE_1}\n{boundary}\n{after}\n"));
    let db = StateDb::open(&fixture.jsonl_path.with_extension("db")).unwrap();
    db.ingest_session_turn(
        CLAUDE_PROVIDER,
        SESSION_A,
        "compact-summary",
        &chrono::DateTime::parse_from_rfc3339("2026-04-17T08:00:01Z")
            .unwrap()
            .with_timezone(&Utc),
        "assistant",
        fixture.jsonl_path.to_string_lossy().as_ref(),
    )
    .unwrap();
    db.flag_compaction_boundary(CLAUDE_PROVIDER, SESSION_A, "compact-summary")
        .unwrap();
    fixture
}

fn component_claude_fixture_with_body(body: &str) -> ComponentExportFixture {
    let dir = tempfile::tempdir().unwrap();
    let jsonl_path = dir.path().join("component-claude.jsonl");
    fs::write(&jsonl_path, body).unwrap();
    let metadata = ExportSessionMetadata {
        session_id: SESSION_A.to_string(),
        chain_id: CHAIN_A.to_string(),
        provider_name: CLAUDE_PROVIDER.to_string(),
        storage_type: SessionStorageType::ClaudeCode,
        jsonl_path: jsonl_path.clone(),
    };
    ComponentExportFixture {
        dir,
        metadata,
        jsonl_path,
    }
}

pub fn parse_stdout_jsonl(output: &Output) -> Vec<Value> {
    String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

pub fn parse_stderr_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stderr).unwrap()
}

pub fn assert_json_error(output: &Output, code: &str) -> Value {
    assert!(output.stdout.is_empty(), "{output:?}");
    let json = parse_stderr_json(output);
    assert_eq!(json["error"]["code"], code, "{json}");
    json
}

pub fn required_canonical_fields() -> [&'static str; 8] {
    [
        "session_id",
        "provider_name",
        "turn_id",
        "role",
        "timestamp",
        "content",
        "source",
        "unsupported_record",
    ]
}

pub fn assert_required_canonical_shape(record: &Value) {
    for field in required_canonical_fields() {
        assert!(record.get(field).is_some(), "missing {field} in {record}");
    }
    assert_eq!(
        record.as_object().unwrap().len(),
        required_canonical_fields().len(),
        "{record}"
    );
    let source = record["source"].as_object().unwrap();
    for field in [
        "storage_type",
        "jsonl_path",
        "line",
        "byte_start",
        "byte_end",
        "sha256",
    ] {
        assert!(
            source.get(field).is_some(),
            "missing source.{field} in {record}"
        );
    }
}

pub fn hardcoded_source_hashes() -> BTreeMap<&'static str, &'static str> {
    BTreeMap::from([
        (
            "claude-turn-1",
            "de06cc0e5c04c831de98afb535d66e7ddc7ac3ce22c46f6b290502a53d93e0af",
        ),
        (
            "claude-turn-2",
            "88c203cdb0b7eecd5c84a957b0a3fce47805d895a303ab1dcc1a7bc718e15a3d",
        ),
        (
            "claude-system-1",
            "d4a3d368ff0df8be2c774aff232500d8b88cacda9243953ea413a206a3f512d1",
        ),
    ])
}

fn read_optional(path: PathBuf) -> Option<Vec<u8>> {
    fs::read(path).ok()
}

fn table_count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .unwrap()
}

fn sh_path(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

fn claude_project_dir_name(path: &Path) -> String {
    let raw = path.to_string_lossy();
    format!("-{}", raw.trim_start_matches('/').replace('/', "-"))
}
