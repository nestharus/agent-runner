//! ## Declared roles
//!
//! Roles: predicate.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/policy/predicates.rs
//!     role: adapter
//!     Translates:
//!       - provider-policy-config-contract
//! ```

use oulipoly_config::ProviderConfig;

pub(super) fn provider_policy_is_needed(provider: &ProviderConfig) -> bool {
    provider.system_prompt_override.is_some() || provider.tool_restrictions.is_some()
}
