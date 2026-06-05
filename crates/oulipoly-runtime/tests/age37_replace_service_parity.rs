use oulipoly_runtime::services::{
    ProductionSessionReplaceService, SessionReplaceServicePort, SessionReplaceServiceRequest,
};
use oulipoly_runtime::session_replace::{self};
use oulipoly_runtime::session_replace::{ReplaceError, ReplaceReceipt, ReplaceSource};
use oulipoly_state::StateDb;
use rusqlite::params;
use serde_json::{Value, json};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const SESSION_A: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
const CHAIN_A: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const MODEL: &str = "claude-opus";
const CLAUDE_PROVIDER: &str = "claude";

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    old_config: Option<OsString>,
    old_oulipoly_data_dir: Option<OsString>,
    old_data: Option<OsString>,
    old_home: Option<OsString>,
    old_path: Option<OsString>,
}

impl EnvGuard {
    fn new(config_home: &Path, data_home: &Path) -> Self {
        let guard = Self {
            old_config: std::env::var_os("XDG_CONFIG_HOME"),
            old_oulipoly_data_dir: std::env::var_os("OULIPOLY_DATA_DIR"),
            old_data: std::env::var_os("XDG_DATA_HOME"),
            old_home: std::env::var_os("HOME"),
            old_path: std::env::var_os("PATH"),
        };
        let scripts_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("scripts");
        let path = std::env::join_paths(std::iter::once(scripts_dir).chain(std::env::split_paths(
            &guard.old_path.clone().unwrap_or_default(),
        )))
        .unwrap();
        unsafe {
            std::env::remove_var("OULIPOLY_DATA_DIR");
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

struct ReplaceFixture {
    _dir: tempfile::TempDir,
    _env: EnvGuard,
    config_root: PathBuf,
    models_dir: PathBuf,
    data_root: PathBuf,
    transcript_path: PathBuf,
    input_path: PathBuf,
}

impl ReplaceFixture {
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
        let projects_dir = data_root.join("claude-projects");
        let workspace_root = data_root.join("workspace");
        fs::create_dir_all(&workspace_root).unwrap();
        let transcript_path = projects_dir
            .join(claude_project_dir_name(&workspace_root))
            .join(format!("{SESSION_A}.jsonl"));
        let input_path = data_root.join("replacement-canonical.jsonl");
        let fixture = Self {
            _dir: dir,
            _env: env,
            config_root,
            models_dir,
            data_root,
            transcript_path,
            input_path,
        };
        fixture.prepare();
        fixture
    }

    fn prepare(&self) {
        fs::write(
            self.models_dir.join(format!("{MODEL}.toml")),
            format!("[[providers]]\nname = \"{CLAUDE_PROVIDER}\"\n"),
        )
        .unwrap();
        fs::write(
            self.config_root.join("providers.toml"),
            format!(
                r#"[{CLAUDE_PROVIDER}]
command = "provider-command-that-must-not-run"
args = []
interactive_args = []
prompt_mode = "arg"

[{CLAUDE_PROVIDER}.resume]
kind = "flag"
flag = "--resume"

[{CLAUDE_PROVIDER}.session_storage]
kind = "claude_code"
projects_dir = "{}"
"#,
                self.data_root.join("claude-projects").display()
            ),
        )
        .unwrap();
        fs::write(
            self.config_root.join("sessions.toml"),
            format!(
                "[{CLAUDE_PROVIDER}]\nturn_script = \"true\"\ntranscript_locator = {:?}\nstate_dir = {:?}\n",
                format!("printf '%s\\n' {}", self.transcript_path.display()),
                self.data_root.join("locator-state").to_string_lossy()
            ),
        )
        .unwrap();
        fs::create_dir_all(self.transcript_path.parent().unwrap()).unwrap();
        fs::write(
            &self.transcript_path,
            format!(
                "{}\n{}\n",
                claude_native_line("old-turn-1", "user", "old user", 0),
                claude_native_line("old-turn-2", "assistant", "old assistant", 1)
            ),
        )
        .unwrap();
        fs::write(&self.input_path, canonical_jsonl(&self.input_path)).unwrap();
        self.seed_state();
    }

    fn seed_state(&self) {
        let db = StateDb::open(&self.data_root.join("state.db")).unwrap();
        db.connection()
            .execute(
                "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
                 VALUES (?1, '2026-04-17T08:00:00Z', '2026-04-17T08:00:00Z', ?2)",
                params![CHAIN_A, MODEL],
            )
            .unwrap();
        db.connection()
            .execute(
                "INSERT INTO session_chain_segments
                    (chain_id, provider_name, session_id, started_at, last_turn_id, transition_reason)
                 VALUES (?1, ?2, ?3, '2026-04-17T08:00:00Z', 'old-turn-2', 'initial')",
                params![CHAIN_A, CLAUDE_PROVIDER, SESSION_A],
            )
            .unwrap();
        for (turn_id, role, offset) in [
            ("old-turn-1", "user", 0_i64),
            ("old-turn-2", "assistant", 1_i64),
        ] {
            db.connection()
                .execute(
                    "INSERT INTO session_turns
                        (provider_name, session_id, turn_id, timestamp, role,
                         parent_turn_id, is_sidechain, is_compaction_boundary, source_file, ingested_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, NULL, 0, 0, ?6, ?4)",
                    params![
                        CLAUDE_PROVIDER,
                        SESSION_A,
                        turn_id,
                        format!("2026-04-17T08:00:{offset:02}Z"),
                        role,
                        self.transcript_path.to_string_lossy(),
                    ],
                )
                .unwrap();
        }
    }
}

#[test]
fn replace_service_delegates_to_run_import_replace_for_happy_path() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let direct_fixture = ReplaceFixture::new();
    let direct =
        session_replace::run_import_replace(SESSION_A, Some(&direct_fixture.input_path), None)
            .unwrap();
    drop(direct_fixture);

    let service_fixture = ReplaceFixture::new();
    let service = ProductionSessionReplaceService::default();
    let output = service
        .replace_session(SessionReplaceServiceRequest {
            session_id: SESSION_A.to_string(),
            source: ReplaceSource::File(service_fixture.input_path.clone()),
            preimage_sha256: None,
            external_provider: None,
        })
        .unwrap();

    assert_receipts_match_except_environment(output.result.unwrap(), direct);
}

#[test]
fn replace_service_preserves_replace_error_invalid_input_transcript() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let fixture = ReplaceFixture::new();
    let missing = fixture.data_root.join("missing.jsonl");
    let service = ProductionSessionReplaceService::default();
    let err = service
        .replace_session(SessionReplaceServiceRequest {
            session_id: SESSION_A.to_string(),
            source: ReplaceSource::File(missing),
            preimage_sha256: None,
            external_provider: None,
        })
        .unwrap()
        .result
        .unwrap_err();

