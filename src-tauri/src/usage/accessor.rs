use crate::usage::row::EnumeratedAccount;
use crate::usage::vendor;
use oulipoly_config::{ModelConfig, ProvidersConfig};
use std::collections::HashSet;

pub(crate) struct CollectAccountsOutput {
    pub(crate) accounts: Vec<EnumeratedAccount>,
    pub(crate) warnings: Vec<String>,
}

pub(crate) fn collect_accounts(
    providers: &ProvidersConfig,
    models: &[ModelConfig],
) -> CollectAccountsOutput {
    let mut accounts = Vec::new();
    let mut warnings = Vec::new();
    let mut seen = HashSet::new();
    for model in models {
        for provider in &model.providers {
            match providers.get(&provider.name) {
                Some(entry) if seen.insert(provider.name.clone()) => {
                    accounts.push(EnumeratedAccount {
                        account_id: provider.name.clone(),
                        vendor: vendor::derive_vendor(entry),
                        provider_entry: entry.clone(),
                    })
                }
                Some(_) => {}
                None => warnings.push(format!(
                    "warning: skipping missing provider {} (referenced by model {})",
                    provider.name, model.name
                )),
            }
        }
    }
    CollectAccountsOutput { accounts, warnings }
}
