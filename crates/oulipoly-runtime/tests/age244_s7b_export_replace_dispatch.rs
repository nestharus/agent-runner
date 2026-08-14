#![cfg(unix)]
#![allow(dead_code)]

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, provider_implementation_ref::ProviderImplementationRef,
};
use oulipoly_runtime::provider_registry::{
    ProviderRegistry, ProviderRegistryHandle, ProviderRegistryOptions,
};
use oulipoly_runtime::services::{
    ProductionSessionExportService, ProductionSessionReplaceService, SessionExportServicePort,
    SessionExportServiceRequest, SessionReplaceServicePort, SessionReplaceServiceRequest,
    SessionServiceExternalProviderIdentity,
};
use oulipoly_runtime::session_export::{
    canonical_jsonl_bytes, read_canonical_transcript, resolve_export_session_metadata,
};
use oulipoly_runtime::session_lock::SessionLock;
use oulipoly_runtime::session_replace::{self, ReplaceError, ReplaceReceipt, ReplaceSource};
use oulipoly_state::StateDb;
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

// Post-WU5a characterization baseline. The guard still catches any genuinely
// new concrete provider-name reference introduced after the manager-approved
// AGE-245 S7c rebaseline point. Bump on intentional structural relocations.
const BASE_REF: &str = "f0844a90d73c9196fc6fe53d510caf4d2c56c076";
const MODEL: &str = "model-alpha";
const UNRELATED_MODEL: &str = "model-unrelated";
const PROVIDER_NAME: &str = "provider-alpha-account";
const PROVIDER_INSTANCE_ID: &str = "provider-alpha-instance";
const SETTINGS_ID: &str = "provider-alpha-settings";
const SESSION_ID: &str = "11111111-1111-4111-8111-111111111111";
const CHAIN_ID: &str = "22222222-2222-4222-8222-222222222222";

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    old_config: Option<OsString>,
    old_oulipoly_data_dir: Option<OsString>,
    old_data: Option<OsString>,
    old_home: Option<OsString>,
    old_path: Option<OsString>,
}

impl EnvGuard {
    fn new(config_home: &Path, data_home: &Path, data_root: &Path) -> Self {
        let guard = Self {
            old_config: std::env::var_os("XDG_CONFIG_HOME"),
            old_oulipoly_data_dir: std::env::var_os("OULIPOLY_DATA_DIR"),
            old_data: std::env::var_os("XDG_DATA_HOME"),
            old_home: std::env::var_os("HOME"),
            old_path: std::env::var_os("PATH"),
        };
        let scripts_dir = repo_root().join("scripts");
        let path = std::env::join_paths(std::iter::once(scripts_dir).chain(std::env::split_paths(
            &guard.old_path.clone().unwrap_or_default(),
        )))
        .expect("test PATH");
        unsafe {
            std::env::set_var("OULIPOLY_DATA_DIR", data_root);
            std::env::set_var("XDG_CONFIG_HOME", config_home);
            std::env::set_var("XDG_DATA_HOME", data_home);
            std::env::set_var("HOME", data_home);
            std::env::set_var("PATH", path);
        }
        guard
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            restore_env("XDG_CONFIG_HOME", self.old_config.take());
            restore_env("OULIPOLY_DATA_DIR", self.old_oulipoly_data_dir.take());
            restore_env("XDG_DATA_HOME", self.old_data.take());
            restore_env("HOME", self.old_home.take());
            restore_env("PATH", self.old_path.take());
        }
    }
}

unsafe fn restore_env(key: &str, value: Option<OsString>) {
    match value {
        Some(value) => unsafe { std::env::set_var(key, value) },
        None => unsafe { std::env::remove_var(key) },
    }
}

struct EnvVarGuard {
    key: &'static str,
    old_value: Option<OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let guard = Self {
            key,
            old_value: std::env::var_os(key),
        };
        unsafe {
            std::env::set_var(key, value);
        }
        guard
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        unsafe {
            restore_env(self.key, self.old_value.take());
        }
    }
}

struct DispatchFixture {
    _dir: tempfile::TempDir,
    _env: EnvGuard,
    config_root: PathBuf,
    models_dir: PathBuf,
    data_root: PathBuf,
    transcript_path: PathBuf,
    input_path: PathBuf,
    provider_path: PathBuf,
    mode_path: PathBuf,
    record_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SqliteSnapshot {
    session_turns: Vec<SessionTurnSnapshotRow>,
    session_chains: Vec<SessionChainSnapshotRow>,
    session_chain_segments: Vec<SessionChainSegmentSnapshotRow>,
}

// `ingested_at` is intentionally excluded because direct-vs-dispatch A/B runs
// execute at different instants; every deterministic session_turns column is
// compared.
type SessionTurnSnapshotRow = (
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    i64,
    i64,
    String,
    Option<String>,
);

type SessionChainSnapshotRow = (String, String, String, String);

type SessionChainSegmentSnapshotRow = (
    i64,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    String,
);

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReplacementTurnRow {
    provider_name: String,
    session_id: String,
    turn_id: String,
    timestamp: String,
    role: String,
    parent_turn_id: Option<String>,
    is_sidechain: i64,
    is_compaction_boundary: i64,
    source_file: Option<String>,
    body: Option<String>,
}

type ComparableReplaceState = (
    Vec<u8>,
    SqliteSnapshot,
    Vec<(String, Vec<u8>)>,
    ReceiptSnapshot,
);

type HostMutationSnapshot = (Vec<u8>, SqliteSnapshot, Vec<(String, Vec<u8>)>);

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReceiptSnapshot {
    session_id: String,
    provider_name: String,
    storage_type: String,
    operation: String,
    state_updated: bool,
    preimage_len: usize,
    postimage_len: usize,
}

impl DispatchFixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_home = dir.path().join("config");
        let data_home = dir.path().join("data");
        let config_root = config_home.join("oulipoly-agent-runner");
        let models_dir = config_root.join("models");
        let data_root = data_home.join("oulipoly-agent-runner");
        let workspace_root = data_root.join("workspace");
        fs::create_dir_all(&models_dir).expect("models dir");
        fs::create_dir_all(&data_root).expect("data dir");
        fs::create_dir_all(&workspace_root).expect("workspace dir");
        let env = EnvGuard::new(&config_home, &data_home, &data_root);
        let transcript_path = data_root.join("provider-alpha-session.jsonl");
        let input_path = data_root.join("replacement-canonical.jsonl");
        let mode_path = dir.path().join("mode.txt");
        let record_path = dir.path().join("provider-records.jsonl");
        let canonical_path = dir.path().join("provider-export-canonical.jsonl");
        let semantic_mismatch_path = dir.path().join("provider-semantic-mismatch.jsonl");
        let replacement_native_path = dir.path().join("provider-replacement-native.jsonl");
        let expected_postimage_path = dir.path().join("provider-expected-postimage.txt");
        let semantic_postimage_path = dir.path().join("provider-semantic-postimage.txt");
        fs::write(&mode_path, "describe_only").expect("mode");
        fs::write(&record_path, "").expect("records");
        fs::write(&canonical_path, canonical_input_jsonl(&input_path)).expect("canonical");
        fs::write(
            &replacement_native_path,
            provider_replacement_native_bytes(),
        )
        .expect("provider native replacement");
        fs::write(
            &semantic_mismatch_path,
            format!(
                "{}\n{}\n",
                native_line("valid-turn-1", "user", "semantic mismatch user", 0),
                native_line(
                    "valid-turn-2",
                    "assistant",
                    "semantic mismatch assistant",
                    1
                ),
            ),
        )
        .expect("semantic mismatch artifact");
        let provider_paths = FakeProviderPaths {
            mode: &mode_path,
            records: &record_path,
            canonical: &canonical_path,
            transcript: &transcript_path,
            semantic_mismatch: &semantic_mismatch_path,
            replacement_native: &replacement_native_path,
            expected_postimage: &expected_postimage_path,
            semantic_postimage: &semantic_postimage_path,
        };
        let provider_path = write_fake_provider(dir.path(), &provider_paths);
        let fixture = Self {
            _dir: dir,
            _env: env,
            config_root,
            models_dir,
            data_root,
            transcript_path,
            input_path,
            provider_path,
            mode_path,
            record_path,
        };
        fixture.prepare_builtin_session();
        write_expected_postimage_hash(
            &fixture.transcript_path,
            &expected_postimage_path,
            &provider_replacement_native_bytes(),
        );
        write_export_hash_for_artifact(&semantic_mismatch_path, &semantic_postimage_path);
        fixture
    }

    fn prepare_builtin_session(&self) {
        self.write_model_file(false);
        self.write_provider_file();
        self.write_native_transcript("old-user", "old-assistant");
        fs::write(&self.input_path, canonical_input_jsonl(&self.input_path))
            .expect("replacement input");
        self.seed_state();
    }

    fn write_model_file(&self, external_ref: bool) {
        let provider_ref = if external_ref {
            format!(
                "\n[provider]\npath = {:?}\n",
                self.provider_path.display().to_string()
            )
        } else {
            String::new()
        };
        fs::write(
            self.models_dir.join(format!("{MODEL}.toml")),
            format!("[[providers]]\nname = \"{PROVIDER_NAME}\"\n{provider_ref}"),
        )
        .expect("model file");
    }

    fn write_provider_file(&self) {
        let transcript_script_path = self.config_root.join("age244-transcript-locator.sh");
        write_shell_script(
            &transcript_script_path,
            &format!(
                "#!/bin/sh\nprintf '%s\\n' {}\n",
                shell_double_quoted(&self.transcript_path.display().to_string())
            ),
        );
        let cwd_script_path = self.config_root.join("age244-cwd-locator.sh");
        write_shell_script(
            &cwd_script_path,
            &format!(
                "#!/bin/sh\nprintf '%s\\n' {}\n",
                shell_double_quoted(
                    &serde_json::to_string(&json!({
                    "found": true,
                    "cwd": self.data_root.join("workspace"),
                        }))
                    .expect("cwd json")
                )
            ),
        );
        let transcript_script = transcript_script_path.display().to_string();
        let cwd_script = cwd_script_path.display().to_string();
        fs::write(
            self.config_root.join("providers.toml"),
            format!(
                r#"[{PROVIDER_NAME}]
command = "provider-alpha-command-that-must-not-run"
args = []
interactive_args = []
prompt_mode = "arg"

[{PROVIDER_NAME}.resume]
kind = "flag"
flag = "--resume"

[{PROVIDER_NAME}.session_storage]
kind = "script"
cwd_script = {cwd_script:?}
transcript_script = {transcript_script:?}
storage_type = "{storage_type}"
"#,
                storage_type = builtin_storage_type()
            ),
        )
        .expect("provider file");
    }

    fn write_native_transcript(&self, user_text: &str, assistant_text: &str) {
        fs::write(
            &self.transcript_path,
            format!(
                "{}\n{}\n",
                native_line("old-turn-1", "user", user_text, 0),
                native_line("old-turn-2", "assistant", assistant_text, 1),
            ),
        )
        .expect("native transcript");
    }

    fn seed_state(&self) {
        let db = StateDb::open(&self.data_root.join("state.db")).expect("state db");
        let connection = rusqlite::Connection::open(db.path()).unwrap();
        connection
            .execute(
                "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
                 VALUES (?1, '2026-05-01T00:00:00Z', '2026-05-01T00:00:00Z', ?2)",
                params![CHAIN_ID, MODEL],
            )
            .expect("chain");
        connection
            .execute(
                "INSERT INTO session_chain_segments
                    (chain_id, provider_name, session_id, started_at, last_turn_id, transition_reason)
                 VALUES (?1, ?2, ?3, '2026-05-01T00:00:00Z', 'old-turn-2', 'initial')",
                params![CHAIN_ID, PROVIDER_NAME, SESSION_ID],
            )
            .expect("segment");
        for (turn_id, role, offset) in [
            ("old-turn-1", "user", 0_i64),
            ("old-turn-2", "assistant", 1_i64),
        ] {
            connection
                .execute(
                    "INSERT INTO session_turns
                        (provider_name, session_id, turn_id, timestamp, role,
                         parent_turn_id, is_sidechain, is_compaction_boundary, source_file, ingested_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, NULL, 0, 0, ?6, ?4)",
                    params![
                        PROVIDER_NAME,
                        SESSION_ID,
                        turn_id,
                        format!("2026-05-01T00:00:{offset:02}Z"),
                        role,
                        self.transcript_path.to_string_lossy(),
                    ],
                )
                .expect("turn");
        }
    }

    fn set_mode(&self, mode: &str) {
        fs::write(&self.mode_path, mode).expect("mode");
    }

    fn registry_handle(&self) -> ProviderRegistryHandle {
        ProviderRegistryHandle::new(Arc::new(
            ProviderRegistry::from_model_configs(
                &[external_model(MODEL, &self.provider_path)],
                ProviderRegistryOptions::default()
                    .with_config_root(self.config_root.clone())
                    .with_data_root(self.data_root.clone()),
            )
            .expect("registry"),
        ))
    }

    fn unrelated_registry_handle(&self) -> ProviderRegistryHandle {
        ProviderRegistryHandle::new(Arc::new(
            ProviderRegistry::from_model_configs(
                &[external_model(UNRELATED_MODEL, &self.provider_path)],
                ProviderRegistryOptions::default()
                    .with_config_root(self.config_root.clone())
                    .with_data_root(self.data_root.clone()),
            )
            .expect("unrelated registry"),
        ))
    }

    fn missing_provider_registry_handle(&self) -> ProviderRegistryHandle {
        let missing_path = self
            .provider_path
            .with_file_name("missing-provider-alpha-session");
        ProviderRegistryHandle::new(Arc::new(
            ProviderRegistry::from_model_configs(
                &[external_model(MODEL, &missing_path)],
                ProviderRegistryOptions::default()
                    .with_config_root(self.config_root.clone())
                    .with_data_root(self.data_root.clone()),
            )
            .expect("missing provider registry"),
        ))
    }

    fn records(&self) -> Vec<Value> {
        fs::read_to_string(&self.record_path)
            .expect("records")
            .lines()
            .map(|line| serde_json::from_str(line).expect("record json"))
            .collect()
    }

    fn request_records_for(&self, subcommand: &str) -> Vec<Value> {
        self.records()
            .into_iter()
            .filter(|record| record["subcommand"] == subcommand)
            .collect()
    }

    fn sqlite_snapshot(&self) -> SqliteSnapshot {
        sqlite_snapshot(&self.conn(), &self.transcript_path)
    }

    fn journal_snapshot(&self) -> Vec<(String, Vec<u8>)> {
        tree_snapshot(&self.data_root.join("replace_journal"))
    }

    fn transcript_bytes(&self) -> Vec<u8> {
        fs::read(&self.transcript_path).expect("transcript bytes")
    }

    fn conn(&self) -> Connection {
        Connection::open(self.data_root.join("state.db")).expect("sqlite")
    }

    fn active_segment_id(&self) -> i64 {
        self.conn()
            .query_row(
                "SELECT id FROM session_chain_segments
                 WHERE chain_id = ?1 AND provider_name = ?2 AND session_id = ?3",
                params![CHAIN_ID, PROVIDER_NAME, SESSION_ID],
                |row| row.get(0),
            )
            .expect("active segment id")
    }

    fn seed_pending_replace_recovery(&self, postimage_bytes: &[u8], _postimage_sha256: &str) {
        let preimage_sha256 = direct_export_hash();
        fs::write(&self.transcript_path, postimage_bytes).expect("postimage transcript");
        let postimage_sha256 = direct_export_hash();
        let journal_root = self.data_root.join("replace_journal");
        fs::create_dir_all(&journal_root).expect("journal root");
        let canonical_records_path =
            journal_root.join(format!("session-{SESSION_ID}.canonical.jsonl"));
        fs::write(
            &canonical_records_path,
            canonical_input_jsonl(&self.input_path),
        )
        .expect("pending canonical records");
        let pending_path = journal_root.join(format!("session-{SESSION_ID}.pending"));
        let journal = json!({
            "schema_version": 1,
            "operation": "import-replace",
            "operation_uuid": "33333333-3333-4333-8333-333333333333",
            "started_at": "2026-05-01T02:00:00Z",
            "session_id": SESSION_ID,
            "chain_id": CHAIN_ID,
            "active_segment_id": self.active_segment_id(),
            "provider_name": PROVIDER_NAME,
            "storage_type": builtin_storage_type(),
            "jsonl_path": self.transcript_path.display().to_string(),
            "preimage_sha256": preimage_sha256,
            "postimage_sha256": postimage_sha256,
            "canonical_records_path": canonical_records_path.display().to_string(),
            "db_state_pending": true,
            "expected_turn_count": 2,
        });
        fs::write(
            pending_path,
            serde_json::to_vec_pretty(&journal).expect("journal json"),
        )
        .expect("pending journal");
    }

    fn seed_external_pending_replace_recovery(
        &self,
        provider_written_bytes: &[u8],
        include_postimage: bool,
    ) {
        let preimage_bytes = self.transcript_bytes();
        let preimage_sha256 = direct_export_hash();
        fs::write(&self.transcript_path, provider_written_bytes)
            .expect("provider-written transcript");
        let postimage_sha256 = direct_export_hash();
        let journal_root = self.data_root.join("replace_journal");
        fs::create_dir_all(&journal_root).expect("journal root");
        let canonical_records_path =
            journal_root.join(format!("session-{SESSION_ID}.canonical.jsonl"));
        fs::write(
            &canonical_records_path,
            canonical_input_jsonl(&self.input_path),
        )
        .expect("pending canonical records");
        let preimage_snapshot_path = journal_root.join(format!("session-{SESSION_ID}.preimage"));
        fs::write(&preimage_snapshot_path, preimage_bytes).expect("preimage snapshot");
        let pending_path = journal_root.join(format!("session-{SESSION_ID}.pending"));
        let mut journal = json!({
            "schema_version": 1,
            "operation": "import-replace",
            "operation_uuid": "44444444-4444-4444-8444-444444444444",
            "started_at": "2026-05-01T02:00:00Z",
            "session_id": SESSION_ID,
            "chain_id": CHAIN_ID,
            "active_segment_id": self.active_segment_id(),
            "provider_name": PROVIDER_NAME,
            "storage_type": builtin_storage_type(),
            "jsonl_path": self.transcript_path.display().to_string(),
            "preimage_sha256": preimage_sha256,
            "canonical_records_path": canonical_records_path.display().to_string(),
            "preimage_snapshot_path": preimage_snapshot_path.display().to_string(),
            "db_state_pending": true,
            "expected_turn_count": 2,
        });
        if include_postimage {
            journal["postimage_sha256"] = Value::String(postimage_sha256);
        }
        fs::write(
            pending_path,
            serde_json::to_vec_pretty(&journal).expect("journal json"),
        )
        .expect("pending journal");
    }

    fn seed_provider_owned_v2_pending_replace_journal(
        &self,
        db_apply_marker: &str,
        recovery_id: Option<&str>,
    ) {
        self.seed_provider_owned_v2_pending_replace_journal_with_preimage(
            db_apply_marker,
            recovery_id,
            self.sqlite_snapshot(),
        );
    }

    fn seed_provider_owned_v2_pending_replace_journal_with_preimage(
        &self,
        db_apply_marker: &str,
        recovery_id: Option<&str>,
        db_preimage: SqliteSnapshot,
    ) {
        let journal_root = self.data_root.join("replace_journal");
        fs::create_dir_all(&journal_root).expect("journal root");
        let mut journal = json!({
            "schema_version": 2,
            "operation": "provider-owned-import-replace",
            "operation_id": provider_owned_operation_id(),
            "started_at": "2026-05-01T02:00:00Z",
            "settings_id": SETTINGS_ID,
            "model_name": MODEL,
            "provider_name": PROVIDER_NAME,
            "provider_instance_id": PROVIDER_INSTANCE_ID,
            "session_id": SESSION_ID,
            "chain_id": CHAIN_ID,
            "active_segment_id": self.active_segment_id(),
            "db_apply_marker": db_apply_marker,
            "db_preimage": {
                "session_turns": db_preimage.session_turns,
                "last_turn_id": db_preimage.session_chain_segments[0].6,
                "last_used_at": db_preimage.session_chains[0].2,
            }
        });
        if let Some(recovery_id) = recovery_id {
            journal["recovery_id"] = Value::String(recovery_id.to_string());
        }
        fs::write(
            journal_root.join(format!("session-{SESSION_ID}.pending")),
            serde_json::to_vec_pretty(&journal).expect("v2 journal json"),
        )
        .expect("v2 pending journal");
    }

    fn delete_active_segment_for_provider_session(&self) {
        self.conn()
            .execute(
                "DELETE FROM session_chain_segments
                 WHERE provider_name = ?1 AND session_id = ?2 AND ended_at IS NULL",
                params![PROVIDER_NAME, SESSION_ID],
            )
            .expect("delete active segment");
    }

    fn insert_ambiguous_active_segment_for_provider_session(&self) {
        let conn = self.conn();
        let ambiguous_chain_id = "33333333-3333-4333-8333-333333333333";
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, '2026-05-01T00:30:00Z', '2026-05-01T00:30:00Z', ?2)",
            params![ambiguous_chain_id, MODEL],
        )
        .expect("ambiguous chain");
        conn
            .execute(
                "INSERT INTO session_chain_segments
                    (chain_id, provider_name, session_id, started_at, last_turn_id, transition_reason)
                 VALUES (?1, ?2, ?3, '2026-05-01T00:30:00Z', 'old-turn-2', 'manual')",
                params![ambiguous_chain_id, PROVIDER_NAME, SESSION_ID],
            )
            .expect("ambiguous active segment");
    }

    fn replace_db_rows_with_provider_owned_postimage(&self) {
        let conn = self.conn();
        conn.execute(
            "DELETE FROM session_turns WHERE provider_name = ?1 AND session_id = ?2",
            params![PROVIDER_NAME, SESSION_ID],
        )
        .expect("delete old turns");
        for row in expected_provider_owned_replacement_turn_rows() {
            conn.execute(
                "INSERT INTO session_turns
                    (provider_name, session_id, turn_id, timestamp, role,
                     parent_turn_id, is_sidechain, is_compaction_boundary, source_file, body, ingested_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?4)",
                params![row.0, row.1, row.2, row.3, row.4, row.5, row.6, row.7, row.8, row.9],
            )
            .expect("insert provider-owned turn");
        }
        conn.execute(
            "UPDATE session_chain_segments
             SET last_turn_id = 'valid-turn-2'
             WHERE provider_name = ?1 AND session_id = ?2 AND ended_at IS NULL",
            params![PROVIDER_NAME, SESSION_ID],
        )
        .expect("update segment last turn");
        conn.execute(
            "UPDATE session_chains SET last_used_at = '2026-05-01T01:00:01.000Z'
             WHERE chain_id = ?1",
            params![CHAIN_ID],
        )
        .expect("update chain last used");
    }
}

