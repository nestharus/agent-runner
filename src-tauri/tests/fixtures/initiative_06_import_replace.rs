#![cfg(unix)]
#![allow(dead_code)]

use agent_runner_session::{Lease, SessionLock};
use agent_runner_state::StateDb;
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

pub const SESSION_A: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
pub const SESSION_B: &str = "8f0a6a1f-9cd2-4c91-b6c6-1f0a0a8c9e22";
pub const CHAIN_A: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
pub const CHAIN_B: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
pub const MODEL: &str = "claude-opus";
pub const CLAUDE_PROVIDER: &str = "claude";
pub const CODEX_PROVIDER: &str = "codex";

pub const TEST_HOOK_ENV: &str = "OULIPOLY_IMPORT_REPLACE_TEST_HOOK";
pub const TEST_SLEEP_AFTER_LOCK_MS: &str = "sleep-after-lock-ms";
pub const TEST_BLOCK_AFTER_RENAME: &str = "block-after-transcript-rename-before-db-commit";
pub const TEST_FAIL_POSTIMAGE_VERIFY: &str = "fail-postimage-verification";

#[derive(Clone, Copy)]
pub enum StorageKind<'a> {
    ClaudeCode { projects_dir: &'a Path },
    Codex { sessions_dir: &'a Path },
    Other,
}

pub struct ImportReplaceFixture {
    dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
}

pub struct PreparedReplace {
    pub fixture: ImportReplaceFixture,
    pub session_id: String,
    pub chain_id: String,
    pub provider_name: String,
    pub jsonl_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationSnapshot {
    pub transcript_bytes: Vec<u8>,
    pub turn_rows: Vec<TurnRow>,
    pub journal_files: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnRow {
    pub provider_name: String,
    pub session_id: String,
    pub turn_id: String,
    pub timestamp: String,
    pub role: String,
    pub parent_turn_id: Option<String>,
    pub is_sidechain: i64,
    pub is_compaction_boundary: i64,
}

impl ImportReplaceFixture {
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

    pub fn replace_journal_dir(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("replace_journal")
    }

    pub fn staging_dir(&self) -> PathBuf {
        self.replace_journal_dir().join("staging")
    }

    pub fn quarantine_dir(&self) -> PathBuf {
        self.replace_journal_dir().join("quarantine")
    }

    pub fn pending_journal_path(&self, session_id: &str) -> PathBuf {
        self.replace_journal_dir()
            .join(format!("session-{session_id}.pending"))
    }

    pub fn canonical_records_path(&self, session_id: &str) -> PathBuf {
        self.replace_journal_dir()
            .join(format!("session-{session_id}.canonical.jsonl"))
    }

    pub fn models_dir(&self) -> &Path {
        &self.models_dir
    }

    pub fn providers_path(&self) -> PathBuf {
        self.app_config_dir.join("providers.toml")
    }

    pub fn sessions_path(&self) -> PathBuf {
        self.app_config_dir.join("sessions.toml")
    }

    pub fn locks_dir(&self) -> PathBuf {
        self.data_home.join("oulipoly-agent-runner").join("locks")
    }

    pub fn lock_path(&self, session_id: &str) -> PathBuf {
        self.locks_dir().join(format!("session-{session_id}.lock"))
    }

    pub fn open_db(&self) -> StateDb {
        StateDb::open(&self.db_path()).unwrap()
    }

    pub fn conn(&self) -> Connection {
        // Initialize the schema before opening a raw rusqlite connection.
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

    pub fn write_provider(&self, provider_name: &str, storage: StorageKind<'_>) {
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
            StorageKind::Other => String::new(),
        };
        fs::create_dir_all(&self.app_config_dir).unwrap();
        let providers_path = self.app_config_dir.join("providers.toml");
        let mut body = fs::read_to_string(&providers_path).unwrap_or_default();
        body.push_str(&format!(
            r#"[{provider_name}]
command = "provider-command-that-must-not-run"
args = []
interactive_args = ["launch"]
prompt_mode = "arg"

[{provider_name}.resume]
kind = "flag"
flag = "--resume"
{storage_block}
"#
        ));
        fs::write(providers_path, body).unwrap();
    }

    pub fn write_sessions_with_locator_path(&self, provider_name: &str, transcript_path: &Path) {
        let locator = self.write_script(
            &format!("{provider_name}-locator.sh"),
            &format!("printf '%s\\n' {}", sh_path(transcript_path)),
        );
        self.write_sessions_with_locator_command(provider_name, &locator.to_string_lossy());
    }

    pub fn write_sessions_with_locator_body(&self, provider_name: &str, body: &str) {
        let locator = self.write_script(&format!("{provider_name}-locator.sh"), body);
        self.write_sessions_with_locator_command(provider_name, &locator.to_string_lossy());
    }

    fn write_sessions_with_locator_command(&self, provider_name: &str, locator: &str) {
        fs::create_dir_all(&self.app_config_dir).unwrap();
        let sessions_path = self.app_config_dir.join("sessions.toml");
        let mut body = fs::read_to_string(&sessions_path).unwrap_or_default();
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
                (chain_id, provider_name, session_id, started_at, last_turn_id, transition_reason)
             VALUES (?1, ?2, ?3, ?4, 'old-turn-2', 'initial')",
            params![chain_id, provider_name, session_id, last_used_at],
        )
        .unwrap();
    }

    pub fn seed_turns_with_metadata(
        &self,
        provider_name: &str,
        session_id: &str,
        source_file: &Path,
    ) {
        let conn = self.conn();
        for (turn_id, role, offset) in [
            ("old-turn-1", "user", 0_i64),
            ("old-turn-2", "assistant", 1_i64),
        ] {
            conn.execute(
                "INSERT INTO session_turns
                    (provider_name, session_id, turn_id, timestamp, role,
                     parent_turn_id, is_sidechain, is_compaction_boundary, source_file, ingested_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'root-turn', 1, 1, ?6, ?4)",
                params![
                    provider_name,
                    session_id,
                    turn_id,
                    format!("2026-04-17T08:00:{offset:02}Z"),
                    role,
                    source_file.to_string_lossy(),
                ],
            )
            .unwrap();
        }
    }

    pub fn write_active_lock(&self, provider_name: &str, session_id: &str) -> Lease {
        let lock = SessionLock::new(&self.locks_dir()).unwrap();
        lock.acquire(session_id, provider_name, Duration::from_secs(300))
            .unwrap()
    }

    pub fn run_import_replace(&self, session_id: &str, input: &str, extra_args: &[&str]) -> Output {
        self.run_import_replace_bytes(session_id, input.as_bytes(), extra_args)
    }

    pub fn run_import_replace_bytes(
        &self,
        session_id: &str,
        input: &[u8],
        extra_args: &[&str],
    ) -> Output {
        let mut cmd = self.import_replace_command(session_id);
        cmd.args(extra_args);
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input)
            .expect("failed to write stdin for import-replace");
        child.wait_with_output().unwrap()
    }

    pub fn run_import_replace_from_file(
        &self,
        session_id: &str,
        input_path: &Path,
        extra_args: &[&str],
    ) -> Output {
        let mut args = vec![
            "--from-file".to_string(),
            input_path.to_string_lossy().to_string(),
        ];
        args.extend(extra_args.iter().map(|arg| arg.to_string()));
        let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
        let mut cmd = self.import_replace_command(session_id);
        cmd.args(refs);
        cmd.output().unwrap()
    }

    pub fn spawn_import_replace(
        &self,
        session_id: &str,
        input: &str,
        extra_args: &[&str],
        envs: &[(&str, &str)],
    ) -> Child {
        let mut cmd = self.import_replace_command(session_id);
        cmd.args(extra_args);
        for (key, value) in envs {
            cmd.env(key, value);
        }
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .expect("failed to write stdin for import-replace");
        child
    }

    pub fn run_export(&self, session_id: &str) -> Output {
        let mut cmd = base_command(&self.config_home, &self.data_home);
        cmd.arg("session").arg("export").arg(session_id);
        cmd.output().unwrap()
    }

    pub fn run_pause_handshake(&self, session_id: &str) -> Output {
        let mut cmd = base_command(&self.config_home, &self.data_home);
        cmd.arg("session").arg("pause-handshake").arg(session_id);
        cmd.output().unwrap()
    }

    pub fn run_resume_handshake(&self, session_id: &str, token: &str) -> Output {
        let mut cmd = base_command(&self.config_home, &self.data_home);
        cmd.arg("session")
            .arg("resume-handshake")
            .arg(session_id)
            .arg("--token")
            .arg(token);
        cmd.output().unwrap()
    }

    pub fn run_recovery_trigger(&self) -> Output {
        let mut cmd = base_command(&self.config_home, &self.data_home);
        cmd.arg("session").arg("export").arg(SESSION_A);
        cmd.output().unwrap()
    }

    pub fn import_replace_command(&self, session_id: &str) -> Command {
        let mut cmd = base_command(&self.config_home, &self.data_home);
        cmd.arg("session").arg("import-replace").arg(session_id);
        cmd
    }

    pub fn mutation_snapshot(
        &self,
        transcript_path: &Path,
        provider_name: &str,
        session_id: &str,
    ) -> MutationSnapshot {
        MutationSnapshot {
            transcript_bytes: fs::read(transcript_path).unwrap(),
            turn_rows: self.turn_rows(provider_name, session_id),
            journal_files: list_regular_files(&self.replace_journal_dir()),
        }
    }

    pub fn turn_rows(&self, provider_name: &str, session_id: &str) -> Vec<TurnRow> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                "SELECT provider_name, session_id, turn_id, timestamp, role,
                        parent_turn_id, is_sidechain, is_compaction_boundary
                 FROM session_turns
                 WHERE provider_name = ?1 AND session_id = ?2
                 ORDER BY timestamp, turn_id",
            )
            .unwrap();
        stmt.query_map(params![provider_name, session_id], |row| {
            Ok(TurnRow {
                provider_name: row.get(0)?,
                session_id: row.get(1)?,
                turn_id: row.get(2)?,
                timestamp: row.get(3)?,
                role: row.get(4)?,
                parent_turn_id: row.get(5)?,
                is_sidechain: row.get(6)?,
                is_compaction_boundary: row.get(7)?,
            })
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
    }

    pub fn segment_state(&self, chain_id: &str) -> Value {
        let conn = self.conn();
        conn.query_row(
            "SELECT provider_name, session_id, last_turn_id, ended_at
             FROM session_chain_segments
             WHERE chain_id = ?1",
            params![chain_id],
            |row| {
                Ok(json!({
                    "provider_name": row.get::<_, String>(0)?,
                    "session_id": row.get::<_, String>(1)?,
                    "last_turn_id": row.get::<_, Option<String>>(2)?,
                    "ended_at": row.get::<_, Option<String>>(3)?,
                }))
            },
        )
        .unwrap()
    }

    pub fn active_segment_id(&self, chain_id: &str) -> i64 {
        let conn = self.conn();
        conn.query_row(
            "SELECT id FROM session_chain_segments WHERE chain_id = ?1 AND ended_at IS NULL",
            params![chain_id],
            |row| row.get(0),
        )
        .unwrap()
    }

    pub fn chain_last_used_at(&self, chain_id: &str) -> String {
        let conn = self.conn();
        conn.query_row(
            "SELECT last_used_at FROM session_chains WHERE chain_id = ?1",
            params![chain_id],
            |row| row.get(0),
        )
        .unwrap()
    }
}

pub fn prepared_claude_replace_fixture() -> PreparedReplace {
    let fixture = ImportReplaceFixture::new();
    let projects_dir = fixture.root().join("claude-projects");
    let workspace_root = fixture.root().join("workspace");
    let jsonl_path = fixture.stage_claude_jsonl(
        &projects_dir,
        &workspace_root,
        SESSION_A,
        &format!(
            "{}\n{}\n",
            claude_native_line(SESSION_A, "old-turn-1", "user", "old user", 0),
            claude_native_line(SESSION_A, "old-turn-2", "assistant", "old assistant", 1)
        ),
    );
    fixture.write_model(MODEL, &[CLAUDE_PROVIDER]);
    fixture.write_provider(
        CLAUDE_PROVIDER,
        StorageKind::ClaudeCode {
            projects_dir: &projects_dir,
        },
    );
    fixture.write_sessions_with_locator_path(CLAUDE_PROVIDER, &jsonl_path);
    fixture.seed_active_chain(
        CHAIN_A,
        CLAUDE_PROVIDER,
        SESSION_A,
        MODEL,
        "2026-04-17T08:00:00Z",
    );
    fixture.seed_turns_with_metadata(CLAUDE_PROVIDER, SESSION_A, &jsonl_path);
    PreparedReplace {
        fixture,
        session_id: SESSION_A.to_string(),
        chain_id: CHAIN_A.to_string(),
        provider_name: CLAUDE_PROVIDER.to_string(),
        jsonl_path,
    }
}

pub fn prepared_codex_replace_fixture() -> PreparedReplace {
    let fixture = ImportReplaceFixture::new();
    let sessions_dir = fixture.root().join("codex-sessions");
    fs::create_dir_all(&sessions_dir).unwrap();
    let workspace_root = fixture.root().join("workspace");
    fs::create_dir_all(&workspace_root).unwrap();
    let jsonl_path = fixture.stage_jsonl(
        "rollout-2026-04-17.jsonl",
        &format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{SESSION_A}\",\"cwd\":\"{}\"}}}}\n\
             {{\"type\":\"response_item\",\"timestamp\":\"2026-04-17T08:00:00Z\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"old codex user\"}}]}}}}\n\
             {{\"type\":\"response_item\",\"timestamp\":\"2026-04-17T08:00:01Z\",\"payload\":{{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{{\"type\":\"output_text\",\"text\":\"old codex assistant\"}}]}}}}\n",
            workspace_root.display()
        ),
    );
    fixture.write_model(MODEL, &[CODEX_PROVIDER]);
    fixture.write_provider(
        CODEX_PROVIDER,
        StorageKind::Codex {
            sessions_dir: &sessions_dir,
        },
    );
    fixture.write_sessions_with_locator_path(CODEX_PROVIDER, &jsonl_path);
    fixture.seed_active_chain(
        CHAIN_A,
        CODEX_PROVIDER,
        SESSION_A,
        MODEL,
        "2026-04-17T08:00:00Z",
    );
    fixture.seed_turns_with_metadata(CODEX_PROVIDER, SESSION_A, &jsonl_path);
    PreparedReplace {
        fixture,
        session_id: SESSION_A.to_string(),
        chain_id: CHAIN_A.to_string(),
        provider_name: CODEX_PROVIDER.to_string(),
        jsonl_path,
    }
}

