use crate::config::{
    AgentConfigRepository, FilesystemAgentConfigRepository, FilesystemModelConfigRepository,
    FilesystemProviderConfigSource, FilesystemSessionsConfigSource, ModelConfigRepository,
    ProviderConfigSource, SessionsConfigSource,
};
use crate::process::{OsProcessRunner, ProcessRunner};
use crate::quota::InFlight;
use crate::session_lock::{FilesystemSessionLockProvider, SessionLockProvider};
use crate::state::{DefaultStateDbOpener, StateDbOpener};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub trait RuntimePaths: Send + Sync {
    fn data_root(&self) -> Result<PathBuf, String>;
    fn config_root(&self) -> PathBuf;
    fn models_dir(&self) -> PathBuf;
    fn agents_dir(&self) -> PathBuf;
    fn state_db_path(&self) -> Result<PathBuf, String>;
    fn providers_path(&self) -> PathBuf;
    fn sessions_path(&self) -> PathBuf;
    fn lock_dir(&self) -> Result<PathBuf, String>;
    fn replace_journal_dir(&self) -> Result<PathBuf, String>;
}

#[derive(Debug, Clone, Default)]
pub struct DefaultRuntimePaths {
    models_dir_override: Option<PathBuf>,
}

impl DefaultRuntimePaths {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_models_dir(models_dir: PathBuf) -> Self {
        Self {
            models_dir_override: Some(models_dir),
        }
    }

    fn default_config_root() -> PathBuf {
        dirs::config_dir()
            .map(|dir| dir.join("oulipoly-agent-runner"))
            .unwrap_or_else(|| PathBuf::from("."))
    }

    pub(crate) fn default_models_dir() -> PathBuf {
        Self::default_config_root().join("models")
    }
}

impl RuntimePaths for DefaultRuntimePaths {
    fn data_root(&self) -> Result<PathBuf, String> {
        dirs::data_dir()
            .map(|dir| dir.join("oulipoly-agent-runner"))
            .ok_or_else(|| "Could not determine data directory".to_string())
    }

    fn config_root(&self) -> PathBuf {
        Self::default_config_root()
    }

    fn models_dir(&self) -> PathBuf {
        self.models_dir_override
            .clone()
            .unwrap_or_else(Self::default_models_dir)
    }

    fn agents_dir(&self) -> PathBuf {
        self.config_root().join("agents")
    }

    fn state_db_path(&self) -> Result<PathBuf, String> {
        if let Some(models_dir) = &self.models_dir_override {
            return Ok(models_dir.parent().unwrap_or(models_dir).join("state.db"));
        }

        Ok(self.data_root()?.join("state.db"))
    }

    fn providers_path(&self) -> PathBuf {
        self.config_root().join("providers.toml")
    }

    fn sessions_path(&self) -> PathBuf {
        self.config_root().join("sessions.toml")
    }

    fn lock_dir(&self) -> Result<PathBuf, String> {
        Ok(self.data_root()?.join("locks"))
    }

    fn replace_journal_dir(&self) -> Result<PathBuf, String> {
        Ok(self.data_root()?.join("replace_journal"))
    }
}

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