    match err {
        ReplaceError::InvalidInputTranscript { reason, line } => {
            assert!(reason.contains("failed to read input file"), "{reason}");
            assert_eq!(line, None);
        }
        other => panic!("expected InvalidInputTranscript, got {other:?}"),
    }
}

#[test]
fn session_replace_active_segment_parity_under_contract() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let fixture = ReplaceFixture::new();
    let service = ProductionSessionReplaceService::default();

    let output = service
        .replace_session(SessionReplaceServiceRequest {
            session_id: SESSION_A.to_string(),
            source: ReplaceSource::File(fixture.input_path.clone()),
            preimage_sha256: None,
            external_provider: None,
        })
        .unwrap();
    let receipt = output.result.unwrap();

    assert_eq!(receipt.session_id, SESSION_A);
    assert_eq!(receipt.provider_name, CLAUDE_PROVIDER);
    assert_eq!(receipt.operation, "import-replace");
    assert!(receipt.state_updated);
    assert_eq!(receipt.preimage_sha256.len(), 64);
    assert_eq!(receipt.postimage_sha256.len(), 64);
}

fn assert_receipts_match_except_environment(actual: ReplaceReceipt, expected: ReplaceReceipt) {
    assert_eq!(actual.session_id, expected.session_id);
    assert_eq!(actual.provider_name, expected.provider_name);
    assert_eq!(actual.storage_type, expected.storage_type);
    assert_eq!(actual.operation, expected.operation);
    assert_eq!(actual.state_updated, expected.state_updated);
    assert_eq!(actual.preimage_sha256.len(), 64);
    assert_eq!(actual.postimage_sha256.len(), 64);
    assert_eq!(expected.preimage_sha256.len(), 64);
    assert_eq!(expected.postimage_sha256.len(), 64);
}

fn claude_native_line(turn_id: &str, role: &str, message: &str, offset: i64) -> String {
    json!({
        "sessionId": SESSION_A,
        "type": role,
        "uuid": turn_id,
        "timestamp": format!("2026-04-17T08:00:{offset:02}Z"),
        "message": message,
    })
    .to_string()
}

fn canonical_jsonl(jsonl_path: &Path) -> String {
    let records = [
        canonical_record(jsonl_path, "valid-turn-1", "user", "valid user", 1),
        canonical_record(
            jsonl_path,
            "valid-turn-2",
            "assistant",
            "valid assistant",
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

fn canonical_record(jsonl_path: &Path, turn_id: &str, role: &str, text: &str, line: u64) -> Value {
    json!({
        "session_id": SESSION_A,
        "provider_name": CLAUDE_PROVIDER,
        "turn_id": turn_id,
        "role": role,
        "timestamp": format!("2026-04-17T09:00:0{}Z", line - 1),
        "content": [{"type": "text", "text": text}],
        "source": {
            "storage_type": "canonical_jsonl",
            "jsonl_path": jsonl_path,
            "line": line,
            "byte_start": (line - 1) * 100,
            "byte_end": line * 100,
            "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
        },
        "unsupported_record": false,
    })
}

fn claude_project_dir_name(path: &Path) -> String {
    let raw = path.to_string_lossy();
    format!("-{}", raw.trim_start_matches('/').replace('/', "-"))
}