pub fn prepared_other_storage_fixture() -> PreparedReplace {
    let fixture = ImportReplaceFixture::new();
    let jsonl_path = fixture.stage_jsonl("other-transcript.jsonl", "{}\n");
    fixture.write_model(MODEL, &[CLAUDE_PROVIDER]);
    fixture.write_provider(CLAUDE_PROVIDER, StorageKind::Other);
    fixture.write_sessions_with_locator_path(CLAUDE_PROVIDER, &jsonl_path);
    fixture.seed_active_chain(
        CHAIN_A,
        CLAUDE_PROVIDER,
        SESSION_A,
        MODEL,
        "2026-04-17T08:00:00Z",
    );
    fixture.seed_turns_with_metadata(CLAUDE_PROVIDER, SESSION_A, &jsonl_path);
    PreparedReplace {
        fixture,
        session_id: SESSION_A.to_string(),
        chain_id: CHAIN_A.to_string(),
        provider_name: CLAUDE_PROVIDER.to_string(),
        jsonl_path,
    }
}

pub fn missing_uuid_fixture() -> ImportReplaceFixture {
    let fixture = ImportReplaceFixture::new();
    let projects_dir = fixture.root().join("claude-projects");
    let workspace_root = fixture.root().join("workspace");
    let jsonl_path = fixture.stage_claude_jsonl(
        &projects_dir,
        &workspace_root,
        SESSION_A,
        &format!(
            "{}\n",
            claude_native_line(SESSION_A, "old-turn-1", "user", "old user", 0)
        ),
    );
    let _ = fixture.open_db();
    fixture.write_model(MODEL, &[CLAUDE_PROVIDER]);
    fixture.write_provider(
        CLAUDE_PROVIDER,
        StorageKind::ClaudeCode {
            projects_dir: &projects_dir,
        },
    );
    fixture.write_sessions_with_locator_path(CLAUDE_PROVIDER, &jsonl_path);
    fixture
}

