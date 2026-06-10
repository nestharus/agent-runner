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
    let Some(provider_index) = model
        .providers
        .iter()
        .position(|provider| provider.name == target_provider)
    else {
        return Err(pinned_provider_not_in_model_error(model, target_provider));
    };
    if eligible_indices.contains(&provider_index) {
        Ok(provider_index)
    } else {
        Err(RoutingError::PinnedProviderIneligible {
            model_name: model.name.clone(),
            target_provider: target_provider.to_string(),
        })
    }
}
