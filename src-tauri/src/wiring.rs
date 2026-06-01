#![allow(dead_code)]
//! ## Declared roles
//!
//! `orchestration`, `accessor`, `mapper`, `formatter`
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: src-tauri/src/wiring.rs
//!     role: intrinsic-surface
//!     Domain: Tauri runtime service composition
//!     Owns:
//!       - production repository and service construction
//!       - shared provider registry handle propagation
//!       - CLI-default service graph construction
//!       - configured runtime path wiring
//! ```

use oulipoly_config as config;
use oulipoly_config::repositories::{
    FilesystemAgentConfigRepository, FilesystemAppConfigRepository,
    FilesystemModelConfigRepository, FilesystemProvidersConfigRepository,
    FilesystemSessionsConfigRepository, ProvidersConfigRepository,
};
use oulipoly_runtime::diagnostics::RuntimeDiagnosticsService;
use oulipoly_runtime::executor::RuntimeExecutorService;
use oulipoly_runtime::ports::{
    DefaultProcessRunner, DefaultUuidGenerator, StderrWriter, StdoutWriter, SystemClock,
};
use oulipoly_runtime::provider_registry::{
    ProviderRegistry, ProviderRegistryHandle, ProviderRegistryOptions,
};
use oulipoly_runtime::quota::RuntimeQuotaService;
use oulipoly_runtime::services::{
    DiagnosticsServicePort, ExecutorServicePort, MigrationServicePort,
    ProductionInvocationLifecycleService, ProductionMigrationService, ProductionResumeService,
    ProductionRoutingService, ProductionSessionExportService, ProductionSessionLifecycleService,
    ProductionSessionLockService, ProductionSessionReplaceService, ProductionTraceService,
    QuotaServicePort, ResumeServicePort, SessionExportServicePort, SessionLifecycleServicePort,
    SessionLockServicePort, SessionReplaceServicePort, TraceServicePort,
};
use oulipoly_state::repositories::ProductionStateDbOpener;
use std::path::{Path, PathBuf};
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
    pub executor_service: Arc<dyn ExecutorServicePort>,
    pub quota_service: Arc<dyn QuotaServicePort>,
    pub diagnostics_service: Arc<dyn DiagnosticsServicePort>,
    pub provider_registry: Arc<ProviderRegistry>,
    pub provider_registry_handle: ProviderRegistryHandle,
    pub provider_registry_options: ProviderRegistryOptions,
    pub resume_service: Arc<dyn ResumeServicePort>,
    pub session_lifecycle_service: Arc<dyn SessionLifecycleServicePort>,
    pub migration_service: Arc<dyn MigrationServicePort>,
    pub trace_service: Arc<dyn TraceServicePort>,
    pub session_export_service: Arc<dyn SessionExportServicePort>,
    pub session_replace_service: Arc<dyn SessionReplaceServicePort>,
    pub session_lock_service: Arc<dyn SessionLockServicePort>,
}

impl AgentRuntimeServices {
    pub fn cli_defaults() -> Self {
        let paths = default_cli_runtime_paths();
        let provider_registry_options = ProviderRegistryOptions::default()
            .with_config_root(paths.config_root.clone())
            .with_data_root(paths.data_root.clone());
        let provider_registry = Arc::new(production_provider_registry(
            &paths,
            provider_registry_options.clone(),
        ));
        let provider_registry_handle = ProviderRegistryHandle::new(provider_registry.clone());
        let session_lifecycle_service =
            Arc::new(ProductionSessionLifecycleService::with_registry_handle(
                provider_registry_handle.clone(),
            ));
        Self {
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
            executor_service: Arc::new(RuntimeExecutorService::with_registry_handle(
                provider_registry_handle.clone(),
            )),
            quota_service: Arc::new(RuntimeQuotaService::with_registry_handle(
                provider_registry_handle.clone(),
            )),
            diagnostics_service: Arc::new(RuntimeDiagnosticsService::with_registry_handle(
                provider_registry_handle.clone(),
            )),
            provider_registry,
            provider_registry_handle: provider_registry_handle.clone(),
            provider_registry_options,
            resume_service: Arc::new(ProductionResumeService::new()),
            session_lifecycle_service,
            migration_service: Arc::new(ProductionMigrationService::with_registry_handle(
                provider_registry_handle.clone(),
            )),
            trace_service: Arc::new(ProductionTraceService::default()),
            session_export_service: Arc::new(ProductionSessionExportService::with_registry_handle(
                provider_registry_handle.clone(),
            )),
            session_replace_service: Arc::new(
                ProductionSessionReplaceService::with_registry_handle(
                    provider_registry_handle.clone(),
                ),
            ),
            session_lock_service: Arc::new(ProductionSessionLockService::default()),
        }
    }

