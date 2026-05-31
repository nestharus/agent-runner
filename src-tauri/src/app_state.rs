//! ## Declared roles
//!
//! `orchestration`, `mapper`
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: src-tauri/src/app_state.rs
//!     role: intrinsic-surface
//!     Domain: application runtime state container for the Tauri host
//!     Owns:
//!       - model cache and models root-path state (models, models_dir)
//!       - setup response input channel state (setup_input_tx, mpsc::Sender<UserResponse>)
//!       - runtime service-bundle input (wiring::AgentRuntimeServices)
//!       - runtime service-port aggregate (state_db_opener, providers_config, routing_service, quota_service, executor_service, diagnostics_service)
//!       - production service defaults (ProductionStateDbOpener, FilesystemProvidersConfigRepository, ProductionRoutingService, RuntimeQuotaService, RuntimeExecutorService, RuntimeDiagnosticsService)
//!       - test service defaults and doubles (AppStateTestServices, provider_settings_test_double, setup_repository)
//!       - provider-settings host surface (ProviderSettingsHost, ProviderSettingsCommandResponses, build_host/from_model_configs/host_options)
//!       - quota in-flight state (quota::InFlight)
//!       - setup/providers repositories + test-only doubles
//! ```

use crate::provider_settings;
use crate::wiring;
use oulipoly_config as config;
use oulipoly_config::repositories::{
    FilesystemProvidersConfigRepository, ProvidersConfigRepository,
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
            provider_settings: Mutex::new(provider_settings),
            provider_settings_test_double: Mutex::new(None),
            setup_repository: None,
        }
    }

    pub fn test_default(models_dir: PathBuf, models: HashMap<String, config::ModelConfig>) -> Self {
        let provider_settings = provider_settings_host_for_models(&models_dir, &models);
        Self {
            models: Mutex::new(models),
            models_dir,
            setup_input_tx: Mutex::new(None),
            quota_in_flight: oulipoly_runtime::quota::InFlight::new(),
            state_db_opener: Arc::new(ProductionStateDbOpener),
            providers_config: Arc::new(FilesystemProvidersConfigRepository),
            routing_service: Arc::new(oulipoly_runtime::services::ProductionRoutingService),
            quota_service: Arc::new(oulipoly_runtime::quota::RuntimeQuotaService),
            executor_service: Arc::new(oulipoly_runtime::executor::RuntimeExecutorService),
            diagnostics_service: Arc::new(oulipoly_runtime::diagnostics::RuntimeDiagnosticsService),
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