fn write_shell_script(path: &Path, body: &str) {
    fs::write(path, body).expect("fixture script");
    let mut permissions = fs::metadata(path)
        .expect("fixture script metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("fixture script chmod");
}

fn shell_double_quoted(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn external_model(name: &str, provider_path: &Path) -> ModelConfig {
    ModelConfig {
        name: name.to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig::model_provider(PROVIDER_NAME, Vec::new())],
        inputs: Vec::new(),
        provider: Some(ProviderImplementationRef {
            path: Some(provider_path.display().to_string()),
            crate_name: None,
            version: None,
            binary: None,
            script: None,
        }),
    }
}

fn provider_identity() -> SessionServiceExternalProviderIdentity {
    SessionServiceExternalProviderIdentity {
        model_name: MODEL.to_string(),
        provider_name: PROVIDER_NAME.to_string(),
        provider_instance_id: Some(PROVIDER_INSTANCE_ID.to_string()),
        settings_id: SETTINGS_ID.to_string(),
    }
}

#[test]
fn no_ref_export_uses_dispatch_service_with_populated_registry_and_preserves_stdout_bytes() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = DispatchFixture::new();
    let metadata = resolve_export_session_metadata(SESSION_ID).expect("metadata");
    let direct =
        canonical_jsonl_bytes(&read_canonical_transcript(&metadata).expect("direct records"))
            .expect("direct bytes");
    let service =
        ProductionSessionExportService::with_registry_handle(fixture.unrelated_registry_handle());

    let output = service
        .export_session(SessionExportServiceRequest {
            session_id: SESSION_ID.to_string(),
            external_provider: None,
        })
        .expect("service output");

    assert_eq!(output.result.expect("export bytes"), direct);
    assert_provider_call_counts(&fixture, 0, 0, 0);
}

#[test]
fn no_ref_replace_uses_dispatch_service_with_populated_registry_and_preserves_host_state() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let direct_fixture = DispatchFixture::new();
    let direct_receipt =
        session_replace::run_import_replace(SESSION_ID, Some(&direct_fixture.input_path), None)
            .expect("direct replace");
    let direct = comparable_replace_state(&direct_fixture, direct_receipt);
    drop(direct_fixture);

    let dispatch_fixture = DispatchFixture::new();
    let service = ProductionSessionReplaceService::with_registry_handle(
        dispatch_fixture.unrelated_registry_handle(),
    );
    let output = service
        .replace_session(SessionReplaceServiceRequest {
            session_id: SESSION_ID.to_string(),
            source: ReplaceSource::File(dispatch_fixture.input_path.clone()),
            preimage_sha256: None,
            external_provider: None,
        })
        .expect("service output");
    let dispatch =
        comparable_replace_state(&dispatch_fixture, output.result.expect("replace receipt"));

    assert_eq!(dispatch, direct);
    assert_replacement_lineage_reset_to_null(
        &dispatch_fixture,
        "no-ref replace with canonical input parent_turn_id must reset all replacement DB lineage to NULL",
    );
    assert_no_ref_replacement_session_turn_rows(&dispatch_fixture);
    assert_provider_call_counts(&dispatch_fixture, 0, 0, 0);
}

#[test]
fn provider_ref_export_returns_external_bytes_without_sqlite_authority() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = DispatchFixture::new();
    fixture.write_model_file(true);
    fixture.set_mode("export_success");
    let before = fixture.sqlite_snapshot();
    let expected = fs::read(&fixture.input_path).expect("expected bytes");
    let service = ProductionSessionExportService::with_registry_handle(fixture.registry_handle());

    let output = service
        .export_session(SessionExportServiceRequest {
            session_id: SESSION_ID.to_string(),
            external_provider: Some(provider_identity()),
        })
        .expect("service output");

    assert_eq!(output.result.expect("export bytes"), expected);
    assert_eq!(fixture.sqlite_snapshot(), before);
    assert_provider_call_counts(&fixture, 1, 1, 0);
    let records = fixture.request_records_for("session.export");
    assert_export_request_shape(&records);
    assert!(
        !records[0]["request"].to_string().contains("state.db"),
        "export request must not expose host SQLite paths"
    );
}

#[test]
fn no_ref_export_matches_direct_bytes_with_unrelated_registry() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = DispatchFixture::new();
    let metadata = resolve_export_session_metadata(SESSION_ID).expect("metadata");
    let direct =
        canonical_jsonl_bytes(&read_canonical_transcript(&metadata).expect("direct records"))
            .expect("direct bytes");
    let service =
        ProductionSessionExportService::with_registry_handle(fixture.unrelated_registry_handle());

    let output = service
        .export_session(SessionExportServiceRequest {
            session_id: SESSION_ID.to_string(),
            external_provider: None,
        })
        .expect("service output");

    assert_eq!(output.result.expect("export bytes"), direct);
    assert_provider_call_counts(&fixture, 0, 0, 0);
}

#[test]
fn historical_ref_export_uses_external_bytes_and_failures_do_not_use_local_parser() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let success = DispatchFixture::new();
    success.write_model_file(true);
    success.write_native_transcript("local-only user", "local-only assistant");
    success.set_mode("export_success");
    let expected = fs::read(&success.input_path).expect("expected canonical bytes");
    let service = ProductionSessionExportService::with_registry_handle(success.registry_handle());

    let bytes = service
        .export_session(SessionExportServiceRequest {
            session_id: SESSION_ID.to_string(),
            external_provider: Some(provider_identity()),
        })
        .expect("service output")
        .result
        .expect("export bytes");

    assert_eq!(bytes, expected);
    assert!(!String::from_utf8_lossy(&bytes).contains("local-only"));
    assert_provider_call_counts(&success, 1, 1, 0);
    assert_export_request_shape(&success.request_records_for("session.export"));

    let failed = DispatchFixture::new();
    failed.write_model_file(true);
    failed.write_native_transcript("local-only user", "local-only assistant");
    failed.set_mode("export_provider_error");
    let before = failed.sqlite_snapshot();
    let service = ProductionSessionExportService::with_registry_handle(failed.registry_handle());

    let err = service
        .export_session(SessionExportServiceRequest {
            session_id: SESSION_ID.to_string(),
            external_provider: Some(provider_identity()),
        })
        .expect("service output")
        .result
        .expect_err("provider failure must not emit local bytes");

    assert_error_token(format!("{err:?}"), "provider_export_failed");
    assert_eq!(failed.sqlite_snapshot(), before);
    assert_provider_call_counts(&failed, 1, 1, 0);

    let transport = DispatchFixture::new();
    transport.write_model_file(true);
    transport.write_native_transcript("local-only user", "local-only assistant");
    let before = transport.sqlite_snapshot();
    let service = ProductionSessionExportService::with_registry_handle(
        transport.missing_provider_registry_handle(),
    );

    let err = service
        .export_session(SessionExportServiceRequest {
            session_id: SESSION_ID.to_string(),
            external_provider: Some(provider_identity()),
        })
        .expect("service output")
        .result
        .expect_err("transport failure must not emit local bytes");

    assert_error_token(format!("{err:?}"), "provider_transport_failure");
    assert_eq!(transport.sqlite_snapshot(), before);
    assert!(transport.records().is_empty());
}

#[test]
fn historical_no_ref_export_matches_local_parser_and_records_no_provider_calls() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = DispatchFixture::new();
    let metadata = resolve_export_session_metadata(SESSION_ID).expect("metadata");
    let direct =
        canonical_jsonl_bytes(&read_canonical_transcript(&metadata).expect("direct records"))
            .expect("direct bytes");
    let service =
        ProductionSessionExportService::with_registry_handle(fixture.unrelated_registry_handle());

    let bytes = service
        .export_session(SessionExportServiceRequest {
            session_id: SESSION_ID.to_string(),
            external_provider: None,
        })
        .expect("service output")
        .result
        .expect("export bytes");

    assert_eq!(bytes, direct);
    assert!(String::from_utf8_lossy(&bytes).contains("old-user"));
    assert_provider_call_counts(&fixture, 0, 0, 0);
}

