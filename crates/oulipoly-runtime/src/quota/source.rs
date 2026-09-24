use super::adapter_derived_source::derived_quota_script_from_provider_entry;
use oulipoly_config::{ProviderEntry, ProvidersConfig};

pub(super) struct RefreshSource {
    pub(super) script: String,
    pub(super) auth_refresh_command: Option<String>,
}

pub(super) fn refresh_source(
    provider_name: &str,
    providers_cfg: &ProvidersConfig,
) -> Option<RefreshSource> {
    provider_refresh_source(provider_name, providers_cfg)
}

pub fn has_refresh_source(provider_name: &str, providers_cfg: &ProvidersConfig) -> bool {
    refresh_source(provider_name, providers_cfg).is_some()
}

/// The fresh broker route needs the same explicit or adapter-derived source
/// that ordinary routing would use, without running the legacy State refresh.
pub fn fresh_refresh_source(
    provider_name: &str,
    providers_cfg: &ProvidersConfig,
) -> Option<(String, Option<String>)> {
    refresh_source(provider_name, providers_cfg)
        .map(|source| (source.script, source.auth_refresh_command))
}

fn provider_refresh_source(
    provider_name: &str,
    providers_cfg: &ProvidersConfig,
) -> Option<RefreshSource> {
    let entry = providers_cfg.get(provider_name)?;
    explicit_refresh_source(entry).or_else(|| provider_storage_refresh_source(entry))
}

fn explicit_refresh_source(entry: &ProviderEntry) -> Option<RefreshSource> {
    entry
        .quota_script
        .as_ref()
        .filter(|script| !script.trim().is_empty())
        .map(|script| RefreshSource {
            script: script.clone(),
            auth_refresh_command: entry.auth_refresh_command.clone(),
        })
}

fn provider_storage_refresh_source(entry: &ProviderEntry) -> Option<RefreshSource> {
    derived_quota_script_from_provider_entry(entry).map(|script| RefreshSource {
        script,
        auth_refresh_command: entry.auth_refresh_command.clone(),
    })
}
