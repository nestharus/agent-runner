//! ## Declared roles
//!
//! `accessor`, `filter`, `mapper`, `orchestration`, `predicate`, `validator`
//!
//! Finalized resume execution-target and migration-pool resolution.

use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig, ProvidersConfig};

#[derive(Clone)]
pub(crate) struct ResumeExecutionTarget {
    pub(crate) model: Option<ModelConfig>,
    pub(crate) provider_index: usize,
    pub(crate) provider: ProviderConfig,
    pub(crate) prompt_mode: PromptMode,
}

pub(super) fn resume_execution_target(
    resolved: &oulipoly_state::ResolvedResume,
    providers_cfg: &ProvidersConfig,
) -> Result<ResumeExecutionTarget, oulipoly_state::ResumeError> {
    if let Some(model) = resolved.model.as_ref() {
        return resume_model_execution_target(model, &resolved.active_provider, providers_cfg);
    }
    resume_provider_execution_target(&resolved.active_provider, providers_cfg)
}

pub(crate) fn interactive_resume_execution_target(
    resolved: &oulipoly_state::ResolvedResume,
    providers_cfg: &ProvidersConfig,
) -> Result<ResumeExecutionTarget, oulipoly_state::ResumeError> {
    let target = resume_execution_target(resolved, providers_cfg)?;
    let (provider, prompt_mode) = providers_cfg
        .runtime_provider(&resolved.active_provider)
        .map_err(resume_db_error)?;
    Ok(map_interactive_execution_target(
        target,
        provider,
        prompt_mode,
    ))
}

fn map_interactive_execution_target(
    target: ResumeExecutionTarget,
    provider: ProviderConfig,
    prompt_mode: PromptMode,
) -> ResumeExecutionTarget {
    ResumeExecutionTarget {
        model: target.model,
        provider_index: target.provider_index,
        provider,
        prompt_mode,
    }
}

fn resume_model_execution_target(
    model: &ModelConfig,
    active_provider: &str,
    providers_cfg: &ProvidersConfig,
) -> Result<ResumeExecutionTarget, oulipoly_state::ResumeError> {
    let provider_index = resolve_model_provider_index(model, active_provider)?;
    let (provider, prompt_mode) = providers_cfg
        .effective_provider(&model.providers[provider_index])
        .map_err(resume_db_error)?;
    Ok(map_model_execution_target(
        model,
        provider_index,
        provider,
        prompt_mode,
    ))
}

fn map_model_execution_target(
    model: &ModelConfig,
    provider_index: usize,
    provider: ProviderConfig,
    prompt_mode: PromptMode,
) -> ResumeExecutionTarget {
    ResumeExecutionTarget {
        model: Some(model.clone()),
        provider_index,
        provider,
        prompt_mode,
    }
}

fn resolve_model_provider_index(
    model: &ModelConfig,
    active_provider: &str,
) -> Result<usize, oulipoly_state::ResumeError> {
    provider_index_in_model(model, active_provider)
        .ok_or_else(|| provider_model_mismatch_for_index(model, active_provider))
}

fn provider_index_in_model(model: &ModelConfig, active_provider: &str) -> Option<usize> {
    model
        .providers
        .iter()
        .position(|provider| provider.name == active_provider)
}

fn provider_model_mismatch_for_index(
    model: &ModelConfig,
    active_provider: &str,
) -> oulipoly_state::ResumeError {
    oulipoly_state::ResumeError::ProviderModelMismatch {
        model_name: model.name.clone(),
        active_provider: active_provider.to_string(),
        suggestions: Vec::new(),
    }
}

fn resume_provider_execution_target(
    active_provider: &str,
    providers_cfg: &ProvidersConfig,
) -> Result<ResumeExecutionTarget, oulipoly_state::ResumeError> {
    let (provider, prompt_mode) = providers_cfg
        .runtime_provider(active_provider)
        .map_err(resume_db_error)?;
    let provider_index = provider_index_in_providers_cfg(providers_cfg, active_provider);
    Ok(map_provider_execution_target(
        provider_index,
        provider,
        prompt_mode,
    ))
}