#[test]
fn provider_ref_replace_success_uses_provider_owned_evidence_without_local_native_transcript() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = DispatchFixture::new();
    fixture.write_model_file(true);
    fixture.set_mode("replace_provider_owned_success");
    let expected_preimage = provider_owned_preimage_sha256();
    let canonical_bytes = fs::read(&fixture.input_path).expect("input bytes");
    let canonical_hash = sha256_hex(&canonical_bytes);
    let before_journal = fixture.journal_snapshot();
    fs::remove_file(&fixture.transcript_path).expect("remove local transcript");
    assert!(!fixture.transcript_path.exists());
    session_replace::reset_forbidden_helper_recorder();
    let service = ProductionSessionReplaceService::with_registry_handle(fixture.registry_handle());

    let receipt = service
        .replace_session(SessionReplaceServiceRequest {
            session_id: SESSION_ID.to_string(),
            source: ReplaceSource::File(fixture.input_path.clone()),
            preimage_sha256: Some(expected_preimage.clone()),
            external_provider: Some(provider_identity()),
        })
        .expect("service output")
        .result
        .expect("replace receipt");

    assert!(receipt.state_updated);
    assert_eq!(receipt.preimage_sha256, expected_preimage);
    assert_eq!(receipt.postimage_sha256, canonical_hash);
    assert_eq!(
        receipt.jsonl_path,
        PathBuf::from(provider_owned_source_id())
    );
    assert!(!fixture.transcript_path.exists());
    assert_eq!(fixture.journal_snapshot(), before_journal);
    assert_eq!(
        fixture.sqlite_snapshot().session_turns,
        expected_provider_owned_replacement_turn_rows()
    );
    assert_provider_owned_chain_state(&fixture);
    assert_provider_call_counts(&fixture, 1, 0, 1);
    assert_provider_owned_replace_request_shape(
        &fixture.request_records_for("session.replace"),
        &canonical_bytes,
        &expected_preimage,
    );
    assert_provider_requests_do_not_expose_sqlite_mutation_authority(&fixture);
    assert_forbidden_helper_counts_zero();
}

#[test]
fn provider_ref_replace_records_zero_forbidden_local_helper_calls() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = DispatchFixture::new();
    fixture.write_model_file(true);
    fixture.set_mode("replace_provider_owned_success");
    fs::remove_file(&fixture.transcript_path).expect("remove local transcript");
    session_replace::reset_forbidden_helper_recorder();
    let service = ProductionSessionReplaceService::with_registry_handle(fixture.registry_handle());

    let receipt = service
        .replace_session(SessionReplaceServiceRequest {
            session_id: SESSION_ID.to_string(),
            source: ReplaceSource::File(fixture.input_path.clone()),
            preimage_sha256: None,
            external_provider: Some(provider_identity()),
        })
        .expect("service output")
        .result
        .expect("replace receipt");

    assert!(receipt.state_updated);
    assert_forbidden_helper_counts_zero();

    let local = DispatchFixture::new();
    session_replace::reset_forbidden_helper_recorder();
    session_replace::run_import_replace(SESSION_ID, Some(&local.input_path), None)
        .expect("no-ref replace");
    assert_no_ref_local_helper_counts_positive();
}

#[test]
fn no_ref_replace_matches_direct_apply_with_unrelated_registry() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let direct_fixture = DispatchFixture::new();
    let direct_receipt =
        session_replace::run_import_replace(SESSION_ID, Some(&direct_fixture.input_path), None)
            .expect("direct replace");
    let direct = comparable_replace_state(&direct_fixture, direct_receipt);
    drop(direct_fixture);

    let dispatch_fixture = DispatchFixture::new();
    let service = ProductionSessionReplaceService::with_registry_handle(
        dispatch_fixture.unrelated_registry_handle(),
    );
    let output = service
        .replace_session(SessionReplaceServiceRequest {
            session_id: SESSION_ID.to_string(),
            source: ReplaceSource::File(dispatch_fixture.input_path.clone()),
            preimage_sha256: None,
            external_provider: None,
        })
        .expect("service output");
    let dispatch =
        comparable_replace_state(&dispatch_fixture, output.result.expect("replace receipt"));

    assert_eq!(dispatch, direct);
    assert_no_ref_replacement_session_turn_rows(&dispatch_fixture);
    assert_provider_call_counts(&dispatch_fixture, 0, 0, 0);
}

#[test]
fn historical_no_ref_replace_rewrites_host_state_and_resets_lineage_without_provider_calls() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let direct_fixture = DispatchFixture::new();
    session_replace::reset_forbidden_helper_recorder();
    let direct_receipt =
        session_replace::run_import_replace(SESSION_ID, Some(&direct_fixture.input_path), None)
            .expect("direct replace");
    assert_no_ref_local_helper_counts_positive();
    let direct = comparable_replace_state(&direct_fixture, direct_receipt);
    drop(direct_fixture);

    let dispatch_fixture = DispatchFixture::new();
    let before = host_mutation_snapshot(&dispatch_fixture);
    let service = ProductionSessionReplaceService::with_registry_handle(
        dispatch_fixture.unrelated_registry_handle(),
    );
    let output = service
        .replace_session(SessionReplaceServiceRequest {
            session_id: SESSION_ID.to_string(),
            source: ReplaceSource::File(dispatch_fixture.input_path.clone()),
            preimage_sha256: None,
            external_provider: None,
        })
        .expect("service output");
    let dispatch =
        comparable_replace_state(&dispatch_fixture, output.result.expect("replace receipt"));

    assert_eq!(dispatch, direct);
    assert_ne!(dispatch.0, before.0);
    assert_ne!(dispatch.1.session_turns, before.1.session_turns);
    assert_replacement_lineage_reset_to_null(
        &dispatch_fixture,
        "no-ref replace resets replacement lineage today",
    );
    assert_no_ref_replacement_session_turn_rows(&dispatch_fixture);
    assert_provider_call_counts(&dispatch_fixture, 0, 0, 0);
}

#[test]
fn external_export_success_validates_provider_bytes_request_shape_and_keeps_host_state_read_only() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = DispatchFixture::new();
    fixture.write_model_file(true);
    fixture.set_mode("export_success");
    let before = fixture.sqlite_snapshot();
    let expected = fs::read(&fixture.input_path).expect("expected canonical bytes");
    let service = ProductionSessionExportService::with_registry_handle(fixture.registry_handle());

    let output = service
        .export_session(SessionExportServiceRequest {
            session_id: SESSION_ID.to_string(),
            external_provider: Some(provider_identity()),
        })
        .expect("service output");

    assert_eq!(output.result.expect("export bytes"), expected);
    assert_eq!(fixture.sqlite_snapshot(), before);
    assert_provider_call_counts(&fixture, 1, 1, 0);
    assert_export_request_shape(&fixture.request_records_for("session.export"));
}

#[test]
fn external_export_provider_error_protocol_and_hash_failures_do_not_fallback_or_mutate_sqlite() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    for (mode, expected_token) in [
        ("session_capability_disabled", "session_capability_missing"),
        ("export_provider_error", "provider_export_failed"),
        ("export_nonzero", "export_nonzero_mode"),
        ("nonzero_no_envelope", "provider_process_nonzero"),
        ("export_empty_stdout", "empty_stdout"),
        ("export_malformed_json", "invalid_json"),
        ("export_trailing_junk", "trailing_non_whitespace"),
        ("export_schema_invalid", "schema_invalid_response"),
        ("export_invalid_base64", "invalid_base64"),
        ("export_wrong_format", "invalid_canonical_format"),
        ("export_hash_mismatch", "hash_mismatch"),
        ("export_count_mismatch", "turn_count_mismatch"),
        ("export_missing_source", "missing_source"),
    ] {
        let fixture = DispatchFixture::new();
        fixture.write_model_file(true);
        fixture.write_native_transcript("builtin fallback must not", "be used");
        fixture.set_mode(mode);
        let before = fixture.sqlite_snapshot();
        let service =
            ProductionSessionExportService::with_registry_handle(fixture.registry_handle());

        let err = service
            .export_session(SessionExportServiceRequest {
                session_id: SESSION_ID.to_string(),
                external_provider: Some(provider_identity()),
            })
            .expect("service output")
            .result
            .expect_err(mode);

        assert_error_token(format!("{err:?}"), expected_token);
        assert_eq!(fixture.sqlite_snapshot(), before, "{mode}");
        assert!(
            fixture.request_records_for("session.export").len() <= 1,
            "external export should make at most one provider export attempt for {mode}"
        );
    }
}

#[test]
fn external_export_malformed_canonical_jsonl_rejects_hash_valid_payload_without_fallback() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = DispatchFixture::new();
    fixture.write_model_file(true);
    fixture.write_native_transcript("builtin fallback must not", "be used");
    fixture.set_mode("export_malformed_canonical_jsonl");
    let before = fixture.sqlite_snapshot();
    let service = ProductionSessionExportService::with_registry_handle(fixture.registry_handle());

    let err = service
        .export_session(SessionExportServiceRequest {
            session_id: SESSION_ID.to_string(),
            external_provider: Some(provider_identity()),
        })
        .expect("service output")
        .result
        .expect_err("malformed canonical export bytes must be rejected");

    assert_error_token(format!("{err:?}"), "canonical_parse_count_mismatch");
    assert_eq!(fixture.sqlite_snapshot(), before);
    assert_provider_call_counts(&fixture, 1, 1, 0);
    assert_export_request_shape(&fixture.request_records_for("session.export"));
}

#[test]
fn external_export_transport_failure_does_not_mutate_sqlite_or_use_builtin_fallback() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = DispatchFixture::new();
    fixture.write_model_file(true);
    fixture.write_native_transcript("builtin fallback must not", "be used");
    let before = fixture.sqlite_snapshot();
    let service = ProductionSessionExportService::with_registry_handle(
        fixture.missing_provider_registry_handle(),
    );

    let err = service
        .export_session(SessionExportServiceRequest {
            session_id: SESSION_ID.to_string(),
            external_provider: Some(provider_identity()),
        })
        .expect("service output")
        .result
        .expect_err("missing provider binary must be an export transport failure");

    assert_error_token(format!("{err:?}"), "provider_transport_failure");
    assert_eq!(fixture.sqlite_snapshot(), before);
    assert!(
        fixture.records().is_empty(),
        "missing provider binary must fail through the export transport seam before provider code records a request"
    );
}

#[test]
fn external_export_requires_registry_handle_without_builtin_fallback() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = DispatchFixture::new();
    fixture.write_model_file(true);
    fixture.write_native_transcript("builtin fallback must not", "be used");
    let before = fixture.sqlite_snapshot();
    let service = ProductionSessionExportService::new();

    let err = service
        .export_session(SessionExportServiceRequest {
            session_id: SESSION_ID.to_string(),
            external_provider: Some(provider_identity()),
        })
        .expect("service output")
        .result
        .expect_err("external export without registry must fail");

    assert_error_token(format!("{err:?}"), "provider_registry_missing");
    assert_eq!(fixture.sqlite_snapshot(), before);
    assert!(fixture.records().is_empty());
}

#[test]
fn external_replace_success_uses_provider_transform_and_host_owned_apply_lifecycle() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = DispatchFixture::new();
    fixture.write_model_file(true);
    fixture.set_mode("replace_provider_owned_success");
    let preimage = provider_owned_preimage_sha256();
    let canonical_bytes = fs::read(&fixture.input_path).expect("input bytes");
    let canonical_hash = sha256_hex(&canonical_bytes);
    let before_journal = fixture.journal_snapshot();
    fs::remove_file(&fixture.transcript_path).expect("remove local transcript");
    session_replace::reset_forbidden_helper_recorder();
    let service = ProductionSessionReplaceService::with_registry_handle(fixture.registry_handle());

    let output = service
        .replace_session(SessionReplaceServiceRequest {
            session_id: SESSION_ID.to_string(),
            source: ReplaceSource::File(fixture.input_path.clone()),
            preimage_sha256: Some(preimage.clone()),
            external_provider: Some(provider_identity()),
        })
        .expect("service output");
    let receipt = output.result.expect("replace receipt");

    assert_eq!(receipt.session_id, SESSION_ID);
    assert_eq!(receipt.provider_name, PROVIDER_NAME);
    assert!(receipt.state_updated);
    assert_eq!(receipt.preimage_sha256, preimage);
    assert_eq!(receipt.postimage_sha256, canonical_hash);
    assert_eq!(
        receipt.jsonl_path,
        PathBuf::from(provider_owned_source_id())
    );
    assert!(!fixture.transcript_path.exists());
    assert_eq!(
        fixture.sqlite_snapshot().session_turns,
        expected_provider_owned_replacement_turn_rows()
    );
    assert_provider_owned_chain_state(&fixture);
    assert_eq!(fixture.journal_snapshot(), before_journal);
    assert_provider_call_counts(&fixture, 1, 0, 1);
    assert_provider_owned_replace_request_shape(
        &fixture.request_records_for("session.replace"),
        &canonical_bytes,
        &preimage,
    );
    assert_provider_requests_do_not_expose_sqlite_mutation_authority(&fixture);
    assert_forbidden_helper_counts_zero();
}

#[test]
fn provider_owned_success_publishes_updates_and_cleans_v2_journal_lifecycle() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());

    let observed = DispatchFixture::new();
    observed.write_model_file(true);
    observed.set_mode("replace_provider_owned_observe_pending_journal");
    session_replace::reset_forbidden_helper_recorder();
    let service = ProductionSessionReplaceService::with_registry_handle(observed.registry_handle());

    service
        .replace_session(SessionReplaceServiceRequest {
            session_id: SESSION_ID.to_string(),
            source: ReplaceSource::File(observed.input_path.clone()),
            preimage_sha256: Some(provider_owned_preimage_sha256()),
            external_provider: Some(provider_identity()),
        })
        .expect("service output")
        .result
        .expect("replace receipt");

    assert_pending_v2_journal_observed_before_provider_mutation(&observed);
    assert_eq!(observed.journal_snapshot(), Vec::<(String, Vec<u8>)>::new());
    assert_provider_owned_prepare_commit_flow(&observed);
    assert_forbidden_helper_counts_zero();

    let recovery_id_updated = DispatchFixture::new();
    recovery_id_updated.write_model_file(true);
    recovery_id_updated.set_mode("replace_provider_owned_prepared_success");
    let _hook = EnvVarGuard::set(
        "OULIPOLY_PROVIDER_OWNED_REPLACE_TEST_HOOK",
        "stop-after-recovery-id-journal-update",
    );
    let before_db = recovery_id_updated.sqlite_snapshot();
    let service = ProductionSessionReplaceService::with_registry_handle(
        recovery_id_updated.registry_handle(),
    );
    let err = service
        .replace_session(SessionReplaceServiceRequest {
            session_id: SESSION_ID.to_string(),
            source: ReplaceSource::File(recovery_id_updated.input_path.clone()),
            preimage_sha256: Some(provider_owned_preimage_sha256()),
            external_provider: Some(provider_identity()),
        })
        .expect("service output")
        .result
        .expect_err("deliberate interruption after recovery id journal update");
    assert_error_token(format!("{err:?}"), "stop-after-recovery-id-journal-update");
    assert_eq!(recovery_id_updated.sqlite_snapshot(), before_db);
    assert_provider_owned_pending_journal(&recovery_id_updated, "not_applied", true);
    drop(_hook);

    let durable_marked = DispatchFixture::new();
    durable_marked.write_model_file(true);
    durable_marked.set_mode("replace_provider_owned_prepared_success");
    let _hook = EnvVarGuard::set(
        "OULIPOLY_PROVIDER_OWNED_REPLACE_TEST_HOOK",
        "stop-after-db-apply-marker",
    );
    let service =
        ProductionSessionReplaceService::with_registry_handle(durable_marked.registry_handle());
    let err = service
        .replace_session(SessionReplaceServiceRequest {
            session_id: SESSION_ID.to_string(),
            source: ReplaceSource::File(durable_marked.input_path.clone()),
            preimage_sha256: Some(provider_owned_preimage_sha256()),
            external_provider: Some(provider_identity()),
        })
        .expect("service output")
        .result
        .expect_err("deliberate interruption after durable marker");
    assert_error_token(format!("{err:?}"), "stop-after-db-apply-marker");
    assert_eq!(
        durable_marked.sqlite_snapshot().session_turns,
        expected_provider_owned_replacement_turn_rows()
    );
    assert_provider_owned_pending_journal(&durable_marked, "applied", true);
}

