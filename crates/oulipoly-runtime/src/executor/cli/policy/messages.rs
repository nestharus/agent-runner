//! ## Declared roles
//!
//! Roles: formatter.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/policy/messages.rs
//!     role: adapter
//!     Translates:
//!       - provider-policy-error-message-contract
//! ```

use oulipoly_config::ProviderConfig;

pub(super) fn provider_restriction_shape_error(provider: &ProviderConfig) -> String {
    format!(
        "provider {} has both Claude and Codex tool restrictions populated",
        provider.name
    )
}

pub(super) fn provider_restriction_kind_mismatch_error(
    provider: &ProviderConfig,
    declared: &str,
    populated: &str,
) -> String {
    format!(
        "provider {} declares {declared} restrictions but populated {populated} settings",
        provider.name
    )
}

pub(super) fn provider_policy_kind_error(provider: &ProviderConfig) -> String {
    format!(
        "provider {} defines policy but its ecosystem could not be inferred",
        provider.name
    )
}
