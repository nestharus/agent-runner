//! ## Declared roles
//! accessor, mapper, validator
//!
//! State resume lookup helpers and effective provider resolution.

use super::MetadataError;
use super::errors::{
    operational_error, provider_model_mismatch_message, provider_resolution_error,
    session_not_found_error,
};
use oulipoly_config::{ModelConfig, ProviderConfig, ProvidersConfig};
use oulipoly_state::StateDb;

pub(super) fn fetch_resume_previews(
    state: &StateDb,
    input: &str,
) -> Result<Vec<oulipoly_state::ChainPreview>, String> {
    state.resume_previews(input)
}

pub(super) fn fetch_active_segment_id(
    state: &StateDb,
    resolved: &oulipoly_state::ResolvedResume,
) -> Result<Option<i64>, String> {
    state.active_segment_id_for_chain_provider_session(
        &resolved.chain_id,
        &resolved.active_provider,
        &resolved.active_session_id,
    )
}

pub(super) fn active_segment_id_to_metadata_error_or_value(
    active_segment_id: Option<i64>,
    chain_id: &str,
) -> Result<i64, MetadataError> {
    active_segment_id.ok_or_else(|| session_not_found_error(chain_id))
}

pub(super) fn effective_provider_for_resolved(
    resolved: &oulipoly_state::ResolvedResume,
    providers_cfg: &ProvidersConfig,
) -> Result<ProviderConfig, MetadataError> {
    if let Some(model) = resolved.model.as_ref() {
        let model_provider = active_model_provider(model, &resolved.active_provider)
            .ok_or_else(|| provider_mismatch_error(model, &resolved.active_provider))?;
        return effective_model_provider(providers_cfg, model_provider, &resolved.active_provider);
    }
    runtime_provider(providers_cfg, &resolved.active_provider)
}

fn active_model_provider<'a>(
    model: &'a ModelConfig,
    active_provider: &str,
) -> Option<&'a ProviderConfig> {
    model
        .providers
        .iter()
        .find(|provider| provider.name == active_provider)
}

fn provider_mismatch_error(model: &ModelConfig, active_provider: &str) -> MetadataError {
    operational_error(provider_model_mismatch_message(
        &model.name,
        active_provider,
    ))
}

fn effective_model_provider(
    providers_cfg: &ProvidersConfig,
    model_provider: &ProviderConfig,
    active_provider: &str,
) -> Result<ProviderConfig, MetadataError> {
    let (provider, _) = providers_cfg
        .effective_provider(model_provider)
        .map_err(|message| provider_resolution_error(active_provider, message))?;
    Ok(provider)
}

fn runtime_provider(
    providers_cfg: &ProvidersConfig,
    active_provider: &str,
) -> Result<ProviderConfig, MetadataError> {
    let (provider, _) = providers_cfg
        .runtime_provider(active_provider)
        .map_err(|message| provider_resolution_error(active_provider, message))?;
    Ok(provider)
}
