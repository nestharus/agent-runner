#![allow(dead_code)]

#[path = "b2_process_runner.rs"]
mod b2_process_runner;

use agent_runner_lib::config::{
    AgentConfigRepository, FilesystemAgentConfigRepository, FilesystemModelConfigRepository,
    FilesystemProviderConfigSource, FilesystemSessionsConfigSource, ModelConfigRepository,
    ProviderConfigSource, SessionsConfigSource,
};
use agent_runner_lib::process::ProcessRunner;
use agent_runner_lib::quota::InFlight;
use agent_runner_lib::session_lock::{FilesystemSessionLockProvider, SessionLockProvider};
use agent_runner_lib::setup::actions::UserResponse;
use agent_runner_lib::state::{DefaultStateDbOpener, StateDb, StateDbOpener};
use agent_runner_lib::{AppState, RuntimePaths, configure_tauri_app};
#[allow(unused_imports)]
pub use b2_process_runner::{FakeProcessRunner, ok_output, timeout_error};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{INVOKE_KEY, mock_builder, mock_context, noop_assets};
use tauri::webview::InvokeRequest;

#[derive(Clone, Debug)]
pub struct B3RuntimePaths {
    pub data_root: PathBuf,
    pub config_root: PathBuf,
    pub models_dir: PathBuf,
    pub agents_dir: PathBuf,
    pub state_db_path: PathBuf,
    pub providers_path: PathBuf,
    pub sessions_path: PathBuf,
    pub lock_dir: PathBuf,
    pub replace_journal_dir: PathBuf,
}

impl B3RuntimePaths {
    pub fn under(root: &Path) -> Self {
        let config_root = root.join("config").join("oulipoly-agent-runner");
        let data_root = root.join("data").join("oulipoly-agent-runner");
        Self {
            models_dir: config_root.join("models"),
            agents_dir: config_root.join("agents"),
            providers_path: config_root.join("providers.toml"),
            sessions_path: config_root.join("sessions.toml"),
            state_db_path: data_root.join("state.db"),
            lock_dir: data_root.join("locks"),
            replace_journal_dir: data_root.join("replace_journal"),
            config_root,
            data_root,
        }
    }
}

impl RuntimePaths for B3RuntimePaths {
    fn data_root(&self) -> Result<PathBuf, String> {
        Ok(self.data_root.clone())
    }

    fn config_root(&self) -> PathBuf {
        self.config_root.clone()
    }

    fn models_dir(&self) -> PathBuf {
        self.models_dir.clone()
    }

    fn agents_dir(&self) -> PathBuf {
        self.agents_dir.clone()
    }

    fn state_db_path(&self) -> Result<PathBuf, String> {
        Ok(self.state_db_path.clone())
    }

    fn providers_path(&self) -> PathBuf {
        self.providers_path.clone()
    }

    fn sessions_path(&self) -> PathBuf {
        self.sessions_path.clone()
    }

    fn lock_dir(&self) -> Result<PathBuf, String> {
        Ok(self.lock_dir.clone())
    }

    fn replace_journal_dir(&self) -> Result<PathBuf, String> {
        Ok(self.replace_journal_dir.clone())
    }
}

pub struct B3ServiceBundle {
    _dir: tempfile::TempDir,
    pub paths: B3RuntimePaths,
    pub runner: Arc<FakeProcessRunner>,
}

impl B3ServiceBundle {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let paths = B3RuntimePaths::under(dir.path());
        fs::create_dir_all(&paths.models_dir).unwrap();
        Self {
            _dir: dir,
            paths,
            runner: Arc::new(FakeProcessRunner::new()),
        }
    }

    pub fn root(&self) -> &Path {
        self._dir.path()
    }

    pub fn open_state_db(&self) -> StateDb {
        StateDb::open(&self.paths.state_db_path).unwrap()
    }

    pub fn write_model(&self, name: &str, body: &str) {
        fs::create_dir_all(&self.paths.models_dir).unwrap();
        fs::write(self.paths.models_dir.join(format!("{name}.toml")), body).unwrap();
    }

    pub fn write_providers(&self, body: &str) {
        write_parented(&self.paths.providers_path, body);
    }

    pub fn write_sessions(&self, body: &str) {
        write_parented(&self.paths.sessions_path, body);
    }

    pub fn app_state(&self) -> AppState {
        AppState {
            paths: Arc::new(self.paths.clone()) as Arc<dyn RuntimePaths>,
            state_opener: Arc::new(DefaultStateDbOpener) as Arc<dyn StateDbOpener + Send + Sync>,
            model_repo: Arc::new(FilesystemModelConfigRepository::new(
                self.paths.models_dir.clone(),
            )) as Arc<dyn ModelConfigRepository + Send + Sync>,
            provider_source: Arc::new(FilesystemProviderConfigSource::new(
                self.paths.providers_path.clone(),
            )) as Arc<dyn ProviderConfigSource + Send + Sync>,
            sessions_source: Arc::new(FilesystemSessionsConfigSource::new(
                self.paths.sessions_path.clone(),
            )) as Arc<dyn SessionsConfigSource + Send + Sync>,
            agent_repo: Arc::new(FilesystemAgentConfigRepository::new(
                self.paths.agents_dir.clone(),
            )) as Arc<dyn AgentConfigRepository + Send + Sync>,
            process_runner: self.runner.clone() as Arc<dyn ProcessRunner>,
            lock_provider: Arc::new(FilesystemSessionLockProvider) as Arc<dyn SessionLockProvider>,
            quota_in_flight: InFlight::new(),
            setup_input_tx: Mutex::<Option<tokio::sync::mpsc::Sender<UserResponse>>>::new(None),
        }
    }

    pub fn mock_app(
        &self,
    ) -> (
        tauri::App<tauri::test::MockRuntime>,
        tauri::WebviewWindow<tauri::test::MockRuntime>,
    ) {
        let app = mock_builder()
            .manage(self.app_state())
            .invoke_handler(configure_tauri_app())
            .build(mock_context(noop_assets()))
            .unwrap();
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        (app, webview)
    }
}

pub fn invoke_json(
    webview: &tauri::WebviewWindow<tauri::test::MockRuntime>,
    cmd: &str,
    body: Value,
) -> Result<Value, Value> {
    let response = tauri::test::get_ipc_response(
        webview,
        InvokeRequest {
            cmd: cmd.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: "http://tauri.localhost".parse().unwrap(),
            body: InvokeBody::Json(body),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        },
    )?;

    response.deserialize::<Value>().map_err(|err| {
        serde_json::json!({
            "error": format!("failed to deserialize IPC response: {err}")
        })
    })
}

fn write_parented(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, body).unwrap();
}
