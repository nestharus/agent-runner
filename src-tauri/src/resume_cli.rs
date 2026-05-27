//! Headless resume CLI orchestration helpers.
//!
//! ## Declared roles
//!
//! `orchestration`, `mapper`, `predicate`, `validator`, `formatter`, `accessor`
//!
//! - `orchestration`: `resume_result_error_category`, `resume_execution_target`,
//!   `renderable_resume_execution_target`, `resume_migration_pool`
//! - `mapper`: `resume_model_execution_target`, `resume_provider_execution_target`,
//!   `resume_db_error`, `resume_migration_model_pool`, `effective_migration_providers`,
//!   `effective_migration_provider_options`, `effective_migration_provider`,
//!   `present_provider_configs`, `provider_default_migration_pool`,
//!   `runtime_migration_providers`, `runtime_migration_provider_options`,
//!   `runtime_provider_config`, `resume_migration_provider_names`,
//!   `provider_model_suggestions`, `provider_models`, `sorted_unique_model_names`
//! - `predicate`: `model_has_provider`, `is_resume_migration_provider`
//! - `validator`: `resolve_model_provider_index`
//! - `formatter`: `resume_model_pool_mismatch_message`,
//!   `format_resume_model_pool_mismatch_message`, `format_resume_error`,
//!   `render_resume_model_pool_mismatch`
//! - `accessor`: `provider_index_in_providers_cfg`, `sorted_provider_names`
//!
//! AGE-166 added typed-signal-first coverage for the headless-resume
//! error-category fallback. `validator` covers the
//! `resume_acceptance_adapter::classify` + `terminal_signal.kind` predicate
//! discrimination (including the `MaybeQuotaExhausted` no-category path)
//! that the new tests pin alongside the diagnostics fallback.
//!
//! AGE-202 added the H7 (resume error + model-pool-mismatch formatting) and H8
//! (resume execution-target + migration-pool resolution) helper clusters,
//! relocated verbatim from main.rs to consolidate the resume concern in one
//! module. Output-preserving.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/resume_cli.rs
//!     role: adapter
//!     Translates:
//!       - resume acceptance result
//!       - typed terminal outcome category
//!       - diagnostics fallback category
//!       - AGE-166 MaybeQuotaExhausted -> no generic ErrorCategory
//!       - AGE-202 H7: ResumeError variants -> user-facing strings
//!       - AGE-202 H7: provider/model pool mismatch -> diagnostic string
//!       - AGE-202 H8: ResolvedResume + ProvidersConfig -> ResumeExecutionTarget
//!       - AGE-202 H8: ResolvedResume + ProvidersConfig -> migration-pool ModelConfig
//! ```

use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig, ProvidersConfig};
use oulipoly_runtime::executor;
use std::collections::HashMap;
use std::path::Path;

pub(super) fn resume_result_error_category(
    agent_runtime_services: &super::wiring::AgentRuntimeServices,
    result: &executor::ExecutionResult,
    models: &HashMap<String, ModelConfig>,
    working_dir: Option<&Path>,
) -> Option<String> {
    if resume_result_succeeded(result) {
        return None;
    }
    if resume_result_is_session_mismatch(result) {
        return Some(crate::dispatch::resume_session_mismatch_category());
    }
    resume_result_terminal_error_category(agent_runtime_services, result, models, working_dir)
}

fn resume_result_succeeded(result: &executor::ExecutionResult) -> bool {
    crate::dispatch::execution_succeeded(result.exit_code)
}

fn resume_result_is_session_mismatch(result: &executor::ExecutionResult) -> bool {
    super::resume_acceptance_adapter::classify(result.resume_acceptance.as_ref())
        == super::resume_acceptance_adapter::ResumeAcceptanceCategory::SessionMismatch
}

fn resume_result_terminal_error_category(
    agent_runtime_services: &super::wiring::AgentRuntimeServices,
    result: &executor::ExecutionResult,
    models: &HashMap<String, ModelConfig>,
    working_dir: Option<&Path>,
) -> Option<String> {
    super::terminal_outcome_adapter::classify_error_category_with_fallback(result, || {
        crate::dispatch::diagnose_execution_error(
            agent_runtime_services,
            result,
            models,
            working_dir,
        )
    })
}

