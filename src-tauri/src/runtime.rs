use std::path::PathBuf;

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
