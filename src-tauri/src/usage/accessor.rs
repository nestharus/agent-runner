use crate::usage::row::EnumeratedAccount;
use crate::usage::vendor;
use oulipoly_config::{ModelConfig, ProvidersConfig};

pub(crate) fn collect_accounts(
    providers: &ProvidersConfig,
    models: &[ModelConfig],
) -> Result<Vec<EnumeratedAccount>, String> {
    let mut accounts = Vec::new();
    for model in models {
        for provider in &model.providers {
            let entry = providers.get(&provider.name).ok_or_else(|| {
                format!(
                    "provider {} is missing from providers.toml; referenced by model {}",
                    provider.name, model.name
                )
            })?;
            accounts.push(EnumeratedAccount {
                account_id: provider.name.clone(),
                vendor: vendor::derive_vendor(entry),
                provider_entry: entry.clone(),
            });
        }
    }
    Ok(accounts)
}
