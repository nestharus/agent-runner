#![allow(dead_code)]

use oulipoly_config::repositories::{
    FilesystemAgentConfigRepository, FilesystemAppConfigRepository,
    FilesystemModelConfigRepository, FilesystemProvidersConfigRepository,
    FilesystemSessionsConfigRepository,
};
use oulipoly_runtime::ports::{
    DefaultProcessRunner, DefaultUuidGenerator, StderrWriter, StdoutWriter, SystemClock,
};
use oulipoly_runtime::services::{ProductionInvocationLifecycleService, ProductionRoutingService};
use oulipoly_state::repositories::ProductionStateDbOpener;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct RuntimePaths {
    pub config_root: PathBuf,
    pub models_dir: PathBuf,
    pub agents_dir: PathBuf,
    pub data_root: PathBuf,
    pub state_db_path: PathBuf,
    pub lock_dir: PathBuf,
    pub working_dir: PathBuf,
}

pub struct AgentRuntimeServices {
    pub state_db_opener: Arc<ProductionStateDbOpener>,
    pub app_config: Arc<FilesystemAppConfigRepository>,
    pub agent_config: Arc<FilesystemAgentConfigRepository>,
    pub model_config: Arc<FilesystemModelConfigRepository>,
    pub providers_config: Arc<FilesystemProvidersConfigRepository>,
    pub sessions_config: Arc<FilesystemSessionsConfigRepository>,
    pub clock: Arc<SystemClock>,
    pub uuid_gen: Arc<DefaultUuidGenerator>,
    pub process_runner: Arc<DefaultProcessRunner>,
    pub stdout_sink: Arc<StdoutWriter>,
    pub stderr_sink: Arc<StderrWriter>,
    pub routing_service: Arc<ProductionRoutingService>,
    pub invocation_lifecycle_service: Arc<ProductionInvocationLifecycleService>,
}

impl AgentRuntimeServices {
    pub fn production(paths: RuntimePaths) -> Result<Self, String> {
        std::fs::create_dir_all(&paths.config_root)
            .map_err(|e| format!("Failed to create config root: {e}"))?;
        std::fs::create_dir_all(&paths.models_dir)
            .map_err(|e| format!("Failed to create models directory: {e}"))?;
        std::fs::create_dir_all(&paths.agents_dir)
            .map_err(|e| format!("Failed to create agents directory: {e}"))?;
        std::fs::create_dir_all(&paths.data_root)
            .map_err(|e| format!("Failed to create data root: {e}"))?;
        std::fs::create_dir_all(&paths.lock_dir)
            .map_err(|e| format!("Failed to create lock directory: {e}"))?;
        std::fs::create_dir_all(&paths.working_dir)
            .map_err(|e| format!("Failed to create working directory: {e}"))?;
        if let Some(parent) = paths.state_db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create state DB directory: {e}"))?;
        }

        Ok(Self {
            state_db_opener: Arc::new(ProductionStateDbOpener),
            app_config: Arc::new(FilesystemAppConfigRepository),
            agent_config: Arc::new(FilesystemAgentConfigRepository),
            model_config: Arc::new(FilesystemModelConfigRepository),
            providers_config: Arc::new(FilesystemProvidersConfigRepository),
            sessions_config: Arc::new(FilesystemSessionsConfigRepository),
            clock: Arc::new(SystemClock),
            uuid_gen: Arc::new(DefaultUuidGenerator),
            process_runner: Arc::new(DefaultProcessRunner),
            stdout_sink: Arc::new(StdoutWriter),
            stderr_sink: Arc::new(StderrWriter),
            routing_service: Arc::new(ProductionRoutingService),
            invocation_lifecycle_service: Arc::new(ProductionInvocationLifecycleService),
        })
    }
}
