//! Declared roles: orchestration, accessor, mapper
//!
//! Command-surface external session provider identity resolution. This module
//! maps resolved session/model state into the service DTO only; provider
//! describe, capability checks, client invocation, and response validation stay
//! in the runtime service seam.

use crate::cli::paths::{default_config_root, default_models_dir};
use oulipoly_config::{ProvidersConfig, load_models};
use oulipoly_runtime::services::SessionServiceExternalProviderIdentity;
use oulipoly_state::{ResolvedResume, ResumeError, StateDb, StoredProviderSessionAuthority};

pub(crate) enum SessionExternalProviderIdentityError {
    AmbiguousSession { input: String },
    Operational { message: String },
}

pub(crate) fn resolve_session_external_provider_identity(
    session_id: &str,
) -> Result<Option<SessionServiceExternalProviderIdentity>, SessionExternalProviderIdentityError> {
    let state = access_default_state_for_identity().map_err(identity_operational_error)?;
    let providers = access_default_providers_for_identity().map_err(identity_operational_error)?;
    let models =
        access_default_models_for_identity(&providers).map_err(identity_operational_error)?;
    let Some(resolved) = access_resolved_resume_for_identity(&state, &models, session_id)? else {
        return Ok(None);
    };
    let authority = state
        .active_provider_session_authority(&resolved.chain_id)
        .map_err(identity_operational_error)?;
    map_resolved_external_provider_identity(resolved, &providers, authority.as_ref())
        .map_err(identity_operational_error)
}

fn identity_operational_error(message: String) -> SessionExternalProviderIdentityError {
    SessionExternalProviderIdentityError::Operational { message }
}

fn access_default_state_for_identity() -> Result<StateDb, String> {
    StateDb::open_default().map_err(|error| format!("failed to open state db: {error}"))
}

fn access_default_providers_for_identity() -> Result<ProvidersConfig, String> {
    ProvidersConfig::load(&default_config_root()?.join("providers.toml"))
        .map_err(|error| format!("failed to load providers: {error}"))
}

fn access_default_models_for_identity(
    providers: &ProvidersConfig,
) -> Result<oulipoly_state::ModelStore, String> {
    load_models(&default_models_dir()?, Some(providers))
        .map_err(|error| format!("failed to load models: {error}"))
}

fn access_resolved_resume_for_identity(
    state: &StateDb,
    models: &oulipoly_state::ModelStore,
    session_id: &str,
) -> Result<Option<ResolvedResume>, SessionExternalProviderIdentityError> {
    let previews = state
        .resume_previews(session_id)
        .map_err(|message| SessionExternalProviderIdentityError::Operational { message })?;
    let cutoff = chrono::Utc::now() - chrono::Duration::hours(24);
    if previews
        .iter()
        .filter(|preview| preview.last_used_at >= cutoff)
        .count()
        > 1
    {
        return Err(SessionExternalProviderIdentityError::AmbiguousSession {
            input: session_id.to_string(),
        });
    }
    match state.resolve_resume(models, session_id, None) {
        Ok(resolved) => Ok(Some(resolved)),
        Err(ResumeError::NoChainFound { .. }) | Err(ResumeError::WrongIdKind { .. }) => Ok(None),
        Err(ResumeError::Ambiguous { input, .. }) => {
            Err(SessionExternalProviderIdentityError::AmbiguousSession { input })
        }
        Err(error) => Err(SessionExternalProviderIdentityError::Operational {
            message: format!("failed to resolve session: {error:?}"),
        }),
    }
}

fn map_resolved_external_provider_identity(
    resolved: ResolvedResume,
    providers: &ProvidersConfig,
    authority: Option<&StoredProviderSessionAuthority>,
) -> Result<Option<SessionServiceExternalProviderIdentity>, String> {
    map_external_model_identity(
        resolved.model_name.as_deref().unwrap_or(""),
        &resolved.active_provider,
        providers,
        authority,
    )
}

fn map_external_model_identity(
    model_name: &str,
    provider_name: &str,
    providers: &ProvidersConfig,
    authority: Option<&StoredProviderSessionAuthority>,
) -> Result<Option<SessionServiceExternalProviderIdentity>, String> {
    validate_external_provider_name(provider_name)?;
    let Some(provider) = providers.get(provider_name) else {
        return Ok(None);
    };
    if provider.implementation.is_none() {
        return Ok(None);
    }
    let authority = authority.ok_or_else(|| {
        format!("provider session has no persisted endpoint authority: {provider_name}")
    })?;
    Ok(Some(SessionServiceExternalProviderIdentity {
        model_name: model_name.to_string(),
        provider_name: provider_name.to_string(),
        provider_instance_id: Some(authority.provider_instance_id.clone()),
        settings_id: authority.settings_id.clone(),
    }))
}

fn validate_external_provider_name(provider_name: &str) -> Result<(), String> {
    if provider_name.trim().is_empty() {
        return Err("external provider configured but active provider name is empty".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::map_external_model_identity;
    use oulipoly_config::{ProviderEndpointConfig, ProviderEntry, ProvidersConfig};
    use oulipoly_state::StoredProviderSessionAuthority;

    #[test]
    fn builtin_account_has_no_external_session_identity() {
        let mut providers = ProvidersConfig::default();
        providers
            .entries
            .insert("builtin".to_string(), ProviderEntry::default());

        assert_eq!(
            map_external_model_identity("model", "builtin", &providers, None).unwrap(),
            None
        );
    }

    #[test]
    fn external_account_identity_uses_explicit_settings() {
        let mut providers = ProvidersConfig::default();
        providers.entries.insert(
            "external".to_string(),
            ProviderEntry {
                implementation: Some(ProviderEndpointConfig {
                    family: "external-family".to_string(),
                    executable: "/provider".to_string(),
                }),
                settings_id: Some("external-settings".to_string()),
                ..ProviderEntry::default()
            },
        );

        let authority = StoredProviderSessionAuthority {
            provider_instance_id: "external-family-instance".to_string(),
            settings_id: "historical-settings".to_string(),
        };
        let identity =
            map_external_model_identity("model", "external", &providers, Some(&authority))
                .unwrap()
                .expect("explicit endpoint should select external session identity");
        assert_eq!(identity.model_name, "model");
        assert_eq!(identity.provider_name, "external");
        assert_eq!(
            identity.provider_instance_id.as_deref(),
            Some("external-family-instance")
        );
        assert_eq!(identity.settings_id, "historical-settings");
    }
}
