#![cfg(unix)]
#![allow(dead_code)]

use agent_runner_config::{
    ModelConfig, PromptMode, ProviderConfig, ProvidersConfig, SessionStorage, SessionsConfig,
};
use agent_runner_state::{ModelStore, StateDb};
use chrono::{Duration, Utc};
use rusqlite::{Connection, params};
use serde_json::Value;
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub const SESSION_A: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
pub const SESSION_B: &str = "8f0a6a1f-9cd2-4c91-b6c6-1f0a0a8c9e22";
pub const CHAIN_A: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
pub const CHAIN_B: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
pub const MODEL: &str = "claude-opus";
pub const CLAUDE_PROVIDER: &str = "claude";
pub const CODEX_PROVIDER: &str = "codex";

pub struct LocateFixture {
    dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
}

pub struct PreparedLocate {
    pub fixture: LocateFixture,
    pub session_id: String,
    pub chain_id: String,
    pub provider_name: String,
    pub workspace_root: PathBuf,
    pub jsonl_path: PathBuf,
}

#[derive(Clone, Copy)]
pub enum StorageKind<'a> {
    ClaudeCode { projects_dir: &'a Path },
    Codex { sessions_dir: &'a Path },
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadOnlySnapshot {
    pub invocations: i64,
    pub session_turns: i64,
    pub session_chains: i64,
    pub session_chain_segments: i64,
    pub provider_quotas: i64,
    pub transcript_mtime: std::time::SystemTime,
}

impl LocateFixture {
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

    pub fn models(&self) -> ModelStore {
        let mut models = HashMap::new();
        for (model, providers) in self.model_specs_from_disk() {
            models.insert(
                model.clone(),
                ModelConfig {
                    name: model,
                    prompt_mode: PromptMode::Arg,
                    providers: providers
                        .into_iter()
                        .map(|name| ProviderConfig::model_provider(name, Vec::new()))
                        .collect(),
                    inputs: Vec::new(),
                },
            );
        }
        models
    }

    pub fn providers_config(&self) -> ProvidersConfig {
        ProvidersConfig::load(&self.app_config_dir.join("providers.toml")).unwrap()
    }

    pub fn sessions_config(&self) -> SessionsConfig {
        SessionsConfig::load(&self.app_config_dir.join("sessions.toml")).unwrap()
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
        fs::write(
            self.app_config_dir.join("sessions.toml"),
            format!(
                "[{provider_name}]\nturn_script = \"true\"\ntranscript_locator = {:?}\nstate_dir = {:?}\n",
                locator,
                self.dir
                    .path()
                    .join(format!("{provider_name}-locator-state"))
                    .to_string_lossy()
            ),
        )
        .unwrap();
    }

    pub fn write_sessions_without_locator(&self, provider_name: &str) {
        fs::create_dir_all(&self.app_config_dir).unwrap();
        fs::write(
            self.app_config_dir.join("sessions.toml"),
            format!(
                "[{provider_name}]\nturn_script = \"true\"\nstate_dir = {:?}\n",
                self.dir
                    .path()
                    .join(format!("{provider_name}-locator-state"))
                    .to_string_lossy()
            ),
        )
        .unwrap();
    }

    pub fn write_sessions_locator_returns_relative(&self, provider_name: &str) {
        self.write_sessions_with_locator_script(provider_name, "printf '%s\\n' relative.jsonl");
    }

    pub fn write_sessions_locator_errors(&self, provider_name: &str) {
        self.write_sessions_with_locator_script(
            provider_name,
            "printf '%s\\n' locator failed >&2; exit 42",
        );
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

    pub fn update_chain_last_used(&self, chain_id: &str, last_used_at: &str) {
        self.conn()
            .execute(
                "UPDATE session_chains SET last_used_at = ?2 WHERE chain_id = ?1",
                params![chain_id, last_used_at],
            )
            .unwrap();
    }

    pub fn seed_segmentless_turn(&self, provider_name: &str, session_id: &str) {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, '2026-04-17T08:00:00Z', '2026-04-17T08:00:00Z', ?2)",
            params![CHAIN_B, MODEL],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_turns
                (provider_name, session_id, turn_id, timestamp, role, source_file, ingested_at, is_compaction_boundary)
             VALUES (?1, ?2, 'partial-turn', '2026-04-17T08:00:00Z', 'assistant', '', '2026-04-17T08:00:00Z', 0)",
            params![provider_name, session_id],
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

    pub fn stage_claude_transcript_for_workspace(
        &self,
        projects_dir: &Path,
        workspace_root: &Path,
        session_id: &str,
    ) -> PathBuf {
        fs::create_dir_all(workspace_root).unwrap();
        self.stage_claude_transcript_for_encoded_path(projects_dir, workspace_root, session_id)
    }

    pub fn stage_claude_transcript_for_encoded_path(
        &self,
        projects_dir: &Path,
        encoded_source_path: &Path,
        session_id: &str,
    ) -> PathBuf {
        let project_dir = claude_project_dir_name(encoded_source_path);
        let transcript_dir = projects_dir.join(project_dir);
        fs::create_dir_all(&transcript_dir).unwrap();
        let path = transcript_dir.join(format!("{session_id}.jsonl"));
        fs::write(
            &path,
            format!(
                "{{\"sessionId\":\"{session_id}\",\"type\":\"assistant\",\"message\":\"fixture\"}}\n"
            ),
        )
        .unwrap();
        path
    }

    pub fn stage_codex_rollout(&self, sessions_dir: &Path, name: &str, body: &str) -> PathBuf {
        fs::create_dir_all(sessions_dir).unwrap();
        let path = sessions_dir.join(name);
        fs::write(&path, body).unwrap();
        path
    }

    pub fn create_non_utf8_dir_and_utf8_symlink(&self) -> PathBuf {
        let non_utf8 = self
            .dir
            .path()
            .join(OsString::from_vec(b"codex-\xFF-workspace".to_vec()));
        fs::create_dir_all(&non_utf8).unwrap();
        let link = self.dir.path().join("codex-utf8-link");
        symlink(&non_utf8, &link).unwrap();
        link
    }

    pub fn snapshot_read_only_state(&self, transcript_path: &Path) -> ReadOnlySnapshot {
        let conn = self.conn();
        ReadOnlySnapshot {
            invocations: table_count(&conn, "invocations"),
            session_turns: table_count(&conn, "session_turns"),
            session_chains: table_count(&conn, "session_chains"),
            session_chain_segments: table_count(&conn, "session_chain_segments"),
            provider_quotas: table_count(&conn, "provider_quotas"),
            transcript_mtime: fs::metadata(transcript_path).unwrap().modified().unwrap(),
        }
    }

    pub fn run_locate(&self, session_id: &str, extra_args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("session").arg("locate").arg(session_id);
        cmd.args(extra_args);
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd.output().unwrap()
    }

    fn model_specs_from_disk(&self) -> Vec<(String, Vec<String>)> {
        let mut specs = Vec::new();
        for entry in fs::read_dir(&self.models_dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
                continue;
            }
            let model_name = path.file_stem().unwrap().to_string_lossy().into_owned();
            let raw = fs::read_to_string(path).unwrap();
            let providers = raw
                .lines()
                .filter_map(|line| line.trim().strip_prefix("name = \""))
                .filter_map(|rest| rest.strip_suffix('"'))
                .map(str::to_string)
                .collect();
            specs.push((model_name, providers));
        }
        specs
    }
}

pub fn cli_claude_success_fixture() -> PreparedLocate {
    component_claude_success_fixture(CLAUDE_PROVIDER, true)
}

pub fn component_claude_success_fixture(provider_name: &str, resume: bool) -> PreparedLocate {
    let fixture = LocateFixture::new();
    let projects_dir = fixture.root().join("claude-projects");
    let workspace_root = fixture.root().join("workspace");
    let jsonl_path =
        fixture.stage_claude_transcript_for_workspace(&projects_dir, &workspace_root, SESSION_A);
    fixture.write_model(MODEL, &[provider_name]);
    fixture.write_provider(
        provider_name,
        StorageKind::ClaudeCode {
            projects_dir: &projects_dir,
        },
        resume,
        None,
    );
    fixture.write_sessions_with_locator_path(provider_name, &jsonl_path);
    fixture.seed_active_chain(
        CHAIN_A,
        provider_name,
        SESSION_A,
        MODEL,
        "2026-04-17T08:00:00Z",
    );
    PreparedLocate {
        fixture,
        session_id: SESSION_A.to_string(),
        chain_id: CHAIN_A.to_string(),
        provider_name: provider_name.to_string(),
        workspace_root,
        jsonl_path,
    }
}

pub fn component_claude_without_resume_fixture() -> PreparedLocate {
    component_claude_success_fixture(CLAUDE_PROVIDER, false)
}

pub fn component_no_storage_fixture(locator_present: bool) -> PreparedLocate {
    let fixture = LocateFixture::new();
    let transcript = fixture.root().join("other-transcript.jsonl");
    fs::write(&transcript, "{}\n").unwrap();
    let workspace_root = fixture.root().join("other-workspace");
    fs::create_dir_all(&workspace_root).unwrap();
    fixture.write_model(MODEL, &[CLAUDE_PROVIDER]);
    fixture.write_provider(CLAUDE_PROVIDER, StorageKind::None, true, None);
    if locator_present {
        fixture.write_sessions_with_locator_path(CLAUDE_PROVIDER, &transcript);
    } else {
        fixture.write_sessions_without_locator(CLAUDE_PROVIDER);
    }
    fixture.seed_active_chain(
        CHAIN_A,
        CLAUDE_PROVIDER,
        SESSION_A,
        MODEL,
        "2026-04-17T08:00:00Z",
    );
    PreparedLocate {
        fixture,
        session_id: SESSION_A.to_string(),
        chain_id: CHAIN_A.to_string(),
        provider_name: CLAUDE_PROVIDER.to_string(),
        workspace_root,
        jsonl_path: transcript,
    }
}

pub fn component_ambiguous_session_fixture() -> PreparedLocate {
    let prepared = component_claude_success_fixture(CLAUDE_PROVIDER, true);
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
    let recent_a = (Utc::now() - Duration::hours(1)).to_rfc3339();
    let recent_b = (Utc::now() - Duration::hours(2)).to_rfc3339();
    prepared.fixture.update_chain_last_used(CHAIN_A, &recent_a);
    prepared
        .fixture
        .seed_active_chain(CHAIN_B, "claude2", SESSION_A, MODEL, &recent_b);
    prepared
}

pub fn component_recency_collapsed_fixture() -> PreparedLocate {
    let prepared = component_claude_success_fixture(CLAUDE_PROVIDER, true);
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
    let recent = (Utc::now() - Duration::hours(1)).to_rfc3339();
    let stale = (Utc::now() - Duration::hours(48)).to_rfc3339();
    prepared.fixture.update_chain_last_used(CHAIN_A, &recent);
    prepared
        .fixture
        .seed_active_chain(CHAIN_B, "claude2", SESSION_A, MODEL, &stale);
    prepared
}

pub fn component_partial_db_fixture() -> PreparedLocate {
    let fixture = LocateFixture::new();
    fixture.write_model(MODEL, &[CLAUDE_PROVIDER]);
    fixture.write_provider(CLAUDE_PROVIDER, StorageKind::None, true, None);
    fixture.write_sessions_without_locator(CLAUDE_PROVIDER);
    fixture.seed_segmentless_turn(CLAUDE_PROVIDER, SESSION_A);
    PreparedLocate {
        fixture,
        session_id: SESSION_A.to_string(),
        chain_id: CHAIN_A.to_string(),
        provider_name: CLAUDE_PROVIDER.to_string(),
        workspace_root: PathBuf::new(),
        jsonl_path: PathBuf::new(),
    }
}

pub fn component_claude_missing_workspace_fixture() -> PreparedLocate {
    let fixture = LocateFixture::new();
    let projects_dir = fixture.root().join("claude-projects");
    let missing_workspace = fixture.root().join("missing-workspace");
    let jsonl_path = fixture.stage_claude_transcript_for_encoded_path(
        &projects_dir,
        &missing_workspace,
        SESSION_A,
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
    PreparedLocate {
        fixture,
        session_id: SESSION_A.to_string(),
        chain_id: CHAIN_A.to_string(),
        provider_name: CLAUDE_PROVIDER.to_string(),
        workspace_root: missing_workspace,
        jsonl_path,
    }
}

pub fn component_claude_ambiguous_workspace_fixture() -> PreparedLocate {
    let fixture = LocateFixture::new();
    let projects_dir = fixture.root().join("claude-projects");
    let base = fixture.root().join("hyphen-fixture");
    let workspace_with_hyphen = base.join("a-b");
    let workspace_with_separator = base.join("a").join("b");
    fs::create_dir_all(&workspace_with_hyphen).unwrap();
    fs::create_dir_all(&workspace_with_separator).unwrap();
    let jsonl_path = fixture.stage_claude_transcript_for_encoded_path(
        &projects_dir,
        &workspace_with_hyphen,
        SESSION_A,
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
    PreparedLocate {
        fixture,
        session_id: SESSION_A.to_string(),
        chain_id: CHAIN_A.to_string(),
        provider_name: CLAUDE_PROVIDER.to_string(),
        workspace_root: workspace_with_hyphen,
        jsonl_path,
    }
}

pub fn component_claude_single_hyphen_workspace_fixture() -> PreparedLocate {
    let fixture = LocateFixture::new();
    let projects_dir = fixture.root().join("claude-projects");
    let workspace_root = fixture.root().join("hyphen-fixture").join("a-b");
    let jsonl_path =
        fixture.stage_claude_transcript_for_workspace(&projects_dir, &workspace_root, SESSION_A);
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
    PreparedLocate {
        fixture,
        session_id: SESSION_A.to_string(),
        chain_id: CHAIN_A.to_string(),
        provider_name: CLAUDE_PROVIDER.to_string(),
        workspace_root,
        jsonl_path,
    }
}

pub fn component_unicode_json_shape_fixture() -> PreparedLocate {
    let provider_name = "claude-prod_1";
    let fixture = LocateFixture::new();
    let projects_dir = fixture.root().join("claude-projects");
    let workspace_root = fixture.root().join("workspace-unicode-é");
    let session_id = "ffffffff-ffff-4fff-8fff-ffffffffffff";
    let chain_id = "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee";
    let jsonl_path =
        fixture.stage_claude_transcript_for_workspace(&projects_dir, &workspace_root, session_id);
    fixture.write_model(MODEL, &[provider_name]);
    fixture.write_provider(
        provider_name,
        StorageKind::ClaudeCode {
            projects_dir: &projects_dir,
        },
        true,
        None,
    );
    fixture.write_sessions_with_locator_path(provider_name, &jsonl_path);
    fixture.seed_active_chain(
        chain_id,
        provider_name,
        session_id,
        MODEL,
        "2026-04-17T08:00:00Z",
    );
    PreparedLocate {
        fixture,
        session_id: session_id.to_string(),
        chain_id: chain_id.to_string(),
        provider_name: provider_name.to_string(),
        workspace_root,
        jsonl_path,
    }
}

pub fn component_codex_success_fixture(provider_name: &str) -> PreparedLocate {
    let fixture = LocateFixture::new();
    let sessions_dir = fixture.root().join("codex-sessions");
    let workspace_root = fixture.root().join("codex-workspace");
    fs::create_dir_all(&workspace_root).unwrap();
    let jsonl_path = fixture.stage_codex_rollout(
        &sessions_dir,
        "rollout.jsonl",
        &format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{SESSION_A}\",\"cwd\":\"{}\"}}}}\n",
            workspace_root.display()
        ),
    );
    fixture.write_model(MODEL, &[provider_name]);
    fixture.write_provider(
        provider_name,
        StorageKind::Codex {
            sessions_dir: &sessions_dir,
        },
        true,
        None,
    );
    fixture.write_sessions_with_locator_path(provider_name, &jsonl_path);
    fixture.seed_active_chain(
        CHAIN_A,
        provider_name,
        SESSION_A,
        MODEL,
        "2026-04-17T08:00:00Z",
    );
    PreparedLocate {
        fixture,
        session_id: SESSION_A.to_string(),
        chain_id: CHAIN_A.to_string(),
        provider_name: provider_name.to_string(),
        workspace_root,
        jsonl_path,
    }
}

pub fn component_codex_failure_fixture(body: &str) -> PreparedLocate {
    let fixture = LocateFixture::new();
    let sessions_dir = fixture.root().join("codex-sessions");
    let workspace_root = fixture.root().join("codex-workspace");
    fs::create_dir_all(&workspace_root).unwrap();
    let jsonl_path = fixture.stage_codex_rollout(&sessions_dir, "rollout.jsonl", body);
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
    PreparedLocate {
        fixture,
        session_id: SESSION_A.to_string(),
        chain_id: CHAIN_A.to_string(),
        provider_name: CODEX_PROVIDER.to_string(),
        workspace_root,
        jsonl_path,
    }
}

pub fn component_codex_non_utf8_cwd_fixture() -> PreparedLocate {
    let fixture = LocateFixture::new();
    let sessions_dir = fixture.root().join("codex-sessions");
    let cwd_link = fixture.create_non_utf8_dir_and_utf8_symlink();
    let jsonl_path = fixture.stage_codex_rollout(
        &sessions_dir,
        "rollout.jsonl",
        &format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{SESSION_A}\",\"cwd\":\"{}\"}}}}\n",
            cwd_link.display()
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
    PreparedLocate {
        fixture,
        session_id: SESSION_A.to_string(),
        chain_id: CHAIN_A.to_string(),
        provider_name: CODEX_PROVIDER.to_string(),
        workspace_root: cwd_link,
        jsonl_path,
    }
}

pub fn cli_missing_uuid_fixture() -> LocateFixture {
    let fixture = LocateFixture::new();
    let _ = fixture.open_db();
    fixture.write_model(MODEL, &[CLAUDE_PROVIDER]);
    fixture.write_provider(CLAUDE_PROVIDER, StorageKind::None, true, None);
    fixture
}

pub fn cli_read_only_fixture() -> PreparedLocate {
    let prepared = component_claude_success_fixture(CLAUDE_PROVIDER, true);
    prepared
        .fixture
        .seed_provider_quota_exhausted(CLAUDE_PROVIDER);
    prepared
}

pub fn cli_no_storage_fixture(locator_present: bool) -> PreparedLocate {
    component_no_storage_fixture(locator_present)
}

pub fn cli_locator_failure_fixture() -> PreparedLocate {
    let prepared = component_claude_success_fixture(CLAUDE_PROVIDER, true);
    prepared
        .fixture
        .write_sessions_locator_errors(CLAUDE_PROVIDER);
    prepared
}

pub fn cli_invalid_uuid_fixture() -> LocateFixture {
    LocateFixture::new()
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

pub fn required_success_fields() -> [&'static str; 8] {
    [
        "session_id",
        "chain_id",
        "provider_name",
        "storage_type",
        "jsonl_path",
        "workspace_root",
        "transcript_state",
        "mutable",
    ]
}

pub fn storage_mapping_inputs() -> (
    Option<SessionStorage>,
    Option<SessionStorage>,
    Option<SessionStorage>,
) {
    (
        Some(SessionStorage::ClaudeCode {
            projects_dir: PathBuf::from("/tmp/claude-projects"),
        }),
        Some(SessionStorage::Codex {
            sessions_dir: PathBuf::from("/tmp/codex-sessions"),
        }),
        None,
    )
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
