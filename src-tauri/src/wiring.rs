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
    ProductionRoutingService, ProductionSessionExportService, ProductionSessionImportService,
    ProductionSessionLifecycleService, ProductionSessionLockService,
    ProductionSessionReplaceService, ProductionTraceService, QuotaServicePort, ResumeServicePort,
    SessionExportServicePort, SessionImportServicePort, SessionLifecycleServicePort,
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
    pub session_import_service: Arc<dyn SessionImportServicePort>,
    pub migration_service: Arc<dyn MigrationServicePort>,
    pub trace_service: Arc<dyn TraceServicePort>,
    pub session_export_service: Arc<dyn SessionExportServicePort>,
    pub session_replace_service: Arc<dyn SessionReplaceServicePort>,
    pub session_lock_service: Arc<dyn SessionLockServicePort>,
}

impl AgentRuntimeServices {
    pub fn cli_defaults() -> Result<Self, String> {
        let paths = default_cli_runtime_paths()?;
        let provider_registry_options = ProviderRegistryOptions::default()
            .with_path_entries_from_process_path()
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
        let session_import_service = Arc::new(
            ProductionSessionImportService::with_registry_handle(provider_registry_handle.clone()),
        );
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
            provider_registry_options,
            resume_service: Arc::new(ProductionResumeService::new()),
            session_lifecycle_service,
            session_import_service,
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

    pub fn production(paths: RuntimePaths) -> Result<Self, String> {
        prepare_runtime_directories(&paths)?;
        let registry_options = ProviderRegistryOptions::default()
            .with_path_entries_from_process_path()
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
        let session_import_service = Arc::new(
            ProductionSessionImportService::with_registry_handle(provider_registry_handle.clone()),
        );

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
            session_import_service,
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

fn default_cli_runtime_paths() -> Result<RuntimePaths, String> {
    let config_root = oulipoly_state::paths::config_dir()?;
    let models_dir = config_root.join("models");
    let data_root = oulipoly_state::paths::data_dir()?;
    let working_dir = std::env::current_dir()
        .map_err(|error| format!("Could not resolve current working directory: {error}"))?;
    Ok(RuntimePaths {
        config_root: config_root.clone(),
        models_dir,
        agents_dir: config_root.join("agents"),
        data_root: data_root.clone(),
        state_db_path: data_root.join("state.db"),
        lock_dir: data_root.join("locks"),
        working_dir,
    })
}

fn production_provider_registry(
    paths: &RuntimePaths,
    options: ProviderRegistryOptions,
) -> ProviderRegistry {
    let fallback_options = options.clone();
    let providers = load_registry_providers(paths);
    let models = load_registry_models(paths, &providers);
    registry_from_model_configs(&models, &providers, options)
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

pub(crate) fn registry_from_model_configs(
    models: &std::collections::HashMap<String, config::ModelConfig>,
    providers: &config::ProvidersConfig,
    options: ProviderRegistryOptions,
) -> Result<ProviderRegistry, oulipoly_runtime::provider_registry::ProviderRegistryError> {
    ProviderRegistry::from_model_configs_with_provider_config(
        &models.values().cloned().collect::<Vec<_>>(),
        providers,
        options,
    )
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use oulipoly_config::provider_implementation_ref::ProviderImplementationRef;
    use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig, ProviderEntry};
    use std::collections::HashMap;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn registry_keeps_account_inferred_and_explicit_artifacts_without_spawning_on_build() {
        let dir = tempfile::tempdir().expect("tempdir");
        let count = dir.path().join("describe-count");
        let inferred = write_describe_provider(dir.path(), "agent-runner-fixture", &count);
        let explicit_count = dir.path().join("explicit-describe-count");
        let explicit = write_describe_provider(dir.path(), "explicit-provider", &explicit_count);
        let account_model = model("account-model", "account", None);
        let explicit_model = model(
            "explicit-model",
            "account",
            Some(ProviderImplementationRef {
                path: Some(explicit.display().to_string()),
                crate_name: None,
                version: None,
                binary: None,
                script: None,
            }),
        );
        let providers = config::ProvidersConfig {
            entries: HashMap::from([(
                "account".to_string(),
                ProviderEntry {
                    command: Some("fixture5".to_string()),
                    ..Default::default()
                },
            )]),
        };
        let models = HashMap::from([
            (account_model.name.clone(), account_model),
            (explicit_model.name.clone(), explicit_model),
        ]);
        let registry = registry_from_model_configs(
            &models,
            &providers,
            ProviderRegistryOptions::default().with_path_entries([dir.path().to_path_buf()]),
        )
        .expect("registry");

        assert!(
            !count.exists(),
            "registry construction must not run describe"
        );
        assert!(
            !explicit_count.exists(),
            "explicit artifact construction must not run describe"
        );
        assert_eq!(
            inferred.file_name().and_then(|name| name.to_str()),
            Some("agent-runner-fixture")
        );
        assert_eq!(
            registry
                .describe_model_provider_instance("account-model", "account")
                .expect("inferred account artifact")
                .provider_id,
            "fixture-provider"
        );
        assert_eq!(fs::read_to_string(&count).expect("inferred count"), "1");
        assert_eq!(
            registry
                .describe_model_provider_instance("explicit-model", "account")
                .expect("explicit model artifact")
                .provider_id,
            "fixture-provider"
        );
        assert_eq!(
            fs::read_to_string(&explicit_count).expect("explicit count"),
            "1"
        );
    }

    fn model(
        name: &str,
        account: &str,
        provider: Option<ProviderImplementationRef>,
    ) -> ModelConfig {
        ModelConfig {
            name: name.to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![ProviderConfig::model_provider(account, Vec::new())],
            inputs: Vec::new(),
            provider,
        }
    }

    fn write_describe_provider(dir: &Path, name: &str, count: &Path) -> PathBuf {
        let path = dir.join(name);
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
  "request_id": request.get("request_id", "wiring-test"),
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
        fs::write(&path, body).expect("provider script");
        let mut permissions = fs::metadata(&path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("chmod");
        path
    }
}
