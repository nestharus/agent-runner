//! ## Declared roles
//!
//! Roles: formatter.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/provider_specific/policy/codex.rs
//!     role: adapter
//!     Translates:
//!       - provider-policy-config-contract
//!       - codex-policy-argv-contract
//!       - codex-policy-prompt-contract
//! ```

use oulipoly_config::{ProviderConfig, ToolRestrictions};

pub(in crate::executor) fn append_codex_provider_policy(
    provider: &ProviderConfig,
    restrictions: Option<&ToolRestrictions>,
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
