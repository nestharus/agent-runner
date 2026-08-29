//! ## Declared roles
//!
//! `filter`, `formatter`, `mapper`, `orchestration`, `predicate`
//!
//! Resume failure classification and user-facing diagnostics.
//!
//! ## Adapter declarations
//!
//! This adapter translates the resume acceptance result.
//! It translates the typed terminal outcome category.
//! It translates the diagnostics fallback category, state `ResumeError`, runtime
//! `ResumeServiceRejection`, and model-pool mismatch diagnostics.

use oulipoly_config::{ModelConfig, ProvidersConfig};
use oulipoly_runtime::executor;
use std::collections::HashMap;
use std::path::Path;

use super::target::{ResumeExecutionTarget, resume_execution_target};

pub(crate) fn resume_result_error_category(
    agent_runtime_services: &crate::wiring::AgentRuntimeServices,
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
    crate::resume_acceptance_adapter::classify(result.resume_acceptance.as_ref())
        == crate::resume_acceptance_adapter::ResumeAcceptanceCategory::SessionMismatch
}

fn resume_result_terminal_error_category(
    agent_runtime_services: &crate::wiring::AgentRuntimeServices,
    result: &executor::ExecutionResult,
    models: &HashMap<String, ModelConfig>,
    working_dir: Option<&Path>,
) -> Option<String> {
    crate::terminal_outcome_adapter::classify_error_category_with_fallback(result, || {
        crate::dispatch::diagnose_execution_error(
            agent_runtime_services,
            result,
            models,
            working_dir,
        )
    })
}

pub(crate) fn resume_model_pool_mismatch_message(
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
    let names = model_names(provider_models(models, provider_name));
    sorted_unique_model_names(names)
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

fn model_names(models: Vec<&ModelConfig>) -> Vec<String> {
    models.into_iter().map(|model| model.name.clone()).collect()
}

fn sorted_unique_model_names(mut names: Vec<String>) -> Vec<String> {
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

pub(crate) fn format_resume_error(err: oulipoly_state::ResumeError) -> String {
    use oulipoly_state::ResumeError;
    match err {
        ResumeError::InvalidUuid { input } => format!("invalid session id: {input}"),
        ResumeError::InvalidResumeInput { reason, .. } => reason,
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
        } => format_wrong_id_kind(
            input,
            provider_session_id,
            agent_runner_invocation_id,
            chain_id,
            provider_name,
        ),
        ResumeError::Ambiguous { input, previews } => format_ambiguous_resume(input, previews),
        ResumeError::ProviderModelMismatch {
            model_name,
            active_provider,
            suggestions,
        } => format_provider_model_mismatch(model_name, active_provider, suggestions),
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

fn format_wrong_id_kind(
    input: String,
    provider_session_id: Option<String>,
    agent_runner_invocation_id: String,
    chain_id: Option<String>,
    provider_name: Option<String>,
) -> String {
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

fn format_ambiguous_resume(input: String, previews: Vec<oulipoly_state::ChainPreview>) -> String {
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

fn format_provider_model_mismatch(
    model_name: String,
    active_provider: String,
    suggestions: Vec<String>,
) -> String {
    let suffix = provider_model_mismatch_suffix(&active_provider, &suggestions);
    format!(
        "session belongs to provider {active_provider}, which is not in model {model_name}'s provider pool. Model {model_name} does not include active segment's owning provider {active_provider}. {suffix}"
    )
}

fn provider_model_mismatch_suffix(active_provider: &str, suggestions: &[String]) -> String {
    if suggestions.is_empty() {
        format!("(no other model in the loaded config includes {active_provider})")
    } else {
        format!("Try one of: {}", suggestions.join(", "))
    }
}

pub(crate) fn format_resume_service_rejection(
    err: oulipoly_runtime::services::ResumeServiceRejection,
) -> String {
    use oulipoly_runtime::services::ResumeServiceRejection;
    match err {
        ResumeServiceRejection::State(error) => format_resume_error(error),
        ResumeServiceRejection::StorageOwnerNotFound { input, candidates } => format!(
            "storage-owner-not-found: native session {input} is absent from candidate storage: {}",
            format_resume_candidates(&candidates)
        ),
        ResumeServiceRejection::StorageOwnershipAmbiguous { input, owners } => format!(
            "storage-ownership-ambiguous: native session {input} is owned by multiple candidate storages: {}",
            format_resume_candidates(&owners)
        ),
        ResumeServiceRejection::StorageOwnershipIndeterminate { input, failures } => {
            format_indeterminate_ownership(input, failures)
        }
        ResumeServiceRejection::StorageOwnerChainAmbiguous {
            input,
            provider_name,
            chain_ids,
        } => format!(
            "storage-owner-chain-ambiguous: native session {input} is owned by {provider_name} but matches multiple chains: {}",
            chain_ids.join(", ")
        ),
    }
}

fn format_indeterminate_ownership(
    input: String,
    failures: Vec<oulipoly_runtime::services::ResumeStorageFailure>,
) -> String {
    let failures = failures
        .into_iter()
        .map(|failure| format!("{} ({})", failure.provider_name, failure.reason))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "storage-ownership-indeterminate: native session {input} ownership could not be established: {failures}"
    )
}

fn format_resume_candidates(candidates: &[oulipoly_state::ResumeNativeCandidate]) -> String {
    candidates
        .iter()
        .map(|candidate| format!("{} ({})", candidate.matching_provider, candidate.chain_id))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn render_resume_model_pool_mismatch(
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

pub(crate) fn renderable_resume_execution_target(
    resolved: &oulipoly_state::ResolvedResume,
    providers_cfg: &ProvidersConfig,
) -> Result<ResumeExecutionTarget, i32> {
    match resume_execution_target(resolved, providers_cfg) {
        Ok(target) => Ok(target),
        Err(error) => {
            crate::dispatch::render_resume_error(error);
            Err(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal_outcome_adapter::resume_terminal_signal_for_outcome;
    use oulipoly_runtime::executor::terminal_signal::TerminalSignalKind;
    use oulipoly_runtime::executor::{
        CapturedChildInvocation, ExecutionResult, SessionCaptureMethod, SessionCaptureResult,
        TerminalSignal,
    };
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
            produced_assistant_response: false,
            prompt_acceptance_attestation: None,
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

    #[test]
    fn resume_fallback_typed_signal_parity() {
        let services = crate::wiring::AgentRuntimeServices::cli_defaults().unwrap();
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
}
