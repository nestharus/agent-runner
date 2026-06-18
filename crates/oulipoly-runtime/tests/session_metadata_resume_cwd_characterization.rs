#![cfg(unix)]

use oulipoly_config::provider_implementation_ref::ProviderImplementationRef;
use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, ProviderEntry, ProvidersConfig, SessionStorage,
};
use oulipoly_runtime::session_metadata::resolve_resume_workspace_root;
use oulipoly_state::mailbox::{MailboxDb, SessionRuntimeUpsert};
use oulipoly_state::{ModelStore, StateDb};
use rusqlite::{Connection, params};
use std::collections::HashMap;
use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

const PROVIDER: &str = "provider-a";
const MODEL_REF: &str = "model-ref";
const MODEL_LOCAL: &str = "model-local";
const SESSION_MAILBOX: &str = "11111111-1111-4111-8111-111111111111";
const SESSION_FALLBACK: &str = "22222222-2222-4222-8222-222222222222";
const CHAIN_MAILBOX: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const CHAIN_FALLBACK: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";

struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    old_data_dir: Option<OsString>,
    old_xdg_data_home: Option<OsString>,
}

impl EnvGuard {
    fn set(data_dir: &Path) -> Self {
        let lock = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let old_data_dir = std::env::var_os("OULIPOLY_DATA_DIR");
        let old_xdg_data_home = std::env::var_os("XDG_DATA_HOME");
        unsafe {
            std::env::set_var("OULIPOLY_DATA_DIR", data_dir);
            std::env::set_var("XDG_DATA_HOME", data_dir.join("xdg-data"));
        }
        Self {
            _lock: lock,
            old_data_dir,
            old_xdg_data_home,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            restore_env("OULIPOLY_DATA_DIR", self.old_data_dir.take());
            restore_env("XDG_DATA_HOME", self.old_xdg_data_home.take());
        }
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    _env: EnvGuard,
    state_path: PathBuf,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let data_dir = root.join("app-data");
        std::fs::create_dir_all(&data_dir).unwrap();
        let env = EnvGuard::set(&data_dir);
        let state_path = root.join("state.db");
        let _ = StateDb::open(&state_path).unwrap();
        Self {
            _dir: dir,
            _env: env,
            state_path,
            root,
        }
    }

    fn open_state(&self) -> StateDb {
        StateDb::open(&self.state_path).unwrap()
    }

    fn conn(&self) -> Connection {
        Connection::open(&self.state_path).unwrap()
    }

    fn seed_active_chain(&self, chain_id: &str, session_id: &str, model_name: &str) {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, '2026-05-01T00:00:00Z', '2026-05-01T00:00:00Z', ?2)",
            params![chain_id, model_name],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, '2026-05-01T00:00:00Z', 'initial')",
            params![chain_id, PROVIDER, session_id],
        )
        .unwrap();
    }
}

#[test]
fn resolve_resume_workspace_root_prefers_mailbox_then_uses_script_fallback() {
    mailbox_precedence_scenario();
    script_fallback_scenario();
}

fn mailbox_precedence_scenario() {
    let fixture = Fixture::new();
    let marker = fixture.root.join("fail-script-touched");
    let script = fixture.root.join("fail-cwd.sh");
    write_shell_script(
        &script,
        &format!(
            "#!/usr/bin/env bash\nprintf touched > {:?}\nexit 44\n",
            marker.display().to_string()
        ),
    );
    let mailbox_cwd = fixture.root.join("mailbox-cwd");
    std::fs::create_dir_all(&mailbox_cwd).unwrap();
    fixture.seed_active_chain(CHAIN_MAILBOX, SESSION_MAILBOX, MODEL_REF);
    let state = fixture.open_state();
    let models = model_store(vec![model_config(MODEL_REF, true)]);
    let providers = providers_config(&script);
    let mut mailbox = MailboxDb::open_default().unwrap();
    let mailbox_cwd_text = mailbox_cwd.display().to_string();
    mailbox
        .upsert_session_runtime(SessionRuntimeUpsert {
            session_id: SESSION_MAILBOX,
            mode: "pty_interactive",
            invocation_uuid: Some("33333333-3333-4333-8333-333333333333"),
            provider_name: Some(PROVIDER),
            model_name: Some(MODEL_REF),
            pty_control_path: Some("/tmp/oulipoly-test.sock"),
            models_dir: None,
            effective_cwd: Some(&mailbox_cwd_text),
        })
        .unwrap();
    drop(mailbox);

    let actual = resolve_resume_workspace_root(&state, &models, &providers, SESSION_MAILBOX)
        .expect("mailbox cwd");

    assert_eq!(actual, mailbox_cwd);
    assert!(!marker.exists());
}

fn script_fallback_scenario() {
    let fixture = Fixture::new();
    let record = fixture.root.join("cwd-record.txt");
    let script = fixture.root.join("record-cwd.sh");
    let script_cwd = fixture.root.join("script-cwd");
    std::fs::create_dir_all(&script_cwd).unwrap();
    let response = serde_json::json!({
        "found": true,
        "cwd": script_cwd,
    })
    .to_string();
    write_shell_script(
        &script,
        &format!(
            "#!/usr/bin/env bash\nset -euo pipefail\nprintf '%s|%s\\n' \"$SESSION_ID\" \"$1\" >> {:?}\nprintf '%s\\n' {:?}\n",
            record.display().to_string(),
            response,
        ),
    );
    fixture.seed_active_chain(CHAIN_FALLBACK, SESSION_FALLBACK, MODEL_LOCAL);
    let state = fixture.open_state();
    let models = model_store(vec![model_config(MODEL_LOCAL, false)]);
    let providers = providers_config(&script);

    let actual = resolve_resume_workspace_root(&state, &models, &providers, SESSION_FALLBACK)
        .expect("script cwd");

    assert_eq!(actual, script_cwd);
    assert_ne!(actual, std::env::current_dir().unwrap());
    let records = std::fs::read_to_string(&record).unwrap();
    assert_eq!(
        records.lines().collect::<Vec<_>>(),
        vec![format!("{SESSION_FALLBACK}|{SESSION_FALLBACK}")]
    );
}

fn model_config(name: &str, has_ref: bool) -> ModelConfig {
    ModelConfig {
        name: name.to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig::model_provider(PROVIDER, Vec::new())],
        inputs: Vec::new(),
        provider: has_ref.then(provider_ref),
    }
}

fn provider_ref() -> ProviderImplementationRef {
    ProviderImplementationRef {
        path: Some("/tmp/provider-ref".to_string()),
        crate_name: None,
        version: None,
        binary: None,
        script: None,
    }
}

fn model_store(models: Vec<ModelConfig>) -> ModelStore {
    models
        .into_iter()
        .map(|model| (model.name.clone(), model))
        .collect()
}

fn providers_config(cwd_script: &Path) -> ProvidersConfig {
    ProvidersConfig {
        entries: HashMap::from([(
            PROVIDER.to_string(),
            ProviderEntry {
                command: Some("provider-command-that-must-not-run".to_string()),
                session_storage: Some(SessionStorage::Script {
                    cwd_script: cwd_script.display().to_string(),
                    transcript_script: None,
                    storage_type: None,
                }),
                ..ProviderEntry::default()
            },
        )]),
    }
}

fn write_shell_script(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

unsafe fn restore_env(name: &str, value: Option<OsString>) {
    match value {
        Some(value) => unsafe { std::env::set_var(name, value) },
        None => unsafe { std::env::remove_var(name) },
    }
}
