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
        let provider_settings =
            oulipoly_runtime::provider_settings::ProviderSettingsHost::with_registry_handle(
                services.provider_registry_handle.clone(),
            );
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
        let providers_config = FilesystemProvidersConfigRepository;
        let provider_registry = test_provider_registry(
            &models,
            &providers_config,
            &provider_config_path(&models_dir),
            provider_registry_options.clone(),
        );
        let provider_settings =
            oulipoly_runtime::provider_settings::ProviderSettingsHost::with_registry_handle(
                provider_registry.clone(),
            );
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
    providers_repository: &(dyn ProvidersConfigRepository + Send + Sync),
    providers_path: &Path,
    provider_registry_options: ProviderRegistryOptions,
) -> ProviderRegistryHandle {
    let provider_registry = empty_provider_registry_handle(provider_registry_options.clone());
    refresh_provider_registry_handle_with_options(
        &provider_registry,
        models,
        providers_repository,
        providers_path,
        provider_registry_options,
    )
    .expect("test provider endpoint registry should build");
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
        let provider_registry = test_provider_registry(
            &models,
            services.providers_config.as_ref(),
            &provider_config_path(&models_dir),
            provider_registry_options.clone(),
        );
        let provider_settings =
            oulipoly_runtime::provider_settings::ProviderSettingsHost::with_registry_handle(
                provider_registry.clone(),
            );
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
            provider_registry,
            provider_registry_options,
            provider_settings: Mutex::new(provider_settings),
            provider_settings_test_double: Mutex::new(None),
            setup_repository: Some(services.setup_repository),
        }
    }
}

pub(crate) fn refresh_provider_registry(state: &AppState) -> Result<(), String> {
    let models = state.models.lock().map_err(|error| error.to_string())?;
    refresh_provider_registry_handle_with_options(
        &state.provider_registry,
        &models,
        state.providers_config.as_ref(),
        &provider_config_path(&state.models_dir),
        state.provider_registry_options.clone(),
    )?;
    Ok(())
}

fn refresh_provider_registry_handle_with_options(
    handle: &ProviderRegistryHandle,
    models: &HashMap<String, config::ModelConfig>,
    providers_repository: &(dyn ProvidersConfigRepository + Send + Sync),
    providers_path: &Path,
    options: ProviderRegistryOptions,
) -> Result<(), String> {
    let providers = providers_repository
        .load_providers(providers_path)
        .map_err(|error| format!("Failed to load provider endpoint configuration: {error}"))?;
    let registry = wiring::registry_from_configs(models, &providers, options)
        .map_err(|error| format!("Failed to build provider endpoint registry: {error}"))?;
    handle.replace(Arc::new(registry));
    Ok(())
}

fn provider_config_path(models_dir: &Path) -> PathBuf {
    models_dir
        .parent()
        .unwrap_or(models_dir)
        .join("providers.toml")
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use oulipoly_config::{
        ModelConfig, PromptMode, ProviderConfig, ProviderEndpointConfig, ProviderEntry,
    };
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    struct FixedProviders(config::ProvidersConfig);

    impl ProvidersConfigRepository for FixedProviders {
        fn load_providers(&self, _path: &Path) -> Result<config::ProvidersConfig, String> {
            Ok(self.0.clone())
        }

        fn get<'a>(
            &self,
            providers: &'a config::ProvidersConfig,
            name: &str,
        ) -> Option<&'a ProviderEntry> {
            providers.get(name)
        }

        fn effective_provider(
            &self,
            providers: &config::ProvidersConfig,
            provider: &ProviderConfig,
        ) -> Result<(ProviderConfig, PromptMode), String> {
            providers.effective_provider(provider)
        }

        fn runtime_provider(
            &self,
            providers: &config::ProvidersConfig,
            name: &str,
        ) -> Result<(ProviderConfig, PromptMode), String> {
            providers.runtime_provider(name)
        }
    }

    #[test]
    fn initial_and_refreshed_handles_keep_provider_account_artifacts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let count = dir.path().join("describe-count");
        write_describe_provider(dir.path().join("agent-runner-fixture"), &count);
        let model = ModelConfig {
            name: "account-model".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![ProviderConfig::model_provider("account", Vec::new())],
            inputs: Vec::new(),
            provider: None,
        };
        let models = HashMap::from([(model.name.clone(), model)]);
        let repository = FixedProviders(config::ProvidersConfig {
            entries: HashMap::from([(
                "account".to_string(),
                ProviderEntry {
                    implementation: Some(ProviderEndpointConfig {
                        family: "fixture".to_string(),
                        executable: dir
                            .path()
                            .join("agent-runner-fixture")
                            .display()
                            .to_string(),
                    }),
                    command: Some("fixture9".to_string()),
                    ..Default::default()
                },
            )]),
        });
        let options =
            ProviderRegistryOptions::default().with_path_entries([dir.path().to_path_buf()]);
        let handle = test_provider_registry(
            &models,
            &repository,
            &dir.path().join("providers.toml"),
            options.clone(),
        );

        assert!(
            !count.exists(),
            "initial construction must not spawn adapter"
        );
        assert_eq!(
            handle
                .current()
                .describe_model_provider_instance("account-model", "account")
                .expect("initial account artifact")
                .provider_id,
            "fixture-provider"
        );
        refresh_provider_registry_handle_with_options(
            &handle,
            &models,
            &repository,
            &dir.path().join("providers.toml"),
            options,
        )
        .expect("refreshed provider endpoint registry");
        assert_eq!(
            handle
                .current()
                .describe_model_provider_instance("account-model", "account")
                .expect("refreshed account artifact")
                .provider_id,
            "fixture-provider"
        );
        assert_eq!(fs::read_to_string(count).expect("count"), "2");
    }

    fn write_describe_provider(path: PathBuf, count: &Path) {
        let body = format!(
            r#"#!/usr/bin/env python3
import json
import pathlib
import sys
request = json.loads(sys.stdin.read() or "{{}}")
count = pathlib.Path({count:?})
value = int(count.read_text()) + 1 if count.exists() else 1
count.write_text(str(value))
print(json.dumps({{
  "contract": request.get("contract", "oulipoly.provider/v1"),
  "request_id": request.get("request_id", "app-state-test"),
  "ok": True,
  "result": {{
    "provider_id": "fixture-provider",
    "display_name": "Fixture Provider",
    "contract_versions": ["oulipoly.provider/v1"],
    "preferred_contract": "oulipoly.provider/v1",
    "capabilities": {{
      "launch": False, "policy": False, "quota": False, "session": True,
      "terminal": False, "rotation": False, "discovery": False,
      "settings": False, "setup_brain": False, "setup": False, "migration": False
    }}
  }}
}}))
"#,
            count = count.display().to_string(),
        );
        fs::write(&path, body).expect("script");
        let mut permissions = fs::metadata(&path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("chmod");
    }
}

fn provider_registry_options(models_dir: &Path) -> ProviderRegistryOptions {
    let root = models_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    ProviderRegistryOptions::default()
        .with_config_root(root.clone())
        .with_data_root(root)
}