    pub fn production(paths: RuntimePaths) -> Result<Self, String> {
        prepare_runtime_directories(&paths)?;
        let registry_options = ProviderRegistryOptions::default()
            .with_config_root(paths.config_root.clone())
            .with_data_root(paths.data_root.clone());
        let provider_registry = Arc::new(production_provider_registry(
            &paths,
            registry_options.clone(),
        ));
        let provider_registry_handle = ProviderRegistryHandle::new(provider_registry.clone());
        let session_lifecycle_service =
            Arc::new(ProductionSessionLifecycleService::with_registry_handle(
                provider_registry_handle.clone(),
            ));

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
            executor_service: Arc::new(RuntimeExecutorService::with_registry_handle(
                provider_registry_handle.clone(),
            )),
            quota_service: Arc::new(RuntimeQuotaService::with_registry_handle(
                provider_registry_handle.clone(),
            )),
            diagnostics_service: Arc::new(RuntimeDiagnosticsService::with_registry_handle(
                provider_registry_handle.clone(),
            )),
            provider_registry,
            provider_registry_handle: provider_registry_handle.clone(),
            provider_registry_options: registry_options,
            resume_service: Arc::new(ProductionResumeService::new()),
            session_lifecycle_service,
            migration_service: Arc::new(ProductionMigrationService::with_registry_handle(
                provider_registry_handle.clone(),
            )),
            trace_service: Arc::new(ProductionTraceService::default()),
            session_export_service: Arc::new(ProductionSessionExportService::with_registry_handle(
                provider_registry_handle.clone(),
            )),
            session_replace_service: Arc::new(
                ProductionSessionReplaceService::with_registry_handle(
                    provider_registry_handle.clone(),
                ),
            ),
            session_lock_service: Arc::new(ProductionSessionLockService::default()),
        })
    }
}

fn default_cli_runtime_paths() -> RuntimePaths {
    let config_root = dirs::config_dir()
        .map(|dir| dir.join("oulipoly-agent-runner"))
        .unwrap_or_else(|| PathBuf::from("."));
    let models_dir = config_root.join("models");
    let data_root = dirs::data_dir()
        .map(|dir| dir.join("oulipoly-agent-runner"))
        .unwrap_or_else(|| config_root.clone());
    RuntimePaths {
        config_root: config_root.clone(),
        models_dir,
        agents_dir: config_root.join("agents"),
        data_root: data_root.clone(),
        state_db_path: data_root.join("state.db"),
        lock_dir: data_root.join("locks"),
        working_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    }
}

fn production_provider_registry(
    paths: &RuntimePaths,
    options: ProviderRegistryOptions,
) -> ProviderRegistry {
    let fallback_options = options.clone();
    let providers = load_registry_providers(paths);
    let models = load_registry_models(paths, &providers);
    registry_from_model_configs(models, options)
        .unwrap_or_else(|_| empty_provider_registry(fallback_options))
}

fn load_registry_providers(paths: &RuntimePaths) -> config::ProvidersConfig {
    FilesystemProvidersConfigRepository
        .load_providers(&paths.config_root.join("providers.toml"))
        .unwrap_or_default()
}

fn load_registry_models(
    paths: &RuntimePaths,
    providers: &config::ProvidersConfig,
) -> std::collections::HashMap<String, config::ModelConfig> {
    config::load_models(&paths.models_dir, Some(providers)).unwrap_or_default()
}

fn registry_from_model_configs(
    models: std::collections::HashMap<String, config::ModelConfig>,
    options: ProviderRegistryOptions,
) -> Result<ProviderRegistry, oulipoly_runtime::provider_registry::ProviderRegistryError> {
    ProviderRegistry::from_model_configs(&models.into_values().collect::<Vec<_>>(), options)
}

fn empty_provider_registry(options: ProviderRegistryOptions) -> ProviderRegistry {
    ProviderRegistry::empty(options)
}

fn prepare_runtime_directories(paths: &RuntimePaths) -> Result<(), String> {
    for (path, label) in runtime_directory_targets(paths) {
        create_runtime_directory(path, label)?;
    }
    if let Some(parent) = paths.state_db_path.parent() {
        create_runtime_directory(parent, "state DB directory")?;
    }
    Ok(())
}

fn runtime_directory_targets(paths: &RuntimePaths) -> [(&Path, &'static str); 6] {
    [
        (&paths.config_root, "config root"),
        (&paths.models_dir, "models directory"),
        (&paths.agents_dir, "agents directory"),
        (&paths.data_root, "data root"),
        (&paths.lock_dir, "lock directory"),
        (&paths.working_dir, "working directory"),
    ]
}

fn create_runtime_directory(path: &Path, label: &str) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|error| format_runtime_directory_error(label, error))
}

fn format_runtime_directory_error(label: &str, error: std::io::Error) -> String {
    format!("Failed to create {label}: {error}")
}
