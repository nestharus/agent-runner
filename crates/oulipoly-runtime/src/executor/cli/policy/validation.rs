//! ## Declared roles
//!
//! Roles: validator.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/policy/validation.rs
//!     role: adapter
//!     Translates:
//!       - provider-policy-config-contract
//!       - provider-policy-validation-contract
//! ```

use super::messages::{provider_restriction_kind_mismatch_error, provider_restriction_shape_error};
use oulipoly_config::{ProviderConfig, ToolRestrictionKind, ToolRestrictions};

pub(super) fn validate_provider_tool_restrictions(
    provider: &ProviderConfig,
    restrictions: Option<&ToolRestrictions>,
) -> Result<(), String> {
    if let Some(restrictions) = restrictions {
        validate_provider_restriction_shape(provider, restrictions)?;
        validate_provider_restriction_kind(provider, restrictions)?;
    }
    Ok(())
}

fn validate_provider_restriction_shape(
    provider: &ProviderConfig,
    restrictions: &ToolRestrictions,
) -> Result<(), String> {
    if !restrictions.claude.is_empty() && !restrictions.codex.is_empty() {
        return Err(provider_restriction_shape_error(provider));
    }
    Ok(())
}

fn validate_provider_restriction_kind(
    provider: &ProviderConfig,
    restrictions: &ToolRestrictions,
) -> Result<(), String> {
    let claude_non_empty = !restrictions.claude.is_empty();
    let codex_non_empty = !restrictions.codex.is_empty();
    match restrictions.kind {
        ToolRestrictionKind::Claude if codex_non_empty => Err(
            provider_restriction_kind_mismatch_error(provider, "Claude", "Codex"),
        ),
        ToolRestrictionKind::Codex if claude_non_empty => Err(
            provider_restriction_kind_mismatch_error(provider, "Codex", "Claude"),
        ),
        _ => Ok(()),
    }
}