fn map_provider_execution_target(
    provider_index: usize,
    provider: ProviderConfig,
    prompt_mode: PromptMode,
) -> ResumeExecutionTarget {
    ResumeExecutionTarget {
        model: None,
        provider_index,
        provider,
        prompt_mode,
    }
}

fn resume_db_error(message: String) -> oulipoly_state::ResumeError {
    oulipoly_state::ResumeError::Db { message }
}

fn provider_index_in_providers_cfg(providers_cfg: &ProvidersConfig, provider_name: &str) -> usize {
    sorted_provider_name_refs(providers_cfg)
        .into_iter()
        .position(|name| name == provider_name)
        .unwrap_or(0)
}

fn sorted_provider_name_refs(providers_cfg: &ProvidersConfig) -> Vec<&String> {
    let mut names = providers_cfg.entries.keys().collect::<Vec<_>>();
    names.sort();
    names
}

pub(crate) fn resume_migration_pool(
    resolved: &oulipoly_state::ResolvedResume,
    providers_cfg: &ProvidersConfig,
) -> ModelConfig {
    if let Some(model) = resolved.model.as_ref() {
        return resume_migration_model_pool(model, providers_cfg);
    }
    provider_default_migration_pool(&resolved.active_provider, providers_cfg)
}

fn resume_migration_model_pool(
    model: &ModelConfig,
    providers_cfg: &ProvidersConfig,
) -> ModelConfig {
    let mut effective = model.clone();
    effective.providers = effective_migration_providers(model, providers_cfg);
    effective
}

fn effective_migration_providers(
    model: &ModelConfig,
    providers_cfg: &ProvidersConfig,
) -> Vec<ProviderConfig> {
    present_provider_configs(effective_migration_provider_options(model, providers_cfg))
}

fn effective_migration_provider_options(
    model: &ModelConfig,
    providers_cfg: &ProvidersConfig,
) -> Vec<Option<ProviderConfig>> {
    model
        .providers
        .iter()
        .map(|provider| effective_migration_provider(provider, providers_cfg))
        .collect()
}

fn effective_migration_provider(
    provider: &ProviderConfig,
    providers_cfg: &ProvidersConfig,
) -> Option<ProviderConfig> {
    providers_cfg
        .effective_provider(provider)
        .ok()
        .map(|provider| provider.0)
}

fn present_provider_configs(options: Vec<Option<ProviderConfig>>) -> Vec<ProviderConfig> {
    options.into_iter().flatten().collect()
}

fn provider_default_migration_pool(
    active_provider: &str,
    providers_cfg: &ProvidersConfig,
) -> ModelConfig {
    provider_default_model_config(runtime_migration_providers(active_provider, providers_cfg))
}

fn provider_default_model_config(providers: Vec<ProviderConfig>) -> ModelConfig {
    ModelConfig {
        name: provider_default_model_name(),
        prompt_mode: provider_default_prompt_mode(),
        providers,
        inputs: Vec::new(),
        provider: None,
    }
}

fn provider_default_model_name() -> String {
    "<provider-default>".to_string()
}

fn provider_default_prompt_mode() -> PromptMode {
    PromptMode::Stdin
}

fn runtime_migration_providers(
    active_provider: &str,
    providers_cfg: &ProvidersConfig,
) -> Vec<ProviderConfig> {
    present_provider_configs(runtime_migration_provider_options(
        resume_migration_provider_names(active_provider, providers_cfg),
        providers_cfg,
    ))
}

fn resume_migration_provider_names(
    active_provider: &str,
    providers_cfg: &ProvidersConfig,
) -> Vec<String> {
    filter_resume_migration_provider_names(
        sorted_provider_names(providers_cfg),
        active_provider,
        providers_cfg,
    )
}

fn filter_resume_migration_provider_names(
    names: Vec<String>,
    active_provider: &str,
    providers_cfg: &ProvidersConfig,
) -> Vec<String> {
    names
        .into_iter()
        .filter(|name| is_resume_migration_provider(name, active_provider, providers_cfg))
        .collect()
}

