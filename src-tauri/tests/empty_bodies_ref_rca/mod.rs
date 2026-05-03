#![allow(dead_code)]

use agent_runner_lib::state::StateDb;
use rusqlite::{Connection, params};
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub mod rc1_schema_contract;
pub mod rc2_ingest_body_payload;
pub mod rc3_export_db_source;
pub mod rc4_trace_inline_transcript;

pub const PROVIDER: &str = "codex";
pub const MODEL: &str = "codex-high";
pub const SESSION_ID: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
pub const CHAIN_ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
pub const INVOCATION_UUID: &str = "11111111-1111-4111-8111-111111111111";
pub const TS_USER: &str = "2026-04-17T08:00:00Z";
pub const TS_ASSISTANT: &str = "2026-04-17T08:00:01Z";

pub struct RcaFixture {
    dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
}

impl RcaFixture {
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

    pub fn session_turns_columns(&self) -> Vec<String> {
        let conn = self.conn();
        let mut stmt = conn.prepare("PRAGMA table_info(session_turns)").unwrap();
        stmt.query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    pub fn seed_chain(&self) {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, ?2, ?3, ?4)",
            params![CHAIN_ID, TS_USER, TS_ASSISTANT, MODEL],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, ?4, 'initial')",
            params![CHAIN_ID, PROVIDER, SESSION_ID, TS_USER],
        )
        .unwrap();
    }

    pub fn seed_body_turns(&self) {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO session_turns
                (provider_name, session_id, turn_id, timestamp, role,
                 parent_turn_id, is_sidechain, is_compaction_boundary, source_file, ingested_at, body)
             VALUES (?1, ?2, 'turn-user', ?3, 'user', NULL, 0, 0, '', ?3, ?4)",
            params![
                PROVIDER,
                SESSION_ID,
                TS_USER,
                r#"[{"type":"text","text":"db stored user body"}]"#
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_turns
                (provider_name, session_id, turn_id, timestamp, role,
                 parent_turn_id, is_sidechain, is_compaction_boundary, source_file, ingested_at, body)
             VALUES (?1, ?2, 'turn-assistant', ?3, 'assistant', 'turn-user', 0, 0, '', ?3, ?4)",
            params![
                PROVIDER,
                SESSION_ID,
                TS_ASSISTANT,
                r#"[{"type":"text","text":"db stored assistant body"}]"#
            ],
        )
        .unwrap();
    }

    pub fn fetch_turn_body(&self, turn_id: &str) -> Result<Value, String> {
        let raw: String = self
            .conn()
            .query_row(
                "SELECT body FROM session_turns
                 WHERE provider_name = ?1 AND session_id = ?2 AND turn_id = ?3",
                params![PROVIDER, SESSION_ID, turn_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("body column must be queryable from session_turns: {e}"))?;
        serde_json::from_str(&raw).map_err(|e| format!("body JSON must parse: {e}"))
    }

    pub fn write_cli_config_with_missing_transcript(&self) {
        fs::create_dir_all(&self.models_dir).unwrap();
        fs::write(
            self.models_dir.join(format!("{MODEL}.toml")),
            format!("[[providers]]\nname = \"{PROVIDER}\"\n"),
        )
        .unwrap();
        fs::write(
            self.app_config_dir.join("providers.toml"),
            format!(
                r#"[{PROVIDER}]
command = "provider-command-that-must-not-run"
args = []
interactive_args = ["launch"]
prompt_mode = "arg"

[{PROVIDER}.resume]
kind = "subcommand"
subcommand = ["resume"]

[{PROVIDER}.session_storage]
kind = "codex"
sessions_dir = "{}"
"#,
                self.root().join("missing-sessions-dir").display()
            ),
        )
        .unwrap();
        let missing_jsonl = self.root().join("missing-rollout.jsonl");
        let locator = self.write_script(
            "missing-transcript-locator.sh",
            &format!("printf '%s\\n' {}", sh_path(&missing_jsonl)),
        );
        fs::write(
            self.app_config_dir.join("sessions.toml"),
            format!(
                "[{PROVIDER}]\nturn_script = \"true\"\ntranscript_locator = {:?}\nstate_dir = {:?}\n",
                locator.to_string_lossy(),
                self.root().join("locator-state").to_string_lossy()
            ),
        )
        .unwrap();
    }

    pub fn write_script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.root().join(name);
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

    pub fn run_export(&self) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("session").arg("export").arg(SESSION_ID);
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd.output().unwrap()
    }
}

pub fn sh_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    format!("'{}'", s.replace('\'', "'\\''"))
}