#[test]
fn external_replace_provider_protocol_hash_plan_and_source_failures_do_not_mutate_host_state() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    for (mode, expected_token) in [
        ("session_capability_disabled", "session_capability_missing"),
        ("replace_provider_error", "provider_replace_failed"),
        ("replace_conflict", "provider_replace_conflict"),
        ("replace_missing_source", "missing_source"),
        ("replace_nonzero", "replace_nonzero_mode"),
        ("nonzero_no_envelope", "provider_process_nonzero"),
        ("replace_empty_stdout", "empty_stdout"),
        ("replace_malformed_json", "invalid_json"),
        ("replace_trailing_junk", "trailing_non_whitespace"),
        ("replace_schema_invalid", "schema_invalid_response"),
        (
            "replace_schema_invalid_error",
            "schema_invalid_error_response",
        ),
        (
            "replace_legacy_scalar_success_missing_m4_evidence",
            "missing_operation_id",
        ),
        ("replace_missing_operation_id", "missing_operation_id"),
        (
            "replace_operation_id_echo_mismatch",
            "operation_id_mismatch",
        ),
        ("replace_missing_recovery_id", "missing_recovery_id"),
        (
            "replace_missing_observed_preimage",
            "missing_preimage_sha256_observed",
        ),
        (
            "replace_corrupt_observed_preimage",
            "invalid_preimage_sha256_observed",
        ),
        (
            "replace_missing_canonical_postimage",
            "missing_canonical_postimage",
        ),
        (
            "replace_canonical_postimage_hash_mismatch",
            "canonical_postimage_hash_mismatch",
        ),
        (
            "replace_canonical_postimage_count_mismatch",
            "canonical_postimage_turn_count_mismatch",
        ),
        ("replace_missing_v2_plan_field", "schema_invalid_response"),
        (
            "replace_v2_plan_operation_id_mismatch",
            "host_state_plan_operation_id_mismatch",
        ),
        (
            "replace_v2_plan_observed_preimage_mismatch",
            "host_state_plan_preimage_mismatch",
        ),
        (
            "replace_v2_plan_missing_source_id",
            "schema_invalid_response",
        ),
        ("replace_recovery_evidence_mismatch", "recovery_id_mismatch"),
    ] {
        let fixture = DispatchFixture::new();
        fixture.write_model_file(true);
        fixture.set_mode(mode);
        let before = (fixture.transcript_bytes(), fixture.sqlite_snapshot());
        session_replace::reset_forbidden_helper_recorder();
        let service =
            ProductionSessionReplaceService::with_registry_handle(fixture.registry_handle());

        let err = service
            .replace_session(SessionReplaceServiceRequest {
                session_id: SESSION_ID.to_string(),
                source: ReplaceSource::File(fixture.input_path.clone()),
                preimage_sha256: Some(direct_export_hash()),
                external_provider: Some(provider_identity()),
            })
            .expect("service output")
            .result
            .expect_err(mode);

        assert_error_token(format!("{err:?}"), expected_token);
        assert_eq!(
            (fixture.transcript_bytes(), fixture.sqlite_snapshot()),
            before,
            "{mode}"
        );
        assert_forbidden_helper_counts_zero();
        assert_provider_requests_do_not_expose_sqlite_mutation_authority(&fixture);
    }
}

#[test]
fn external_replace_transport_failure_does_not_mutate_host_state_or_use_builtin_fallback() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = DispatchFixture::new();
    fixture.write_model_file(true);
    let before = host_mutation_snapshot(&fixture);
    session_replace::reset_forbidden_helper_recorder();
    let service = ProductionSessionReplaceService::with_registry_handle(
        fixture.missing_provider_registry_handle(),
    );

    let err = service
        .replace_session(SessionReplaceServiceRequest {
            session_id: SESSION_ID.to_string(),
            source: ReplaceSource::File(fixture.input_path.clone()),
            preimage_sha256: Some(direct_export_hash()),
            external_provider: Some(provider_identity()),
        })
        .expect("service output")
        .result
        .expect_err("missing provider binary must be a transport failure");

    assert_error_token(format!("{err:?}"), "provider_transport_failure");
    assert_eq!(host_mutation_snapshot(&fixture), before);
    assert_forbidden_helper_counts_zero();
    assert!(
        fixture.records().is_empty(),
        "missing provider binary must fail through the transport seam before provider code records a request"
    );
}

#[test]
fn external_replace_requires_registry_handle_before_host_mutation_or_builtin_fallback() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = DispatchFixture::new();
    fixture.write_model_file(true);
    let before = host_mutation_snapshot(&fixture);
    session_replace::reset_forbidden_helper_recorder();
    let service = ProductionSessionReplaceService::new();

    let err = service
        .replace_session(SessionReplaceServiceRequest {
            session_id: SESSION_ID.to_string(),
            source: ReplaceSource::File(fixture.input_path.clone()),
            preimage_sha256: Some(direct_export_hash()),
            external_provider: Some(provider_identity()),
        })
        .expect("service output")
        .result
        .expect_err("external replace without registry must fail");

    assert_error_token(format!("{err:?}"), "provider_registry_missing");
    assert_eq!(host_mutation_snapshot(&fixture), before);
    assert_forbidden_helper_counts_zero();
    assert!(fixture.records().is_empty());
}

#[test]
fn external_replace_session_busy_rejects_before_provider_mutates_storage() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = DispatchFixture::new();
    fixture.write_model_file(true);
    fixture.set_mode("replace_success");
    let before = host_mutation_snapshot(&fixture);
    let lock = SessionLock::new(&fixture.data_root.join("locks")).expect("session lock");
    let lease = lock
        .acquire(SESSION_ID, PROVIDER_NAME, Duration::from_secs(300))
        .expect("held lock");
    let service = ProductionSessionReplaceService::with_registry_handle(fixture.registry_handle());

    let err = service
        .replace_session(SessionReplaceServiceRequest {
            session_id: SESSION_ID.to_string(),
            source: ReplaceSource::File(fixture.input_path.clone()),
            preimage_sha256: Some(direct_export_hash()),
            external_provider: Some(provider_identity()),
        })
        .expect("service output")
        .result
        .expect_err("held lock must reject before provider invocation");

    match err {
        ReplaceError::SessionBusy { .. } => {}
        other => panic!("expected SessionBusy before provider dispatch, got {other:?}"),
    }
    assert_eq!(host_mutation_snapshot(&fixture), before);
    assert_provider_call_counts(&fixture, 1, 0, 0);
    lock.release(SESSION_ID, &lease.token)
        .expect("release held lock");
}

#[test]
fn external_replace_provider_owned_postimage_and_v2_plan_mismatch_fail_closed() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    for (mode, expected_token) in [
        (
            "replace_canonical_postimage_hash_mismatch",
            "canonical_postimage_hash_mismatch",
        ),
        (
            "replace_provider_owned_plan_postimage_mismatch",
            "host_state_plan_postimage_mismatch",
        ),
        (
            "replace_provider_owned_plan_input_mismatch",
            "host_state_plan_input_mismatch",
        ),
    ] {
        let fixture = DispatchFixture::new();
        fixture.write_model_file(true);
        fixture.set_mode(mode);
        let before_db = fixture.sqlite_snapshot();
        session_replace::reset_forbidden_helper_recorder();
        let service =
            ProductionSessionReplaceService::with_registry_handle(fixture.registry_handle());

        let err = service
            .replace_session(SessionReplaceServiceRequest {
                session_id: SESSION_ID.to_string(),
                source: ReplaceSource::File(fixture.input_path.clone()),
                preimage_sha256: Some(provider_owned_preimage_sha256()),
                external_provider: Some(provider_identity()),
            })
            .expect("service output")
            .result
            .expect_err(mode);

        assert_error_token(format!("{err:?}"), expected_token);
        assert_eq!(fixture.sqlite_snapshot(), before_db, "{mode}");
        assert_provider_call_counts(&fixture, 1, 0, 1);
        assert_provider_requests_do_not_expose_sqlite_mutation_authority(&fixture);
        assert_forbidden_helper_counts_zero();
    }
}

#[test]
fn external_replace_provider_owned_validation_failure_keeps_recovery_journal_without_local_rollback()
 {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = DispatchFixture::new();
    fixture.write_model_file(true);
    fixture.set_mode("replace_provider_owned_plan_postimage_mismatch");
    let before_db = fixture.sqlite_snapshot();
    session_replace::reset_forbidden_helper_recorder();
    let service = ProductionSessionReplaceService::with_registry_handle(fixture.registry_handle());

    let err = service
        .replace_session(SessionReplaceServiceRequest {
            session_id: SESSION_ID.to_string(),
            source: ReplaceSource::File(fixture.input_path.clone()),
            preimage_sha256: Some(provider_owned_preimage_sha256()),
            external_provider: Some(provider_identity()),
        })
        .expect("service output")
        .result
        .expect_err("provider-owned v2 plan mismatch");

    assert_error_token(format!("{err:?}"), "host_state_plan_postimage_mismatch");
    assert_eq!(fixture.sqlite_snapshot(), before_db);
    assert_provider_owned_pending_journal(&fixture, "not_applied", true);
    assert_provider_call_counts(&fixture, 1, 0, 1);
    assert_provider_requests_do_not_expose_sqlite_mutation_authority(&fixture);
    assert_forbidden_helper_counts_zero();
}

#[test]
fn external_replace_provider_ref_db_identity_missing_or_ambiguous_fails_before_db_apply() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    for (label, mutate, expected_token) in [
        (
            "missing_active_segment",
            DispatchFixture::delete_active_segment_for_provider_session as fn(&DispatchFixture),
            "provider_db_identity_missing",
        ),
        (
            "ambiguous_active_segment",
            DispatchFixture::insert_ambiguous_active_segment_for_provider_session
                as fn(&DispatchFixture),
            "provider_db_identity_ambiguous",
        ),
    ] {
        let fixture = DispatchFixture::new();
        fixture.write_model_file(true);
        fixture.set_mode("replace_provider_owned_success");
        mutate(&fixture);
        let before_db = fixture.sqlite_snapshot();
        session_replace::reset_forbidden_helper_recorder();
        let service =
            ProductionSessionReplaceService::with_registry_handle(fixture.registry_handle());

        let err = service
            .replace_session(SessionReplaceServiceRequest {
                session_id: SESSION_ID.to_string(),
                source: ReplaceSource::File(fixture.input_path.clone()),
                preimage_sha256: Some(provider_owned_preimage_sha256()),
                external_provider: Some(provider_identity()),
            })
            .expect("service output")
            .result
            .expect_err(label);

        assert_error_token(format!("{err:?}"), expected_token);
        assert_eq!(fixture.sqlite_snapshot(), before_db, "{label}");
        assert_provider_call_counts(&fixture, 1, 0, 1);
        assert_provider_requests_do_not_expose_sqlite_mutation_authority(&fixture);
        assert_forbidden_helper_counts_zero();
    }
}

#[test]
fn external_replace_preimage_mismatch_rejects_stale_write_without_host_mutation() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = DispatchFixture::new();
    fixture.write_model_file(true);
    fixture.set_mode("replace_provider_owned_conflict_observed_preimage_mismatch");
    let before = host_mutation_snapshot(&fixture);
    session_replace::reset_forbidden_helper_recorder();
    let service = ProductionSessionReplaceService::with_registry_handle(fixture.registry_handle());

    let err = service
        .replace_session(SessionReplaceServiceRequest {
            session_id: SESSION_ID.to_string(),
            source: ReplaceSource::File(fixture.input_path.clone()),
            preimage_sha256: Some(
                "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            ),
            external_provider: Some(provider_identity()),
        })
        .expect("service output")
        .result
        .expect_err("provider-owned conflict response for stale expected preimage");

    assert_error_token(format!("{err:?}"), "provider_replace_conflict");
    assert_eq!(host_mutation_snapshot(&fixture), before);
    assert_provider_call_counts(&fixture, 1, 0, 1);
    assert_provider_owned_preimage_conflict_observed(&fixture);
    assert_provider_requests_do_not_expose_sqlite_mutation_authority(&fixture);
    assert_forbidden_helper_counts_zero();
}

#[test]
fn external_replace_rejects_malformed_canonical_input_before_provider_invocation() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    for (label, input) in [
        ("empty", String::new()),
        (
            "invalid_timestamp",
            invalid_canonical_input_jsonl(|record| {
                record["timestamp"] = Value::String("not-a-timestamp".to_string());
            }),
        ),
        (
            "empty_required_field",
            invalid_canonical_input_jsonl(|record| {
                record["provider_name"] = Value::String(String::new());
            }),
        ),
        (
            "unsupported_only",
            invalid_canonical_input_jsonl(|record| {
                record["role"] = Value::String("tool".to_string());
                record["unsupported_record"] = Value::Bool(true);
            }),
        ),
    ] {
        let fixture = DispatchFixture::new();
        fixture.write_model_file(true);
        fixture.set_mode("replace_provider_owned_success");
        fs::write(&fixture.input_path, input).expect("invalid input");
        let before = host_mutation_snapshot(&fixture);
        session_replace::reset_forbidden_helper_recorder();
        let service =
            ProductionSessionReplaceService::with_registry_handle(fixture.registry_handle());

        let err = service
            .replace_session(SessionReplaceServiceRequest {
                session_id: SESSION_ID.to_string(),
                source: ReplaceSource::File(fixture.input_path.clone()),
                preimage_sha256: Some(direct_export_hash()),
                external_provider: Some(provider_identity()),
            })
            .expect("service output")
            .result
            .expect_err(label);

        assert_error_token(format!("{err:?}"), "InvalidInputTranscript");
        assert_eq!(host_mutation_snapshot(&fixture), before, "{label}");
        assert_provider_call_counts(&fixture, 0, 0, 0);
        assert_forbidden_helper_counts_zero();
    }
}

#[test]
fn external_replace_provider_ref_accepts_nonrenderable_canonical_input_and_dispatches() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = DispatchFixture::new();
    fixture.write_model_file(true);
    fixture.set_mode("replace_provider_owned_success");
    let input = invalid_canonical_input_jsonl(|record| {
        record["content"] = json!([{"type": "image"}]);
    });
    fs::write(&fixture.input_path, input.as_bytes()).expect("nonrenderable canonical input");
    session_replace::reset_forbidden_helper_recorder();
    let service = ProductionSessionReplaceService::with_registry_handle(fixture.registry_handle());

    let receipt = service
        .replace_session(SessionReplaceServiceRequest {
            session_id: SESSION_ID.to_string(),
            source: ReplaceSource::File(fixture.input_path.clone()),
            preimage_sha256: Some(provider_owned_preimage_sha256()),
            external_provider: Some(provider_identity()),
        })
        .expect("service output")
        .result
        .expect("provider-ref accepts canonical content that no-ref rendering rejects");

    assert!(receipt.state_updated);
    assert_eq!(receipt.postimage_sha256, sha256_hex(input.as_bytes()));
    assert_eq!(
        fixture.sqlite_snapshot().session_turns,
        expected_nonrenderable_provider_owned_rows()
    );
    assert_provider_call_counts(&fixture, 1, 0, 1);
    assert_provider_owned_replace_request_shape(
        &fixture.request_records_for("session.replace"),
        input.as_bytes(),
        &provider_owned_preimage_sha256(),
    );
    assert_forbidden_helper_counts_zero();
}

