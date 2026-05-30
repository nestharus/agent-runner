//! ## Declared roles
//!
//! Roles: accessor, formatter.
//!
//! - accessor: [`provider_for_index`] retrieves the configured provider for
//!   fresh executor facade requests.
//! - formatter: [`provider_index_out_of_range_message`] preserves the
//!   canonical provider-index error string.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/provider_lookup.rs
//!     role: adapter
//!     Translates:
//!       - executor-provider-index-error-contract
//! ```

use oulipoly_config::{ModelConfig, ProviderConfig};

pub(super) fn provider_for_index(
    model: &ModelConfig,
    provider_index: usize,
) -> Result<&ProviderConfig, String> {
    model
        .providers
        .get(provider_index)
        .ok_or_else(|| provider_index_out_of_range_message(model, provider_index))
}

fn provider_index_out_of_range_message(model: &ModelConfig, provider_index: usize) -> String {
    format!(
        "Provider index {} out of range for model {}",
        provider_index, model.name
    )
}
