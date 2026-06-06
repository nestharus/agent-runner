//! ## Declared roles
//!
//! `orchestration`, `mapper`, `accessor`
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: src-tauri/src/app_state.rs
//!     role: intrinsic-surface
//!     Domain: app_state_quota_registry_wiring
//!     Owns:
//!       - AppState::test_default
//!       - test_provider_registry
//!       - empty_provider_registry_handle
//!       - RuntimeQuotaService::with_registry_handle provider_registry.clone()
//!       - RuntimeExecutorService::with_registry_handle provider_registry.clone()
//!       - provider_registry field initialization
//! ```

use crate::provider_settings;
use crate::wiring;
use oulipoly_config as config;
use oulipoly_config::repositories::{
    FilesystemProvidersConfigRepository, ProvidersConfigRepository,
};
use oulipoly_runtime::provider_registry::{
    ProviderRegistry, ProviderRegistryHandle, ProviderRegistryOptions,
};
use oulipoly_setup::actions::UserResponse;
use oulipoly_state::repositories::ProductionStateDbOpener;
use oulipoly_state::repositories::SetupRepository;
use oulipoly_state::repositories::StateDbOpener;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

const EMPTY_PROVIDER_SETTINGS_HOST_EXPECT_MESSAGE: &str =
    "empty provider settings host should build";

pub struct AppState {
    pub models: Mutex<HashMap<String, config::ModelConfig>>,
    pub models_dir: PathBuf,
    pub setup_input_tx: Mutex<Option<mpsc::Sender<UserResponse>>>,
    /// Tracks quota-refresh calls in flight so duplicate callers collapse.
    pub quota_in_flight: oulipoly_runtime::quota::InFlight,
    pub(crate) state_db_opener: Arc<dyn StateDbOpener + Send + Sync>,
    pub(crate) providers_config: Arc<dyn ProvidersConfigRepository + Send + Sync>,
    pub(crate) routing_service: Arc<dyn oulipoly_runtime::services::RoutingServicePort>,
    pub(crate) quota_service: Arc<dyn oulipoly_runtime::services::QuotaServicePort>,
    pub(crate) executor_service: Arc<dyn oulipoly_runtime::services::ExecutorServicePort>,
    pub(crate) diagnostics_service: Arc<dyn oulipoly_runtime::services::DiagnosticsServicePort>,
    pub(crate) provider_registry: ProviderRegistryHandle,
    pub(crate) provider_registry_options: ProviderRegistryOptions,
    pub(crate) provider_settings: Mutex<oulipoly_runtime::provider_settings::ProviderSettingsHost>,
    pub(crate) provider_settings_test_double:
        Mutex<Option<provider_settings::ProviderSettingsCommandResponses>>,
    pub(crate) setup_repository: Option<Arc<dyn SetupRepository + Send + Sync>>,
}

impl AppState {
    pub(crate) fn new(
        models_dir: PathBuf,
        models: HashMap<String, config::ModelConfig>,
        services: &wiring::AgentRuntimeServices,
    ) -> Self {
        let state_db_opener: Arc<dyn StateDbOpener + Send + Sync> =
            services.state_db_opener.clone();
        let providers_config: Arc<dyn ProvidersConfigRepository + Send + Sync> =
            services.providers_config.clone();
        let routing_service: Arc<dyn oulipoly_runtime::services::RoutingServicePort> =
            services.routing_service.clone();
        let provider_registry_options = services.provider_registry_options.clone();
        refresh_provider_registry_handle_with_options(
            &services.provider_registry_handle,
            &models,
            provider_registry_options.clone(),
        );
        let provider_settings = provider_settings_host_for_models(&models_dir, &models);
        Self {
            models: Mutex::new(models),
            models_dir,
            setup_input_tx: Mutex::new(None),
            quota_in_flight: oulipoly_runtime::quota::InFlight::new(),
            state_db_opener,
            providers_config,
            routing_service,
            quota_service: Arc::clone(&services.quota_service),
            executor_service: Arc::clone(&services.executor_service),
            diagnostics_service: Arc::clone(&services.diagnostics_service),
            provider_registry: services.provider_registry_handle.clone(),
            provider_registry_options,
            provider_settings: Mutex::new(provider_settings),
            provider_settings_test_double: Mutex::new(None),
            setup_repository: None,
        }
    }