pub(super) fn resume_model_pool_mismatch_message(
    models: &HashMap<String, ModelConfig>,
    model_name: &str,
    session_id: &str,
    provider_name: &str,
) -> String {
    let suggestions = provider_model_suggestions(models, provider_name);
    format_resume_model_pool_mismatch_message(model_name, session_id, provider_name, &suggestions)
}

fn provider_model_suggestions(
    models: &HashMap<String, ModelConfig>,
    provider_name: &str,
) -> Vec<String> {
    sorted_unique_model_names(provider_models(models, provider_name))
}

fn provider_models<'a>(
    models: &'a HashMap<String, ModelConfig>,
    provider_name: &str,
) -> Vec<&'a ModelConfig> {
    collect_provider_models(models.values(), provider_name)
}

fn collect_provider_models<'a>(
    models: impl Iterator<Item = &'a ModelConfig>,
    provider_name: &str,
) -> Vec<&'a ModelConfig> {
    models
        .filter(|model| model_has_provider(model, provider_name))
        .collect()
}

fn model_has_provider(model: &ModelConfig, provider_name: &str) -> bool {
    model
        .providers
        .iter()
        .any(|provider| provider.name == provider_name)
}

fn sorted_unique_model_names(models: Vec<&ModelConfig>) -> Vec<String> {
    let mut names: Vec<String> = models.into_iter().map(|model| model.name.clone()).collect();
    names.sort();
    names.dedup();
    names
}

fn format_resume_model_pool_mismatch_message(
    model_name: &str,
    session_id: &str,
    provider_name: &str,
    suggestions: &[String],
) -> String {
    if suggestions.is_empty() {
        format!(
            "session {session_id} belongs to provider {provider_name}, which is not in model {model_name}'s provider pool.\nTry a model that includes {provider_name}: (no other model in the loaded config includes {provider_name})"
        )
    } else {
        format!(
            "session {session_id} belongs to provider {provider_name}, which is not in model {model_name}'s provider pool.\nTry a model that includes {provider_name}: {}",
            suggestions.join(", ")
        )
    }
}

pub(super) fn format_resume_error(err: oulipoly_state::ResumeError) -> String {
    use oulipoly_state::ResumeError;
    match err {
        ResumeError::InvalidUuid { input } => format!("invalid session UUID: {input}"),
        ResumeError::NoChainFound { input } => format!(
            "No session found matching {input}. Check that session ingestion is configured and that the provider still has resumable local state."
        ),
        ResumeError::WrongIdKind {
            input,
            provider_session_id,
            agent_runner_invocation_id,
            chain_id,
            provider_name,
            ..
        } => {
            let provider_hint = provider_name
                .as_deref()
                .map(|name| format!(" for provider {name}"))
                .unwrap_or_default();
            let chain_hint = chain_id
                .as_deref()
                .map(|id| format!(" chain={id}."))
                .unwrap_or_default();
            match provider_session_id {
                Some(provider_session_id) => format!(
                    "wrong id kind: {input} is an agent-runner invocation id{provider_hint}, not a provider session id. Use `agents --resume {provider_session_id}` to resume. Use `agents trace --json {agent_runner_invocation_id}` to inspect the runner trace.{chain_hint}"
                ),
                None => format!(
                    "wrong id kind: {input} is an agent-runner invocation id{provider_hint}, but no provider_session_id is bound yet. Use `agents trace --json {agent_runner_invocation_id}` to inspect the runner trace.{chain_hint}"
                ),
            }
        }
        ResumeError::Ambiguous { input, previews } => {
            let mut out = format!(
                "[resume] session {input} matches {} chains:\n",
                previews.len()
            );
            for preview in previews {
                out.push_str(&format!(
                    "  chain {} — last used {} — {} — {} turns\n",
                    preview.chain_id,
                    preview.last_used_at.to_rfc3339(),
                    preview.active_provider,
                    preview.turn_count
                ));
            }
            out.push_str("Re-run with: agents resume <chain_id>");
            out
        }
        ResumeError::ProviderModelMismatch {
            model_name,
            active_provider,
            suggestions,
        } => {
            let suffix = if suggestions.is_empty() {
                format!("(no other model in the loaded config includes {active_provider})")
            } else {
                format!("Try one of: {}", suggestions.join(", "))
            };
            format!(
                "session belongs to provider {active_provider}, which is not in model {model_name}'s provider pool. Model {model_name} does not include active segment's owning provider {active_provider}. {suffix}"
            )
        }
        ResumeError::UnknownModel { model_name } => format!("Unknown model: {model_name}"),
        ResumeError::ActiveSegmentMissing { chain_id } => {
            format!("No active segment found for chain {chain_id}")
        }
        ResumeError::ProviderNotConfigured { provider } => {
            format!("provider {provider} is not configured in any loaded model")
        }
        ResumeError::ProviderMissingResume { provider_name } => {
            format!("provider {provider_name} has no [providers.resume] block; cannot resume")
        }
        ResumeError::Db { message } => message,
    }
}

