//! ## Declared roles
//!
//! `orchestration`, `filter`, `validator`, `mapper`, `formatter`, `predicate`
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: src-tauri/src/commands/quota_refresh/identity.rs
//!     role: intrinsic-surface
//!     Domain: quota_refresh_identity_bridge
//!     Owns:
//!       - quota_service_external_identity_for_provider
//!       - external_provider_candidate_models
//!       - external_provider_candidates_from_models
//!       - validate_single_external_identity_candidate
//!       - quota_service_external_identity_from_candidate
//!       - format_identity_selection_error
//!       - model_has_provider_name
//!       - external_identity_candidate
//! ```

use oulipoly_config::ModelConfig;
use oulipoly_runtime::services::QuotaServiceExternalProviderIdentity;
use std::collections::HashMap;

const S6B_NEUTRAL_SETTINGS_ID: &str = "provider-a-test";

#[derive(Debug, Clone)]
struct ExternalIdentityCandidate {
    model_name: String,
    provider_instance_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdentitySelectionError {
    Ambiguous,
}

pub(crate) fn quota_service_external_identity_for_provider(
    models: &HashMap<String, ModelConfig>,
    provider_name: &str,
) -> Result<Option<QuotaServiceExternalProviderIdentity>, String> {
    let candidate_models = external_provider_candidate_models(models, provider_name);
    let candidates = external_provider_candidates_from_models(candidate_models, provider_name);
    let selected = validate_single_external_identity_candidate(candidates)
        .map_err(|error| format_identity_selection_error(error, provider_name))?;
    Ok(selected.map(quota_service_external_identity_from_candidate))
}

fn external_provider_candidate_models<'a>(
    models: &'a HashMap<String, ModelConfig>,
    provider_name: &str,
) -> Vec<&'a ModelConfig> {
    models
        .values()
        .filter(|model| model.provider.is_some())
        .filter(|model| model_has_provider_name(model, provider_name))
        .collect()
}

fn external_provider_candidates_from_models(
    models: Vec<&ModelConfig>,
    provider_name: &str,
) -> Vec<ExternalIdentityCandidate> {
    models
        .into_iter()
        .map(|model| external_identity_candidate(model, provider_name))
        .collect()
}

fn validate_single_external_identity_candidate(
    candidates: Vec<ExternalIdentityCandidate>,
) -> Result<Option<ExternalIdentityCandidate>, IdentitySelectionError> {
    match candidates.len() {
        0 => Ok(None),
        1 => Ok(candidates.into_iter().next()),
        _ => Err(IdentitySelectionError::Ambiguous),
    }
}

fn quota_service_external_identity_from_candidate(
    candidate: ExternalIdentityCandidate,
) -> QuotaServiceExternalProviderIdentity {
    QuotaServiceExternalProviderIdentity {
        model_name: candidate.model_name,
        provider_instance_id: candidate.provider_instance_id,
        settings_id: S6B_NEUTRAL_SETTINGS_ID.to_string(),
    }
}

fn format_identity_selection_error(error: IdentitySelectionError, provider_name: &str) -> String {
    match error {
        IdentitySelectionError::Ambiguous => {
            format!("external provider quota identity is ambiguous for provider: {provider_name}")
        }
    }
}

fn model_has_provider_name(model: &ModelConfig, provider_name: &str) -> bool {
    model
        .providers
        .iter()
        .any(|provider| provider.name == provider_name)
}

fn external_identity_candidate(
    model: &ModelConfig,
    provider_name: &str,
) -> ExternalIdentityCandidate {
    ExternalIdentityCandidate {
        model_name: model.name.clone(),
        provider_instance_id: provider_name.to_string(),
    }
}
