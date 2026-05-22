//! ## Declared roles
//!
//! Roles: orchestration, validator, formatter, mapper, predicate.
//!
//! - orchestration: [`apply_provider_policy`] dispatches on
//!   [`provider_policy_kind`] and delegates to the Claude/Codex appenders.
//! - orchestration: [`provider_policy_launch_parts`] composes the policy
//!   over base args + prompt for the headless launch path.
//! - validator: [`validate_provider_tool_restrictions`],
//!   [`validate_provider_restriction_shape`],
//!   [`validate_provider_restriction_kind`].
//! - formatter: [`append_claude_provider_policy`],
//!   [`append_codex_provider_policy`], [`append_codex_config_pairs`],
//!   [`append_codex_disabled_features`], [`codex_policy_prompt`],
//!   [`provider_restriction_shape_error`],
//!   [`provider_restriction_kind_mismatch_error`].
//! - mapper: error-shape mappers such as [`provider_policy_kind_error`].
//! - predicate: [`provider_policy_is_needed`].
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/policy.rs
//!     role: adapter
//!     Translates:
//!       - provider-policy-config-contract
//!       - claude-policy-argv-contract
//!       - codex-policy-argv-contract
//! ```

use super::provider_identity::provider_policy_kind;
use oulipoly_config::{ProviderConfig, ToolRestrictionKind};

pub(super) fn apply_provider_policy(
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

fn provider_policy_is_needed(provider: &ProviderConfig) -> bool {
    provider.system_prompt_override.is_some() || provider.tool_restrictions.is_some()
}

pub(super) fn provider_policy_launch_parts(
    provider: &ProviderConfig,
    provider_args: &[String],
    prompt: Option<&str>,
) -> Result<(Vec<String>, Option<String>), String> {
    let mut base_args = provider_args.to_vec();
    let mut policy_prompt = prompt.map(str::to_string);
    apply_provider_policy(provider, &mut base_args, &mut policy_prompt)?;
    Ok((base_args, policy_prompt))
}

fn validate_provider_tool_restrictions(
    provider: &ProviderConfig,
    restrictions: Option<&oulipoly_config::ToolRestrictions>,
) -> Result<(), String> {
    if let Some(restrictions) = restrictions {
        validate_provider_restriction_shape(provider, restrictions)?;
        validate_provider_restriction_kind(provider, restrictions)?;
    }
    Ok(())
}

fn validate_provider_restriction_shape(
    provider: &ProviderConfig,
    restrictions: &oulipoly_config::ToolRestrictions,
) -> Result<(), String> {
    if !restrictions.claude.is_empty() && !restrictions.codex.is_empty() {
        return Err(provider_restriction_shape_error(provider));
    }
    Ok(())
}

fn provider_restriction_shape_error(provider: &ProviderConfig) -> String {
    format!(
        "provider {} has both Claude and Codex tool restrictions populated",
        provider.name
    )
}

fn validate_provider_restriction_kind(
    provider: &ProviderConfig,
    restrictions: &oulipoly_config::ToolRestrictions,
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

fn provider_restriction_kind_mismatch_error(
    provider: &ProviderConfig,
    declared: &str,
    populated: &str,
) -> String {
    format!(
        "provider {} declares {declared} restrictions but populated {populated} settings",
        provider.name
    )
}

fn append_claude_provider_policy(
    provider: &ProviderConfig,
    restrictions: Option<&oulipoly_config::ToolRestrictions>,
    base_args: &mut Vec<String>,
) {
    if let Some(override_text) = &provider.system_prompt_override {
        base_args.push("--append-system-prompt".to_string());
        base_args.push(override_text.clone());
    }
    if let Some(restrictions) = restrictions {
        if !restrictions.claude.disallowed_tools.is_empty() {
            base_args.push("--disallowed-tools".to_string());
            base_args.push(restrictions.claude.disallowed_tools.join(","));
        }
        if !restrictions.claude.allowed_tools.is_empty() {
            base_args.push("--allowed-tools".to_string());
            base_args.push(restrictions.claude.allowed_tools.join(","));
        }
        if restrictions.claude.disable_slash_commands {
            base_args.push("--disable-slash-commands".to_string());
        }
    }
}

fn append_codex_provider_policy(
    provider: &ProviderConfig,
    restrictions: Option<&oulipoly_config::ToolRestrictions>,
    base_args: &mut Vec<String>,
    prompt: &mut Option<String>,
) -> Result<(), String> {
    if let Some(override_text) = &provider.system_prompt_override
        && let Some(existing_prompt) = prompt.take()
    {
        *prompt = Some(codex_policy_prompt(override_text, &existing_prompt));
    }
    if let Some(restrictions) = restrictions {
        append_codex_config_pairs(base_args, &restrictions.codex.config_pairs);
        append_codex_disabled_features(base_args, &restrictions.codex.disabled_features);
    }
    Ok(())
}

fn codex_policy_prompt(override_text: &str, existing_prompt: &str) -> String {
    format!("<<<NESTHARUS-POLICY>>>\n{override_text}\n<<<END-POLICY>>>\n\n{existing_prompt}")
}

fn append_codex_config_pairs(base_args: &mut Vec<String>, config_pairs: &[String]) {
    for pair in config_pairs {
        base_args.push("-c".to_string());
        base_args.push(pair.clone());
    }
}

fn append_codex_disabled_features(base_args: &mut Vec<String>, disabled_features: &[String]) {
    for feature in disabled_features {
        base_args.push("--disable".to_string());
        base_args.push(feature.clone());
    }
}

fn provider_policy_kind_error(provider: &ProviderConfig) -> String {
    format!(
        "provider {} defines policy but its ecosystem could not be inferred",
        provider.name
    )
}
