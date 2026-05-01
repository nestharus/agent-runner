#![cfg(unix)]
#![allow(dead_code)]

use agent_runner_lib::state::StateDb;
use chrono::{Duration, Utc};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output};

pub const SESSION_A: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
pub const SESSION_B: &str = "8f0a6a1f-9cd2-4c91-b6c6-1f0a0a8c9e22";
pub const CHAIN_A: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
pub const CHAIN_B: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
pub const MODEL: &str = "claude-opus";
pub const CLAUDE_PROVIDER: &str = "claude";
pub const CODEX_PROVIDER: &str = "codex";

pub struct PauseFixture {
    dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
}

pub struct PreparedPause {
    pub fixture: PauseFixture,
    pub session_id: String,
    pub chain_id: String,
    pub provider_name: String,
}

impl PauseFixture {
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

    pub fn locks_dir(&self) -> PathBuf {
        self.data_home.join("oulipoly-agent-runner").join("locks")
    }

    pub fn lock_path(&self, session_id: &str) -> PathBuf {
        self.locks_dir().join(format!("session-{session_id}.lock"))
    }

    pub fn release_marker_path(&self, session_id: &str) -> PathBuf {
        self.locks_dir()
            .join(format!("session-{session_id}.released"))
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

    pub fn write_provider_shells(&self, providers: &[&str]) {
        fs::create_dir_all(&self.app_config_dir).unwrap();
        let mut body = String::new();
        for provider in providers {
            body.push_str(&format!(
                r#"[{provider}]
command = "provider-command-that-must-not-run"
args = []
interactive_args = []
prompt_mode = "arg"

"#
            ));
        }
        fs::write(self.app_config_dir.join("providers.toml"), body).unwrap();
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

    pub fn run_pause(&self, session_id: &str, ttl_ms: Option<u64>) -> Output {
        let mut cmd = self.pause_command(session_id);
        if let Some(ttl_ms) = ttl_ms {
            cmd.arg("--ttl-ms").arg(ttl_ms.to_string());
        }
        cmd.output().unwrap()
    }

    pub fn run_pause_args(&self, session_id: &str, extra_args: &[&str]) -> Output {
        let mut cmd = self.pause_command(session_id);
        cmd.args(extra_args);
        cmd.output().unwrap()
    }

    pub fn spawn_pause(&self, session_id: &str, ttl_ms: Option<u64>) -> Child {
        let mut cmd = self.pause_command(session_id);
        if let Some(ttl_ms) = ttl_ms {
            cmd.arg("--ttl-ms").arg(ttl_ms.to_string());
        }
        cmd.stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        cmd.spawn().unwrap()
    }

    pub fn run_resume(&self, session_id: &str, token: &str) -> Output {
        self.resume_command(session_id, token).output().unwrap()
    }

    pub fn pause_command(&self, session_id: &str) -> Command {
        let mut cmd = base_command(&self.config_home, &self.data_home);
        cmd.arg("session").arg("pause-handshake").arg(session_id);
        cmd
    }

    pub fn resume_command(&self, session_id: &str, token: &str) -> Command {
        let mut cmd = base_command(&self.config_home, &self.data_home);
        cmd.arg("session")
            .arg("resume-handshake")
            .arg(session_id)
            .arg("--token")
            .arg(token);
        cmd
    }

    pub fn write_expired_lock(&self, session_id: &str) {
        fs::create_dir_all(self.locks_dir()).unwrap();
        let created_at = (Utc::now() - Duration::minutes(10)).to_rfc3339();
        let expires_at = (Utc::now() - Duration::minutes(5)).to_rfc3339();
        let body = json!({
            "version": 1,
            "session_id": session_id,
            "token_hash": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            "created_at": created_at,
            "expires_at": expires_at,
            "owner_pid": 0,
        });
        write_private_json(&self.lock_path(session_id), &body);
    }

    pub fn read_lock_json(&self, session_id: &str) -> Value {
        read_json_file(&self.lock_path(session_id))
    }

    pub fn read_release_marker_json(&self, session_id: &str) -> Value {
        read_json_file(&self.release_marker_path(session_id))
    }
}

pub fn prepared_single_session() -> PreparedPause {
    let fixture = PauseFixture::new();
    fixture.write_model(MODEL, &[CLAUDE_PROVIDER]);
    fixture.write_provider_shells(&[CLAUDE_PROVIDER]);
    fixture.seed_active_chain(
        CHAIN_A,
        CLAUDE_PROVIDER,
        SESSION_A,
        MODEL,
        "2026-04-17T08:00:00Z",
    );
    PreparedPause {
        fixture,
        session_id: SESSION_A.to_string(),
        chain_id: CHAIN_A.to_string(),
        provider_name: CLAUDE_PROVIDER.to_string(),
    }
}

pub fn prepared_two_sessions() -> PauseFixture {
    let fixture = PauseFixture::new();
    fixture.write_model(MODEL, &[CLAUDE_PROVIDER, CODEX_PROVIDER]);
    fixture.write_provider_shells(&[CLAUDE_PROVIDER, CODEX_PROVIDER]);
    fixture.seed_active_chain(
        CHAIN_A,
        CLAUDE_PROVIDER,
        SESSION_A,
        MODEL,
        "2026-04-17T08:00:00Z",
    );
    fixture.seed_active_chain(
        CHAIN_B,
        CODEX_PROVIDER,
        SESSION_B,
        MODEL,
        "2026-04-17T08:01:00Z",
    );
    fixture
}

pub fn missing_uuid_fixture() -> PauseFixture {
    let fixture = PauseFixture::new();
    let _ = fixture.open_db();
    fixture.write_model(MODEL, &[CLAUDE_PROVIDER]);
    fixture.write_provider_shells(&[CLAUDE_PROVIDER]);
    fixture
}

pub fn invalid_uuid_fixture() -> PauseFixture {
    PauseFixture::new()
}

pub fn ambiguous_session_fixture() -> PauseFixture {
    let fixture = PauseFixture::new();
    fixture.write_model(MODEL, &[CLAUDE_PROVIDER, CODEX_PROVIDER]);
    fixture.write_provider_shells(&[CLAUDE_PROVIDER, CODEX_PROVIDER]);
    fixture.seed_active_chain(
        CHAIN_A,
        CLAUDE_PROVIDER,
        SESSION_A,
        MODEL,
        &Utc::now().to_rfc3339(),
    );
    fixture.seed_active_chain(
        CHAIN_B,
        CODEX_PROVIDER,
        SESSION_A,
        MODEL,
        &(Utc::now() - Duration::minutes(5)).to_rfc3339(),
    );
    fixture
}

pub fn model_resolution_failure_fixture() -> PauseFixture {
    let fixture = PauseFixture::new();
    fixture.write_provider_shells(&[CLAUDE_PROVIDER]);
    fixture.seed_active_chain(
        CHAIN_A,
        CLAUDE_PROVIDER,
        SESSION_A,
        "missing-model",
        "2026-04-17T08:00:00Z",
    );
    fixture
}

pub fn collect_outputs(children: Vec<Child>) -> Vec<Output> {
    children
        .into_iter()
        .map(|child| child.wait_with_output().unwrap())
        .collect()
}

pub fn parse_stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap()
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

pub fn assert_pause_token(value: &Value) {
    let token = value["token"].as_str().unwrap();
    assert!(token.starts_with("pause_"), "{token}");
    let hex = &token["pause_".len()..];
    assert_eq!(hex.len(), 32, "{token}");
    assert!(
        hex.chars()
            .all(|ch| ch.is_ascii_digit() || ('a'..='f').contains(&ch)),
        "{token}"
    );
}

pub fn required_pause_fields() -> [&'static str; 6] {
    [
        "session_id",
        "chain_id",
        "provider_name",
        "token",
        "expires_at",
        "lock_path",
    ]
}

pub fn required_resume_fields() -> [&'static str; 4] {
    ["session_id", "token", "released_at", "already_released"]
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

fn read_json_file(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}
