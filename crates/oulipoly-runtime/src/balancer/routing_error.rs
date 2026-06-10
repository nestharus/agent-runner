//! ## Declared roles
//!
//! `formatter`, `mapper`.

use oulipoly_config::ModelConfig;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutingError {
    AllProvidersQuotaExhausted {
        model_name: String,
        provider_names: Vec<String>,
    },
    PinnedProviderNotInModel {
        model_name: String,
        target_provider: String,
        provider_names: Vec<String>,
    },
    PinnedProviderIneligible {
        model_name: String,
        target_provider: String,
    },
}

impl fmt::Display for RoutingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RoutingError::AllProvidersQuotaExhausted {
                model_name,
                provider_names,
            } => write!(
                f,
                "all providers in pool {model_name} are quota-exhausted: {}",
                provider_list(provider_names)
            ),
            RoutingError::PinnedProviderNotInModel {
                model_name,
                target_provider,
                provider_names,
            } => write!(
                f,
                "pinned provider {target_provider:?} is not in model {model_name} provider list: {}",
                provider_list(provider_names)
            ),
            RoutingError::PinnedProviderIneligible {
                model_name,
                target_provider,
            } => write!(
                f,
                "pinned provider {target_provider:?} is not eligible for model {model_name}"
            ),
        }
    }
}

impl std::error::Error for RoutingError {}

pub(super) fn all_providers_quota_exhausted_error(model: &ModelConfig) -> RoutingError {
    RoutingError::AllProvidersQuotaExhausted {
        model_name: model.name.clone(),
        provider_names: provider_names(model),
    }
}

pub(super) fn pinned_provider_not_in_model_error(
    model: &ModelConfig,
    target_provider: &str,
) -> RoutingError {
    RoutingError::PinnedProviderNotInModel {
        model_name: model.name.clone(),
        target_provider: target_provider.to_string(),
        provider_names: provider_names(model),
    }
}

fn provider_names(model: &ModelConfig) -> Vec<String> {
    model
        .providers
        .iter()
        .map(|provider| provider.name.clone())
        .collect()
}

fn provider_list(provider_names: &[String]) -> String {
    if provider_names.is_empty() {
        "<empty>".to_string()
    } else {
        provider_names.join(", ")
    }
}
