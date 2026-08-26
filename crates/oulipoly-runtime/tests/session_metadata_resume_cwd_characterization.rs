#![cfg(unix)]

use oulipoly_config::provider_implementation_ref::ProviderImplementationRef;
use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, ProviderEntry, ProvidersConfig, SessionStorage,
};
use oulipoly_runtime::session_metadata::resolve_resume_workspace_root;
use oulipoly_state::mailbox::{MailboxDb, SessionMetadataUpsert};
use oulipoly_state::{InvocationStart, ModelStore, ProviderSessionBinding, StateDb};
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
const SESSION_LIVE_RUNTIME: &str = "33333333-3333-4333-8333-333333333333";
const SESSION_IMPORTED: &str = "ses_importedWorkspace123456";
const CHAIN_MAILBOX: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const CHAIN_FALLBACK: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const CHAIN_LIVE_RUNTIME: &str = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";
const CHAIN_IMPORTED: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";

struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    old_data_dir: Option<OsString>,
    old_xdg_data_home: Option<OsString>,
}

struct PathGuard {
    old_path: Option<OsString>,
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

impl PathGuard {
    fn prepend(path: &Path) -> Self {
        let old_path = std::env::var_os("PATH");
        let next = std::env::join_paths(
            std::iter::once(path.to_path_buf())
                .chain(std::env::split_paths(&old_path.clone().unwrap_or_default())),
        )
        .unwrap();
        unsafe {
            std::env::set_var("PATH", next);
        }
        Self { old_path }
    }
}

impl Drop for PathGuard {
    fn drop(&mut self) {
        unsafe {
            restore_env("PATH", self.old_path.take());
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

#[test]
fn historical_no_ref_direct_storage_runs_cwd_command_with_session_env_and_uses_returned_path() {
    let fixture = Fixture::new();
    let bin_dir = fixture.root.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let _path = PathGuard::prepend(&bin_dir);
    let projects_dir = fixture.root.join("native-projects");
    std::fs::create_dir_all(&projects_dir).unwrap();
    let record = fixture.root.join("cwd-record.txt");
    let script_cwd = fixture.root.join("script-cwd");
    std::fs::create_dir_all(&script_cwd).unwrap();
    let response = serde_json::json!({
        "found": true,
        "cwd": script_cwd,
    })
    .to_string();
    write_shell_script(
        &bin_dir.join(direct_cwd_command()),
        &format!(
            "#!/usr/bin/env bash\nset -euo pipefail\nprintf '%s|%s|%s\n' \"$SESSION_ID\" \"$2\" \"$1\" >> {:?}\nprintf '%s\n' {:?}\n",
            record.display().to_string(),
            response,
        ),
    );
    fixture.seed_active_chain(CHAIN_FALLBACK, SESSION_FALLBACK, MODEL_LOCAL);
    let state = fixture.open_state();
    let models = model_store(vec![model_config(MODEL_LOCAL, false)]);
    let providers = direct_storage_providers_config(&projects_dir);

    let resolved = state
        .resolve_resume(&models, SESSION_FALLBACK, None)
        .unwrap();
    let actual = resolve_resume_workspace_root(&state, &providers, &resolved).expect("direct cwd");

    assert_eq!(actual, script_cwd);
    assert_ne!(actual, std::env::current_dir().unwrap());
    let records = std::fs::read_to_string(&record).unwrap();
    assert_eq!(
        records.lines().collect::<Vec<_>>(),
        vec![format!(
            "{SESSION_FALLBACK}|{SESSION_FALLBACK}|{}",
            projects_dir.display()
        )]
    );
}

#[test]
fn imported_script_session_cwd_precedes_stale_mailbox_and_failing_script() {
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
    let imported_cwd = fixture.root.join("rfq");
    let stale_mailbox_cwd = fixture.root.join("stale-mailbox-cwd");
    std::fs::create_dir_all(&imported_cwd).unwrap();
    std::fs::create_dir_all(&stale_mailbox_cwd).unwrap();
    fixture.seed_active_chain(CHAIN_IMPORTED, SESSION_IMPORTED, MODEL_LOCAL);
    let state = fixture.open_state();
    seed_provider_session_resolved_account(&state, SESSION_IMPORTED, &imported_cwd);
    let models = model_store(vec![model_config(MODEL_LOCAL, false)]);
    let providers = providers_config(&script);
    let mut mailbox = MailboxDb::open_default().unwrap();
    let stale_mailbox_cwd_text = stale_mailbox_cwd.display().to_string();
    mailbox
        .wake_sessions()
        .upsert_session_metadata(SessionMetadataUpsert {
            session_id: SESSION_IMPORTED,
            mode: "pty_interactive",
            invocation_uuid: Some("44444444-4444-4444-8444-444444444444"),
            provider_name: Some(PROVIDER),
            model_name: Some(MODEL_LOCAL),
            models_dir: None,
            effective_cwd: Some(&stale_mailbox_cwd_text),
        })
        .unwrap();
    drop(mailbox);

    let resolved = state
        .resolve_resume(&models, SESSION_IMPORTED, None)
        .unwrap();
    let actual =
        resolve_resume_workspace_root(&state, &providers, &resolved).expect("imported cwd");

    assert_eq!(actual, imported_cwd);
    assert!(!marker.exists());
}

#[test]
fn external_provider_runtime_cwd_precedes_recorded_script_session_cwd() {
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
    let recorded_cwd = fixture.root.join("recorded-cwd");
    let live_runtime_cwd = fixture.root.join("live-runtime-cwd");
    std::fs::create_dir_all(&recorded_cwd).unwrap();
    std::fs::create_dir_all(&live_runtime_cwd).unwrap();
    fixture.seed_active_chain(CHAIN_LIVE_RUNTIME, SESSION_LIVE_RUNTIME, MODEL_REF);
    let state = fixture.open_state();
    seed_provider_session_resolved_account_for_model(
        &state,
        SESSION_LIVE_RUNTIME,
        &recorded_cwd,
        MODEL_REF,
    );
    let models = model_store(vec![model_config(MODEL_REF, true)]);
    let providers = providers_config(&script);
    let mut mailbox = MailboxDb::open_default().unwrap();
    let live_runtime_cwd_text = live_runtime_cwd.display().to_string();
    mailbox
        .wake_sessions()
        .upsert_session_metadata(SessionMetadataUpsert {
            session_id: SESSION_LIVE_RUNTIME,
            mode: "pty_interactive",
            invocation_uuid: Some("55555555-5555-4555-8555-555555555555"),
            provider_name: Some(PROVIDER),
            model_name: Some(MODEL_REF),
            models_dir: None,
            effective_cwd: Some(&live_runtime_cwd_text),
        })
        .unwrap();
    drop(mailbox);

    let resolved = state
        .resolve_resume(&models, SESSION_LIVE_RUNTIME, None)
        .unwrap();
    let actual =
        resolve_resume_workspace_root(&state, &providers, &resolved).expect("live runtime cwd");

    assert_eq!(actual, live_runtime_cwd);
    assert_ne!(actual, recorded_cwd);
    assert!(!marker.exists());
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
        .wake_sessions()
        .upsert_session_metadata(SessionMetadataUpsert {
            session_id: SESSION_MAILBOX,
            mode: "pty_interactive",
            invocation_uuid: Some("33333333-3333-4333-8333-333333333333"),
            provider_name: Some(PROVIDER),
            model_name: Some(MODEL_REF),
            models_dir: None,
            effective_cwd: Some(&mailbox_cwd_text),
        })
        .unwrap();
    drop(mailbox);

    let resolved = state
        .resolve_resume(&models, SESSION_MAILBOX, None)
        .unwrap();
    let actual = resolve_resume_workspace_root(&state, &providers, &resolved).expect("mailbox cwd");

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

    let resolved = state
        .resolve_resume(&models, SESSION_FALLBACK, None)
        .unwrap();
    let actual = resolve_resume_workspace_root(&state, &providers, &resolved).expect("script cwd");

    assert_eq!(actual, script_cwd);
    assert_ne!(actual, std::env::current_dir().unwrap());
    let records = std::fs::read_to_string(&record).unwrap();
    assert_eq!(
        records.lines().collect::<Vec<_>>(),
        vec![format!("{SESSION_FALLBACK}|{SESSION_FALLBACK}")]
    );
}

fn seed_provider_session_resolved_account(state: &StateDb, session_id: &str, workspace: &Path) {
    seed_provider_session_resolved_account_for_model(state, session_id, workspace, MODEL_LOCAL);
}

fn seed_provider_session_resolved_account_for_model(
    state: &StateDb,
    session_id: &str,
    workspace: &Path,
    model_name: &str,
) {
    let invocation_row_id = state
        .start_invocation(&InvocationStart {
            invocation_uuid: uuid::Uuid::new_v4().to_string(),
            model_name: model_name.to_string(),
            provider_name: PROVIDER.to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    state
        .bind_invocation_provider_session_start(
            invocation_row_id,
            &ProviderSessionBinding {
                provider_session_id: session_id.to_string(),
                capture_method: "turn_script",
                resume_input_id: None,
                provider_session_resolved_account: Some(workspace.display().to_string()),
            },
        )
        .unwrap();
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

fn direct_storage_providers_config(projects_dir: &Path) -> ProvidersConfig {
    ProvidersConfig {
        entries: HashMap::from([(
            PROVIDER.to_string(),
            ProviderEntry {
                command: Some("provider-command-that-must-not-run".to_string()),
                session_storage: Some(SessionStorage::ClaudeCode {
                    projects_dir: projects_dir.to_path_buf(),
                }),
                ..ProviderEntry::default()
            },
        )]),
    }
}

fn direct_cwd_command() -> String {
    let mut command = real_provider_token(&["cla", "ude"]);
    command.push_str("-code-cwd");
    command
}

fn real_provider_token(parts: &[&str]) -> String {
    parts.concat()
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
