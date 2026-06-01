//! Declared roles: orchestration, accessor, mapper
//!
//! Command-surface external session provider identity resolution. This module
//! maps resolved session/model state into the service DTO only; provider
//! describe, capability checks, client invocation, and response validation stay
//! in the runtime service seam.

use crate::cli::paths::{default_config_root, default_models_dir};
use oulipoly_config::{ModelConfig, ProvidersConfig, load_models};
use oulipoly_runtime::services::SessionServiceExternalProviderIdentity;
use oulipoly_runtime::session_provider::S7A_NEUTRAL_SETTINGS_ID;
use oulipoly_state::{ResolvedResume, ResumeError, StateDb};

pub(crate) fn resolve_session_external_provider_identity(
    session_id: &str,
) -> Result<Option<SessionServiceExternalProviderIdentity>, String> {
    let Some(resolved) = access_resolved_session_for_external_identity(session_id)? else {
        return Ok(None);
    };
    map_resolved_external_provider_identity(resolved)
}

fn access_resolved_session_for_external_identity(
    session_id: &str,
) -> Result<Option<ResolvedResume>, String> {
    let state = access_default_state_for_identity()?;
    let providers = access_default_providers_for_identity()?;
    let models = access_default_models_for_identity(&providers)?;
    access_resolved_resume_for_identity(&state, &models, session_id)
}

fn access_default_state_for_identity() -> Result<StateDb, String> {
    StateDb::open_default().map_err(|error| format!("failed to open state db: {error}"))
}

fn access_default_providers_for_identity() -> Result<ProvidersConfig, String> {
    ProvidersConfig::load(&default_config_root().join("providers.toml"))
        .map_err(|error| format!("failed to load providers: {error}"))
}

fn access_default_models_for_identity(
    providers: &ProvidersConfig,
) -> Result<oulipoly_state::ModelStore, String> {
    load_models(&default_models_dir(), Some(providers))
        .map_err(|error| format!("failed to load models: {error}"))
}

fn access_resolved_resume_for_identity(
    state: &StateDb,
    models: &oulipoly_state::ModelStore,
    session_id: &str,
) -> Result<Option<ResolvedResume>, String> {
    match state.resolve_resume(models, session_id, None) {
        Ok(resolved) => Ok(Some(resolved)),
        Err(ResumeError::NoChainFound { .. })
        | Err(ResumeError::WrongIdKind { .. })
        | Err(ResumeError::Ambiguous { .. }) => Ok(None),
        Err(error) => Err(format!("failed to resolve session: {error:?}")),
    }
}

fn map_resolved_external_provider_identity(
    resolved: ResolvedResume,
) -> Result<Option<SessionServiceExternalProviderIdentity>, String> {
    let Some(model) = resolved.model.as_ref() else {
        return Ok(None);
    };
    map_external_model_identity(model, &resolved.active_provider)
}

fn map_external_model_identity(
    model: &ModelConfig,
    provider_name: &str,
) -> Result<Option<SessionServiceExternalProviderIdentity>, String> {
    if model.provider.is_none() {
        return Ok(None);
    }
    validate_external_provider_name(provider_name)?;
    Ok(Some(SessionServiceExternalProviderIdentity {
        model_name: model.name.clone(),
        provider_name: provider_name.to_string(),
        provider_instance_id: None,
        settings_id: default_settings_id(),
    }))
}

fn validate_external_provider_name(provider_name: &str) -> Result<(), String> {
    if provider_name.trim().is_empty() {
        return Err("external provider configured but active provider name is empty".to_string());
    }
    Ok(())
}

fn default_settings_id() -> String {
    S7A_NEUTRAL_SETTINGS_ID.to_string()
}