pub fn ambiguous_session_fixture() -> ImportReplaceFixture {
    let fixture = ImportReplaceFixture::new();
    let projects_dir = fixture.root().join("claude-projects");
    let workspace_root = fixture.root().join("workspace");
    let jsonl_path = fixture.stage_claude_jsonl(
        &projects_dir,
        &workspace_root,
        SESSION_A,
        &format!(
            "{}\n",
            claude_native_line(SESSION_A, "old-turn-1", "user", "old user", 0)
        ),
    );
    fixture.write_model(MODEL, &[CLAUDE_PROVIDER, "claude2"]);
    for provider in [CLAUDE_PROVIDER, "claude2"] {
        fixture.write_provider(
            provider,
            StorageKind::ClaudeCode {
                projects_dir: &projects_dir,
            },
        );
        fixture.write_sessions_with_locator_path(provider, &jsonl_path);
    }
    let recent = "2099-04-17T08:00:00Z";
    let older = "2099-04-17T07:55:00Z";
    fixture.seed_active_chain(CHAIN_A, CLAUDE_PROVIDER, SESSION_A, MODEL, recent);
    fixture.seed_active_chain(CHAIN_B, "claude2", SESSION_A, MODEL, older);
    fixture
}

pub fn canonical_jsonl(
    session_id: &str,
    provider_name: &str,
    jsonl_path: &Path,
    prefix: &str,
) -> String {
    let records = [
        canonical_record(
            session_id,
            provider_name,
            jsonl_path,
            &format!("{prefix}-turn-1"),
            "user",
            "2026-04-17T09:00:00Z",
            &format!("{prefix} user"),
            1,
        ),
        canonical_record(
            session_id,
            provider_name,
            jsonl_path,
            &format!("{prefix}-turn-2"),
            "assistant",
            "2026-04-17T09:00:01Z",
            &format!("{prefix} assistant"),
            2,
        ),
    ];
    records
        .into_iter()
        .map(|record| serde_json::to_string(&record).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

pub fn malformed_canonical_jsonl() -> &'static str {
    "{\"session_id\":\"5169694d-de0f-40d1-890c-6e28e55bab27\"\n"
}

pub fn unsupported_record_only_jsonl(
    session_id: &str,
    provider_name: &str,
    jsonl_path: &Path,
) -> String {
    serde_json::to_string(&json!({
        "session_id": session_id,
        "provider_name": provider_name,
        "turn_id": "unsupported-only",
        "role": "system",
        "timestamp": "2026-04-17T09:00:00Z",
        "content": [],
        "source": source_json("other", jsonl_path, 1),
        "unsupported_record": true,
    }))
    .unwrap()
        + "\n"
}

pub fn parse_stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap()
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

pub fn assert_success(output: &Output) -> Value {
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    parse_stdout_json(output)
}

pub fn assert_success_allowing_test_hook_stderr(output: &Output) -> Value {
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    parse_stdout_json(output)
}

pub fn required_receipt_fields() -> [&'static str; 8] {
    [
        "session_id",
        "provider_name",
        "storage_type",
        "operation",
        "preimage_sha256",
        "postimage_sha256",
        "jsonl_path",
        "state_updated",
    ]
}

pub fn assert_receipt_shape(
    receipt: &Value,
    session_id: &str,
    provider_name: &str,
    storage_type: &str,
    jsonl_path: &Path,
) {
    for field in required_receipt_fields() {
        assert!(receipt.get(field).is_some(), "missing {field} in {receipt}");
    }
    assert_eq!(receipt["session_id"], session_id);
    assert_eq!(receipt["provider_name"], provider_name);
    assert_eq!(receipt["storage_type"], storage_type);
    assert_eq!(receipt["operation"], "import-replace");
    assert_eq!(receipt["jsonl_path"], jsonl_path.to_string_lossy().as_ref());
    assert_eq!(receipt["state_updated"], true);
    assert_hash_hex(receipt["preimage_sha256"].as_str().unwrap());
    assert_hash_hex(receipt["postimage_sha256"].as_str().unwrap());
    assert!(receipt["committed_at"].as_str().is_some(), "{receipt}");
}

pub fn assert_hash_hex(value: &str) {
    assert_eq!(value.len(), 64, "{value}");
    assert!(value.chars().all(|ch| ch.is_ascii_hexdigit()), "{value}");
}

pub fn assert_no_replace_journal_pollution(fixture: &ImportReplaceFixture, session_id: &str) {
    assert!(
        !fixture.pending_journal_path(session_id).exists(),
        "{:?}",
        fixture.pending_journal_path(session_id)
    );
    assert!(
        !fixture.canonical_records_path(session_id).exists(),
        "{:?}",
        fixture.canonical_records_path(session_id)
    );
    let staging = list_regular_files(&fixture.staging_dir());
    assert!(staging.is_empty(), "{staging:?}");
}

pub fn assert_export_matches_canonical(
    fixture: &ImportReplaceFixture,
    session_id: &str,
    expected: &str,
) {
    let output = fixture.run_export(session_id);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        normalize_jsonl(&String::from_utf8(output.stdout).unwrap()),
        normalize_jsonl(expected)
    );
}