#[test]
fn external_replace_provider_ref_rejects_invalid_target_uuid_and_session_id_mismatch_before_dispatch()
 {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    for (label, target_session_id, input, expected_token) in [
        (
            "invalid_target_uuid",
            "not-a-uuid".to_string(),
            canonical_input_jsonl(Path::new("/tmp/valid-canonical.jsonl")),
            "invalid_session_id",
        ),
        (
            "canonical_session_id_mismatch",
            SESSION_ID.to_string(),
            invalid_canonical_input_jsonl(|record| {
                record["session_id"] =
                    Value::String("99999999-9999-4999-8999-999999999999".to_string());
            }),
            "canonical_session_id_mismatch",
        ),
    ] {
        let fixture = DispatchFixture::new();
        fixture.write_model_file(true);
        fixture.set_mode("replace_provider_owned_success");
        fs::write(&fixture.input_path, input).expect("provider-ref invalid input");
        let before = host_mutation_snapshot(&fixture);
        session_replace::reset_forbidden_helper_recorder();
        let service =
            ProductionSessionReplaceService::with_registry_handle(fixture.registry_handle());

        let err = service
            .replace_session(SessionReplaceServiceRequest {
                session_id: target_session_id,
                source: ReplaceSource::File(fixture.input_path.clone()),
                preimage_sha256: Some(provider_owned_preimage_sha256()),
                external_provider: Some(provider_identity()),
            })
            .expect("service output")
            .result
            .expect_err(label);

        assert_error_token(format!("{err:?}"), expected_token);
        assert_eq!(host_mutation_snapshot(&fixture), before, "{label}");
        assert_provider_call_counts(&fixture, 0, 0, 0);
        assert_forbidden_helper_counts_zero();
    }
}

#[test]
fn startup_pending_replace_recovery_is_host_owned_and_ordered_before_provider_dispatch() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    assert_tauri_startup_recovery_order_and_host_ownership();

    let completed = completed_replace_postimage_fixture();
    let fixture = DispatchFixture::new();
    fixture.write_model_file(true);
    fixture.set_mode("replace_success");
    fixture.seed_pending_replace_recovery(&completed.transcript_bytes, &completed.postimage_sha256);

    session_replace::recover_pending_replaces().expect("pending replace recovery");

    assert_eq!(fixture.transcript_bytes(), completed.transcript_bytes);
    assert_eq!(
        fixture.sqlite_snapshot().session_turns,
        expected_replacement_turn_rows(&fixture)
    );
    assert_eq!(fixture.journal_snapshot(), Vec::<(String, Vec<u8>)>::new());
    assert_provider_call_counts(&fixture, 0, 0, 0);
}

#[test]
fn startup_provider_owned_recovery_is_after_registry_and_before_normal_dispatch() {
    assert_tauri_provider_owned_startup_recovery_order_after_registry();
}

#[test]
fn provider_owned_recovery_rolls_forward_with_db_preimage_only_and_no_local_snapshot() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = DispatchFixture::new();
    fixture.write_model_file(true);
    fixture.set_mode("recovery_prepared_before_db_apply");
    fs::write(
        &fixture.transcript_path,
        b"local transcript is not recovery authority\n",
    )
    .expect("non-authoritative local transcript");
    fixture.seed_provider_owned_v2_pending_replace_journal(
        "not_applied",
        Some(provider_owned_recovery_id().as_str()),
    );
    session_replace::reset_forbidden_helper_recorder();

    oulipoly_runtime::session_external_provider::recover_pending_provider_owned_replaces(
        fixture.registry_handle(),
    )
    .expect("provider-owned pending replace recovery");

    assert_eq!(
        fixture.sqlite_snapshot().session_turns,
        expected_provider_owned_replacement_turn_rows()
    );
    assert_provider_owned_chain_state(&fixture);
    assert_eq!(fixture.journal_snapshot(), Vec::<(String, Vec<u8>)>::new());
    assert_recorded_recovery_actions(&fixture, &["query", "commit"]);
    assert_forbidden_helper_counts_zero();
}

#[test]
fn provider_owned_recovery_rolls_back_db_apply_from_durable_db_preimage_without_local_restore() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = DispatchFixture::new();
    fixture.write_model_file(true);
    fixture.set_mode("recovery_rollback_retry");
    let db_preimage = fixture.sqlite_snapshot();
    fixture.replace_db_rows_with_provider_owned_postimage();
    fs::write(
        &fixture.transcript_path,
        b"provider storage is not host rollback authority\n",
    )
    .expect("provider storage marker");
    fixture.seed_provider_owned_v2_pending_replace_journal_with_preimage(
        "applied",
        Some(provider_owned_recovery_id().as_str()),
        db_preimage.clone(),
    );
    session_replace::reset_forbidden_helper_recorder();

    oulipoly_runtime::session_external_provider::recover_pending_provider_owned_replaces(
        fixture.registry_handle(),
    )
    .expect("provider-owned rollback recovery");

    assert_eq!(fixture.sqlite_snapshot(), db_preimage);
    assert_eq!(fixture.journal_snapshot(), Vec::<(String, Vec<u8>)>::new());
    assert_recorded_recovery_actions(&fixture, &["query", "rollback"]);
    assert_forbidden_helper_counts_zero();
}

#[test]
fn provider_owned_v2_recovery_is_idempotent_across_prepare_commit_atomic_and_rollback() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    for (mode, marker, expected_actions) in [
        (
            "recovery_prepared_before_db_apply",
            "not_applied",
            vec!["query", "commit"],
        ),
        (
            "recovery_db_applied_before_commit",
            "applied",
            vec!["query", "commit"],
        ),
        ("recovery_atomic_before_cleanup", "applied", vec!["query"]),
        (
            "recovery_rollback_retry",
            "not_applied",
            vec!["query", "rollback"],
        ),
    ] {
        let fixture = DispatchFixture::new();
        fixture.write_model_file(true);
        fixture.set_mode(mode);
        fixture.seed_provider_owned_v2_pending_replace_journal(
            marker,
            Some(provider_owned_recovery_id().as_str()),
        );
        let before_recovery_db = fixture.sqlite_snapshot();
        assert_provider_owned_pending_journal(&fixture, marker, true);
        session_replace::reset_forbidden_helper_recorder();

        oulipoly_runtime::session_external_provider::recover_pending_provider_owned_replaces(
            fixture.registry_handle(),
        )
        .expect(mode);

        assert_eq!(
            fixture.journal_snapshot(),
            Vec::<(String, Vec<u8>)>::new(),
            "{mode}"
        );
        assert_forbidden_helper_counts_zero();
        assert_recorded_recovery_actions(&fixture, &expected_actions);
        if mode == "recovery_rollback_retry" {
            assert_eq!(
                fixture.sqlite_snapshot(),
                before_recovery_db,
                "rollback retry must leave the DB preimage intact"
            );
        } else {
            assert_eq!(
                fixture.sqlite_snapshot().session_turns,
                expected_provider_owned_replacement_turn_rows(),
                "{mode}"
            );
            assert_provider_owned_chain_state(&fixture);
        }
        let after_first = host_mutation_snapshot(&fixture);
        let calls_after_first = fixture.records().len();

        oulipoly_runtime::session_external_provider::recover_pending_provider_owned_replaces(
            fixture.registry_handle(),
        )
        .expect("second provider-owned recovery pass is a no-op");

        assert_eq!(host_mutation_snapshot(&fixture), after_first, "{mode}");
        assert_eq!(fixture.records().len(), calls_after_first, "{mode}");
    }
}

#[test]
fn provider_owned_v2_recovery_provider_unavailable_keeps_journal_without_local_restore() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = DispatchFixture::new();
    fixture.write_model_file(true);
    fixture.seed_provider_owned_v2_pending_replace_journal(
        "not_applied",
        Some(provider_owned_recovery_id().as_str()),
    );
    let before = host_mutation_snapshot(&fixture);
    session_replace::reset_forbidden_helper_recorder();

    let err = oulipoly_runtime::session_external_provider::recover_pending_provider_owned_replaces(
        fixture.missing_provider_registry_handle(),
    )
    .expect_err("provider-owned recovery must fail closed when provider is unavailable");

    assert_error_token(format!("{err:?}"), "provider_owned_recovery_unavailable");
    assert_eq!(host_mutation_snapshot(&fixture), before);
    assert_provider_owned_pending_journal(&fixture, "not_applied", true);
    assert_forbidden_helper_counts_zero();
}

#[test]
fn legacy_v1_recovery_is_idempotent_and_skips_provider_owned_v2_journals() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let completed = completed_replace_postimage_fixture();
    let fixture = DispatchFixture::new();
    fixture.write_model_file(true);
    fixture.seed_pending_replace_recovery(&completed.transcript_bytes, &completed.postimage_sha256);
    let v2_path = fixture
        .data_root
        .join("replace_journal")
        .join(format!("session-{SESSION_ID}.provider-owned.pending"));
    fs::write(
        &v2_path,
        serde_json::to_vec_pretty(&json!({
            "schema_version": 2,
            "operation": "provider-owned-import-replace",
            "operation_id": provider_owned_operation_id(),
            "provider_name": PROVIDER_NAME,
            "session_id": SESSION_ID,
            "db_apply_marker": "not_applied"
        }))
        .expect("v2 sidecar json"),
    )
    .expect("v2 sidecar journal");

    session_replace::recover_pending_replaces().expect("legacy recovery first pass");
    assert_eq!(fixture.transcript_bytes(), completed.transcript_bytes);
    assert!(
        v2_path.exists(),
        "legacy scanner must leave v2 journal for provider-aware recovery"
    );
    assert_eq!(
        tree_snapshot(&fixture.data_root.join("replace_journal").join("quarantine")),
        Vec::<(String, Vec<u8>)>::new(),
        "legacy scanner must not quarantine v2 provider-owned journals"
    );
    let after_first = host_mutation_snapshot(&fixture);

    session_replace::recover_pending_replaces().expect("legacy recovery second pass");

    assert_eq!(host_mutation_snapshot(&fixture), after_first);
    assert_provider_call_counts(&fixture, 0, 0, 0);
}

#[test]
fn external_replace_dry_run_no_change_returns_no_state_update_and_no_host_mutation() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = DispatchFixture::new();
    fixture.write_model_file(true);
    fixture.set_mode("replace_no_change");
    let before = host_mutation_snapshot(&fixture);
    let service = ProductionSessionReplaceService::with_registry_handle(fixture.registry_handle());

    let receipt = service
        .replace_session(SessionReplaceServiceRequest {
            session_id: SESSION_ID.to_string(),
            source: ReplaceSource::File(fixture.input_path.clone()),
            preimage_sha256: Some(direct_export_hash()),
            external_provider: Some(provider_identity()),
        })
        .expect("service output")
        .result
        .expect("dry-run/no-change receipt");

    assert!(!receipt.state_updated);
    assert_eq!(host_mutation_snapshot(&fixture), before);
    assert_provider_call_counts(&fixture, 1, 0, 1);
}

#[test]
fn no_new_concrete_provider_name_grep_hits_are_introduced_against_base_ref() {
    let root = repo_root();
    let base = grep_snapshot(&root, &grep_scope_args(Some(BASE_REF), false));
    let current = grep_snapshot(&root, &grep_scope_args(None, true));
    let new_hits = current.difference(&base).cloned().collect::<Vec<_>>();

    assert!(
        new_hits.is_empty(),
        "AGE-244 S7b must not introduce concrete built-in provider-name grep hits: {new_hits:#?}"
    );
}

