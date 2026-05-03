use agent_runner_config::{
    AgentConfigRepository, FilesystemAgentConfigRepository, FilesystemModelConfigRepository,
    FilesystemProviderConfigSource, FilesystemSessionsConfigSource, ModelConfigRepository,
    ProviderConfigSource, SessionsConfigSource,
};
use agent_runner_executor::{OsProcessRunner, ProcessRunner};
use agent_runner_quota::InFlight;
pub use agent_runner_runtime::{DefaultRuntimePaths, RuntimePaths};
use agent_runner_session::{FilesystemSessionLockProvider, SessionLockProvider};
use agent_runner_state::{DefaultStateDbOpener, StateDbOpener};
use std::path::Path;
use std::sync::Arc;

pub struct RuntimeServices {
    pub paths: Arc<dyn RuntimePaths>,
    pub state_opener: Arc<dyn StateDbOpener + Send + Sync>,
    pub model_repo: Arc<dyn ModelConfigRepository + Send + Sync>,
    pub provider_source: Arc<dyn ProviderConfigSource + Send + Sync>,
    pub sessions_source: Arc<dyn SessionsConfigSource + Send + Sync>,
    pub agent_repo: Arc<dyn AgentConfigRepository + Send + Sync>,
    pub process_runner: Arc<dyn ProcessRunner>,
    pub lock_provider: Arc<dyn SessionLockProvider>,
    pub quota_in_flight: InFlight,
}

impl RuntimeServices {
    pub fn from_paths(paths: Arc<dyn RuntimePaths>) -> Self {
        Self {
            model_repo: Arc::new(FilesystemModelConfigRepository::new(paths.models_dir())),
            provider_source: Arc::new(FilesystemProviderConfigSource::new(paths.providers_path())),
            sessions_source: Arc::new(FilesystemSessionsConfigSource::new(paths.sessions_path())),
            agent_repo: Arc::new(FilesystemAgentConfigRepository::new(paths.agents_dir())),
            paths,
            state_opener: Arc::new(DefaultStateDbOpener),
            process_runner: Arc::new(OsProcessRunner),
            lock_provider: Arc::new(FilesystemSessionLockProvider),
            quota_in_flight: InFlight::new(),
        }
    }
}

pub fn cli_services(models_dir_override: Option<&Path>) -> RuntimeServices {
    let paths = match models_dir_override {
        Some(models_dir) => DefaultRuntimePaths::with_models_dir(models_dir.to_path_buf()),
        None => DefaultRuntimePaths::new(),
    };
    RuntimeServices::from_paths(Arc::new(paths))
}