    pub fn test_default(models_dir: PathBuf, models: HashMap<String, config::ModelConfig>) -> Self {
        let provider_registry_options = provider_registry_options(&models_dir);
        let provider_registry = test_provider_registry(&models, provider_registry_options.clone());
        let provider_settings = provider_settings_host_for_models(&models_dir, &models);
        Self {
            models: Mutex::new(models),
            models_dir,
            setup_input_tx: Mutex::new(None),
            quota_in_flight: oulipoly_runtime::quota::InFlight::new(),
            state_db_opener: Arc::new(ProductionStateDbOpener),
            providers_config: Arc::new(FilesystemProvidersConfigRepository),
            routing_service: Arc::new(oulipoly_runtime::services::ProductionRoutingService),
            quota_service: Arc::new(
                oulipoly_runtime::quota::RuntimeQuotaService::with_registry_handle(
                    provider_registry.clone(),
                ),
            ),
            executor_service: Arc::new(
                oulipoly_runtime::executor::RuntimeExecutorService::with_registry_handle(
                    provider_registry.clone(),
                ),
            ),
            diagnostics_service: Arc::new(
                oulipoly_runtime::diagnostics::RuntimeDiagnosticsService::with_registry_handle(
                    provider_registry.clone(),
                ),
            ),
            provider_registry,
            provider_registry_options,
            provider_settings: Mutex::new(provider_settings),
            provider_settings_test_double: Mutex::new(None),
            setup_repository: None,
        }
    }

    pub fn db_path(&self) -> PathBuf {
        self.models_dir
            .parent()
            .unwrap_or(&self.models_dir)
            .join("state.db")
    }
}

fn test_provider_registry(
    models: &HashMap<String, config::ModelConfig>,
    provider_registry_options: ProviderRegistryOptions,
) -> ProviderRegistryHandle {
    let provider_registry = empty_provider_registry_handle(provider_registry_options.clone());
    refresh_provider_registry_handle_with_options(
        &provider_registry,
        models,
        provider_registry_options,
    );
    provider_registry
}

fn empty_provider_registry_handle(
    provider_registry_options: ProviderRegistryOptions,
) -> ProviderRegistryHandle {
    ProviderRegistryHandle::new(Arc::new(ProviderRegistry::empty(provider_registry_options)))
}

pub struct AppStateTestServices {
    pub providers_config: Arc<dyn ProvidersConfigRepository + Send + Sync>,
    pub state_db_opener: Arc<dyn StateDbOpener + Send + Sync>,
    pub setup_repository: Arc<dyn SetupRepository + Send + Sync>,
    pub quota_service: Arc<dyn oulipoly_runtime::services::QuotaServicePort>,
    pub executor_service: Arc<dyn oulipoly_runtime::services::ExecutorServicePort>,
    pub diagnostics_service: Arc<dyn oulipoly_runtime::services::DiagnosticsServicePort>,
}

impl AppState {
    pub fn with_services(
        models_dir: PathBuf,
        models: HashMap<String, config::ModelConfig>,
        services: AppStateTestServices,
    ) -> AppState {
        let provider_registry_options = provider_registry_options(&models_dir);
        let provider_settings = provider_settings_host_for_models(&models_dir, &models);
        AppState {
            models: Mutex::new(models),
            models_dir,
            setup_input_tx: Mutex::new(None),
            quota_in_flight: oulipoly_runtime::quota::InFlight::new(),
            state_db_opener: services.state_db_opener,
            providers_config: services.providers_config,
            routing_service: Arc::new(oulipoly_runtime::services::ProductionRoutingService),
            quota_service: services.quota_service,
            executor_service: services.executor_service,
            diagnostics_service: services.diagnostics_service,
            provider_registry: ProviderRegistryHandle::new(Arc::new(ProviderRegistry::empty(
                provider_registry_options.clone(),
            ))),
            provider_registry_options,
            provider_settings: Mutex::new(provider_settings),
            provider_settings_test_double: Mutex::new(None),
            setup_repository: Some(services.setup_repository),
        }
    }
}

fn provider_settings_host_for_models(
    models_dir: &Path,
    models: &HashMap<String, config::ModelConfig>,
) -> oulipoly_runtime::provider_settings::ProviderSettingsHost {
    provider_settings::build_host(models_dir, models).unwrap_or_else(|_| {
        oulipoly_runtime::provider_settings::ProviderSettingsHost::from_model_configs(
            &[],
            provider_settings::host_options(models_dir),
        )
        .expect(EMPTY_PROVIDER_SETTINGS_HOST_EXPECT_MESSAGE)
    })
}

pub(crate) fn refresh_provider_registry(state: &AppState) -> Result<(), String> {
    let models = state.models.lock().map_err(|error| error.to_string())?;
    refresh_provider_registry_handle_with_options(
        &state.provider_registry,
        &models,
        state.provider_registry_options.clone(),
    );
    Ok(())
}

fn refresh_provider_registry_handle_with_options(
    handle: &ProviderRegistryHandle,
    models: &HashMap<String, config::ModelConfig>,
    options: ProviderRegistryOptions,
) {
    let model_values = models.values().cloned().collect::<Vec<_>>();
    let registry = ProviderRegistry::from_model_configs(&model_values, options.clone())
        .unwrap_or_else(|_| ProviderRegistry::empty(options));
    handle.replace(Arc::new(registry));
}

fn provider_registry_options(models_dir: &Path) -> ProviderRegistryOptions {
    let root = models_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    ProviderRegistryOptions::default()
        .with_path_entries_from_process_path()
        .with_config_root(root.clone())
        .with_data_root(root)
}