fn grep_scope_args(base_ref: Option<&'static str>, include_untracked: bool) -> Vec<&'static str> {
    let mut args = Vec::new();
    if include_untracked {
        args.push("--untracked");
    }
    if let Some(base_ref) = base_ref {
        args.push(base_ref);
    }
    args.extend([
        "--",
        ".",
        ":(exclude)planning/code-quality-sweep/**",
        ":(exclude)planning/*-gate/**",
        ":(exclude)planning/s10-moveout/**",
        ":(exclude)planning/wu-e/**",
        ":(exclude)planning/opencode-contract/**",
    ]);
    args
}

fn comparable_replace_state(
    fixture: &DispatchFixture,
    receipt: ReplaceReceipt,
) -> ComparableReplaceState {
    (
        fixture.transcript_bytes(),
        fixture.sqlite_snapshot(),
        fixture.journal_snapshot(),
        receipt_snapshot(receipt),
    )
}

fn receipt_snapshot(receipt: ReplaceReceipt) -> ReceiptSnapshot {
    ReceiptSnapshot {
        session_id: receipt.session_id,
        provider_name: receipt.provider_name,
        storage_type: receipt.storage_type,
        operation: receipt.operation,
        state_updated: receipt.state_updated,
        preimage_len: receipt.preimage_sha256.len(),
        postimage_len: receipt.postimage_sha256.len(),
    }
}

fn host_mutation_snapshot(fixture: &DispatchFixture) -> HostMutationSnapshot {
    (
        fixture.transcript_bytes(),
        fixture.sqlite_snapshot(),
        fixture.journal_snapshot(),
    )
}

fn direct_export_hash() -> String {
    let metadata = resolve_export_session_metadata(SESSION_ID).expect("metadata");
    let bytes = canonical_jsonl_bytes(&read_canonical_transcript(&metadata).expect("records"))
        .expect("canonical bytes");
    sha256_hex(&bytes)
}

struct CompletedReplacePostimage {
    transcript_bytes: Vec<u8>,
    postimage_sha256: String,
}

fn completed_replace_postimage_fixture() -> CompletedReplacePostimage {
    let fixture = DispatchFixture::new();
    let receipt = session_replace::run_import_replace(SESSION_ID, Some(&fixture.input_path), None)
        .expect("completed replace fixture");
    CompletedReplacePostimage {
        transcript_bytes: fixture.transcript_bytes(),
        postimage_sha256: receipt.postimage_sha256,
    }
}

fn assert_provider_call_counts(
    fixture: &DispatchFixture,
    describe: usize,
    export: usize,
    replace: usize,
) {
    assert_eq!(fixture.request_records_for("describe").len(), describe);
    assert_eq!(fixture.request_records_for("session.export").len(), export);
    assert_eq!(
        fixture.request_records_for("session.replace").len(),
        replace
    );
}

fn assert_export_request_shape(records: &[Value]) {
    assert_eq!(records.len(), 1, "expected one export request");
    let request = &records[0]["request"];
    assert_eq!(records[0]["subcommand"], "session.export");
    assert_eq!(request["provider_instance_id"], PROVIDER_INSTANCE_ID);
    assert_eq!(request["params"]["settings_id"], SETTINGS_ID);
    assert_eq!(request["params"]["session_id"], SESSION_ID);
    assert!(
        request.to_string().contains("provider-alpha"),
        "request should carry neutral provider identity evidence: {request}"
    );
}

fn assert_replace_request_shape(records: &[Value], canonical_bytes: &[u8], preimage: &str) {
    assert_eq!(records.len(), 1, "expected one replace request");
    let request = &records[0]["request"];
    assert_eq!(records[0]["subcommand"], "session.replace");
    assert_eq!(request["provider_instance_id"], PROVIDER_INSTANCE_ID);
    assert_eq!(request["params"]["settings_id"], SETTINGS_ID);
    assert_eq!(request["params"]["session_id"], SESSION_ID);
    assert_eq!(
        request["params"]["canonical_format"],
        "oulipoly.canonical_transcript/v1"
    );
    assert_eq!(
        request["params"]["data_base64"],
        BASE64.encode(canonical_bytes)
    );
    assert_eq!(request["params"]["preimage_sha256"], preimage);
}

fn assert_provider_owned_replace_request_shape(
    records: &[Value],
    canonical_bytes: &[u8],
    preimage_expected: &str,
) {
    assert!(!records.is_empty(), "expected at least one replace request");
    let request = &records[0]["request"];
    let params = &request["params"];
    assert_eq!(records[0]["subcommand"], "session.replace");
    assert_eq!(request["provider_instance_id"], PROVIDER_INSTANCE_ID);
    assert_eq!(params["settings_id"], SETTINGS_ID);
    assert_eq!(params["session_id"], SESSION_ID);
    assert_eq!(params["model_name"], MODEL);
    assert_eq!(params["provider_name"], PROVIDER_NAME);
    assert_eq!(
        params["replace_protocol"],
        provider_owned_replace_protocol()
    );
    assert_eq!(params["operation_id"], provider_owned_operation_id());
    assert!(
        params.get("operation_mode").is_none(),
        "initial provider-owned replace must not be a recovery request: {params}"
    );
    assert!(
        params.get("recovery_action").is_none(),
        "initial provider-owned replace must not carry recovery action: {params}"
    );
    assert_eq!(
        params["canonical_format"],
        "oulipoly.canonical_transcript/v1"
    );
    assert_eq!(
        params["host_apply_capability"],
        "replace_session_turns_from_canonical_v1"
    );
    assert_eq!(params["preimage_sha256_expected"], preimage_expected);
    assert!(
        params.get("preimage_sha256").is_none(),
        "request must not send a host-observed preimage hash: {params}"
    );
    assert!(
        params.get("preimage_sha256_observed").is_none(),
        "request must not echo provider-observed preimage evidence: {params}"
    );
    assert_eq!(params["canonical_transcript"]["kind"], "bytes");
    assert_eq!(
        params["canonical_transcript"]["data_base64"],
        BASE64.encode(canonical_bytes)
    );
    assert_eq!(
        params["canonical_transcript"]["sha256"],
        sha256_hex(canonical_bytes)
    );
    assert_eq!(
        params["canonical_transcript"]["turn_count"],
        canonical_bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .count()
    );
}

fn assert_provider_owned_prepare_commit_flow(fixture: &DispatchFixture) {
    let records = fixture.request_records_for("session.replace");
    assert_eq!(
        records.len(),
        2,
        "prepared success must commit through provider recovery mode"
    );
    let canonical_bytes = fs::read(&fixture.input_path).expect("input bytes");
    assert_provider_owned_replace_request_shape(
        &records,
        &canonical_bytes,
        &provider_owned_preimage_sha256(),
    );
    let commit = &records[1]["request"];
    let params = &commit["params"];
    assert_eq!(records[1]["subcommand"], "session.replace");
    assert_eq!(commit["provider_instance_id"], PROVIDER_INSTANCE_ID);
    assert_eq!(
        params["replace_protocol"],
        provider_owned_replace_protocol()
    );
    assert_eq!(params["operation_id"], provider_owned_operation_id());
    assert_eq!(params["recovery_id"], provider_owned_recovery_id());
    assert_eq!(params["operation_mode"], "recover");
    assert_eq!(params["recovery_action"], "commit");
}

fn assert_provider_owned_preimage_conflict_observed(fixture: &DispatchFixture) {
    let records = fixture.request_records_for("session.replace");
    assert_provider_owned_replace_request_shape(
        &records,
        &fs::read(&fixture.input_path).expect("input bytes"),
        "0000000000000000000000000000000000000000000000000000000000000000",
    );
    let observations = fixture.request_records_for("session.replace.preimage_conflict_observed");
    assert_eq!(
        observations.len(),
        1,
        "provider must report that it owned stale preimage conflict detection"
    );
    assert_eq!(
        observations[0]["observation"]["preimage_sha256_expected"],
        "0000000000000000000000000000000000000000000000000000000000000000"
    );
    assert_eq!(
        observations[0]["observation"]["preimage_sha256_observed"],
        provider_owned_preimage_sha256()
    );
}

fn assert_forbidden_helper_counts_zero() {
    let counts = session_replace::forbidden_helper_call_counts();
    assert_eq!(
        counts.resolve_replace_metadata, 0,
        "provider-ref must not resolve local metadata"
    );
    assert_eq!(
        counts.canonical_records_from_provider_file, 0,
        "provider-ref must not parse native provider files"
    );
    assert_eq!(
        counts.render_for_storage, 0,
        "provider-ref must not enter built-in render fallback"
    );
}

fn assert_no_ref_local_helper_counts_positive() {
    let counts = session_replace::forbidden_helper_call_counts();
    assert!(
        counts.resolve_replace_metadata > 0,
        "no-ref must still resolve local metadata"
    );
    assert!(
        counts.canonical_records_from_provider_file > 0,
        "no-ref must still verify through the local provider-file parser"
    );
    assert!(
        counts.render_for_storage > 0,
        "no-ref must still use the built-in renderer"
    );
}

fn assert_provider_owned_chain_state(fixture: &DispatchFixture) {
    let snapshot = fixture.sqlite_snapshot();
    assert_eq!(
        snapshot.session_chains,
        vec![(
            CHAIN_ID.to_string(),
            "2026-05-01T00:00:00Z".to_string(),
            "2026-05-01T01:00:01.000Z".to_string(),
            MODEL.to_string(),
        ),]
    );
    assert_eq!(
        snapshot.session_chain_segments[0].6,
        Some("valid-turn-2".to_string())
    );
}

fn assert_recorded_recovery_actions(fixture: &DispatchFixture, expected: &[&str]) {
    let actions = fixture
        .request_records_for("session.replace")
        .into_iter()
        .filter_map(|record| {
            let params = &record["request"]["params"];
            (params["operation_mode"] == "recover").then(|| {
                assert_eq!(
                    params["replace_protocol"],
                    provider_owned_replace_protocol()
                );
                assert_eq!(params["operation_id"], provider_owned_operation_id());
                params["recovery_action"]
                    .as_str()
                    .expect("recovery action")
                    .to_string()
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(actions, expected, "recorded provider recovery requests");
}

fn assert_provider_owned_pending_journal(
    fixture: &DispatchFixture,
    expected_marker: &str,
    expect_recovery_id: bool,
) {
    let journals = fixture.journal_snapshot();
    let pending = journals
        .iter()
        .find(|(name, _)| name.ends_with(".pending"))
        .unwrap_or_else(|| panic!("expected pending provider-owned journal: {journals:#?}"));
    let json: Value = serde_json::from_slice(&pending.1).expect("pending journal json");
    assert_eq!(json["schema_version"], 2);
    assert_eq!(json["operation"], "provider-owned-import-replace");
    assert_eq!(json["operation_id"], provider_owned_operation_id());
    assert_provider_owned_journal_db_preimage_and_identity(&json, fixture, expected_marker);
    if expect_recovery_id {
        assert_eq!(json["recovery_id"], provider_owned_recovery_id());
    } else {
        assert!(
            json.get("recovery_id").is_none(),
            "recovery id must not be present before provider response: {json}"
        );
    }
}

fn assert_pending_v2_journal_observed_before_provider_mutation(fixture: &DispatchFixture) {
    let observations = fixture.request_records_for("session.replace.journal_observed");
    assert_eq!(
        observations.len(),
        1,
        "expected one provider journal observation"
    );
    let entries = observations[0]["observation"]["entries"]
        .as_array()
        .expect("journal observation entries");
    let pending = entries
        .iter()
        .find(|entry| {
            entry["name"]
                .as_str()
                .is_some_and(|name| name.ends_with(".pending"))
        })
        .unwrap_or_else(|| panic!("provider did not observe pending v2 journal: {entries:#?}"));
    let json = &pending["json"];
    assert_eq!(json["schema_version"], 2);
    assert_eq!(json["operation"], "provider-owned-import-replace");
    assert_eq!(json["operation_id"], provider_owned_operation_id());
    assert_provider_owned_journal_db_preimage_and_identity(json, fixture, "not_applied");
    assert!(
        json.get("recovery_id").is_none(),
        "provider must observe journal publication before response recovery id is recorded: {json}"
    );
}

fn assert_provider_owned_journal_db_preimage_and_identity(
    json: &Value,
    fixture: &DispatchFixture,
    expected_marker: &str,
) {
    assert_eq!(json["settings_id"], SETTINGS_ID);
    assert_eq!(json["model_name"], MODEL);
    assert_eq!(json["provider_name"], PROVIDER_NAME);
    assert_eq!(json["provider_instance_id"], PROVIDER_INSTANCE_ID);
    assert_eq!(json["session_id"], SESSION_ID);
    assert_eq!(json["chain_id"], CHAIN_ID);
    assert_eq!(
        json["active_segment_id"].as_i64(),
        Some(fixture.active_segment_id())
    );
    assert_eq!(json["db_apply_marker"].as_str(), Some(expected_marker));
    assert_eq!(
        json["db_preimage"],
        expected_provider_owned_journal_db_preimage()
    );

    let journal_text = json.to_string();
    for forbidden in [
        "jsonl_path",
        "canonical_records_path",
        "preimage_snapshot_path",
        "canonical_transcript",
        "canonical_input_bytes",
        "canonical_postimage_bytes",
        "input_bytes",
        "postimage_bytes",
        "data_base64",
        ".preimage",
    ] {
        assert!(
            !journal_text.contains(forbidden),
            "provider-owned v2 journal must not carry local canonical/input/postimage bytes or local-file recovery authority {forbidden:?}: {json}"
        );
    }
}

fn expected_provider_owned_journal_db_preimage() -> Value {
    json!({
        "session_turns": [
            [
                PROVIDER_NAME,
                SESSION_ID,
                "old-turn-1",
                "2026-05-01T00:00:00Z",
                "user",
                null,
                0,
                0,
                "<session-transcript>",
                null
            ],
            [
                PROVIDER_NAME,
                SESSION_ID,
                "old-turn-2",
                "2026-05-01T00:00:01Z",
                "assistant",
                null,
                0,
                0,
                "<session-transcript>",
                null
            ]
        ],
        "last_turn_id": "old-turn-2",
        "last_used_at": "2026-05-01T00:00:00Z",
    })
}

fn assert_provider_requests_do_not_expose_sqlite_mutation_authority(fixture: &DispatchFixture) {
    let text = fixture
        .records()
        .into_iter()
        .map(|record| record.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !text.contains("state.db"),
        "external session provider requests must not expose runner SQLite paths: {text}"
    );
    assert!(
        !text.contains("journal") && !text.contains("transaction") && !text.contains("sql"),
        "provider request must not carry journal, transaction, or SQL mutation authority: {text}"
    );
}

fn assert_error_token(message: String, expected_token: &str) {
    assert!(
        message.contains(expected_token),
        "expected stable token {expected_token:?} in {message:?}"
    );
}

fn assert_replacement_lineage_reset_to_null(fixture: &DispatchFixture, message: &str) {
    let parents = query_rows(
        &fixture.conn(),
        "SELECT turn_id, parent_turn_id FROM session_turns ORDER BY turn_id",
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
    );
    assert_eq!(
        parents,
        vec![
            ("valid-turn-1".to_string(), None),
            ("valid-turn-2".to_string(), None),
        ],
        "{message}"
    );
}

fn assert_no_ref_replacement_session_turn_rows(fixture: &DispatchFixture) {
    let rows = query_rows(
        &fixture.conn(),
        "SELECT provider_name, session_id, turn_id, timestamp, role,
                parent_turn_id, is_sidechain, is_compaction_boundary, source_file, body
         FROM session_turns ORDER BY provider_name, session_id, turn_id",
        |row| {
            Ok(ReplacementTurnRow {
                provider_name: row.get(0)?,
                session_id: row.get(1)?,
                turn_id: row.get(2)?,
                timestamp: row.get(3)?,
                role: row.get(4)?,
                parent_turn_id: row.get(5)?,
                is_sidechain: row.get(6)?,
                is_compaction_boundary: row.get(7)?,
                source_file: row.get(8)?,
                body: row.get(9)?,
            })
        },
    );
    let source_file = Some(fixture.transcript_path.to_string_lossy().to_string());

    assert_eq!(
        rows,
        vec![
            ReplacementTurnRow {
                provider_name: PROVIDER_NAME.to_string(),
                session_id: SESSION_ID.to_string(),
                turn_id: "valid-turn-1".to_string(),
                timestamp: "2026-05-01T01:00:00.000Z".to_string(),
                role: "user".to_string(),
                parent_turn_id: None,
                is_sidechain: 0,
                is_compaction_boundary: 0,
                source_file: source_file.clone(),
                body: Some(r#"[{"type":"text","text":"valid user"}]"#.to_string()),
            },
            ReplacementTurnRow {
                provider_name: PROVIDER_NAME.to_string(),
                session_id: SESSION_ID.to_string(),
                turn_id: "valid-turn-2".to_string(),
                timestamp: "2026-05-01T01:00:01.000Z".to_string(),
                role: "assistant".to_string(),
                parent_turn_id: None,
                is_sidechain: 0,
                is_compaction_boundary: 0,
                source_file,
                body: Some(r#"[{"type":"text","text":"valid assistant"}]"#.to_string()),
            },
        ],
        "no-ref replace must insert exact canonical rows in stable row order, preserving timestamp spelling verbatim while resetting canonical input parent_turn_id lineage"
    );
}

fn assert_tauri_startup_recovery_order_and_host_ownership() {
    let dispatch_source =
        fs::read_to_string(repo_root().join("src-tauri/src/dispatch.rs")).expect("dispatch source");
    let run_start = dispatch_source
        .find("pub(crate) fn run")
        .expect("dispatch run function");
    let recovery_call = find_after(
        &dispatch_source,
        run_start,
        "recover_pending_session_replaces()",
    );
    for later_call in [
        "wiring::AgentRuntimeServices::cli_defaults()",
        "dispatch_subcommand(",
        "dispatch_top_level_resume(",
        "run_direct_model_cli(",
        "run_agent_cli(",
    ] {
        let position = find_after(&dispatch_source, run_start, later_call);
        assert!(
            recovery_call < position,
            "startup pending replace recovery must run before {later_call}"
        );
    }

    let wrapper_start = dispatch_source
        .find("fn recover_pending_session_replaces")
        .expect("recovery wrapper");
    let wrapper_end = find_after(
        &dispatch_source,
        wrapper_start,
        "fn handle_pending_session_replace_error",
    );
    let wrapper = &dispatch_source[wrapper_start..wrapper_end];
    assert!(
        wrapper.contains("session_replace::recover_pending_replaces()"),
        "startup recovery must delegate to the host-owned runtime recovery function"
    );
    for forbidden in [
        "ProviderRegistry",
        "ProviderClient",
        "provider_registry",
        "external_provider",
        "cli_defaults",
    ] {
        assert!(
            !wrapper.contains(forbidden),
            "startup recovery wrapper must not invoke provider registry/client dispatch: {forbidden}"
        );
    }
}

fn assert_tauri_provider_owned_startup_recovery_order_after_registry() {
    let dispatch_source =
        fs::read_to_string(repo_root().join("src-tauri/src/dispatch.rs")).expect("dispatch source");
    let run_start = dispatch_source
        .find("pub(crate) fn run")
        .expect("dispatch run function");
    let legacy_recovery_call = find_after(
        &dispatch_source,
        run_start,
        "recover_pending_session_replaces()",
    );
    let registry_call = find_after(
        &dispatch_source,
        run_start,
        "wiring::AgentRuntimeServices::cli_defaults()",
    );
    let provider_recovery_call = find_after(
        &dispatch_source,
        run_start,
        "recover_pending_provider_owned_session_replaces(&agent_runtime_services)",
    );
    assert!(
        legacy_recovery_call < registry_call,
        "phase-1 host-owned v1 recovery must remain before provider registry construction"
    );
    assert!(
        registry_call < provider_recovery_call,
        "phase-2 provider-owned recovery must run after provider registry construction"
    );
    for later_call in [
        "run_usage_command(",
        "dispatch_subcommand(",
        "dispatch_top_level_resume(",
        "run_direct_model_cli(",
        "run_agent_cli(",
    ] {
        let position = find_after(&dispatch_source, run_start, later_call);
        assert!(
            provider_recovery_call < position,
            "phase-2 provider-owned recovery must run before {later_call}"
        );
    }

    let legacy_wrapper_start = dispatch_source
        .find("fn recover_pending_session_replaces")
        .expect("legacy recovery wrapper");
    let legacy_wrapper_end = find_after(
        &dispatch_source,
        legacy_wrapper_start,
        "fn handle_pending_session_replace_error",
    );
    let legacy_wrapper = &dispatch_source[legacy_wrapper_start..legacy_wrapper_end];
    assert!(
        !legacy_wrapper.contains("recover_pending_provider_owned_replaces"),
        "provider-owned phase-2 recovery must not be merged into the legacy host-only wrapper"
    );

    let provider_wrapper_start = dispatch_source
        .find("fn recover_pending_provider_owned_session_replaces")
        .expect("provider-owned recovery wrapper");
    let provider_wrapper_end = dispatch_source[provider_wrapper_start + 2..]
        .find("\nfn ")
        .map(|offset| provider_wrapper_start + 2 + offset)
        .unwrap_or(dispatch_source.len());
    let provider_wrapper = &dispatch_source[provider_wrapper_start..provider_wrapper_end];
    assert!(
        provider_wrapper
            .contains("session_external_provider::recover_pending_provider_owned_replaces("),
        "provider-owned startup wrapper must delegate to provider-aware v2 recovery"
    );
    assert!(
        !provider_wrapper.contains("session_replace::recover_pending_replaces()"),
        "provider-owned startup wrapper must not call the legacy v1 host-only recovery pass"
    );
}

fn find_after(haystack: &str, start: usize, needle: &str) -> usize {
    haystack[start..]
        .find(needle)
        .map(|offset| start + offset)
        .unwrap_or_else(|| panic!("missing {needle:?} after byte {start}"))
}

fn sqlite_snapshot(conn: &Connection, transcript_path: &Path) -> SqliteSnapshot {
    SqliteSnapshot {
        session_turns: query_rows(
            conn,
            "SELECT provider_name, session_id, turn_id, timestamp, role,
                    parent_turn_id, is_sidechain, is_compaction_boundary, source_file, body
             FROM session_turns ORDER BY provider_name, session_id, turn_id",
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    normalize_snapshot_source_file(row.get(8)?, transcript_path),
                    row.get(9)?,
                ))
            },
        ),
        session_chains: query_rows(
            conn,
            "SELECT chain_id, created_at, last_used_at, model_name
             FROM session_chains ORDER BY chain_id",
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        ),
        session_chain_segments: query_rows(
            conn,
            "SELECT id, chain_id, provider_name, session_id, started_at,
                    ended_at, last_turn_id, transition_reason
             FROM session_chain_segments
             ORDER BY id, chain_id, provider_name, session_id",
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        ),
    }
}

fn normalize_snapshot_source_file(source_file: String, transcript_path: &Path) -> String {
    if Path::new(&source_file) == transcript_path {
        "<session-transcript>".to_string()
    } else {
        source_file
    }
}

fn query_rows<T, F>(conn: &Connection, sql: &str, mut mapper: F) -> Vec<T>
where
    F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
{
    let mut stmt = conn.prepare(sql).expect(sql);
    stmt.query_map([], |row| mapper(row))
        .expect("query")
        .map(|row| row.expect("row"))
        .collect()
}

fn tree_snapshot(root: &Path) -> Vec<(String, Vec<u8>)> {
    if !root.exists() {
        return Vec::new();
    }
    let mut out = Vec::new();
    collect_tree_snapshot(root, root, &mut out);
    out.sort_by(|left, right| left.0.cmp(&right.0));
    out
}

fn collect_tree_snapshot(root: &Path, path: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    for entry in fs::read_dir(path).expect("read snapshot dir") {
        let entry = entry.expect("entry");
        let entry_path = entry.path();
        if entry_path.is_dir() {
            collect_tree_snapshot(root, &entry_path, out);
        } else {
            out.push((
                entry_path
                    .strip_prefix(root)
                    .expect("relative")
                    .display()
                    .to_string(),
                fs::read(&entry_path).expect("snapshot file"),
            ));
        }
    }
}

fn expected_replacement_turn_rows(fixture: &DispatchFixture) -> Vec<SessionTurnSnapshotRow> {
    let source_file = normalize_snapshot_source_file(
        fixture.transcript_path.to_string_lossy().to_string(),
        &fixture.transcript_path,
    );
    vec![
        (
            PROVIDER_NAME.to_string(),
            SESSION_ID.to_string(),
            "valid-turn-1".to_string(),
            "2026-05-01T01:00:00.000Z".to_string(),
            "user".to_string(),
            None,
            0,
            0,
            source_file.clone(),
            Some(r#"[{"type":"text","text":"valid user"}]"#.to_string()),
        ),
        (
            PROVIDER_NAME.to_string(),
            SESSION_ID.to_string(),
            "valid-turn-2".to_string(),
            "2026-05-01T01:00:01.000Z".to_string(),
            "assistant".to_string(),
            None,
            0,
            0,
            source_file,
            Some(r#"[{"type":"text","text":"valid assistant"}]"#.to_string()),
        ),
    ]
}

fn expected_provider_owned_replacement_turn_rows() -> Vec<SessionTurnSnapshotRow> {
    let source_file = provider_owned_source_id();
    vec![
        (
            PROVIDER_NAME.to_string(),
            SESSION_ID.to_string(),
            "valid-turn-1".to_string(),
            "2026-05-01T01:00:00.000Z".to_string(),
            "user".to_string(),
            None,
            0,
            0,
            source_file.clone(),
            Some(r#"[{"type":"text","text":"valid user"}]"#.to_string()),
        ),
        (
            PROVIDER_NAME.to_string(),
            SESSION_ID.to_string(),
            "valid-turn-2".to_string(),
            "2026-05-01T01:00:01.000Z".to_string(),
            "assistant".to_string(),
            None,
            0,
            0,
            source_file,
            Some(r#"[{"type":"text","text":"valid assistant"}]"#.to_string()),
        ),
    ]
}

fn expected_nonrenderable_provider_owned_rows() -> Vec<SessionTurnSnapshotRow> {
    vec![(
        PROVIDER_NAME.to_string(),
        SESSION_ID.to_string(),
        "valid-turn-1".to_string(),
        "2026-05-01T01:00:00.000Z".to_string(),
        "user".to_string(),
        None,
        0,
        0,
        provider_owned_source_id(),
        Some(r#"[{"type":"image"}]"#.to_string()),
    )]
}

fn provider_owned_replace_protocol() -> &'static str {
    "oulipoly.provider_owned_replace/v1"
}

fn provider_owned_operation_id() -> String {
    "55555555-5555-4555-8555-555555555555".to_string()
}

fn provider_owned_recovery_id() -> String {
    "66666666-6666-4666-8666-666666666666".to_string()
}

fn provider_owned_preimage_sha256() -> String {
    "1".repeat(64)
}

fn provider_owned_source_id() -> String {
    "provider-alpha-owned-canonical-source".to_string()
}

fn native_fixture_bytes() -> Vec<u8> {
    format!(
        "{}\n{}\n",
        native_line("old-turn-1", "user", "old-user", 0),
        native_line("old-turn-2", "assistant", "old-assistant", 1),
    )
    .into_bytes()
}

fn provider_replacement_native_bytes() -> Vec<u8> {
    format!(
        "{}\n{}\n",
        provider_replacement_native_line(
            "valid-turn-1",
            "user",
            "2026-05-01T01:00:00.000Z",
            "valid user"
        ),
        provider_replacement_native_line(
            "valid-turn-2",
            "assistant",
            "2026-05-01T01:00:01.000Z",
            "valid assistant"
        ),
    )
    .into_bytes()
}

fn provider_replacement_native_line(
    turn_id: &str,
    role: &str,
    timestamp: &str,
    text: &str,
) -> String {
    json!({
        "sessionId": SESSION_ID,
        "type": role,
        "uuid": turn_id,
        "timestamp": timestamp,
        "message": {
            "role": role,
            "content": [{"type": "text", "text": text}],
        },
    })
    .to_string()
}

fn write_expected_postimage_hash(transcript_path: &Path, output_path: &Path, bytes: &[u8]) {
    let original = fs::read(transcript_path).expect("original transcript");
    fs::write(transcript_path, bytes).expect("replacement transcript for expected hash");
    let expected = direct_export_hash();
    fs::write(output_path, expected).expect("expected postimage");
    fs::write(transcript_path, original).expect("restore original transcript");
}

fn write_export_hash_for_artifact(artifact_path: &Path, output_path: &Path) {
    let mut metadata = resolve_export_session_metadata(SESSION_ID).expect("metadata");
    metadata.jsonl_path = artifact_path.to_path_buf();
    let bytes = canonical_jsonl_bytes(&read_canonical_transcript(&metadata).expect("records"))
        .expect("canonical bytes");
    fs::write(output_path, sha256_hex(&bytes)).expect("artifact postimage");
}

fn native_line(turn_id: &str, role: &str, message: &str, offset: i64) -> String {
    json!({
        "sessionId": SESSION_ID,
        "type": role,
        "uuid": turn_id,
        "timestamp": format!("2026-05-01T00:00:{offset:02}Z"),
        "message": {
            "role": role,
            "content": [{"type": "text", "text": message}],
        },
    })
    .to_string()
}

fn canonical_input_jsonl(jsonl_path: &Path) -> String {
    let records = [
        canonical_record(jsonl_path, "valid-turn-1", "user", "valid user", 1, None),
        canonical_record(
            jsonl_path,
            "valid-turn-2",
            "assistant",
            "valid assistant",
            2,
            Some("valid-turn-1"),
        ),
    ];
    records
        .into_iter()
        .map(|record| serde_json::to_string(&record).expect("canonical record"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn canonical_record(
    jsonl_path: &Path,
    turn_id: &str,
    role: &str,
    text: &str,
    line: u64,
    parent_turn_id: Option<&str>,
) -> Value {
    let timestamp = match line {
        1 => "2026-05-01T01:00:00.000Z",
        2 => "2026-05-01T01:00:01.000Z",
        _ => panic!("unexpected canonical record line: {line}"),
    };
    let mut record = json!({
        "session_id": SESSION_ID,
        "provider_name": PROVIDER_NAME,
        "turn_id": turn_id,
        "role": role,
        "timestamp": timestamp,
        "content": [{"type": "text", "text": text}],
        "source": {
            "storage_type": "canonical_jsonl",
            "jsonl_path": jsonl_path,
            "line": line,
            "byte_start": (line - 1) * 100,
            "byte_end": line * 100,
            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        },
        "unsupported_record": false,
    });
    if let Some(parent_turn_id) = parent_turn_id {
        record["parent_turn_id"] = Value::String(parent_turn_id.to_string());
    }
    record
}

fn invalid_canonical_input_jsonl(mut mutate: impl FnMut(&mut Value)) -> String {
    let mut record = canonical_record(
        Path::new("/tmp/invalid-canonical.jsonl"),
        "valid-turn-1",
        "user",
        "valid user",
        1,
        None,
    );
    mutate(&mut record);
    serde_json::to_string(&record).expect("invalid canonical record") + "\n"
}

fn builtin_storage_type() -> String {
    let mut storage_type = String::from("clau");
    storage_type.push_str("de_code");
    storage_type
}

fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

struct FakeProviderPaths<'a> {
    mode: &'a Path,
    records: &'a Path,
    canonical: &'a Path,
    transcript: &'a Path,
    semantic_mismatch: &'a Path,
    replacement_native: &'a Path,
    expected_postimage: &'a Path,
    semantic_postimage: &'a Path,
}

fn write_fake_provider(dir: &Path, paths: &FakeProviderPaths<'_>) -> PathBuf {
    let script = dir.join("provider-alpha-session.py");
    fs::write(&script, fake_provider_body(paths)).expect("provider script");
    let mut permissions = fs::metadata(&script).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&script, permissions).expect("chmod");
    script
}

fn fake_provider_body(paths: &FakeProviderPaths<'_>) -> String {
    format!(
        r#"#!/usr/bin/env python3
import base64
import hashlib
import json
import pathlib
import sys

CONTRACT = "oulipoly.provider/v1"
mode = pathlib.Path({mode_path}).read_text().strip()
subcommand = sys.argv[1] if len(sys.argv) > 1 else ""
raw = sys.stdin.read() or "{{}}"
request = json.loads(raw)
record_path = pathlib.Path({record_path})
with record_path.open("a", encoding="utf-8") as handle:
    handle.write(json.dumps({{"subcommand": subcommand, "request": request}}, sort_keys=True) + "\n")

def record_observation(subcommand, observation):
    with record_path.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps({{"subcommand": subcommand, "observation": observation}}, sort_keys=True) + "\n")

def observe_pending_journal():
    journal_root = pathlib.Path({transcript_path}).parent / "replace_journal"
    entries = []
    if journal_root.exists():
        for path in sorted(journal_root.glob("*")):
            if path.is_file():
                try:
                    payload = json.loads(path.read_text(encoding="utf-8"))
                except Exception as exc:
                    payload = {{"error": str(exc)}}
                entries.append({{"name": path.name, "json": payload}})
    record_observation("session.replace.journal_observed", {{"entries": entries}})

def envelope(result):
    return {{
        "contract": request.get("contract", CONTRACT),
        "request_id": request.get("request_id", "request-age244"),
        "ok": True,
        "result": result,
    }}

def error(code, category="failed"):
    return {{
        "contract": request.get("contract", CONTRACT),
        "request_id": request.get("request_id", "request-age244"),
        "ok": False,
        "error": {{
            "category": category,
            "code": code,
            "message": code,
            "retryable": False,
        }},
    }}

def describe():
    session_enabled = mode != "session_capability_disabled"
    return envelope({{
        "provider_id": "provider-alpha",
        "display_name": "Provider Alpha",
        "contract_versions": [CONTRACT],
        "preferred_contract": CONTRACT,
        "capabilities": {{
            "launch": False,
            "policy": False,
            "quota": False,
            "session": session_enabled,
            "terminal": False,
            "rotation": False,
            "discovery": False,
            "settings": False,
            "setup_brain": False,
            "setup": False,
            "migration": False,
        }},
    }})

def canonical_bytes():
    return pathlib.Path({canonical_path}).read_bytes()

def native_bytes_from_canonical(data):
    lines = []
    for raw_line in data.decode("utf-8").splitlines():
        record = json.loads(raw_line)
        lines.append(json.dumps({{
            "sessionId": record["session_id"],
            "type": record["role"],
            "uuid": record["turn_id"],
            "timestamp": record["timestamp"],
            "message": {{
                "role": record["role"],
                "content": record["content"],
            }},
        }}, separators=(",", ":")))
    return ("\n".join(lines) + "\n").encode("utf-8")

def canonical_bytes_from_native(native_bytes, path):
    records = []
    offset = 0
    line_no = 1
    for raw_line in native_bytes.splitlines():
        start = offset
        end = start + len(raw_line)
        value = json.loads(raw_line.decode("utf-8"))
        records.append({{
            "session_id": value["sessionId"],
            "provider_name": "{provider_name}",
            "turn_id": value["uuid"],
            "role": value["type"],
            "timestamp": value["timestamp"],
            "content": value["message"]["content"],
            "source": {{
                "storage_type": "{storage_type}",
                "jsonl_path": path,
                "line": line_no,
                "byte_start": start,
                "byte_end": end,
                "sha256": hashlib.sha256(raw_line).hexdigest(),
            }},
            "unsupported_record": False,
        }})
        offset = end + 1
        line_no += 1
    return ("".join(json.dumps(record, separators=(",", ":")) + "\n" for record in records)).encode("utf-8")

def export_result():
    data = canonical_bytes()
    result = {{
        "canonical_format": "oulipoly.canonical_transcript/v1",
        "data_base64": base64.b64encode(data).decode("ascii"),
        "turn_count": 2,
        "sha256": hashlib.sha256(data).hexdigest(),
    }}
    if mode == "export_success":
        return envelope(result)
    if mode == "export_provider_error":
        return error("provider_export_failed")
    if mode == "export_nonzero":
        print(json.dumps(error("export_nonzero_mode")))
        raise SystemExit(42)
    if mode == "export_empty_stdout":
        raise SystemExit(0)
    if mode == "export_malformed_json":
        print("{{")
        raise SystemExit(0)
    if mode == "export_trailing_junk":
        print(json.dumps(envelope(result)) + " trailing")
        raise SystemExit(0)
    if mode == "export_schema_invalid":
        broken = dict(result)
        broken.pop("data_base64", None)
        return envelope(broken)
    if mode == "export_invalid_base64":
        broken = dict(result)
        broken["data_base64"] = "not-base64!"
        return envelope(broken)
    if mode == "export_wrong_format":
        broken = dict(result)
        broken["canonical_format"] = "provider-alpha/wrong"
        return envelope(broken)
    if mode == "export_hash_mismatch":
        broken = dict(result)
        broken["sha256"] = "0" * 64
        return envelope(broken)
    if mode == "export_malformed_canonical_jsonl":
        malformed = b'{{"session_id": "broken"\n'
        broken = dict(result)
        broken["data_base64"] = base64.b64encode(malformed).decode("ascii")
        broken["turn_count"] = 1
        broken["sha256"] = hashlib.sha256(malformed).hexdigest()
        return envelope(broken)
    if mode == "export_count_mismatch":
        broken = dict(result)
        broken["turn_count"] = 99
        return envelope(broken)
    if mode == "export_missing_source":
        return error("missing_source", "unavailable")
    return error("unexpected_export_mode")

def replace_result():
    params = request.get("params", {{}})
    transcript_param = params.get("canonical_transcript", {{}})
    encoded = transcript_param.get("data_base64") or params.get("data_base64", "")
    try:
        data = base64.b64decode(encoded)
    except Exception:
        data = b""
    native_data = pathlib.Path({replacement_native_path}).read_bytes()
    canonical_hash = hashlib.sha256(data).hexdigest()
    turn_count = len([line for line in data.splitlines() if line.strip()])
    canonical_records = [json.loads(line) for line in data.decode("utf-8").splitlines() if line.strip()]
    last_record = canonical_records[-1] if canonical_records else {{}}
    last_turn_id = last_record.get("turn_id")
    last_used_at = last_record.get("timestamp")
    postimage = pathlib.Path({expected_postimage_path}).read_text().strip()
    artifact = {{"kind": "file", "path": {transcript_path}, "sha256": postimage}}
    plan = {{
        "schema_version": 1,
        "operation": "session.replace",
        "session_id": "{session_id}",
        "provider_name": "{provider_name}",
        "canonical_format": "oulipoly.canonical_transcript/v1",
        "turn_count": 2,
        "records_sha256": canonical_hash,
        "postimage_sha256": postimage,
        "artifacts": [artifact],
    }}
    result = {{
        "changed": True,
        "postimage_sha256": postimage,
        "artifacts": [artifact],
        "host_state_plan": plan,
    }}
    operation_id = params.get("operation_id", "{operation_id}")
    recovery_id = params.get("recovery_id") or "{recovery_id}"
    source_id = "{source_id}"
    provider_preimage = "{preimage_sha256}"
    canonical_postimage = {{
        "format_id": "oulipoly.canonical_transcript/v1",
        "sha256": canonical_hash,
        "turn_count": turn_count,
        "source_id": source_id,
        "data_base64": base64.b64encode(data).decode("ascii"),
    }}
    provider_owned_plan = {{
        "schema_version": 2,
        "operation": "session.replace",
        "replace_protocol": "{replace_protocol}",
        "operation_id": operation_id,
        "recovery_id": recovery_id,
        "session_id": "{session_id}",
        "provider_name": "{provider_name}",
        "canonical_format": "oulipoly.canonical_transcript/v1",
        "input_sha256": canonical_hash,
        "postimage_sha256": canonical_hash,
        "preimage_sha256_observed": provider_preimage,
        "turn_count": turn_count,
        "db_apply": "replace_session_turns_from_canonical_v1",
        "source_id": source_id,
        "last_turn_id": last_turn_id,
        "last_used_at": last_used_at,
    }}
    provider_owned_result = {{
        "changed": True,
        "operation_id": operation_id,
        "recovery_id": recovery_id,
        "operation_state": "atomic_committed",
        "preimage_sha256_observed": provider_preimage,
        "postimage_sha256": canonical_hash,
        "canonical_postimage": canonical_postimage,
        "provider_preimage_artifact": {{
            "role": "provider_preimage",
            "format_id": "provider-alpha/native-v1",
            "uri": "provider-alpha://preimage/{operation_id}",
            "sha256": provider_preimage,
        }},
        "provider_postimage_artifact": {{
            "role": "provider_postimage",
            "format_id": "provider-alpha/native-v1",
            "uri": "provider-alpha://postimage/{operation_id}",
            "sha256": canonical_hash,
            "turn_count": turn_count,
        }},
        "artifacts": [],
        "host_state_plan": provider_owned_plan,
    }}
    provider_owned_prepared_result = dict(provider_owned_result, operation_state="prepared")
    if params.get("operation_mode") == "recover":
        action = params.get("recovery_action")
        recovery_result = dict(provider_owned_prepared_result)
        recovery_result["operation_state"] = "prepared"
        if mode == "recovery_atomic_before_cleanup":
            recovery_result["operation_state"] = "atomic_committed"
        if mode == "recovery_rollback_retry":
            recovery_result["operation_state"] = "rolled_back"
        if mode == "recovery_provider_unavailable":
            return error("provider_owned_recovery_unavailable", "unavailable")
        return envelope(recovery_result)
    if mode == "replace_provider_owned_success":
        return envelope(provider_owned_result)
    if mode == "replace_provider_owned_prepared_success":
        return envelope(provider_owned_prepared_result)
    if mode == "replace_provider_owned_observe_pending_journal":
        observe_pending_journal()
        return envelope(provider_owned_prepared_result)
    if mode == "replace_provider_owned_conflict_observed_preimage_mismatch":
        record_observation("session.replace.preimage_conflict_observed", {{
            "operation_id": operation_id,
            "preimage_sha256_expected": params.get("preimage_sha256_expected"),
            "preimage_sha256_observed": provider_preimage,
        }})
        return error("provider_replace_conflict", "conflict")
    if mode == "replace_legacy_scalar_success_missing_m4_evidence":
        legacy_scalar = dict(result)
        legacy_scalar["artifacts"] = []
        legacy_scalar.pop("operation_id", None)
        legacy_scalar.pop("recovery_id", None)
        legacy_scalar.pop("preimage_sha256_observed", None)
        legacy_scalar.pop("canonical_postimage", None)
        return envelope(legacy_scalar)
    if mode == "replace_missing_operation_id":
        broken = dict(provider_owned_result)
        broken.pop("operation_id", None)
        return envelope(broken)
    if mode == "replace_operation_id_echo_mismatch":
        return envelope(dict(provider_owned_result, operation_id="77777777-7777-4777-8777-777777777777"))
    if mode == "replace_missing_recovery_id":
        broken = dict(provider_owned_result)
        broken.pop("recovery_id", None)
        return envelope(broken)
    if mode == "replace_missing_observed_preimage":
        broken = dict(provider_owned_result)
        broken.pop("preimage_sha256_observed", None)
        return envelope(broken)
    if mode == "replace_corrupt_observed_preimage":
        return envelope(dict(provider_owned_result, preimage_sha256_observed="not-a-hash"))
    if mode == "replace_missing_canonical_postimage":
        broken = dict(provider_owned_result)
        broken.pop("canonical_postimage", None)
        return envelope(broken)
    if mode == "replace_canonical_postimage_hash_mismatch":
        broken_postimage = dict(canonical_postimage, sha256=("0" * 64))
        return envelope(dict(provider_owned_result, canonical_postimage=broken_postimage))
    if mode == "replace_canonical_postimage_count_mismatch":
        broken_postimage = dict(canonical_postimage, turn_count=turn_count + 1)
        return envelope(dict(provider_owned_result, canonical_postimage=broken_postimage))
    if mode == "replace_missing_v2_plan_field":
        broken_plan = dict(provider_owned_plan)
        broken_plan.pop("db_apply", None)
        return envelope(dict(provider_owned_result, host_state_plan=broken_plan))
    if mode == "replace_v2_plan_operation_id_mismatch":
        broken_plan = dict(provider_owned_plan, operation_id="99999999-9999-4999-8999-999999999999")
        return envelope(dict(provider_owned_result, host_state_plan=broken_plan))
    if mode == "replace_v2_plan_observed_preimage_mismatch":
        broken_plan = dict(provider_owned_plan, preimage_sha256_observed=("0" * 64))
        return envelope(dict(provider_owned_result, host_state_plan=broken_plan))
    if mode == "replace_v2_plan_missing_source_id":
        broken_plan = dict(provider_owned_plan)
        broken_plan.pop("source_id", None)
        return envelope(dict(provider_owned_result, host_state_plan=broken_plan))
    if mode == "replace_provider_owned_plan_postimage_mismatch":
        broken_plan = dict(provider_owned_plan, postimage_sha256=("0" * 64))
        return envelope(dict(provider_owned_result, host_state_plan=broken_plan))
    if mode == "replace_provider_owned_plan_input_mismatch":
        broken_plan = dict(provider_owned_plan, input_sha256=("0" * 64))
        return envelope(dict(provider_owned_result, host_state_plan=broken_plan))
    if mode == "replace_recovery_evidence_mismatch":
        broken_plan = dict(provider_owned_plan, recovery_id="88888888-8888-4888-8888-888888888888")
        return envelope(dict(provider_owned_result, host_state_plan=broken_plan))
    if mode == "replace_success":
        pathlib.Path({transcript_path}).write_bytes(native_data)
        return envelope(result)
    if mode == "replace_no_change":
        return envelope({{"changed": False, "artifacts": []}})
    if mode == "replace_provider_error":
        return error("provider_replace_failed")
    if mode == "replace_conflict":
        return error("provider_replace_conflict", "conflict")
    if mode == "replace_missing_source":
        return error("missing_source", "unavailable")
    if mode == "replace_nonzero":
        print(json.dumps(error("replace_nonzero_mode")))
        raise SystemExit(42)
    if mode == "replace_empty_stdout":
        raise SystemExit(0)
    if mode == "replace_malformed_json":
        print("{{")
        raise SystemExit(0)
    if mode == "replace_trailing_junk":
        print(json.dumps(envelope(result)) + " trailing")
        raise SystemExit(0)
    if mode == "replace_schema_invalid":
        broken = dict(result)
        broken.pop("artifacts", None)
        return envelope(broken)
    if mode == "replace_schema_invalid_error":
        print(json.dumps({{
            "contract": request.get("contract", CONTRACT),
            "request_id": request.get("request_id", "request-age244"),
            "ok": False,
            "error": {{"category": "failed", "message": "missing code and retryable"}},
        }}))
        raise SystemExit(0)
    if mode == "replace_semantic_mismatch":
        mismatch_data = pathlib.Path({semantic_mismatch_path}).read_bytes()
        mismatch_hash = pathlib.Path({semantic_postimage_path}).read_text().strip()
        mismatch_artifact = {{"kind": "file", "path": {semantic_mismatch_path}, "sha256": mismatch_hash}}
        semantic_plan = dict(
            plan,
            records_sha256=canonical_hash,
            postimage_sha256=mismatch_hash,
            artifacts=[mismatch_artifact],
        )
        semantic_result = dict(
            result,
            postimage_sha256=mismatch_hash,
            artifacts=[mismatch_artifact],
            host_state_plan=semantic_plan,
        )
        return envelope(semantic_result)
    return error("unexpected_replace_mode")

if mode == "nonzero_no_envelope":
    raise SystemExit(42)

if subcommand == "describe":
    response = describe()
elif subcommand == "session.export":
    response = export_result()
elif subcommand == "session.replace":
    response = replace_result()
else:
    response = error("unsupported_subcommand")
print(json.dumps(response))
"#,
        mode_path = json_string(paths.mode),
        record_path = json_string(paths.records),
        canonical_path = json_string(paths.canonical),
        transcript_path = json_string(paths.transcript),
        semantic_mismatch_path = json_string(paths.semantic_mismatch),
        replacement_native_path = json_string(paths.replacement_native),
        expected_postimage_path = json_string(paths.expected_postimage),
        semantic_postimage_path = json_string(paths.semantic_postimage),
        session_id = SESSION_ID,
        provider_name = PROVIDER_NAME,
        storage_type = builtin_storage_type(),
        replace_protocol = provider_owned_replace_protocol(),
        operation_id = provider_owned_operation_id(),
        recovery_id = provider_owned_recovery_id(),
        preimage_sha256 = provider_owned_preimage_sha256(),
        source_id = provider_owned_source_id(),
    )
}

fn json_string(path: &Path) -> String {
    serde_json::to_string(&path.display().to_string()).expect("json path")
}

fn grep_snapshot(root: &Path, args: &[&str]) -> BTreeSet<String> {
    let pattern = concrete_provider_pattern();
    let mut command = Command::new("git");
    command
        .current_dir(root)
        .arg("grep")
        .arg("-n")
        .arg("-I")
        .arg("-E");
    if args.contains(&"--untracked") {
        command.arg("--untracked");
    }
    let output = command
        .arg(pattern)
        .args(args.iter().copied().filter(|arg| *arg != "--untracked"))
        .output()
        .expect("git grep");
    if !output.status.success() && output.status.code() != Some(1) {
        panic!(
            "git grep failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(normalize_grep_hit)
        .collect()
}

fn normalize_grep_hit(line: &str) -> Option<String> {
    if line.is_empty() {
        return None;
    }
    let normalized = line
        .strip_prefix(BASE_REF)
        .and_then(|rest| rest.strip_prefix(':'))
        .unwrap_or(line);
    let mut parts = normalized.splitn(3, ':');
    let path = parts.next()?;
    let _line_number = parts.next()?;
    let text = parts.next()?;
    Some(format!("{path}:{text}"))
}

fn concrete_provider_pattern() -> String {
    let left = String::from_utf8(vec![99, 108, 97, 117, 100, 101]).expect("left token");
    let right = String::from_utf8(vec![99, 111, 100, 101, 120]).expect("right token");
    format!("{left}|{right}")
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates dir")
        .parent()
        .expect("repo root")
        .to_path_buf()
}