fn runtime_migration_provider_options(
    names: Vec<String>,
    providers_cfg: &ProvidersConfig,
) -> Vec<Option<ProviderConfig>> {
    names
        .into_iter()
        .map(|name| runtime_provider_config(&name, providers_cfg))
        .collect()
}

fn runtime_provider_config(name: &str, providers_cfg: &ProvidersConfig) -> Option<ProviderConfig> {
    providers_cfg
        .runtime_provider(name)
        .ok()
        .map(|provider| provider.0)
}

fn sorted_provider_names(providers_cfg: &ProvidersConfig) -> Vec<String> {
    let mut names = providers_cfg.entries.keys().cloned().collect::<Vec<_>>();
    names.sort();
    names
}

fn is_resume_migration_provider(
    name: &str,
    active_provider: &str,
    providers_cfg: &ProvidersConfig,
) -> bool {
    name == active_provider
        || providers_cfg
            .get(name)
            .is_some_and(|entry| entry.session_storage.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_config::{ProviderEntry, ResumeKind, ResumeStrategy, SessionStorage};

    fn providers_cfg_with_storage(names: &[&str]) -> ProvidersConfig {
        let mut cfg = ProvidersConfig::default();
        for name in names {
            insert_provider_entry_with_storage(&mut cfg, name);
        }
        cfg
    }

    fn insert_provider_entry_with_storage(cfg: &mut ProvidersConfig, name: &str) {
        cfg.entries
            .insert(name.to_string(), provider_entry_with_storage(name));
    }

    fn provider_entry_with_storage(name: &str) -> ProviderEntry {
        ProviderEntry {
            command: Some(name.to_string()),
            session_storage: Some(script_session_storage(name)),
            resume: Some(resume_flag_strategy()),
            ..ProviderEntry::default()
        }
    }

    fn script_session_storage(name: &str) -> SessionStorage {
        SessionStorage::Script {
            cwd_script: format!("fixture-cwd-{name}"),
            transcript_script: None,
            storage_type: None,
        }
    }

    fn resume_flag_strategy() -> ResumeStrategy {
        ResumeStrategy {
            kind: ResumeKind::Flag,
            flag: Some("--resume".to_string()),
            subcommand: None,
        }
    }

    #[test]
    fn migration_target_pool_when_model_none_is_all_storage_providers() {
        let resolved = oulipoly_state::ResolvedResume {
            chain_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_string(),
            model_name: None,
            model: None,
            active_provider: "fixture-a".to_string(),
            active_session_id: "5169694d-de0f-40d1-890c-6e28e55bab27".to_string(),
        };
        let providers_cfg = providers_cfg_with_storage(&["fixture-a", "fixture-b", "fixture-c"]);

        let pool = resume_migration_pool(&resolved, &providers_cfg);
        let names = pool
            .providers
            .iter()
            .map(|provider| provider.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["fixture-a", "fixture-b", "fixture-c"]);
    }

    #[test]
    fn migration_target_pool_when_model_set_is_model_pool() {
        let model = ModelConfig::from_toml_with_name(
            "fixture-model",
            r#"
[[providers]]
name = "fixture-a"
args = ["--model", "opus"]

[[providers]]
name = "fixture-b"
args = ["--model", "opus"]
"#,
            None,
        )
        .unwrap();
        let resolved = oulipoly_state::ResolvedResume {
            chain_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_string(),
            model_name: Some("fixture-model".to_string()),
            model: Some(model),
            active_provider: "fixture-a".to_string(),
            active_session_id: "5169694d-de0f-40d1-890c-6e28e55bab27".to_string(),
        };
        let providers_cfg = providers_cfg_with_storage(&["fixture-a", "fixture-b", "fixture-c"]);

        let pool = resume_migration_pool(&resolved, &providers_cfg);
        let names = pool
            .providers
            .iter()
            .map(|provider| provider.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["fixture-a", "fixture-b"]);
    }
}
