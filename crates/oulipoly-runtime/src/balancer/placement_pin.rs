//! ## Declared roles
//!
//! `validator`, `filter`.

use super::routing_error::{RoutingError, pinned_provider_not_in_model_error};
use oulipoly_config::ModelConfig;

pub(super) fn select_pinned_provider(
    model: &ModelConfig,
    eligible_indices: &[usize],
    target_provider: &str,
) -> Result<usize, RoutingError> {
    let provider_index = pinned_provider_index(model, target_provider)?;
    validate_pinned_provider_eligibility(model, eligible_indices, target_provider, provider_index)?;
    Ok(provider_index)
}

fn pinned_provider_index(
    model: &ModelConfig,
    target_provider: &str,
) -> Result<usize, RoutingError> {
    model
        .providers
        .iter()
        .position(|provider| provider.name == target_provider)
        .ok_or_else(|| pinned_provider_not_in_model_error(model, target_provider))
}

fn validate_pinned_provider_eligibility(
    model: &ModelConfig,
    eligible_indices: &[usize],
    target_provider: &str,
    provider_index: usize,
) -> Result<(), RoutingError> {
    if pinned_provider_is_eligible(eligible_indices, provider_index) {
        return Ok(());
    }
    Err(pinned_provider_ineligible_error(model, target_provider))
}

fn pinned_provider_is_eligible(eligible_indices: &[usize], provider_index: usize) -> bool {
    eligible_indices.contains(&provider_index)
}

fn pinned_provider_ineligible_error(model: &ModelConfig, target_provider: &str) -> RoutingError {
    RoutingError::PinnedProviderIneligible {
        model_name: model.name.clone(),
        target_provider: target_provider.to_string(),
    }
}
