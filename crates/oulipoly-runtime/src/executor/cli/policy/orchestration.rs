//! ## Declared roles
//!
//! Roles: orchestration.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/policy/orchestration.rs
//!     role: adapter
//!     Translates:
//!       - provider-policy-config-contract
//!       - executor-policy-launch-contract
//!       - provider-specific-policy-appender-contract
//! ```

use super::messages::provider_policy_kind_error;
use super::predicates::provider_policy_is_needed;
use super::validation::validate_provider_tool_restrictions;
use crate::executor::cli::provider_identity::provider_policy_kind;
use crate::executor::provider_specific::policy::{
    append_claude_provider_policy, append_codex_provider_policy,
};
use oulipoly_config::{ProviderConfig, ToolRestrictionKind};

pub(in crate::executor::cli) fn apply_provider_policy(
    provider: &ProviderConfig,
    base_args: &mut Vec<String>,
    prompt: &mut Option<String>,
) -> Result<(), String> {
    if !provider_policy_is_needed(provider) {
        return Ok(());
    }

    let kind = provider_policy_kind(provider);
    let restrictions = provider.tool_restrictions.as_ref();
    validate_provider_tool_restrictions(provider, restrictions)?;

    match kind {
        Some(ToolRestrictionKind::Claude) => {
            append_claude_provider_policy(provider, restrictions, base_args)
        }
        Some(ToolRestrictionKind::Codex) => {
            append_codex_provider_policy(provider, restrictions, base_args, prompt)?
        }
        None => return Err(provider_policy_kind_error(provider)),
    }

    Ok(())
}

pub(in crate::executor::cli) fn provider_policy_launch_parts(
    provider: &ProviderConfig,
    provider_args: &[String],
    prompt: Option<&str>,
) -> Result<(Vec<String>, Option<String>), String> {
    let mut base_args = provider_args.to_vec();
    let mut policy_prompt = prompt.map(str::to_string);
    apply_provider_policy(provider, &mut base_args, &mut policy_prompt)?;
    Ok((base_args, policy_prompt))
}