pub fn assert_export_semantics_match_canonical(
    fixture: &ImportReplaceFixture,
    session_id: &str,
    expected: &str,
) {
    let output = fixture.run_export(session_id);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let mut actual = normalize_jsonl(&String::from_utf8(output.stdout).unwrap());
    let mut expected = normalize_jsonl(expected);
    for record in actual.iter_mut().chain(expected.iter_mut()) {
        record.as_object_mut().unwrap().remove("source");
    }
    assert_eq!(actual, expected);
}

pub fn normalize_jsonl(input: &str) -> Vec<Value> {
    input
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

pub fn sha256sum_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn collect_outputs(children: Vec<Child>) -> Vec<Output> {
    children
        .into_iter()
        .map(|child| child.wait_with_output().unwrap())
        .collect()
}

pub fn wait_for_test_hook_line(child: &mut Child, phase: &str) {
    let stderr = child.stderr.take().unwrap();
    let mut reader = BufReader::new(stderr);
    let marker = format!("import-replace-test-hook:{phase}");
    let mut line = String::new();
    loop {
        line.clear();
        let bytes = reader.read_line(&mut line).unwrap();
        assert!(bytes > 0, "child exited before emitting {marker}");
        if line.contains(&marker) {
            break;
        }
    }
}

/// Crash-recovery tests use a deterministic test hook instead of a timing-only
/// race. Step 6c should print `import-replace-test-hook:<phase>` to stderr and
/// block when `OULIPOLY_IMPORT_REPLACE_TEST_HOOK` equals the requested phase.
/// The test then sends SIGKILL through `Child::kill()` after seeing that line.
pub fn kill_after_test_hook(mut child: Child, phase: &str) -> Output {
    // stderr is consumed up to the hook marker; callers use the returned Output
    // for exit status only.
    let stderr = child.stderr.take().unwrap();
    let mut reader = BufReader::new(stderr);
    let marker = format!("import-replace-test-hook:{phase}");
    let mut line = String::new();
    loop {
        line.clear();
        let bytes = reader.read_line(&mut line).unwrap();
        assert!(bytes > 0, "child exited before emitting {marker}");
        if line.contains(&marker) {
            break;
        }
    }
    child.kill().unwrap();
    child.wait_with_output().unwrap()
}

#[allow(clippy::too_many_arguments)]
fn canonical_record(
    session_id: &str,
    provider_name: &str,
    jsonl_path: &Path,
    turn_id: &str,
    role: &str,
    timestamp: &str,
    text: &str,
    line: u64,
) -> Value {
    json!({
        "session_id": session_id,
        "provider_name": provider_name,
        "turn_id": turn_id,
        "role": role,
        "timestamp": timestamp,
        "content": [{"type": "text", "text": text}],
        "source": source_json("canonical_jsonl", jsonl_path, line),
        "unsupported_record": false,
    })
}

fn source_json(storage_type: &str, jsonl_path: &Path, line: u64) -> Value {
    // Tests that use this helper compare canonical semantics after stripping
    // source; byte_start, byte_end, and sha256 are dummy provenance values.
    json!({
        "storage_type": storage_type,
        "jsonl_path": jsonl_path,
        "line": line,
        "byte_start": (line - 1) * 100,
        "byte_end": line * 100,
        "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
    })
}

fn claude_native_line(
    session_id: &str,
    turn_id: &str,
    role: &str,
    message: &str,
    offset: i64,
) -> String {
    json!({
        "sessionId": session_id,
        "type": role,
        "uuid": turn_id,
        "timestamp": format!("2026-04-17T08:00:{offset:02}Z"),
        "message": message,
    })
    .to_string()
}

fn base_command(config_home: &Path, data_home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
    cmd.env("XDG_CONFIG_HOME", config_home);
    cmd.env("XDG_DATA_HOME", data_home);
    cmd.env("HOME", data_home);
    cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
    cmd
}

fn write_private_json(path: &Path, value: &Value) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o600);
    fs::set_permissions(path, perms).unwrap();
}

fn list_regular_files(path: &Path) -> Vec<PathBuf> {
    if !path.exists() {
        return Vec::new();
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(list_regular_files(&path));
        } else {
            files.push(path);
        }
    }
    files.sort();
    files
}

fn sh_path(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

fn claude_project_dir_name(path: &Path) -> String {
    let raw = path.to_string_lossy();
    format!("-{}", raw.trim_start_matches('/').replace('/', "-"))
}
