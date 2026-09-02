//! Declared roles: orchestration, accessor, mapper, filter, validator

use crate::cli::paths::{default_config_root, default_models_dir};
use crate::wiring;
use chrono::Utc;
use oulipoly_config::{ModelConfig, ProvidersConfig, load_models};
use oulipoly_runtime::provider_registry::{ProviderRegistry, ProviderRegistryOptions};
use oulipoly_runtime::services::{SessionImportProviderTarget, SessionImportServiceRequest};
use oulipoly_state::StateDb;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(crate) struct SessionImportCliArgs<'a> {
    pub(crate) provider: Option<&'a str>,
    pub(crate) limit: Option<u64>,
    pub(crate) since_unix_ms: Option<u64>,
    pub(crate) backfill_turns: bool,
    pub(crate) json: bool,
}

pub(crate) fn run_session_import(
    args: SessionImportCliArgs<'_>,
    agent_runtime_services: &wiring::AgentRuntimeServices,
) -> Result<i32, String> {
    let env = load_session_import_environment()?;
    let targets = session_import_targets(&env.models, &env.provider_registry, args.provider)?;
    agent_runtime_services
        .provider_registry_handle
        .replace(Arc::new(env.provider_registry));

    let effective_cwd = current_effective_cwd();
    let output = agent_runtime_services
        .session_import_service
        .import_sessions(SessionImportServiceRequest {
            state: &env.state,
            providers: &targets,
            observed_at: Utc::now(),
            limit: args.limit,
            since_unix_ms: args.since_unix_ms,
            effective_cwd: effective_cwd.as_deref(),
            backfill_turns: args.backfill_turns,
        })
        .map_err(|error| error.to_string())?;

    super::formatter::render_session_import_report(&output.report, args.provider, args.json)?;
    Ok(0)
}

struct SessionImportEnvironment {
    state: StateDb,
    models: Vec<ModelConfig>,
    provider_registry: ProviderRegistry,
}

fn load_session_import_environment() -> Result<SessionImportEnvironment, String> {
    let config_root = default_config_root()?;
    let state = StateDb::open_default().map_err(format_session_import_state_error)?;
    let providers_cfg = load_default_session_import_providers(&config_root)?;
    let models = load_default_session_import_models(&providers_cfg)?;
    let provider_registry =
        build_session_import_provider_registry(&models, &providers_cfg, config_root)?;
    Ok(SessionImportEnvironment {
        state,
        models,
        provider_registry,
    })
}

fn load_default_session_import_providers(config_root: &Path) -> Result<ProvidersConfig, String> {
    ProvidersConfig::load(&config_root.join("providers.toml"))
        .map_err(|error| format!("Failed to load providers: {error}"))
}

fn load_default_session_import_models(
    providers_cfg: &ProvidersConfig,
) -> Result<Vec<ModelConfig>, String> {
    let mut models = load_models(&default_models_dir()?, Some(providers_cfg))
        .map_err(|error| format!("Failed to load models: {error}"))?
        .into_values()
        .collect::<Vec<_>>();
    models.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(models)
}

fn build_session_import_provider_registry(
    models: &[ModelConfig],
    providers_cfg: &ProvidersConfig,
    config_root: PathBuf,
) -> Result<ProviderRegistry, String> {
    ProviderRegistry::from_configs(
        models,
        providers_cfg,
        ProviderRegistryOptions::default()
            .with_config_root(config_root)
            .with_data_root(default_data_root()?),
    )
    .map_err(|error| format!("Failed to build provider registry: {error}"))
}

fn default_data_root() -> Result<PathBuf, String> {
    StateDb::default_path()?
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "Could not determine data root".to_string())
}

pub(super) fn session_import_targets(
    models: &[ModelConfig],
    provider_registry: &ProviderRegistry,
    provider_filter: Option<&str>,
) -> Result<Vec<SessionImportProviderTarget>, String> {
    let mut targets = Vec::new();
    for provider_name in provider_registry.configured_account_names() {
        let mut routed_models = models
            .iter()
            .filter(|model| {
                model
                    .providers
                    .iter()
                    .any(|provider| provider.name == provider_name)
            })
            .collect::<Vec<_>>();
        routed_models.sort_by(|left, right| left.name.cmp(&right.name));
        if !provider_matches_filter(&provider_name, &routed_models, provider_filter) {
            continue;
        }
        let settings_id = provider_registry
            .account_settings_id(&provider_name)
            .map_err(|error| error.to_string())?;
        targets.push(SessionImportProviderTarget {
            model_name: routed_models
                .first()
                .map(|model| model.name.clone())
                .unwrap_or_default(),
            provider_name: provider_name.clone(),
            provider_instance_id: None,
            settings_id: settings_id.to_string(),
        });
    }
    Ok(targets)
}

fn provider_matches_filter(
    provider_name: &str,
    models: &[&ModelConfig],
    filter: Option<&str>,
) -> bool {
    match filter.map(str::trim).filter(|value| !value.is_empty()) {
        Some(filter) => provider_name == filter || models.iter().any(|model| model.name == filter),
        None => true,
    }
}

fn current_effective_cwd() -> Option<PathBuf> {
    std::env::current_dir().ok()
}

fn format_session_import_state_error(error: String) -> String {
    format!("Failed to open state DB: {error}")
}