#[derive(Clone)]
pub(super) struct ResumeExecutionTarget {
    pub(super) model: Option<ModelConfig>,
    pub(super) provider_index: usize,
    pub(super) provider: ProviderConfig,
    pub(super) prompt_mode: PromptMode,
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

fn resume_model_execution_target(
    model: &ModelConfig,
    active_provider: &str,
    providers_cfg: &ProvidersConfig,
) -> Result<ResumeExecutionTarget, oulipoly_state::ResumeError> {
    let provider_index = resolve_model_provider_index(model, active_provider)?;
    let (provider, prompt_mode) = providers_cfg
        .effective_provider(&model.providers[provider_index])
        .map_err(resume_db_error)?;
    Ok(ResumeExecutionTarget {
        model: Some(model.clone()),
        provider_index,
        provider,
        prompt_mode,
    })
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
    Ok(ResumeExecutionTarget {
        model: None,
        provider_index: provider_index_in_providers_cfg(providers_cfg, active_provider),
        provider,
        prompt_mode,
    })
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

pub(super) fn resume_migration_pool(
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

pub(super) fn render_resume_model_pool_mismatch(
    models: &HashMap<String, ModelConfig>,
    model_name: &str,
    session_id: &str,
    active_provider: &str,
) {
    eprintln!(
        "{}",
        resume_model_pool_mismatch_message(models, model_name, session_id, active_provider)
    );
}

pub(super) fn renderable_resume_execution_target(
    resolved: &oulipoly_state::ResolvedResume,
    providers_cfg: &ProvidersConfig,
) -> Result<ResumeExecutionTarget, i32> {
    resume_execution_target(resolved, providers_cfg).map_err(|err| {
        crate::dispatch::render_resume_error(err);
        1
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal_outcome_adapter::resume_terminal_signal_for_outcome;
    use oulipoly_config::{ProviderEntry, ResumeKind, ResumeStrategy, SessionStorage};
    use oulipoly_runtime::executor::terminal_signal::TerminalSignalKind;
    use oulipoly_runtime::executor::{
        CapturedChildInvocation, ExecutionResult, SessionCaptureMethod, SessionCaptureResult,
        TerminalSignal,
    };
    use std::path::PathBuf;
    use std::time::SystemTime;

    fn result_with_signal(kind: TerminalSignalKind, exit_code: i32) -> ExecutionResult {
        execution_result_with_signal(terminal_signal(kind), exit_code)
    }

    fn execution_result_with_signal(
        terminal_signal: TerminalSignal,
        exit_code: i32,
    ) -> ExecutionResult {
        ExecutionResult {
            stdout: Vec::new(),
            stderr: "ordinary provider failure".to_string(),
            exit_code,
            provider_index: 0,
            session_capture: SessionCaptureResult {
                session_id: None,
                method: SessionCaptureMethod::None,
            },
            resume_acceptance: None,
            terminal_reason: None,
            terminal_signal: Some(terminal_signal),
            captured_child_invocations: Vec::<CapturedChildInvocation>::new(),
            returned_artifacts: Vec::new(),
        }
    }

    fn terminal_signal(kind: TerminalSignalKind) -> TerminalSignal {
        TerminalSignal {
            kind,
            provider_name: "provider-a".to_string(),
            evidence: "typed evidence".to_string(),
            observed_at: SystemTime::UNIX_EPOCH,
        }
    }

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
            session_storage: Some(claude_code_session_storage(name)),
            resume: Some(resume_flag_strategy()),
            ..ProviderEntry::default()
        }
    }

    fn claude_code_session_storage(name: &str) -> SessionStorage {
        SessionStorage::ClaudeCode {
            projects_dir: PathBuf::from(format!("/tmp/{name}/projects")),
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
    fn resume_fallback_typed_signal_parity() {
        let services = crate::wiring::AgentRuntimeServices::cli_defaults();
        let models = HashMap::new();

        let quota = resume_result_error_category(
            &services,
            &result_with_signal(TerminalSignalKind::QuotaExhaustedInband, 1),
            &models,
            None,
        );
        let maybe = resume_result_error_category(
            &services,
            &result_with_signal(TerminalSignalKind::MaybeQuotaExhausted, 1),
            &models,
            None,
        );
        let clean = resume_result_error_category(
            &services,
            &result_with_signal(TerminalSignalKind::CleanExit, 0),
            &models,
            None,
        );

        assert_eq!(quota.as_deref(), Some("quota_exhausted"));
        assert_eq!(maybe, None);
        assert_eq!(clean, None);
    }

    #[test]
    fn resume_terminal_signal_for_outcome_handles_new_kind() {
        let maybe = result_with_signal(TerminalSignalKind::MaybeQuotaExhausted, 1).terminal_signal;
        let quota = result_with_signal(TerminalSignalKind::QuotaExhaustedInband, 1).terminal_signal;

        assert_eq!(
            resume_terminal_signal_for_outcome(&maybe).map(|signal| signal.kind),
            Some(TerminalSignalKind::MaybeQuotaExhausted)
        );
        assert_eq!(
            resume_terminal_signal_for_outcome(&quota).map(|signal| signal.kind),
            Some(TerminalSignalKind::QuotaExhaustedInband)
        );
    }

    #[test]
    fn migration_target_pool_when_model_none_is_all_storage_providers() {
        let resolved = oulipoly_state::ResolvedResume {
            chain_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_string(),
            model_name: None,
            model: None,
            active_provider: "claude".to_string(),
            active_session_id: "5169694d-de0f-40d1-890c-6e28e55bab27".to_string(),
        };
        let providers_cfg = providers_cfg_with_storage(&["claude", "claude2", "claude3"]);

        let pool = resume_migration_pool(&resolved, &providers_cfg);
        let names = pool
            .providers
            .iter()
            .map(|provider| provider.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["claude", "claude2", "claude3"]);
    }

    #[test]
    fn migration_target_pool_when_model_set_is_model_pool() {
        let model = ModelConfig::from_toml_with_name(
            "claude-opus",
            r#"
[[providers]]
name = "claude"
args = ["--model", "opus"]

[[providers]]
name = "claude2"
args = ["--model", "opus"]
"#,
            None,
        )
        .unwrap();
        let resolved = oulipoly_state::ResolvedResume {
            chain_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_string(),
            model_name: Some("claude-opus".to_string()),
            model: Some(model),
            active_provider: "claude".to_string(),
            active_session_id: "5169694d-de0f-40d1-890c-6e28e55bab27".to_string(),
        };
        let providers_cfg = providers_cfg_with_storage(&["claude", "claude2", "claude3"]);

        let pool = resume_migration_pool(&resolved, &providers_cfg);
        let names = pool
            .providers
            .iter()
            .map(|provider| provider.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["claude", "claude2"]);
    }
}
