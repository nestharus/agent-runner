use crate::config::{
    AgentConfigRepository, FilesystemAgentConfigRepository, FilesystemModelConfigRepository,
    FilesystemProviderConfigSource, FilesystemSessionsConfigSource, ModelConfigRepository,
    ProviderConfigSource, SessionsConfigSource,
};
use crate::process::{OsProcessRunner, ProcessRunner};
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

#[derive(Debug, Clone)]
pub struct DefaultRuntimePaths {
    models_dir_override: Option<PathBuf>,
}

impl DefaultRuntimePaths {
    pub fn new() -> Self {
        Self {
            models_dir_override: None,
        }
    }

    pub fn with_models_dir(models_dir: PathBuf) -> Self {
        Self {
            models_dir_override: Some(models_dir),
        }
    }
}

impl Default for DefaultRuntimePaths {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimePaths for DefaultRuntimePaths {
    fn data_root(&self) -> Result<PathBuf, String> {
        dirs::data_dir()
            .map(|dir| dir.join("oulipoly-agent-runner"))
            .ok_or_else(|| "could not determine data directory".to_string())
    }

    fn config_root(&self) -> PathBuf {
        dirs::config_dir()
            .map(|dir| dir.join("oulipoly-agent-runner"))
            .unwrap_or_else(|| PathBuf::from("."))
    }

    fn models_dir(&self) -> PathBuf {
        self.models_dir_override
            .clone()
            .unwrap_or_else(|| self.config_root().join("models"))
    }

    fn agents_dir(&self) -> PathBuf {
        self.config_root().join("agents")
    }

    fn state_db_path(&self) -> Result<PathBuf, String> {
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

#[derive(Clone)]
pub struct RuntimeServices {
    pub paths: Arc<dyn RuntimePaths>,
    pub state_opener: Arc<dyn StateDbOpener>,
    pub model_repo: Arc<dyn ModelConfigRepository>,
    pub provider_source: Arc<dyn ProviderConfigSource>,
    pub sessions_source: Arc<dyn SessionsConfigSource>,
    pub agent_repo: Arc<dyn AgentConfigRepository>,
    pub process_runner: Arc<dyn ProcessRunner>,
    pub lock_provider: Arc<dyn SessionLockProvider>,
}

pub fn cli_services_for_paths<P>(
    paths: P,
    models_dir_override: Option<&Path>,
) -> Result<RuntimeServices, String>
where
    P: RuntimePaths + Clone + 'static,
{
    let paths: Arc<dyn RuntimePaths> = match models_dir_override {
        Some(models_dir) => Arc::new(ModelsDirOverridePaths {
            inner: paths,
            models_dir: models_dir.to_path_buf(),
        }),
        None => Arc::new(paths),
    };
    services_from_paths(paths)
}

pub fn default_cli_services(models_dir_override: Option<&Path>) -> Result<RuntimeServices, String> {
    let paths = match models_dir_override {
        Some(models_dir) => DefaultRuntimePaths::with_models_dir(models_dir.to_path_buf()),
        None => DefaultRuntimePaths::new(),
    };
    cli_services_for_paths(paths, None)
}

pub fn services_from_paths(paths: Arc<dyn RuntimePaths>) -> Result<RuntimeServices, String> {
    Ok(RuntimeServices {
        model_repo: Arc::new(FilesystemModelConfigRepository::new(paths.models_dir())),
        provider_source: Arc::new(FilesystemProviderConfigSource::new(paths.providers_path())),
        sessions_source: Arc::new(FilesystemSessionsConfigSource::new(paths.sessions_path())),
        agent_repo: Arc::new(FilesystemAgentConfigRepository::new(paths.agents_dir())),
        state_opener: Arc::new(DefaultStateDbOpener),
        process_runner: Arc::new(OsProcessRunner),
        lock_provider: Arc::new(FilesystemSessionLockProvider::default()),
        paths,
    })
}

#[derive(Clone)]
struct ModelsDirOverridePaths<P> {
    inner: P,
    models_dir: PathBuf,
}

impl<P> RuntimePaths for ModelsDirOverridePaths<P>
where
    P: RuntimePaths + Clone,
{
    fn data_root(&self) -> Result<PathBuf, String> {
        self.inner.data_root()
    }

    fn config_root(&self) -> PathBuf {
        self.inner.config_root()
    }

    fn models_dir(&self) -> PathBuf {
        self.models_dir.clone()
    }

    fn agents_dir(&self) -> PathBuf {
        self.inner.agents_dir()
    }

    fn state_db_path(&self) -> Result<PathBuf, String> {
        self.inner.state_db_path()
    }

    fn providers_path(&self) -> PathBuf {
        self.inner.providers_path()
    }

    fn sessions_path(&self) -> PathBuf {
        self.inner.sessions_path()
    }

    fn lock_dir(&self) -> Result<PathBuf, String> {
        self.inner.lock_dir()
    }

    fn replace_journal_dir(&self) -> Result<PathBuf, String> {
        self.inner.replace_journal_dir()
    }
}
