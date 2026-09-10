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

use oulipoly_runtime::provider_registry::ProviderRegistry;
use oulipoly_runtime::services::QuotaServiceExternalProviderIdentity;

pub(crate) fn quota_service_external_identity_for_provider(
    registry: &ProviderRegistry,
    provider_name: &str,
) -> Result<Option<QuotaServiceExternalProviderIdentity>, String> {
    if !registry.has_account_endpoint(provider_name) {
        return Ok(None);
    }
    let endpoint = registry
        .preflight_account(provider_name)
        .map_err(|error| error.to_string())?;
    Ok(Some(QuotaServiceExternalProviderIdentity {
        provider_instance_id: format!("{}-instance", endpoint.capabilities().provider_id),
        settings_id: endpoint
            .settings_id()
            .map_err(|error| error.to_string())?
            .to_string(),
    }))
}

#[cfg(test)]
mod tests {
    use super::quota_service_external_identity_for_provider;
    use oulipoly_runtime::provider_registry::{ProviderRegistry, ProviderRegistryOptions};

    #[test]
    fn builtin_account_has_no_external_quota_identity() {
        let registry = ProviderRegistry::empty(ProviderRegistryOptions::default());

        assert_eq!(
            quota_service_external_identity_for_provider(&registry, "builtin").unwrap(),
            None
        );
    }
}
